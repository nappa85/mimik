"""Router driver timeouts, restarts and records (decision-router plan §3, §5; rev. 3.1 A1-A5, N3).

New module: these tests define its behaviour (red = module absent). The sequence they pin is
the plan's: first timeout -> restart + one retry; a timeout on the retry -> `timeout` and
restart; at most 3 restarts per unit; then `not_run`; deadline and no legacy budget.
"""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import decision_router as dr
import router_driver as rd


class Clock:
    def __init__(self, start=1_000_000.0):
        self.t = start

    def now(self):
        return self.t

    def advance(self, seconds):
        self.t += seconds


def good_probs(top="ask_rule"):
    p = {k: 0.0 for k in dr.CANONICAL}
    p[top] = 1.0
    return p


class FakeWorker:
    """behaviour[key] is a list consumed per attempt: ok | timeout | exit | invalid | failed."""

    log = []

    def __init__(self, n, behaviour, clock, seconds=10.0, ready_ok=True):
        self.n = n
        self.behaviour = behaviour
        self.clock = clock
        self.seconds = seconds
        self.dead = False
        self.ready_ok = ready_ok
        self.process = SimpleNamespace(poll=lambda: -9 if self.dead else None)
        FakeWorker.log.append(("start", n))

    def receive(self, timeout):
        self.clock.advance(5)
        if not self.ready_ok:
            raise TimeoutError("ready")
        return {"ready": {"start": self.n}}

    def predict(self, payload, timeout):
        if payload.get("count_only"):
            return {"status": "counted", "prompt_tokens": len(payload["state"])}
        key = payload["state"]
        script = self.behaviour.get(key, [])
        action = script.pop(0) if script else "ok"
        if action == "timeout":
            self.clock.advance(timeout)
            self.dead = True
            raise TimeoutError("Native worker exceeded absolute deadline")
        if action == "exit":
            self.dead = True
            raise RuntimeError("Native worker exited: -11")
        self.clock.advance(self.seconds)
        if action == "invalid":
            return {"status": "failed", "error_class": "invalid_distribution", "error": "sum"}
        if action == "bad_sum":
            p = good_probs()
            p["proposal"] = 0.5
            return {"status": "completed", "probabilities": p, "question_sha256": dr.OPTION_HASH}
        return {
            "status": "completed",
            "probabilities": good_probs(),
            "seconds": self.seconds,
            "question_sha256": dr.OPTION_HASH,
        }

    def close(self):
        self.dead = True


def states_for(keys):
    return {k: {"key": k, "state": k, "request_only": "{}"} for k in keys}


def plan_for(keys):
    return {
        "unit": "screen",
        "decisions": [{"key": k, "variant": "full", "order": "canonical"} for k in keys],
    }


