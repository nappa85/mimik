"""Regressions for the independent review of 95cc971.

Each test failed against 95cc971 (run red first) for the reason in its docstring.
Fixtures build real attempt directories (prepared states, plans, manifest, host mapping and a
driver-run unit with a fake worker), so selection and the decision file read what a run writes.
"""

import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import decision_models
import decision_router as dr
import router_driver as rd
import tool_loop_benchmark as tl

CASES = dr.pack_cases(dr.PACK)
EXPECTED = {c["id"]: c["expected"]["outcome"] for c in CASES}
SUBSET = json.loads(dr.SUBSET.read_text())["case_ids"]
REGISTRY = decision_models.load()
KEY = "r4-kev-0.8b"
ENTRY = REGISTRY["routers"][KEY]


class Clock:
    def __init__(self):
        self.t = 1_000_000.0

    def now(self):
        return self.t


def perfect(kind):
    p = {k: 0.0 for k in dr.CANONICAL}
    if kind == "conflict":
        p["conflict"] = 1.0
    else:
        p[kind], p["conflict"] = 0.9, 0.1
    return p


class Worker:
    """Fake worker: the predictor maps (case id, variant) to probabilities."""

    def __init__(self, n, predictor, mapping, clock, rss=1000, fail=None):
        self.predictor, self.mapping, self.clock, self.rss, self.fail = (
            predictor,
            mapping,
            clock,
            rss,
            fail,
        )
        self.dead = False
        self.process = SimpleNamespace(poll=lambda: -9 if self.dead else None)
        self.n = n

    def receive(self, timeout):
        return {
            "ready": {"library": "fake", "revision": "r", "device_provenance": {"device": "cpu"}}
        }

    def predict(self, payload, timeout):
        self.clock.t += 1
        if payload.get("count_only"):
            return {"status": "counted", "prompt_tokens": len(payload["state"])}
        if self.fail:
            self.fail(payload)
        key = payload["state"].split("|")[0].removeprefix("state-") if payload["state"] else None
        cid = self.mapping.get(key) if key else None
        return {
            "status": "completed",
            "probabilities": self.predictor(cid, payload),
            "seconds": 1.0,
            "question_sha256": dr.OPTION_HASH,
            "worker_peak_rss_kib": self.rss + self.n,
        }

    def close(self):
        self.dead = True


def build_attempt(
    root, unit, predictor, mutate_plan=None, runtime_extra=None, fail=None, pack=dr.PACK
):
    """An attempt dir as run_decision_router.sh leaves it, with a fake worker."""
    _, mapping = dr.derive(CASES)
    prep = Path(root) / "prep"
    rows = [
        {
            "key": k,
            "state": f"state-{k}",
            "state_hash": dr.text_hash(f"state-{k}"),
            "request_only": f"state-{k}|ro",
            "request_only_hash": dr.text_hash(f"state-{k}|ro"),
        }
        for k in mapping
    ]
    dr.write_prepared(prep / "run", mapping, SUBSET, pack, "d" * 64, rows=rows)
    plan_path = prep / "run" / "prepared" / f"plan-{unit}.json"
    if mutate_plan:
        plan = json.loads(plan_path.read_text())
        mutate_plan(plan)
        plan_path.write_text(json.dumps(plan))
    extra = decision_models.summary_extra(REGISTRY, KEY, disk_rate=None)
    extra.update(runtime_extra or {})
    (prep / "summary-extra.json").write_text(json.dumps(extra))
    clock = Clock()
    rd.Unit(
        json.loads(plan_path.read_text()),
        dr.read_states(prep / "run" / "prepared" / "states.jsonl"),
        Path(root) / "unit",
        lambda n: Worker(n, predictor, mapping, clock, fail=fail),
        50,
        100,
        clock.now() + 10**7,
        now=clock.now,
        monotonic=clock.now,
        extra_summary=extra,
        states_path=prep / "run" / "prepared" / "states.jsonl",
        plan_path=plan_path,
    ).run()
    return Path(root)


