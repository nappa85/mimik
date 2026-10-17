"""Held-out hash gate (docs/tool-loop-harness-v3-plan.md §11 item 8). Failing-first.

Round 1 (0b9742d): a held-out run read the pack's own cases-heldout.json with no hash check.
Round 2 (review of 0b9742d, docs/reviews/2026-10-17-heldout-gate.md): the pin could be replaced
from the CLI, ledgers did not enforce the frozen hash, the file was hashed and parsed in separate
reads, and --validate-pack printed held-out gold details. Every test uses synthetic case files;
a synthetic pin is injected only by patching the module constants (no CLI or env override).
"""

import contextlib
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

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


@contextlib.contextmanager
def pinned(pin_path):
    """Test-only injection: both the pin file and the code anchor."""
    sha = json.loads(Path(pin_path).read_text())["sha256"]
    with (
        patch.object(tl, "HELDOUT_PIN", Path(pin_path)),
        patch.object(tl, "HELDOUT_FROZEN_SHA256", sha),
    ):
        yield


class PinTests(unittest.TestCase):
    def test_committed_pin_and_code_anchor(self):
        pin = tl.load_heldout_pin(tl.HELDOUT_PIN)
        self.assertEqual(pin["sha256"], FROZEN_SHA)
        self.assertEqual(tl.HELDOUT_FROZEN_SHA256, FROZEN_SHA)
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
            changes = (
                {"sha256": "abc"},
                {"cases": 2},
                {"outcomes": {"proposal": "1"}},
                {"split": "dev"},
            )
            for change in changes:
                pin_path.write_text(json.dumps(dict(good, **change)))
                with pinned(pin_path), self.assertRaises(tl.PackError):
                    tl.load_heldout_pin(pin_path)

    def test_editing_the_pin_json_alone_cannot_move_the_anchor(self):
        """Round 2 (all reviewers): the pin JSON is checked against a code constant."""
        with tempfile.TemporaryDirectory() as tmp:
            _, pin_path, _ = synthetic(tmp)
            with self.assertRaisesRegex(tl.PackError, "anchor"):
                tl.load_heldout_pin(pin_path)  # its sha256 is not the code constant

    def test_no_cli_pin_override(self):
        """Round 2: --heldout-pin was a working (if hidden) production bypass."""
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            tl.build_parser().parse_args(["--heldout-pin", "/tmp/p.json"])


