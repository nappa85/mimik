"""Harness side of the v3 run infrastructure (plan §11 items 3, 19, 26, 27): failing-first.

Each test was run against commit 2566040 first; the red reasons are in
docs/tool-loop-benchmark.md ("Run infrastructure").
"""

import collections
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import tool_loop_benchmark as tl
from test_tool_loop_benchmark import FIXTURE, Base
from test_tool_loop_v3 import DEMO, CliMixin, demo_cases, episode_row, write_ledger

SUBSET_FILE = Path(__file__).resolve().parent / "case-subsets" / "dev-screen-40.json"
SERVER = {
    "image": "img@sha256:1",
    "args": ["--ctx-size", "24576", "--cache-type-k", "q8_0"],
}


class RunnerCliMixin(CliMixin):
    def run_split(self, tmp, *extra):
        """Dry run; returns (code, ledger rows, stdout, stderr) with the streams separate."""
        out = Path(tmp) / f"ledger-{len(list(Path(tmp).iterdir()))}.jsonl"
        argv = [
            "--pack",
            str(FIXTURE),
            "--model-label",
            "fake",
            "--dry-run",
            "--output",
            str(out),
        ]
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = self.run_main(argv + list(extra))
        rows = [json.loads(x) for x in out.read_text().splitlines()] if out.exists() else []
        return code, rows, stdout.getvalue(), stderr.getvalue()


