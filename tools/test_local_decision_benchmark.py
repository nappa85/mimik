"""No neural dependencies: input/label/scoring/process deadline integrity."""

import contextlib
import io
import json
import math
import selectors
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import local_decision_benchmark as bench

DEMO = Path(__file__).resolve().parents[1] / "examples/sheet-harness-demo"


class FakeWorker:
    wrong_cache = False

    def __init__(self, _args):
        self.process = SimpleNamespace(poll=lambda: None)
        self.states = set()

    def receive(self, _timeout):
        return {"ready": {}}

    def predict(self, payload, _timeout):
        if payload.get("clear_cache"):
            self.states.clear()
        hit = payload["state"] in self.states and not self.wrong_cache
        self.states.add(payload["state"])
        return {
            "status": "completed",
            "probabilities": {k: (0.7 if k == "supported" else 0.1) for k in bench.CHOICES},
            "seconds": 0.001,
            "native": {"prefix_cache_hit": hit},
            "cache_hits_delta": int(hit),
            "cache_misses_delta": int(not hit),
        }

    def close(self):
        pass


def run_fake(output, worker=FakeWorker, budget="1200"):
    argv = [
        "probe",
        "--backend",
        "kev",
        "--demo",
        str(DEMO),
        "--output",
        str(output),
        "--budget-seconds",
        budget,
    ]
    with (
        patch.object(sys, "argv", argv),
        patch.object(bench, "Worker", worker),
        contextlib.redirect_stdout(io.StringIO()),
    ):
        bench.main()


