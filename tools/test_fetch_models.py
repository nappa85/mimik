"""No network/models: the outer fetch controller keeps one elapsed deadline."""

import json
import tempfile
import unittest
from pathlib import Path

import fetch_models as fetch
from screen_limits import setup_remaining

GOOD = "a" * 64
BAD = "b" * 64


class Clock:
    def __init__(self, now=1000.0):
        self.now = now

    def __call__(self):
        return self.now


class FakeRunner:
    """Scripted phase outcomes; advances the clock to simulate elapsed work."""

    def __init__(self, clock, script, out):
        self.clock = clock
        self.script = script
        self.out = out
        self.calls = []

    def __call__(self, name, argv, timeout):
        phase = name.split("-")[-2]
        self.calls.append((phase, timeout, self.clock()))
        advance, code, stdout, *written = self.script.get(phase, (1, 0, ""))
        if written:
            (self.out / "m.gguf").write_bytes(b"x" * written[0])
        self.clock.now += advance
        return fetch.RunResult(code, stdout, timed_out=False)


def headers(size=100, sha=GOOD):
    return (
        f"HTTP/2 302\r\nx-repo-commit: {'0' * 40}\r\n"
        f'x-linked-size: {size}\r\nx-linked-etag: "{sha}"\r\n\r\n'
    )


def candidate(cap=1000):
    return fetch.Candidate(
        ident="t",
        repo="org/repo",
        revision="0" * 40,
        filename="m.gguf",
        sha256=GOOD,
        approx_bytes=100,
        license="apache-2.0",
        cap_seconds=cap,
    )


def run(directory, script, cap=1000, cutoff=10_000.0, clock=None):
    clock = clock or Clock()
    runner = FakeRunner(clock, script, Path(directory))
    controller = fetch.Controller(Path(directory), cutoff, runner, clock=clock)
    status = controller.fetch(candidate(cap))
    ledger = [json.loads(x) for x in (Path(directory) / "ledger.jsonl").read_text().splitlines()]
    return status, runner, ledger


class FetchControllerTests(unittest.TestCase):
    def test_setup_remaining_explicit_cap_and_default(self):
        self.assertEqual(setup_remaining(10_000, 0, now=1000, cap_seconds=2500), 1500)
        self.assertEqual(setup_remaining(1200, 0, now=500), 400)
        for bad in (0, -1, float("inf"), True):
            with self.assertRaises(ValueError):
                setup_remaining(10_000, 0, now=1, cap_seconds=bad)
        with self.assertRaises(ValueError):
            setup_remaining(10_000, 0, now=2600, cap_seconds=2500)

    def test_transport_switch_keeps_original_candidate_start(self):
        with tempfile.TemporaryDirectory() as directory:
            script = {
                "metadata": (10, 0, headers()),
                "provenance": (5, 0, ""),
                "curl": (400, 1, ""),
                "aria2": (100, 0, "", 100),
                "verify": (5, 0, GOOD + "  /out/m.gguf\n"),
            }
            status, runner, ledger = run(directory, script, cap=1000)
        self.assertEqual(status, "verified")
        timeouts = {phase: (timeout, at) for phase, timeout, at in runner.calls}
        # The fallback transport starts at t=1000+10+5+400 and must get the
        # remainder of the ORIGINAL 1000 s cap, not a fresh 1000 s.
        fallback_timeout, fallback_at = timeouts["aria2"]
        self.assertAlmostEqual(fallback_timeout, 2000 - fallback_at)
        self.assertLess(fallback_timeout, 600)
        starts = {row["candidate_started_unix"] for row in ledger if "phase" in row}
        self.assertEqual(starts, {1000.0})

    def test_exhausted_phase_is_not_launched(self):
        with tempfile.TemporaryDirectory() as directory:
            script = {
                "metadata": (10, 0, headers()),
                "provenance": (5, 0, ""),
                "curl": (990, 0, "", 100),
            }
            status, runner, ledger = run(directory, script, cap=1000)
            marker = Path(directory, "m.gguf.verified.json").exists()
        self.assertEqual(status, "deadline_exhausted")
        self.assertNotIn("verify", [phase for phase, _, _ in runner.calls])
        self.assertFalse(marker)
        self.assertEqual(ledger[-1]["status"], "deadline_exhausted")

    def test_global_cutoff_bounds_candidate_cap(self):
        with tempfile.TemporaryDirectory() as directory:
            script = {"metadata": (10, 0, headers())}
            _, runner, _ = run(directory, script, cap=1000, cutoff=1050.0)
        self.assertEqual(runner.calls[0][1], 50)

    def test_hash_mismatch_fails_and_is_not_marked_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            script = {
                "metadata": (10, 0, headers()),
                "provenance": (5, 0, ""),
                "curl": (100, 0, "", 100),
                "verify": (5, 0, BAD + "  /out/m.gguf\n"),
            }
            status, _, ledger = run(directory, script)
            marker = Path(directory, "m.gguf.verified.json").exists()
        self.assertEqual(status, "hash_mismatch")
        self.assertFalse(marker)
        self.assertEqual(ledger[-1]["status"], "hash_mismatch")

    def test_metadata_hash_disagreeing_with_pin_stops_before_transfer(self):
        with tempfile.TemporaryDirectory() as directory:
            script = {"metadata": (10, 0, headers(sha=BAD))}
            status, runner, _ = run(directory, script)
        self.assertEqual(status, "metadata_mismatch")
        self.assertEqual([phase for phase, _, _ in runner.calls], ["metadata"])

    def test_size_mismatch_is_not_hashed_or_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            script = {
                "metadata": (10, 0, headers(size=100)),
                "provenance": (5, 0, ""),
                "curl": (100, 0, "", 99),
                "aria2": (100, 0, "", 99),
            }
            status, runner, _ = run(directory, script)
            marker = Path(directory, "m.gguf.verified.json").exists()
        self.assertEqual(status, "transfer_incomplete")
        self.assertNotIn("verify", [phase for phase, _, _ in runner.calls])
        self.assertFalse(marker)

    def test_caps_follow_size_over_sixty_percent_throughput(self):
        caps = fetch.caps_for(3 * 1024**2)
        self.assertAlmostEqual(
            caps["qwen35-9b"], fetch.CANDIDATES[0].approx_bytes / (0.6 * 3 * 1024**2)
        )
        self.assertEqual([c.ident for c in fetch.CANDIDATES], ["qwen35-9b", "qwen36-35b-a3b"])
        with self.assertRaises(ValueError):
            fetch.caps_for(0)

    def test_container_command_is_contained(self):
        argv = fetch.container_command("n", "/tmp/out", ["true"], 12.5)
        joined = " ".join(argv)
        for needle in (
            "--user 1000:1000",
            "--cap-drop ALL",
            "--read-only",
            "no-new-privileges",
        ):
            self.assertIn(needle, joined)
        self.assertIn("type=bind,src=/tmp/out,dst=/out", joined)
        self.assertEqual(argv.count("--mount"), 1)
        self.assertIn("timeout -s KILL 13", joined)


if __name__ == "__main__":
    unittest.main()