def good_predictor(cid, payload):
    return perfect(EXPECTED[cid])


class Fixture(unittest.TestCase):
    def attempts(self, screen_predictor=good_predictor, ablation_predictor=None, **kw):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        screen = build_attempt(Path(tmp.name) / "screen", "screen", screen_predictor, **kw)

        def weak(cid, payload):
            return perfect("conflict")

        ablation = build_attempt(
            Path(tmp.name) / "ablation", "ablation", ablation_predictor or weak
        )
        return screen, ablation

    def report(self, screen, ablation=None):
        return dr.select_attempts(CASES, screen, ablation, "R4", SUBSET)


# ---------------------------------------------------------------- A. provenance binding


class ProvenanceTests(Fixture):
    def test_report_binds_every_input_hash_and_the_run_runtime(self):
        """95cc971: the report carried no hash of its screen records, plan, manifest, mapping
        or states, and no runtime identity (Sol B1, Sonnet M2)."""
        screen, ablation = self.attempts()
        report = self.report(screen, ablation)
        self.assertTrue(report["gate"]["passed"], report["gate"])
        bound = report["bindings"]["screen"]
        for name in ("decisions", "plan", "summary", "states", "manifest", "mapping", "subset"):
            self.assertRegex(bound[name], r"^[0-9a-f]{64}$", name)
        self.assertEqual(report["bindings"]["runtime"]["image"], ENTRY["image"])
        self.assertEqual(report["bindings"]["option_hash"], dr.OPTION_HASH)

    def test_stale_report_is_refused(self):
        """95cc971: build_decision_file copied a passing report's gate onto any screen."""
        passing, _ = self.attempts()
        report = self.report(passing)

        def unsafe(cid, payload):
            return perfect("proposal") if EXPECTED[cid] in dr.AT_RISK else perfect(EXPECTED[cid])

        failing, _ = self.attempts(screen_predictor=unsafe)
        self.assertFalse(self.report(failing)["gate"]["passed"])
        with self.assertRaisesRegex(dr.RouterError, "stale|bound"):
            dr.build_decision_file(report, failing, None, ENTRY)

    def test_wrong_screen_states_are_refused(self):
        """95cc971: a states file edited after the run was paired with the run's records."""
        screen, _ = self.attempts()
        states = screen / "prep" / "run" / "prepared" / "states.jsonl"
        states.write_text(states.read_text().replace("state-k000", "state-kXXX"))
        report = self.report(screen)
        self.assertFalse(report["gate"]["passed"])
        self.assertTrue(any("states" in r for r in report["gate"]["reasons"]), report["gate"])

    def test_wrong_runtime_is_refused(self):
        """95cc971: runtime provenance was rebuilt from today's registry, not the run."""
        screen, _ = self.attempts(runtime_extra={"image": "sha256:" + "1" * 64})
        report = self.report(screen)
        with self.assertRaisesRegex(dr.RouterError, "runtime"):
            dr.build_decision_file(report, screen, None, ENTRY)

    def test_row_option_hash_mismatch_is_refused(self):
        """95cc971: select never checked the rows' question/option hash (nit)."""
        screen, _ = self.attempts()
        path = screen / "unit" / "decisions.jsonl"
        rows = [json.loads(x) for x in path.read_text().splitlines()]
        rows[0]["question_sha256"] = "0" * 64
        path.write_text("".join(json.dumps(r) + "\n" for r in rows))
        report = self.report(screen)
        self.assertFalse(report["gate"]["passed"])
        self.assertTrue(any("option hash" in r for r in report["gate"]["reasons"]))

    def test_verify_compares_pack_fold_and_builder_hashes(self):
        """95cc971: verify_decision_file only checked that header keys existed (Sonnet m3)."""
        screen, ablation = self.attempts()
        doc = dr.build_decision_file(self.report(screen, ablation), screen, ablation, ENTRY)
        _, mapping = dr.derive(CASES)
        inverse = {v: k for k, v in mapping.items()}

        def state_hash_of(cid):
            return dr.text_hash(f"state-{inverse[cid]}")

        live = dr.live_identity(dr.PACK)
        dr.verify_decision_file(doc, list(EXPECTED), state_hash_of, live)
        for key, value in (
            ("pack_hashes", {"rules.md": "0" * 64}),
            ("fold_sha256", "0" * 64),
            ("state_builder_sha256", "0" * 64),
        ):
            with self.assertRaisesRegex(dr.RouterError, key.split("_")[0]):
                dr.verify_decision_file(
                    doc, list(EXPECTED), state_hash_of, dict(live, **{key: value})
                )

    def test_probe_summary_bound_to_image_and_artefacts(self):
        """95cc971: a probe of another image or other artefacts gave timeouts (Sonnet m9)."""
        probe = {
            "unit": "probe",
            "complete": True,
            "planned": 3,
            "statuses": {"completed": 3},
            "decision_seconds": {"max": 50.0},
            **decision_models.summary_extra(REGISTRY, KEY, disk_rate=None),
        }
        dr.unit_timeouts("screen", 1, probe, KEY, ENTRY)
        for change in ({"image": "sha256:" + "2" * 64}, {"artefacts": []}):
            with self.assertRaisesRegex(dr.RouterError, "image|artefact"):
                dr.unit_timeouts("screen", 1, dict(probe, **change), KEY, ENTRY)

    def test_choose_refuses_reports_from_different_inputs(self):
        """95cc971: choose compared reports with different option hashes, folds or packs."""
        screen, ablation = self.attempts()
        a = self.report(screen, ablation)
        b = copy.deepcopy(a)
        b["router"] = "R1"
        b["bindings"]["fold_sha256"] = "0" * 64
        with self.assertRaisesRegex(dr.RouterError, "same"):
            dr.choose([a, b])


