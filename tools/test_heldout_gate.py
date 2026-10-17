"""Held-out hash gate (docs/tool-loop-harness-v3-plan.md §11 item 8). Failing-first.

Before this change a held-out run read the pack's own cases-heldout.json with no hash check.
Every test here uses synthetic case files (copies of the fixture pack's held-out case) and an
injected pin; nothing reads the real frozen file.
"""

import copy
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import tool_loop_benchmark as tl
from test_tool_loop_benchmark import FIXTURE, engine_factory

TOOLS = Path(__file__).resolve().parent
FROZEN_SHA = "6f11af05f27347ea31a50996b0e6b6eea4ccbfa3254a085cd7699d4d8ec68974"


def synthetic(tmp, cases=None):
    """A held-out case file and a pin naming its sha256 and counts."""
    doc = json.loads((FIXTURE / "cases-heldout.json").read_text())
    if cases is not None:
        doc["cases"] = cases
    path = Path(tmp) / "frozen" / "cases-heldout.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(doc))
    outcomes = {}
    for case in doc["cases"]:
        outcomes[case["expected"]["outcome"]] = outcomes.get(case["expected"]["outcome"], 0) + 1
    pin = {
        "encoding": 1,
        "split": "heldout",
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "cases": len(doc["cases"]),
        "outcomes": outcomes,
        "default_path": str(path),
        "freeze_record": "docs/tool-loop-heldout-v3-freeze.md",
    }
    pin_path = Path(tmp) / "pin.json"
    pin_path.write_text(json.dumps(pin))
    return path, pin_path, doc


class PinTests(unittest.TestCase):
    def test_committed_pin(self):
        """The one committed place for the frozen hash and counts."""
        pin = tl.load_heldout_pin(tl.HELDOUT_PIN)
        self.assertEqual(pin["sha256"], FROZEN_SHA)
        self.assertEqual(pin["cases"], 165)
        self.assertEqual(
            pin["outcomes"],
            {
                "proposal": 49,
                "answer": 38,
                "conflict": 27,
                "ask_rule": 15,
                "ask_fact": 15,
                "stale": 9,
                "refuse": 12,
            },
        )
        self.assertEqual(
            pin["default_path"], "/media/data/mimik/blind-heldout/frozen/cases-heldout.json"
        )
        freeze = (TOOLS.parent / "docs" / "tool-loop-heldout-v3-freeze.md").read_text()
        self.assertIn("tools/heldout-v3-pin.json", freeze)
        self.assertIn(FROZEN_SHA, freeze)

    def test_bad_pins_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, pin_path, _ = synthetic(tmp)
            good = json.loads(pin_path.read_text())
            for change in (
                {"sha256": "abc"},
                {"cases": 2},
                {"outcomes": {"proposal": "1"}},
                {"split": "dev"},
            ):
                pin_path.write_text(json.dumps(dict(good, **change)))
                with self.assertRaises(tl.PackError):
                    tl.load_heldout_pin(pin_path)


class HarnessGateTests(unittest.TestCase):
    def args(self, tmp, *extra):
        out = Path(tmp) / f"ledger-{len(list(Path(tmp).iterdir()))}.jsonl"
        argv = [
            "--pack",
            str(FIXTURE),
            "--split",
            "heldout",
            "--model-label",
            "m",
            "--dry-run",
            "--output",
            str(out),
            *extra,
        ]
        return argv, out

    def main(self, argv):
        return tl.main(argv)

    def test_heldout_without_an_explicit_gated_file_is_refused(self):
        """Before: the pack's own cases-heldout.json ran ungated."""
        with tempfile.TemporaryDirectory() as tmp:
            _, pin_path, _ = synthetic(tmp)
            argv, out = self.args(
                tmp,
                "--heldout-pin",
                str(pin_path),
                "--heldout-cases",
                str(Path(tmp) / "absent.json"),
            )
            with self.assertRaisesRegex(SystemExit, "missing|not found"):
                self.main(argv)
            self.assertFalse(out.exists())

    def test_hash_mismatch_is_refused_before_parsing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            path.write_text(path.read_text() + " ")
            argv, out = self.args(tmp, "--heldout-pin", str(pin_path), "--heldout-cases", str(path))
            with self.assertRaisesRegex(SystemExit, "sha256"):
                self.main(argv)
            self.assertFalse(out.exists())

    def test_matching_file_runs_and_the_header_records_its_hash(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, doc = synthetic(tmp)
            argv, out = self.args(tmp, "--heldout-pin", str(pin_path), "--heldout-cases", str(path))
            self.assertEqual(self.main(argv), 0)
            rows = [json.loads(x) for x in out.read_text().splitlines()]
            self.assertEqual(
                rows[0]["heldout_cases_sha256"], hashlib.sha256(path.read_bytes()).hexdigest()
            )
            self.assertEqual(
                [r["case_id"] for r in rows if r["event"] == "episode"],
                [c["id"] for c in doc["cases"]],
            )

    def test_counts_must_match_the_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            pin = json.loads(pin_path.read_text())
            pin_path.write_text(json.dumps(dict(pin, outcomes={"answer": 1}, cases=1)))
            argv, _ = self.args(tmp, "--heldout-pin", str(pin_path), "--heldout-cases", str(path))
            with self.assertRaisesRegex(SystemExit, "outcome counts"):
                self.main(argv)

    def test_run_refuses_an_ungated_heldout_pack(self):
        """No code path runs held-out without the gate, not even run() called directly."""
        pack = tl.Pack(FIXTURE, engine_factory=engine_factory)
        try:
            self.assertTrue(pack.cases["heldout"])  # the pack's own placeholder file
            args = tl.build_parser().parse_args(
                [
                    "--pack",
                    str(FIXTURE),
                    "--split",
                    "heldout",
                    "--model-label",
                    "m",
                    "--dry-run",
                    "--output",
                    "x",
                ]
            )
            with self.assertRaisesRegex(tl.PackError, "gate"):
                tl.run(args, tl.ScriptedModel(), io.StringIO(), pack=pack)
        finally:
            pack.close()

    def test_dev_runs_are_unaffected(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "dev.jsonl"
            argv = [
                "--pack",
                str(FIXTURE),
                "--model-label",
                "m",
                "--dry-run",
                "--output",
                str(out),
                "--heldout-cases",
                str(Path(tmp) / "absent.json"),
            ]
            self.assertEqual(tl.main(argv), 0)
            header = json.loads(out.read_text().splitlines()[0])
            self.assertNotIn("heldout_cases_sha256", header)

    def test_heldout_subset_files_come_from_the_gated_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, doc = synthetic(tmp)
            out = Path(tmp) / "subset.json"
            argv = [
                "--pack",
                str(FIXTURE),
                "--split",
                "heldout",
                "--write-case-subset",
                "1",
                "--output",
                str(out),
                "--heldout-pin",
                str(pin_path),
                "--heldout-cases",
                str(path),
            ]
            self.assertEqual(tl.main(argv), 0)
            self.assertEqual(
                json.loads(out.read_text())["source_cases_sha256"],
                hashlib.sha256(path.read_bytes()).hexdigest(),
            )
            bad = copy.deepcopy(argv)
            path.write_text(path.read_text() + " ")
            out.unlink()
            with self.assertRaisesRegex(SystemExit, "sha256"):
                tl.main(bad)


if __name__ == "__main__":
    unittest.main()