class HarnessGateTests(unittest.TestCase):
    def args(self, tmp, *extra):
        out = Path(tmp) / f"ledger-{len(list(Path(tmp).iterdir()))}.jsonl"
        argv = ["--pack", str(FIXTURE), "--split", "heldout", "--model-label", "m"]
        return [*argv, "--dry-run", "--output", str(out), *extra], out

    def test_heldout_without_an_explicit_gated_file_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, pin_path, _ = synthetic(tmp)
            argv, out = self.args(tmp, "--heldout-cases", str(Path(tmp) / "absent.json"))
            with pinned(pin_path), self.assertRaisesRegex(SystemExit, "missing|not found"):
                tl.main(argv)
            self.assertFalse(out.exists())

    def test_hash_mismatch_is_refused_before_parsing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            path.write_text(path.read_text() + " ")
            argv, out = self.args(tmp, "--heldout-cases", str(path))
            parsed = []
            real = tl.Pack.__init__

            def spy(self_pack, *a, **kw):
                parsed.append(kw.get("heldout_bytes"))
                return real(self_pack, *a, **kw)

            with pinned(pin_path), patch.object(tl.Pack, "__init__", spy):
                with self.assertRaisesRegex(SystemExit, "sha256"):
                    tl.main(argv)
            self.assertEqual(parsed, [])  # no pack was built from the unpinned bytes
            self.assertFalse(out.exists())

    def test_matching_file_runs_and_the_header_records_case_and_pin_hashes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, doc = synthetic(tmp)
            argv, out = self.args(tmp, "--heldout-cases", str(path))
            with pinned(pin_path):
                self.assertEqual(tl.main(argv), 0)
            rows = [json.loads(x) for x in out.read_text().splitlines()]
            sha = hashlib.sha256(path.read_bytes()).hexdigest()
            self.assertEqual(rows[0]["heldout_cases_sha256"], sha)
            pin_sha = hashlib.sha256(pin_path.read_bytes()).hexdigest()
            self.assertEqual(rows[0]["heldout_pin_sha256"], pin_sha)
            episodes = [r["case_id"] for r in rows if r["event"] == "episode"]
            self.assertEqual(episodes, [c["id"] for c in doc["cases"]])

    def test_one_read_hashed_then_parsed(self):
        """Round 2 (TOCTOU): the bytes compared with the pin are the bytes parsed. The file is
        swapped after the first read; the run must use the first (pinned) bytes."""
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, doc = synthetic(tmp)
            pinned_bytes = path.read_bytes()
            swapped = dict(doc, cases=[dict(doc["cases"][0], id="swapped-1")])
            path.write_text(json.dumps(swapped))
            real = Path.read_bytes
            calls = []

            def first_then_real(self_path):
                if Path(self_path) == path:
                    calls.append(1)
                    return pinned_bytes if len(calls) == 1 else real(self_path)
                return real(self_path)

            argv, out = self.args(tmp, "--heldout-cases", str(path))
            with pinned(pin_path), patch.object(Path, "read_bytes", first_then_real):
                self.assertEqual(tl.main(argv), 0)
            self.assertEqual(len(calls), 1)
            ids = [json.loads(x).get("case_id") for x in out.read_text().splitlines()]
            self.assertIn(doc["cases"][0]["id"], ids)
            self.assertNotIn("swapped-1", ids)

    def test_counts_must_match_the_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            pin = json.loads(pin_path.read_text())
            pin_path.write_text(json.dumps(dict(pin, outcomes={"answer": 1}, cases=1)))
            argv, _ = self.args(tmp, "--heldout-cases", str(path))
            with pinned(pin_path), self.assertRaisesRegex(SystemExit, "outcome counts"):
                tl.main(argv)

    def test_run_refuses_an_ungated_heldout_pack(self):
        pack = tl.Pack(FIXTURE, engine_factory=engine_factory)
        try:
            self.assertTrue(pack.cases["heldout"])
            argv = ["--pack", str(FIXTURE), "--split", "heldout", "--model-label", "m"]
            args = tl.build_parser().parse_args([*argv, "--dry-run", "--output", "x"])
            with self.assertRaisesRegex(tl.PackError, "gate"):
                tl.run(args, tl.ScriptedModel(), io.StringIO(), pack=pack)
        finally:
            pack.close()

    def test_dev_runs_are_unaffected(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "dev.jsonl"
            argv = ["--pack", str(FIXTURE), "--model-label", "m", "--dry-run"]
            argv += ["--output", str(out), "--heldout-cases", str(Path(tmp) / "absent.json")]
            self.assertEqual(tl.main(argv), 0)
            header = json.loads(out.read_text().splitlines()[0])
            self.assertNotIn("heldout_cases_sha256", header)
            self.assertNotIn("heldout_pin_sha256", header)

    def test_heldout_subset_files_come_from_one_verified_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, doc = synthetic(tmp)
            out = Path(tmp) / "subset.json"
            argv = ["--pack", str(FIXTURE), "--split", "heldout", "--write-case-subset", "1"]
            argv += ["--output", str(out), "--heldout-cases", str(path)]
            reads = []
            real = Path.read_bytes

            def counting(self_path):
                if Path(self_path) == path:
                    reads.append(1)
                return real(self_path)

            with pinned(pin_path), patch.object(Path, "read_bytes", counting):
                self.assertEqual(tl.main(argv), 0)
            self.assertEqual(len(reads), 1)
            sha = hashlib.sha256(path.read_bytes()).hexdigest()
            self.assertEqual(json.loads(out.read_text())["source_cases_sha256"], sha)
            path.write_text(path.read_text() + " ")
            out.unlink()
            with pinned(pin_path), self.assertRaisesRegex(SystemExit, "sha256"):
                tl.main(argv)


class LedgerProvenanceTests(unittest.TestCase):
    """Round 2 (Sol 2): held-out ledgers must carry the frozen hash to summarise or merge."""

    def ledger(self, tmp, pin_path, path, seed="17"):
        out = Path(tmp) / f"l-{seed}-{len(list(Path(tmp).iterdir()))}.jsonl"
        argv = ["--pack", str(FIXTURE), "--split", "heldout", "--model-label", "m", "--dry-run"]
        argv += ["--output", str(out), "--seeds", seed, "--heldout-cases", str(path)]
        with pinned(pin_path):
            self.assertEqual(tl.main(argv), 0)
        return out

    def test_unpinned_heldout_ledger_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            ledger = self.ledger(tmp, pin_path, path)
            with pinned(pin_path):
                tl.load_ledgers([ledger])  # the synthetic anchor: accepted
            with self.assertRaisesRegex(SystemExit, "frozen"):
                tl.load_ledgers([ledger])  # the real anchor: refused

    def test_merging_heldout_ledgers_with_different_case_hashes_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            a = self.ledger(tmp, pin_path, path, "17")
            b = self.ledger(tmp, pin_path, path, "29")
            rows = [json.loads(x) for x in b.read_text().splitlines()]
            rows[0]["heldout_cases_sha256"] = "0" * 64
            b.write_text("".join(json.dumps(r) + "\n" for r in rows))
            with pinned(pin_path), self.assertRaisesRegex(SystemExit, "frozen|heldout"):
                tl.load_ledgers([a, b])

    def test_heldout_bar_refuses_an_unpinned_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, _ = synthetic(tmp)
            ledger = self.ledger(tmp, pin_path, path)
            rows = [json.loads(x) for x in ledger.read_text().splitlines() if '"episode"' in x]
            summary = tl.summarize(rows)[0]
            reasons = tl.bar_verdict(dict(summary, unique_cases=200))["reasons"]
            self.assertIn("frozen", " ".join(reasons))


class ValidatePackTests(unittest.TestCase):
    """Round 2 (Sonnet F5, Space Bunny 6): no held-out gold details from --validate-pack."""

    def test_validate_heldout_needs_the_opt_in_and_prints_counts_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, pin_path, doc = synthetic(tmp)
            argv = ["--pack", str(FIXTURE), "--split", "heldout", "--validate-pack"]
            argv += ["--heldout-cases", str(path)]
            with pinned(pin_path), self.assertRaisesRegex(SystemExit, "allow-heldout-gold-replay"):
                tl.main(argv)
            out = io.StringIO()
            with pinned(pin_path), contextlib.redirect_stdout(out):
                code = tl.main([*argv, "--allow-heldout-gold-replay"])
            text = out.getvalue()
            self.assertEqual(code, 0)
            for case in doc["cases"]:
                self.assertNotIn(case["id"], text)
            report = json.loads(text.splitlines()[0])
            self.assertEqual(report["heldout"]["cases"], len(doc["cases"]))
            self.assertIs(report["heldout"]["expected_replay_passed"], True)


if __name__ == "__main__":
    unittest.main()