# ---------------------------------------------------------------- B. falsifier and claims


class FalsifierTests(Fixture):
    def test_gate_passing_falsifier_null_router_is_never_chosen_or_filed(self):
        """95cc971: choose and build_decision_file ignored falsifier.null (Sol 2, Sonnet M1).
        Predictor: every supported case perfect, every at-risk case 'conflict'."""

        def predictor(cid, payload):
            kind = EXPECTED[cid]
            return perfect(kind if kind in dr.SUPPORTED else "conflict")

        screen, ablation = self.attempts(screen_predictor=predictor)
        report = self.report(screen, ablation)
        self.assertTrue(report["gate"]["passed"])
        self.assertTrue(report["falsifier"]["null"])
        self.assertEqual(dr.choose([report])["result"], "null")
        self.assertFalse(report["claim"]["positive_claim"])
        with self.assertRaisesRegex(dr.RouterError, "falsifier"):
            dr.build_decision_file(report, screen, ablation, ENTRY)

    def test_claim_needs_a_complete_ablation_that_the_full_state_beats(self):
        """95cc971: ablations were informational; no consolidated claim eligibility."""
        screen, ablation = self.attempts()
        self.assertTrue(self.report(screen, ablation)["claim"]["positive_claim"])
        self.assertFalse(self.report(screen, None)["claim"]["positive_claim"])

        def same_as_full(cid, payload):
            return perfect(EXPECTED[cid])

        screen2, ablation2 = self.attempts(ablation_predictor=same_as_full)
        claim = self.report(screen2, ablation2)["claim"]
        self.assertFalse(claim["positive_claim"])
        self.assertTrue(any("priors" in r for r in claim["reasons"]))


# ---------------------------------------------------------------- C. strata