class DriverTests(unittest.TestCase):
    def run_unit(
        self,
        keys,
        behaviour,
        deadline_in=10**6,
        decision_timeout=100,
        seconds=10.0,
        plan=None,
        max_restarts=3,
        ready_ok=True,
    ):
        clock = Clock()
        FakeWorker.log = []
        workers = []

        def factory(n):
            w = FakeWorker(n, behaviour, clock, seconds, ready_ok=ready_ok)
            workers.append(w)
            return w

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        unit = rd.Unit(
            plan or plan_for(keys),
            states_for(keys),
            Path(tmp.name),
            factory,
            ready_timeout=50,
            decision_timeout=decision_timeout,
            deadline_epoch=clock.now() + deadline_in,
            now=clock.now,
            monotonic=clock.now,
            max_restarts=max_restarts,
        )
        summary = unit.run()
        rows = [
            json.loads(x) for x in (Path(tmp.name) / "decisions.jsonl").read_text().splitlines()
        ]
        attempts = [
            json.loads(x) for x in (Path(tmp.name) / "attempts.jsonl").read_text().splitlines()
        ]
        return summary, rows, attempts, workers, Path(tmp.name)

    def test_one_deterministic_timeout_does_not_collapse_the_unit(self):
        keys = ["a", "b", "c"]
        summary, rows, attempts, workers, _ = self.run_unit(keys, {"b": ["timeout"]})
        self.assertTrue(summary["complete"])
        self.assertEqual([r["status"] for r in rows], ["completed"] * 3)
        self.assertEqual(summary["restarts"], 1)
        b = [a for a in attempts if a["key"] == "b"]
        self.assertEqual([a["status"] for a in b], ["timeout", "completed"])
        self.assertEqual(rows[1]["attempt"], 2)  # the final record is the retry's

    def test_timeout_on_the_retry_records_timeout_and_continues(self):
        summary, rows, attempts, _, _ = self.run_unit(
            ["a", "b", "c"], {"b": ["timeout", "timeout"]}
        )
        self.assertEqual([r["status"] for r in rows], ["completed", "timeout", "completed"])
        self.assertEqual(rows[1]["error_class"], "timeout")
        self.assertIsNone(rows[1]["probabilities"])
        self.assertEqual(summary["restarts"], 2)
        self.assertTrue(summary["complete"])

    def test_fourth_restart_makes_the_rest_not_run(self):
        keys = ["a", "b", "c", "d", "e"]
        behaviour = {"a": ["timeout", "timeout"], "b": ["timeout"], "c": ["timeout"]}
        summary, rows, _, _, _ = self.run_unit(keys, behaviour)
        self.assertEqual(summary["restarts"], 3)
        statuses = [r["status"] for r in rows]
        self.assertEqual(statuses[:2], ["timeout", "completed"])
        self.assertEqual(statuses[2:], ["not_run"] * 3)
        self.assertFalse(summary["complete"])
        self.assertIn("restart cap", summary["stopped"])

    def test_worker_exit_is_failed_worker_exit_and_retried(self):
        summary, rows, attempts, _, _ = self.run_unit(["a", "b"], {"a": ["exit", "exit"]})
        self.assertEqual(rows[0]["status"], "failed")
        self.assertEqual(rows[0]["error_class"], "worker_exit")
        self.assertEqual(rows[1]["status"], "completed")
        self.assertEqual(len([a for a in attempts if a["key"] == "a"]), 2)

    def test_invalid_distribution_is_final_never_repaired(self):
        summary, rows, attempts, _, _ = self.run_unit(
            ["a", "b"], {"a": ["invalid"], "b": ["bad_sum"]}
        )
        self.assertEqual(
            [r["status"] for r in rows], ["invalid_distribution", "invalid_distribution"]
        )
        self.assertTrue(all(r["probabilities"] is None for r in rows))
        self.assertEqual(summary["restarts"], 0)
        self.assertEqual(len(attempts), 2)

    def test_a_unit_longer_than_the_legacy_budget_completes(self):
        """A5: no 1,200 s legacy budget in router mode; 200 decisions x 10 s = 2,000 s."""
        keys = [f"k{n:03d}" for n in range(200)]
        summary, rows, _, _, _ = self.run_unit(keys, {})
        self.assertTrue(summary["complete"])
        self.assertEqual(len(rows), 200)

    def test_deadline_stops_new_decisions_and_the_unit_is_incomplete(self):
        keys = ["a", "b", "c", "d"]
        # 5 s ready + 10 s per decision; deadline margin 120 s; decision timeout 100 s.
        summary, rows, _, _, _ = self.run_unit(keys, {}, deadline_in=5 + 120 + 100 + 15)
        statuses = [r["status"] for r in rows]
        self.assertEqual(statuses[0], "completed")
        self.assertIn("not_run", statuses)
        self.assertFalse(summary["complete"])
        self.assertIn("deadline", summary["stopped"])

    def test_first_start_failure_makes_everything_not_run(self):
        summary, rows, _, _, _ = self.run_unit(["a", "b"], {}, ready_ok=False)
        self.assertEqual([r["status"] for r in rows], ["not_run", "not_run"])
        self.assertFalse(summary["complete"])

    def test_requests_carry_question_options_and_shared_tolerance(self):
        seen = []

        class Spy(FakeWorker):
            def predict(self, payload, timeout):
                seen.append(payload)
                return super().predict(payload, timeout)

        clock = Clock()
        with tempfile.TemporaryDirectory() as tmp:
            unit = rd.Unit(
                {
                    "unit": "screen",
                    "decisions": [{"key": "a", "variant": "full", "order": "reversed"}],
                },
                states_for(["a"]),
                Path(tmp),
                lambda n: Spy(n, {}, clock),
                50,
                100,
                clock.now() + 10**6,
                now=clock.now,
                monotonic=clock.now,
            )
            unit.run()
        self.assertEqual(seen[0]["question"], dr.QUESTION)
        self.assertEqual(seen[0]["choices"], dr.CHOICES)
        self.assertEqual(seen[0]["tolerance"], dr.SUM_TOLERANCE)
        self.assertEqual(seen[0]["order"], list(reversed(dr.CANONICAL)))

    def test_question_hash_mismatch_stops_the_unit(self):
        class Wrong(FakeWorker):
            def predict(self, payload, timeout):
                out = super().predict(payload, timeout)
                out["question_sha256"] = "0" * 64
                return out

        clock = Clock()
        with tempfile.TemporaryDirectory() as tmp:
            unit = rd.Unit(
                plan_for(["a", "b"]),
                states_for(["a", "b"]),
                Path(tmp),
                lambda n: Wrong(n, {}, clock),
                50,
                100,
                clock.now() + 10**6,
                now=clock.now,
                monotonic=clock.now,
            )
            summary = unit.run()
        self.assertFalse(summary["complete"])
        self.assertIn("hash mismatch", summary["stopped"])

    def test_probe_counts_tokens_and_picks_quantiles(self):
        keys = [f"{'x' * n}" for n in range(1, 21)]
        plan = {
            "unit": "probe",
            "select": "token_quantiles",
            "quantiles": ["p50", "p90", "max"],
            "candidates": keys,
        }
        summary, rows, _, _, _ = self.run_unit(keys, {}, plan=plan)
        self.assertEqual([r["probe"] for r in rows], ["p50", "p90", "max"])
        self.assertEqual(summary["probe_pick_tokens"], {"p50": 10, "p90": 18, "max": 20})
        self.assertEqual(summary["prompt_tokens"]["max"], 20)
        self.assertTrue(summary["complete"])

    def test_worker_logs_are_per_start(self):
        args = SimpleNamespace(
            backend="kev",
            model=None,
            model_sha256=None,
            kev_checkpoint="/k",
            device="cpu",
            threads=6,
            n_ctx=8192,
        )
        made = []

        class Capture:
            def __init__(self, ns):
                made.append(ns)

        with tempfile.TemporaryDirectory() as tmp:
            orig = rd.bench.Worker
            rd.bench.Worker = Capture
            try:
                factory = rd.worker_factory(args, Path(tmp))
                factory(1)
                factory(2)
            finally:
                rd.bench.Worker = orig
        self.assertEqual([m.worker_log.name for m in made], ["worker-1.log", "worker-2.log"])
        self.assertIsNone(made[0].device)  # Kev is CPU-native; device applies to AnyJev
        self.assertFalse(made[0].raw_only)