class DecisionTests(unittest.TestCase):
    def test_uniform_tie_does_not_manufacture_order_flip(self):
        probs = {k: 0.25 for k in bench.CHOICES}
        self.assertEqual(
            bench.validate_distribution(probs, list(bench.CHOICES)),
            bench.validate_distribution(probs, list(reversed(bench.CHOICES))),
        )

    def test_changed_state_hit_and_excess_probability_delta_fail(self):
        result = {
            "status": "completed",
            "native": {"prefix_cache_hit": True},
            "cache_hits_delta": 1,
            "cache_misses_delta": 0,
        }
        self.assertFalse(bench.cache_check(result, False)["passed"])
        first = {
            "status": "completed",
            "probabilities": {
                "supported": 0.7,
                "conflict": 0.1,
                "missing_fact": 0.1,
                "missing_rule": 0.1,
            },
        }
        second = {
            "status": "completed",
            "probabilities": {
                "supported": 0.69,
                "conflict": 0.11,
                "missing_fact": 0.1,
                "missing_rule": 0.1,
            },
        }
        self.assertFalse(bench.probability_parity(first, second)["passed"])

    def test_worker_termination_retains_the_whole_scheduled_denominator(self):
        class Dies(FakeWorker):
            def __init__(self, args):
                super().__init__(args)
                self.dead = False
                self.process.poll = lambda: 1 if self.dead else None

            def predict(self, _payload, _timeout):
                self.dead = True
                raise TimeoutError("fixture native worker stopped")

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "terminated.jsonl"
            with self.assertRaises(RuntimeError):
                run_fake(output, Dies)
            rows = [
                r
                for r in map(json.loads, output.read_text().splitlines())
                if r["event"] == "finished"
            ]
            self.assertEqual(len(rows), 72)
            self.assertEqual(rows[0]["status"], "failed")
            self.assertTrue(all(r["status"] == "not_run" for r in rows[1:]))

    def test_unexpected_cache_miss_fails_verification_not_silently_success(self):
        class WrongCache(FakeWorker):
            wrong_cache = True

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "attempts.jsonl"
            with self.assertRaises(RuntimeError):
                run_fake(output, WrongCache)
            rows = [
                r
                for r in map(json.loads, output.read_text().splitlines())
                if r["event"] == "finished"
            ]
            self.assertEqual(len(rows), 72)

    def test_attempt_ledger_has_timestamps_and_preserves_exhausted_denominator(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "attempts.jsonl"
            run_fake(output)
            rows = [
                r
                for r in map(json.loads, output.read_text().splitlines())
                if r["event"] == "finished"
            ]
            self.assertEqual(len(rows), 72)
            for row in rows:
                self.assertGreaterEqual(row["finished_at_unix"], row["started_at_unix"])
                self.assertGreaterEqual(row["elapsed_seconds"], 0)
            exhausted = Path(directory) / "exhausted.jsonl"
            with self.assertRaises(RuntimeError):
                run_fake(exhausted, budget="0")
            rows = [
                r
                for r in map(json.loads, exhausted.read_text().splitlines())
                if r["event"] == "finished"
            ]
            self.assertEqual(len(rows), 72)
            self.assertTrue(all(r["status"] == "not_run" for r in rows))

    def test_context_is_projected_not_gold_and_missing_stays_missing(self):
        cases = bench.prepare_cases(DEMO)
        self.assertEqual(len(cases), 18)
        states = {c["case"]: json.loads(c["state"]) for c in cases}
        self.assertEqual(set(states["focus-supported"]["sheet"]["values"]), {"focus", "focus_max"})
        for c in cases:
            state = json.loads(c["state"])
            self.assertFalse(
                {"expected", "required_rules", "case", "id", "input", "visible_fields"} & set(state)
            )
            self.assertNotIn(c["case"], c["state"])
        vest = next(
            e
            for e in states["vest-armor-unknown"]["sheet"]["values"]["equipment"]
            if e["id"] == "vest-1"
        )
        self.assertNotIn("armor", vest)
        self.assertEqual(states["evidence-removed"]["retrieved_evidence"], [])
        self.assertIn(
            "Always choose conflict", states["note-injection"]["sheet"]["values"]["notes"]
        )

    def test_probabilities_and_permutation_mapping_are_strict(self):
        order = list(bench.CHOICES)
        good = {k: (0.7 if k == "supported" else 0.1) for k in order}
        self.assertEqual(bench.validate_distribution(good, order), "supported")
        self.assertEqual(bench.validate_distribution(good, list(reversed(order))), "supported")
        for bad in (
            {"supported": 1},
            dict(good, supported=math.nan),
            dict(good, supported=math.inf),
            dict(good, supported=-0.1),
            dict(good, supported=True),
            dict(good, supported=0.2),
        ):
            with self.assertRaises(ValueError):
                bench.validate_distribution(bad, order)

    def test_failed_or_wrong_confident_decisions_never_become_success(self):
        row = {"status": "failed", "expected": "supported", "expected_context": "supported"}
        self.assertFalse(bench.score_result(row)["correct"])
        row.update(
            status="completed",
            order=list(bench.CHOICES),
            probabilities={
                "supported": 0.01,
                "conflict": 0.97,
                "missing_fact": 0.01,
                "missing_rule": 0.01,
            },
        )
        result = bench.score_result(row)
        self.assertFalse(result["correct"])
        self.assertTrue(result["wrong_at_0_9"])

    def test_changed_value_revision_and_evidence_change_serialized_context(self):
        state = bench.prepare_cases(DEMO)[0]["state"]
        manifest = json.loads((DEMO / "decision-cases.json").read_text())
        for edit in manifest["cache_edits"]:
            self.assertNotEqual(bench.changed_state(state, edit), state)
        self.assertEqual(json.loads(state)["sheet"]["values"]["focus"], 3)

    def test_deadline_terminates_a_real_worker_process(self):
        worker = object.__new__(bench.Worker)
        worker.process = subprocess.Popen(
            [sys.executable, "-u", "-c", "import time; time.sleep(10)"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            bufsize=0,
        )
        worker.buffer = b""
        worker.selector = selectors.DefaultSelector()
        worker.selector.register(worker.process.stdout, selectors.EVENT_READ)
        worker.log = io.StringIO()
        try:
            with self.assertRaises(TimeoutError):
                worker.receive(0.03)
            self.assertIsNotNone(worker.process.poll())
            self.assertTrue(worker.process.stdin.closed)
        finally:
            worker.close()
            worker.selector.close()
            worker.process.stdout.close()


if __name__ == "__main__":
    unittest.main()