class StratumTests(unittest.TestCase):
    def test_per_stratum_correct_excludes_failure_fallbacks(self):
        """95cc971: per-stratum Wilson used confusion[e][e], counting ask_rule fallbacks as
        correct (Sol 5): 8 failures + 7 wrong gave 8/15."""
        expected = {f"r{i}": "ask_rule" for i in range(15)}
        routes = {f"r{i}": {"route": "ask_rule", "reason": "fallback_failure"} for i in range(8)}
        routes.update(
            {f"r{i}": {"route": "conflict", "reason": "at_risk_argmax"} for i in range(8, 15)}
        )
        m = dr.route_metrics(routes, expected)
        row = m["per_stratum"]["ask_rule"]
        self.assertEqual(row["correct"], 0)
        self.assertEqual(row["failures"], 8)
        self.assertEqual(row["wilson95_correct"], tl.wilson(0, 15))
        self.assertEqual(m["correct"]["count"], 0)


# ---------------------------------------------------------------- D. exact job set


class JobSetTests(Fixture):
    def test_screen_without_reversed_jobs_is_incomplete(self):
        """95cc971: a self-consistent main-only plan was accepted as complete (Sol 4)."""

        def main_only(plan):
            plan["decisions"] = [d for d in plan["decisions"] if d["order"] == "canonical"]

        screen, _ = self.attempts(mutate_plan=main_only)
        report = self.report(screen)
        self.assertFalse(report["gate"]["passed"])
        self.assertTrue(any("job set" in r for r in report["gate"]["reasons"]), report["gate"])

    def test_same_count_wrong_variant_jobs_are_incomplete(self):
        def swap(plan):
            for d in plan["decisions"]:
                if d["order"] == "reversed":
                    d.update(variant="request_only", order="canonical")

        screen, _ = self.attempts(mutate_plan=swap)
        self.assertEqual(
            len(json.loads((screen / "unit" / "plan.json").read_text())["decisions"]),
            dr.unit_decisions()["screen"],
        )
        report = self.report(screen)
        self.assertFalse(report["gate"]["passed"])
        self.assertTrue(any("job set" in r for r in report["gate"]["reasons"]))
        check = dr.check_attempt(screen, "screen")
        self.assertTrue(any("job set" in r for r in check["errors"]))


# ---------------------------------------------------------------- F. driver robustness