class EndToEndTests(unittest.TestCase):
    """Driver output -> nested CV selection -> decision file, on the demo pack's dev cases."""

    def test_units_select_and_decision_file(self):
        cases = dr.pack_cases(dr.PACK)
        expected = {c["id"]: c["expected"]["outcome"] for c in cases}
        _, mapping = dr.derive(cases)
        subset = json.loads(dr.SUBSET.read_text())["case_ids"]
        plans = dr.unit_plans(mapping, subset)
        states = {
            k: {"key": k, "state": f"state-{k}", "state_hash": f"h-{k}", "request_only": "{}"}
            for k in mapping
        }
        clock = Clock()

        class Oracle(FakeWorker):
            def predict(self, payload, timeout):
                self.clock.advance(10)
                key = payload["state"].removeprefix("state-")
                kind = expected[mapping[key]] if key in mapping else "ask_rule"
                p = good_probs(kind)
                if kind != "conflict":
                    p[kind], p["conflict"] = 0.9, 0.1
                return {
                    "status": "completed",
                    "probabilities": p,
                    "seconds": 10,
                    "question_sha256": dr.OPTION_HASH,
                }

        with tempfile.TemporaryDirectory() as tmp:
            dirs = {}
            for unit in ("screen", "ablation"):
                dirs[unit] = Path(tmp) / unit
                rd.Unit(
                    plans[unit],
                    states,
                    dirs[unit],
                    lambda n: Oracle(n, {}, clock),
                    50,
                    100,
                    clock.now() + 10**7,
                    now=clock.now,
                    monotonic=clock.now,
                ).run()
            report = dr.select_report(
                cases, mapping, dirs["screen"], dirs["ablation"], "R4", subset
            )
            self.assertTrue(report["gate"]["passed"], report["gate"])
            self.assertEqual(report["refit"]["tau_star"], 0.5)
            self.assertEqual(
                report["cv"]["out_of_fold"]["supported_recall"]["count"],
                sum(e in dr.SUPPORTED for e in expected.values()),
            )
            self.assertEqual(report["order_flips"], {"evaluated": 40, "flips": 0, "of": 40})
            self.assertTrue(report["ablation"]["complete"])
            states_file = Path(tmp) / "states.jsonl"
            states_file.write_text("".join(json.dumps(r) + "\n" for r in states.values()))
            header = {
                k: "x"
                for k in (
                    "runtime",
                    "image",
                    "library_revision",
                    "artefacts",
                    "device",
                    "device_provenance",
                    "threads",
                    "n_ctx",
                    "pack_hashes",
                )
            }
            header["gate"] = {"passed": False}  # never wins over the report's gate
            doc = dr.build_decision_file(report, mapping, dirs["screen"], states_file, header)
            inverse = {v: k for k, v in mapping.items()}
            seen = dr.verify_decision_file(doc, list(expected), lambda cid: f"h-{inverse[cid]}")
            self.assertEqual(len(seen), len(expected))