class QuietProgressTests(RunnerCliMixin, Base):
    def test_quiet_progress_hides_case_outcomes(self):
        """§11 item 3: progress lines printed case id, outcome and correctness; there was no
        --quiet-progress (argparse error)."""
        with tempfile.TemporaryDirectory() as tmp:
            code, rows, _, err = self.run_split(tmp, "--quiet-progress")
        self.assertEqual(code, 0)
        episodes = [r for r in rows if r["event"] == "episode"]
        self.assertEqual(len(episodes), 6)
        for word in ("outcome=", "correct=", "error="):
            self.assertNotIn(word, err)
        for row in episodes:
            self.assertNotIn(row["case_id"], err)
        self.assertIn("6/6", err)

    def test_default_progress_still_shows_outcomes(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, _, _, err = self.run_split(tmp)
        self.assertIn("outcome=", err)
        self.assertIn("spend-focus", err)


class ServerAndThinkingHeaderTests(RunnerCliMixin, Base):
    def test_server_config_recorded(self):
        """The served configuration (image, llama.cpp args incl. ctx and KV cache type) was
        only in provenance.txt, never in the ledger header."""
        with tempfile.TemporaryDirectory() as tmp:
            code, rows, _, _ = self.run_split(
                tmp, "--cases", "spend-focus", "--server-config", json.dumps(SERVER)
            )
        self.assertEqual(code, 0)
        self.assertEqual(rows[0]["server"], SERVER)

    def test_server_config_must_be_an_object_with_image_and_args(self):
        with tempfile.TemporaryDirectory() as tmp:
            for bad in ("[]", "{", '{"image": "x"}', '{"image": "x", "args": "--a"}'):
                with self.subTest(bad=bad):
                    with self.assertRaises(SystemExit) as raised:
                        self.run_split(tmp, "--cases", "spend-focus", "--server-config", bad)
                    self.assertIn("--server-config", str(raised.exception.code))

    def test_thinking_effective_follows_capability(self):
        """LFM2.5 templates ignore enable_thinking: an "always" model must not be recorded as
        thinking off."""
        cases = {
            ("toggle", "off"): "off",
            ("toggle", "on"): "on",
            ("toggle", "default"): "template_default",
            ("always", "off"): "on",
            ("always", "default"): "on",
            ("never", "off"): "off",
        }
        with tempfile.TemporaryDirectory() as tmp:
            for (capability, requested), effective in cases.items():
                with self.subTest(capability=capability, requested=requested):
                    code, rows, _, _ = self.run_split(
                        tmp,
                        "--cases",
                        "spend-focus",
                        "--thinking-capability",
                        capability,
                        "--thinking",
                        requested,
                    )
                    self.assertEqual(code, 0)
                    header = rows[0]
                    self.assertEqual(header["thinking_capability"], capability)
                    self.assertEqual(header["thinking_effective"], effective)
                    self.assertEqual(header["sampling"]["thinking"], requested)

    def test_thinking_on_refused_for_never_model(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            self.assertRaises(SystemExit) as raised,
        ):
            self.run_split(
                tmp,
                "--cases",
                "spend-focus",
                "--thinking-capability",
                "never",
                "--thinking",
                "on",
            )
        self.assertIn("never", str(raised.exception.code))

    def test_default_capability_is_toggle(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, rows, _, _ = self.run_split(tmp, "--cases", "spend-focus")
        self.assertEqual(
            (rows[0]["thinking_capability"], rows[0]["thinking_effective"]),
            ("toggle", "off"),
        )
        self.assertIsNone(rows[0]["server"])


class RunnerIdentityTests(unittest.TestCase):
    def test_server_and_thinking_refuse_a_merge(self):
        """H8: two runs of one label and arm at different KV cache types, contexts or
        effective thinking merged silently."""
        changes = {
            "server": {"image": "i", "args": ["--cache-type-k", "f16"]},
            "thinking_effective": "on",
            "thinking_capability": "always",
        }
        with tempfile.TemporaryDirectory() as tmp:
            a = write_ledger(
                tmp,
                "a.jsonl",
                server=SERVER,
                thinking_effective="off",
                thinking_capability="toggle",
            )
            for n, (key, value) in enumerate(changes.items()):
                base = {"server": SERVER, "thinking_effective": "off"}
                base["thinking_capability"] = "toggle"
                base[key] = value
                b = write_ledger(tmp, f"b{n}.jsonl", episodes=[episode_row("c1", seed=29)], **base)
                with self.subTest(key=key):
                    with self.assertRaises(SystemExit) as raised:
                        tl.load_ledgers([a, b])
                    self.assertIn(key, str(raised.exception))


class CaseSubsetTests(RunnerCliMixin, Base):
    def test_screen_subset_is_interleaved_prefix_with_proportions(self):
        """Screen arm: the first 40 of interleave_by_outcome over the 174 dev cases. There
        was no case_subset helper."""
        cases = demo_cases("dev")
        self.assertEqual(len(cases), 174)
        doc = tl.case_subset(cases, 40, name="dev-screen-40", split="dev", source_sha256="x")
        ordered = [c["id"] for c in tl.interleave_by_outcome(cases)]
        self.assertEqual(doc["case_ids"], ordered[:40])
        self.assertEqual(doc["sha256"], tl.digest(doc["case_ids"]))
        self.assertEqual(doc["rule"], tl.SUBSET_RULE)
        total = collections.Counter(c["expected"]["outcome"] for c in cases)
        by_id = {c["id"]: c["expected"]["outcome"] for c in cases}
        got = collections.Counter(by_id[i] for i in doc["case_ids"])
        for outcome, n in total.items():
            self.assertLessEqual(abs(got[outcome] - 40 * n / 174), 1, outcome)
        self.assertEqual(doc["outcome_counts"], dict(sorted(got.items())))

    def test_committed_screen_subset_matches_the_pack(self):
        doc = json.loads(SUBSET_FILE.read_text(encoding="utf-8"))
        expected = tl.case_subset(
            demo_cases("dev"),
            40,
            name="dev-screen-40",
            split="dev",
            source_sha256=tl.file_digest(DEMO / "cases-dev.json"),
        )
        self.assertEqual(doc, expected)

    def write_subset(self, tmp, n=2, **change):
        path = Path(tmp) / f"subset-{len(list(Path(tmp).iterdir()))}.json"
        stdout, stderr = io.StringIO(), io.StringIO()
        argv = [
            "--pack",
            str(FIXTURE),
            "--write-case-subset",
            str(n),
            "--output",
            str(path),
        ]
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            self.assertEqual(tl.main(argv + ["--subset-name", "fx"]), 0)
        if change:
            doc = json.loads(path.read_text())
            doc.update(change)
            path.write_text(json.dumps(doc))
        return path

    def test_cases_file_runs_the_subset_and_records_its_hash(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.write_subset(tmp)
            doc = json.loads(path.read_text())
            code, rows, _, _ = self.run_split(tmp, "--cases-file", str(path))
        self.assertEqual(code, 0)
        header = rows[0]
        self.assertEqual(header["case_filter"], doc["case_ids"])
        self.assertEqual(header["case_order"], doc["case_ids"])
        self.assertEqual(
            header["case_subset"],
            {"name": "fx", "rule": tl.SUBSET_RULE, "size": 2, "sha256": doc["sha256"]},
        )

    def test_cases_file_tampering_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad = {
                "sha": {"sha256": "0" * 64},
                "source": {"source_cases_sha256": "0" * 64},
                "split": {"split": "heldout"},
                "order": {"case_ids": ["swap-armor", "spend-focus"]},
            }
            for name, change in bad.items():
                path = self.write_subset(tmp, **change)
                if name == "order":  # keep the hash consistent so only the rule check trips
                    doc = json.loads(path.read_text())
                    doc["sha256"] = tl.digest(doc["case_ids"])
                    path.write_text(json.dumps(doc))
                with (
                    self.subTest(name=name),
                    self.assertRaises((SystemExit, tl.PackError)),
                ):
                    self.run_split(tmp, "--cases-file", str(path))

    def test_cases_and_cases_file_are_exclusive(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.write_subset(tmp)
            with self.assertRaises(SystemExit) as raised:
                self.run_split(tmp, "--cases-file", str(path), "--cases", "spend-focus")
            self.assertIn("exclusive", str(raised.exception.code))

    def test_write_subset_refuses_existing_file_and_oversize(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.write_subset(tmp)
            with self.assertRaises(FileExistsError):
                tl.main(
                    [
                        "--pack",
                        str(FIXTURE),
                        "--write-case-subset",
                        "2",
                        "--output",
                        str(path),
                    ]
                )
            with self.assertRaises(SystemExit):
                tl.main(
                    [
                        "--pack",
                        str(FIXTURE),
                        "--write-case-subset",
                        "7",
                        "--output",
                        f"{tmp}/x",
                    ]
                )


if __name__ == "__main__":
    unittest.main()