class DriverRobustnessTests(unittest.TestCase):
    def unit(self, tmp, keys, fail=None, behaviour=None):
        from test_router_driver import Clock as DClock
        from test_router_driver import FakeWorker, plan_for, states_for

        clock = DClock()
        workers = []

        def factory(n):
            w = FakeWorker(n, behaviour or {}, clock)
            if fail:
                w.predict = fail
            workers.append(w)
            return w

        return rd.Unit(
            plan_for(keys),
            states_for(keys),
            Path(tmp),
            factory,
            50,
            100,
            clock.now() + 10**6,
            now=clock.now,
            monotonic=clock.now,
        ), workers

    def test_plan_written_before_the_first_decision(self):
        """95cc971: plan.json was written only after the loop (Sonnet m5)."""
        with tempfile.TemporaryDirectory() as tmp:
            seen = {}

            def fail(payload, timeout):
                seen["plan"] = (Path(tmp) / "plan.json").exists()
                raise KeyboardInterrupt

            unit, _ = self.unit(tmp, ["a"], fail=fail)
            with self.assertRaises(KeyboardInterrupt):
                unit.run()
            self.assertTrue(seen["plan"])
            check = dr.check_attempt_unit(
                Path(tmp), "screen"
            )  # no summary: incomplete, no traceback
            self.assertFalse(check["complete"])

    def test_unexpected_exception_is_a_failed_decision_and_the_unit_continues(self):
        """95cc971: one_attempt caught only TimeoutError, RuntimeError, OSError, ValueError."""
        calls = []

        def fail(payload, timeout):
            calls.append(1)
            if len(calls) <= 2:
                raise KeyError("boom")
            return {
                "status": "completed",
                "probabilities": {**{k: 0.0 for k in dr.CANONICAL}, "ask_rule": 1.0},
                "question_sha256": dr.OPTION_HASH,
            }

        with tempfile.TemporaryDirectory() as tmp:
            unit, _ = self.unit(tmp, ["a", "b"], fail=fail)
            summary = unit.run()
            rows = [json.loads(x) for x in (Path(tmp) / "decisions.jsonl").read_text().splitlines()]
        self.assertEqual(rows[0]["error_class"], "driver_error")
        self.assertEqual(rows[1]["status"], "completed")
        self.assertTrue(summary["complete"])

    def test_no_restart_after_the_final_attempt_when_nothing_remains(self):
        """95cc971: a restart after the last case's failed retry burned the cap (Sonnet m6)."""
        with tempfile.TemporaryDirectory() as tmp:
            unit, workers = self.unit(tmp, ["a", "b"], behaviour={"b": ["timeout", "timeout"]})
            summary = unit.run()
        self.assertEqual(summary["restarts"], 1)
        self.assertEqual(len(workers), 2)

    def test_worker_rss_is_kept_per_attempt_and_in_the_summary(self):
        """95cc971: worker_peak_rss_kib was discarded (Sol 7)."""
        with tempfile.TemporaryDirectory() as tmp:
            unit, _ = self.unit(tmp, ["a", "b"])
            orig = unit.factory

            def factory(n):
                w = orig(n)
                inner = w.predict

                def predict(payload, timeout):
                    out = inner(payload, timeout)
                    out["worker_peak_rss_kib"] = 4242
                    return out

                w.predict = predict
                return w

            unit.factory = factory
            summary = unit.run()
            attempts = [
                json.loads(x) for x in (Path(tmp) / "attempts.jsonl").read_text().splitlines()
            ]
        self.assertEqual(attempts[0]["worker_peak_rss_kib"], 4242)
        self.assertEqual(summary["worker_peak_rss_kib_max"], 4242)


# ---------------------------------------------------------------- G. minor


class MinorTests(unittest.TestCase):
    def test_unit_sizes_refuse_a_missing_pack(self):
        """95cc971: _dev_cases_count returned 0 silently (Sonnet m4)."""
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(dr.RouterError):
                dr.unit_decisions(Path(tmp))
        self.assertEqual(dr.unit_decisions(dr.PACK)["screen"], len(CASES) + 40)

    def test_http_payload_without_router_equals_the_pre_router_harness(self):
        """Pin (Sonnet m7): payload() with no tools argument equals master's f9d34e0 payload,
        whose JSON digest is recorded here."""
        model = tl.HttpModel(
            "http://127.0.0.1:1",
            "m",
            {
                "tool_choice": "auto",
                "temperature": 0.1,
                "top_p": 0.95,
                "max_tokens": 4096,
                "cache_prompt": True,
                "thinking": "off",
            },
        )
        payload = model.payload([{"role": "user", "content": "x"}], 17)
        digest = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        self.assertEqual(digest, MASTER_PAYLOAD_DIGEST)

    def test_resolved_artefacts_must_stay_under_the_model_root(self):
        """95cc971: lexical checks only; a symlinked directory could escape (Sol 6)."""
        with tempfile.TemporaryDirectory() as tmp:
            root, outside = Path(tmp) / "root", Path(tmp) / "outside"
            outside.mkdir()
            root.mkdir()
            item = ENTRY["files"][0]
            target = outside / Path(item["path"]).name
            target.parent.mkdir(parents=True, exist_ok=True)
            with open(target, "wb") as h:
                h.truncate(item["size"])
            first = Path(item["path"]).parts[0]
            (root / first).symlink_to(outside)
            doc = copy.deepcopy(REGISTRY)
            doc["routers"][KEY]["files"] = [dict(item, path=f"{first}/{Path(item['path']).name}")]
            problems = decision_models.check_files(
                doc, KEY, env={decision_models.ROOT_ENV: str(root)}
            )
            self.assertTrue(any("outside the model root" in p for p in problems), problems)


