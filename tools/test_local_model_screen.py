"""No models: fail-closed scheduling, verification and physical accounting."""

import contextlib
import hashlib
import io
import json
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import local_model_screen as screen
from run_model_screen import reservation
from screen_limits import setup_remaining

DEMO = Path(__file__).resolve().parents[1] / "examples/sheet-harness-demo"


class FakeWorker:
    fails_setup = False
    fails_predict = False
    observed = 0

    def __init__(self, _args):
        self.process = SimpleNamespace(poll=lambda: None)

    def receive(self, _timeout):
        if self.fails_setup:
            raise RuntimeError("fixture startup failure")
        return {"ready": {"synthetic": True}}

    def predict(self, payload, _timeout):
        assert "expected" not in payload
        if self.fails_predict:
            raise RuntimeError("fixture forward failure")
        return {
            "status": "completed",
            "probabilities": {k: 0.25 for k in screen.bench.CHOICES},
            "physical_evaluations": self.observed,
        }

    def close(self):
        pass


def fake_run(directory, worker, budget=100):
    root = Path(directory)
    screen.freeze(DEMO, root / "cases.json")
    args = SimpleNamespace(
        cases=root / "cases.json",
        output=root / "results.jsonl",
        budget_seconds=budget,
        backend="anyjev",
        cutoff_unix=time.time() + 100,
        remaining_evaluations=48,
        free_bytes=20 * 1024**3,
    )
    with (
        patch.object(screen.bench, "Worker", worker),
        contextlib.redirect_stdout(io.StringIO()),
    ):
        screen.run(args)


class ScreenTests(unittest.TestCase):
    def test_setup_transport_change_keeps_original_elapsed_deadline(self):
        self.assertEqual(setup_remaining(1200, 0, now=500), 400)
        self.assertEqual(setup_remaining(700, 0, now=500), 200)
        with self.assertRaises(ValueError):
            setup_remaining(1200, 0, now=927.2)

    def test_aggregate_candidate_reservations_never_exceed_384(self):
        self.assertEqual(reservation(336, time.time() + 10, 20 * 1024**3), 384)
        with self.assertRaises(ValueError):
            reservation(384, time.time() + 10, 20 * 1024**3)

    def test_complete_manifest_detects_modified_helper_tokenizer_config_and_weight(
        self,
    ):
        names = [
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "model.safetensors",
            "decider_config.json",
            "decider/__init__.py",
            "decider/infer.py",
            "decider/model.py",
            "decider/prompt.py",
            "decider/temperature.py",
        ]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in names:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"original fixture")
            entries = [
                {
                    "file": name,
                    "bytes": 16,
                    "sha256": hashlib.sha256(b"original fixture").hexdigest(),
                }
                for name in names
            ]
            record = {
                "repo": "Mapika/decider-2b",
                "revision": "ff99089b6810691bd79134ec6febb5d9afa6e619",
                "verified_files": entries,
            }
            manifest = root / "mimik-verified.json"
            manifest.write_text(json.dumps(record))
            screen.verify_snapshot(root, "mapika")
            for name in (
                "decider/infer.py",
                "config.json",
                "tokenizer.json",
                "model.safetensors",
            ):
                with self.subTest(name=name):
                    (root / name).write_bytes(b"tampered fixture")
                    with self.assertRaises(ValueError):
                        screen.verify_snapshot(root, "mapika")
                    (root / name).write_bytes(b"original fixture")
            record["verified_files"] = entries[:-1]
            manifest.write_text(json.dumps(record))
            with self.assertRaises(ValueError):
                screen.verify_snapshot(root, "mapika")

    def test_missing_or_incomplete_snapshot_is_rejected_before_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises((ValueError, FileNotFoundError)):
                screen.verify_snapshot(root, "mapika")
            (root / "mimik-verified.json").write_text(
                json.dumps(
                    {
                        "repo": "Mapika/decider-2b",
                        "revision": "incorrect",
                        "verified_files": [],
                    }
                )
            )
            with self.assertRaises(ValueError):
                screen.verify_snapshot(root, "mapika")

    def test_admission_rejects_cutoff_counter_and_low_memory(self):
        for cutoff, remaining, memory in (
            (0, 48, 20 * 1024**3),
            (time.time() + 100, 0, 20 * 1024**3),
            (time.time() + 100, 48, 1),
        ):
            with self.subTest(remaining=remaining), self.assertRaises(ValueError):
                screen.admit_execution(cutoff, remaining, memory)

    def test_second_forward_is_prevented_not_retried(self):
        guard = screen.ForwardGuard()
        guard.enter()
        with self.assertRaises(RuntimeError):
            guard.enter()
        self.assertEqual(guard.count, 1)

    def test_startup_failure_preserves_all_48_not_run_and_fails_gate(self):
        class Startup(FakeWorker):
            fails_setup = True

        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(RuntimeError):
                fake_run(directory, Startup)
            rows = [
                json.loads(line)
                for line in (Path(directory) / "results.jsonl").read_text().splitlines()
                if json.loads(line)["event"] == "finished"
            ]
            self.assertEqual(len(rows), 48)
            self.assertTrue(all(r["status"] == "not_run" for r in rows))

    def test_failed_decisions_do_not_return_success(self):
        class Fails(FakeWorker):
            fails_predict = True

        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(RuntimeError):
                fake_run(directory, Fails)

    def test_zero_forward_validation_result_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            fake_run(directory, FakeWorker)
            rows = [
                json.loads(line)
                for line in (Path(directory) / "results.jsonl").read_text().splitlines()
                if json.loads(line)["event"] == "finished"
            ]
            self.assertEqual(sum(r["physical_evaluations"] for r in rows), 0)

    def test_exhausted_budget_has_whole_denominator_and_failed_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(RuntimeError):
                fake_run(directory, FakeWorker, 0)


if __name__ == "__main__":
    unittest.main()