class QuantileTests(unittest.TestCase):
    def test_probe_picks_are_distinct_states(self):
        picks = rd.quantile_keys({"a": 1, "b": 2, "c": 3})
        self.assertEqual(picks["max"], "c")
        self.assertEqual(len(set(picks.values())), 3)


class ProjectionTests(unittest.TestCase):
    def test_probe_caps_do_not_depend_on_probe_results(self):
        self.assertEqual(dr.ready_timeout(5_000_000_000, probe=True), 5_000_000_000 / 5e6 + 600)
        self.assertEqual(dr.decision_timeout(probe=True), 1800)
        with self.assertRaises(dr.RouterError):
            dr.decision_timeout(None)  # a missing probe never yields an estimate
        self.assertEqual(dr.ready_timeout(5e9, measured_rate=None), 5e9 / 5e6 + 600)

    def test_ready_timeout_never_assumes_a_disk_faster_than_40_mb_s(self):
        self.assertEqual(dr.ready_timeout(4e9, measured_rate=4e9), 4e9 / 40e6 + 600)
        self.assertEqual(dr.ready_timeout(4e9, measured_rate=10e6), 4e9 / 10e6 + 600)

    def test_decision_timeout_clamped(self):
        self.assertEqual(dr.decision_timeout(10), 180)
        self.assertEqual(dr.decision_timeout(100), 300)
        self.assertEqual(dr.decision_timeout(1000), 1800)

    def test_projection_charges_every_allowed_restart(self):
        wall = dr.unit_projection(214, load_seconds=300, timeout=600, seconds=100)
        self.assertAlmostEqual(wall, 300 + 3 * (600 + 15 + 300) + 214 * 1.05 * 1.25 * 100)


if __name__ == "__main__":
    unittest.main()