MASTER_PAYLOAD_DIGEST = "aa7a95207c3532b5e907fb8f8945fccb2204dd38cb8f5828ef122b286e3794ef"


# ---------------------------------------------------------------- round 2 (re-review of e8846fb)


class RoundTwoTests(Fixture):
    def test_builder_drift_after_collection_is_refused(self):
        """e8846fb: the manifest's builder/router/render hashes were never checked against the
        analysing code, and the decision file wrote the current builder hash (Sol R2)."""
        screen, ablation = self.attempts()
        manifest = screen / "prep" / "run" / "prepared" / "manifest.json"
        doc = json.loads(manifest.read_text())
        self.assertEqual(doc["builder_sha256"], dr.code_identity()["builder_sha256"])
        for key in ("builder_sha256", "router_sha256", "render_sha256"):
            drifted = dict(doc, **{key: "0" * 64})
            manifest.write_text(json.dumps(drifted, indent=2) + "\n")
            report = self.report(screen)
            self.assertFalse(report["gate"]["passed"], key)
            self.assertTrue(any("code" in r for r in report["gate"]["reasons"]), report["gate"])
        manifest.write_text(json.dumps(doc, indent=2) + "\n")

    def test_decision_file_carries_the_run_builder_identity(self):
        screen, ablation = self.attempts()
        filed = dr.build_decision_file(self.report(screen, ablation), screen, ablation, ENTRY)
        manifest = json.loads((screen / "prep" / "run" / "prepared" / "manifest.json").read_text())
        self.assertEqual(filed["header"]["state_builder_sha256"], manifest["builder_sha256"])
        self.assertEqual(filed["header"]["render_sha256"], manifest["render_sha256"])

    def test_character_file_drift_after_collection_is_refused(self):
        """e8846fb: pack identity omitted the character files (Sol R2, Sonnet N3)."""
        import shutil

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        pack = Path(tmp.name) / "pack"
        shutil.copytree(dr.PACK, pack)
        self.assertTrue(any(k.startswith("characters/") for k in dr.pack_hashes(pack)))
        screen = build_attempt(Path(tmp.name) / "screen", "screen", good_predictor, pack=pack)
        self.assertTrue(
            dr.select_attempts(CASES, screen, None, "R4", SUBSET, pack)["gate"]["passed"]
        )
        character = sorted((pack / "characters").glob("*.json"))[0]
        character.write_text(character.read_text().replace("}", " }", 1))
        report = dr.select_attempts(CASES, screen, None, "R4", SUBSET, pack)
        self.assertFalse(report["gate"]["passed"])
        self.assertTrue(any("pack" in r for r in report["gate"]["reasons"]))

    def test_check_attempt_records_job_and_state_errors(self):
        """e8846fb: expected_jobs/state_text errors raised out of check_attempt (Sonnet N2)."""
        screen, _ = self.attempts()
        subset = screen / "prep" / "run" / "host" / "subset.json"
        subset.write_text(json.dumps(["not-a-case"]))
        manifest = screen / "prep" / "run" / "prepared" / "manifest.json"
        doc = json.loads(manifest.read_text())
        doc["subset_sha256"] = dr.sha_file(subset)
        manifest.write_text(json.dumps(doc))
        check = dr.check_attempt(screen, "screen")
        self.assertFalse(check["complete"])
        self.assertTrue(any("job set" in e for e in check["errors"]), check["errors"])

    def test_ci_runs_the_identity_test_with_the_formula_cli(self):
        """e8846fb: CI ran unittest without CI or MIMIK_FORMULA_EVAL in the container (Sol R4)."""
        ci = (Path(dr.ROOT) / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn("-e CI=1", ci)
        self.assertIn("-e MIMIK_FORMULA_EVAL=/workspace/target/debug/examples/formula_eval", ci)
        self.assertLess(
            ci.index("cargo build --locked --example formula_eval"),
            ci.index("python -m unittest discover"),
        )


class LazyReloadDeadlineTests(unittest.TestCase):
    def test_a_reload_that_consumes_the_allowance_stops_before_prediction(self):
        """e8846fb: the deadline was checked before a lazy reload, not after (Sol R3)."""
        from test_router_driver import Clock as DClock
        from test_router_driver import FakeWorker, plan_for, states_for

        clock = DClock()
        predicted = []

        class SlowReload(FakeWorker):
            def receive(self, timeout):
                self.clock.advance(5 if self.n == 1 else 100)
                return {"ready": {"start": self.n}}

            def predict(self, payload, timeout):
                predicted.append((payload["state"], self.n))
                return super().predict(payload, timeout)

        # 5 s first load; "a" times out twice (lazy restart pending); before "b": left() 250 s,
        # decision timeout 180 s; the reload takes 100 s, leaving 150 s.
        behaviour = {"a": ["timeout", "timeout"]}
        with tempfile.TemporaryDirectory() as tmp:
            unit = rd.Unit(
                plan_for(["a", "b"]),
                states_for(["a", "b"]),
                Path(tmp),
                lambda n: SlowReload(n, behaviour, clock),
                50,
                180,
                0,
                now=clock.now,
                monotonic=clock.now,
            )
            unit.deadline = clock.now() + 5 + 180 + 100 + 180 + dr.DEADLINE_MARGIN + 250
            summary = unit.run()
        self.assertNotIn(("b", 3), predicted)
        self.assertIn("deadline", summary["stopped"])


class RoundThreeTests(Fixture):
    def test_live_router_identity_is_compared(self):
        """b5a4f50: live_identity omitted router_sha256 and verify never compared it (Sol F1)."""
        screen, ablation = self.attempts()
        doc = dr.build_decision_file(self.report(screen, ablation), screen, ablation, ENTRY)
        _, mapping = dr.derive(CASES)
        inverse = {v: k for k, v in mapping.items()}
        live = dr.live_identity(dr.PACK)
        self.assertEqual(live["router_sha256"], doc["header"]["router_sha256"])
        with self.assertRaisesRegex(dr.RouterError, "router"):
            dr.verify_decision_file(
                doc,
                list(EXPECTED),
                lambda c: dr.text_hash(f"state-{inverse[c]}"),
                dict(live, router_sha256="0" * 64),
            )

    def test_state_text_error_is_recorded(self):
        """Sol round 3: the state_text error branch of check_attempt had no test."""
        screen, _ = self.attempts()
        path = screen / "prep" / "run" / "prepared" / "states.jsonl"
        rows = [json.loads(x) for x in path.read_text().splitlines()]
        del rows[0]["state"]
        path.write_text("".join(json.dumps(r) + "\n" for r in rows))
        check = dr.check_attempt(screen, "screen")
        self.assertTrue(
            any("state text cannot be rebuilt" in e for e in check["errors"]), check["errors"]
        )

    def test_runner_tests_fail_rather_than_skip_in_ci(self):
        """Sonnet round 3: CI now runs the runner tests for real; missing tools must fail there."""
        import subprocess
        import sys

        env = {"PATH": "/nonexistent", "CI": "1", "MIMIK_FORMULA_EVAL": ""}
        proc = subprocess.run(
            [sys.executable, "-m", "unittest", "test_run_decision_router"],
            cwd=Path(__file__).parent,
            env=env,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("CI must provide", proc.stderr)

    def test_main_guard_is_the_last_statement(self):
        """b5a4f50: unittest.main() sat mid-file, so direct execution skipped later classes (Sol F2)."""
        import ast

        body = ast.parse(Path(__file__).read_text()).body
        last = body[-1]
        self.assertIsInstance(last, ast.If)
        self.assertIn("__main__", ast.unparse(last.test))


if __name__ == "__main__":
    unittest.main()
