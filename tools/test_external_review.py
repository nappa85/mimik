import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import external_review


class FakeProc:
    def __init__(self):
        read_fd, write_fd = os.pipe()  # selectors cannot poll regular files
        os.write(write_fd, b"PONG\n")
        os.close(write_fd)
        self.stdout = os.fdopen(read_fd)

    def poll(self):
        return 0

    def wait(self):
        return 0


class OpencodeDirectoryTests(unittest.TestCase):
    def test_runs_are_pinned_to_the_snapshot_not_the_launch_pwd(self):
        """Regression: opencode uses `$PWD`, so a run launched from the real repo ignored
        the snapshot's `reviewer` agent and failed with UnknownError (2026-10-16)."""
        calls = []

        def fake_popen(command, **kwargs):
            calls.append((command, kwargs))
            return FakeProc()

        with (
            tempfile.TemporaryDirectory() as tmp,
            mock.patch.dict("os.environ", {"PWD": "/real/repo"}),
            mock.patch.object(subprocess, "Popen", fake_popen),
        ):
            snap = Path(tmp)
            self.assertTrue(external_review.preflight("m", snap, attempts=1))
        command, kwargs = calls[0]
        self.assertEqual(command[command.index("--dir") + 1], str(snap))
        self.assertEqual(kwargs["env"]["PWD"], str(snap))
        self.assertEqual(kwargs["cwd"], snap)
        self.assertIs(kwargs["stdin"], subprocess.DEVNULL)


class ReviewerAgentTests(unittest.TestCase):
    def test_reviewer_cannot_reach_network_shell_writes_or_subagents(self):
        agent = external_review.AGENT_CONFIG["agent"]["reviewer"]
        for tool in ("write", "edit", "bash", "webfetch", "websearch", "task", "skill"):
            self.assertIs(agent["tools"][tool], False, tool)
        for tool in (
            "edit",
            "bash",
            "webfetch",
            "websearch",
            "task",
            "skill",
            "external_directory",
        ):
            self.assertEqual(agent["permission"][tool], "deny", tool)


class FallbackTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        for name, value in (
            ("STATE_DIR", Path(tmp.name)),
            ("OPENCODE_LOG", Path(tmp.name) / "none.log"),
        ):
            patcher = mock.patch.object(external_review, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_chain(self, working):
        """`working` is the set of models whose preflight and review succeed."""
        calls = []

        def fake_preflight(model, snap):
            calls.append(("preflight", model))
            return model in working

        def fake_run(command, cwd, total, idle):
            model = command[command.index("-m") + 1]
            calls.append(("review", model))
            return 0, f"review by {model}\n\nVerdict: FIX FIRST"

        with (
            mock.patch.object(external_review, "preflight", fake_preflight),
            mock.patch.object(external_review, "run_with_idle_timeout", fake_run),
        ):
            result = external_review.review_with_fallback(
                external_review.THIRD_REVIEWER_CHAIN, Path("/snap"), "p", 10, 10
            )
        return result, calls

    def test_third_reviewer_order_is_minimax_deepseek_beam(self):
        self.assertEqual(
            external_review.THIRD_REVIEWER_CHAIN,
            (
                "opencode/minimax-m3.1-flash-free",
                "opencode/deepseek-v4.1-flash-free",
                "opencode/beam-free",
            ),
        )
        self.assertEqual(external_review.CHAINS["main"], ("openai/gpt-6.1-sol",))

    def test_first_working_model_is_used_and_later_ones_not_tried(self):
        bunny, deepseek, muse = external_review.THIRD_REVIEWER_CHAIN
        (model, output), calls = self.run_chain({bunny, deepseek})
        self.assertEqual(model, bunny)
        self.assertNotIn(("preflight", deepseek), calls)

    def test_falls_through_to_the_last_model(self):
        bunny, deepseek, muse = external_review.THIRD_REVIEWER_CHAIN
        (model, output), calls = self.run_chain({muse})
        self.assertEqual(model, muse)
        self.assertEqual(
            [c for c in calls if c[0] == "preflight"],
            [("preflight", bunny), ("preflight", deepseek), ("preflight", muse)],
        )

    def test_tool_trace_without_a_verdict_falls_through(self):
        """Regression (2026-10-22): GLM 5.3 Flash and Nemotron 3.5 Super VL exited 0 with only a
        tool trace, and DeepSeek with an unfinished summary; each was taken as a review."""
        first, second, _ = external_review.THIRD_REVIEWER_CHAIN

        def fake_run(command, cwd, total, idle):
            model = command[command.index("-m") + 1]
            if model == first:
                return 0, "\u2192 Read docs/plan.md\n\u2192 Read docs/decisions.md\n" * 40
            return 0, f"findings by {model}\n\n**Verdict: READY**"

        with (
            mock.patch.object(external_review, "preflight", lambda model, snap: True),
            mock.patch.object(external_review, "run_with_idle_timeout", fake_run),
        ):
            model, _ = external_review.review_with_fallback(
                external_review.THIRD_REVIEWER_CHAIN, Path("/snap"), "p", 10, 10
            )
        self.assertEqual(model, second)

    def test_classify_requires_a_verdict(self):
        self.assertEqual(external_review.classify(0, "x" * 5000), "incomplete")
        self.assertEqual(external_review.classify(0, "notes\n## Verdict\nGO"), "ok")
        self.assertEqual(external_review.classify(0, "Final verdict: FIX FIRST"), "ok")

    def test_no_model_available_returns_none(self):
        (model, log), _ = self.run_chain(set())
        self.assertIsNone(model)
        self.assertEqual(log.count("preflight failed"), 3)


class FreeTierTests(unittest.TestCase):
    """Free models have daily limits and are throttled when run in parallel (2026-10-22: GLM,
    Nemotron Super VL and MiMo opened a stream that never returned a byte)."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        patcher = mock.patch.object(external_review, "STATE_DIR", Path(self.tmp.name))
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_idle_kill_is_a_stall_not_a_plain_failure(self):
        self.assertEqual(
            external_review.classify(124, "\u2192 Read a\n[killed: idle timeout]"), "stalled"
        )
        self.assertEqual(
            external_review.classify(124, "partial\n[killed: total timeout]"), "unavailable"
        )

    def test_limit_error_in_the_opencode_log_is_a_quota_failure(self):
        log = Path(self.tmp.name) / "opencode.log"
        log.write_text(
            "timestamp=2026-10-22T06:00:00.000Z level=ERROR modelID=x-free "
            'error="AI_APICallError: Rate limit exceeded for today"\n'
            "timestamp=2026-10-22T07:00:00.000Z level=ERROR modelID=x-free "
            'error="AI_APICallError: 429 Too Many Requests"\n'
        )
        with mock.patch.object(external_review, "OPENCODE_LOG", log):
            lines = external_review.opencode_errors("x-free", "2026-10-22T06:30:00")
        self.assertEqual(len(lines), 1)
        self.assertIn("429", lines[0])
        self.assertTrue(external_review.UNAVAILABLE.search(lines[0]))

    def test_a_limited_model_is_skipped_for_the_rest_of_the_day(self):
        first, second, _ = external_review.THIRD_REVIEWER_CHAIN
        external_review.mark_limited(first, "stalled")
        tried = []

        def fake_run(command, cwd, total, idle):
            model = command[command.index("-m") + 1]
            tried.append(model)
            return 0, "ok\nVerdict: READY"

        with (
            mock.patch.object(external_review, "preflight", lambda model, snap: True),
            mock.patch.object(external_review, "run_with_idle_timeout", fake_run),
        ):
            model, _ = external_review.review_with_fallback(
                external_review.THIRD_REVIEWER_CHAIN, Path("/snap"), "p", 10, 10
            )
        self.assertEqual(model, second)
        self.assertEqual(tried, [second])

    def test_a_stalled_run_marks_the_model_limited(self):
        first, second, _ = external_review.THIRD_REVIEWER_CHAIN

        def fake_run(command, cwd, total, idle):
            model = command[command.index("-m") + 1]
            if model == first:
                return 124, "\u2192 Read a\n[killed: idle timeout]"
            return 0, "ok\nVerdict: READY"

        with (
            mock.patch.object(external_review, "preflight", lambda model, snap: True),
            mock.patch.object(external_review, "run_with_idle_timeout", fake_run),
            mock.patch.object(external_review, "opencode_errors", lambda model, since: []),
        ):
            external_review.review_with_fallback(
                external_review.THIRD_REVIEWER_CHAIN, Path("/snap"), "p", 10, 10
            )
        self.assertTrue(external_review.limited_today(first))
        self.assertFalse(external_review.limited_today(second))

    def test_a_failed_run_with_logged_stream_errors_marks_the_model_limited(self):
        """2026-10-22: MiniMax failed with three 'stream error' log lines, no limit wording."""
        first, second, _ = external_review.THIRD_REVIEWER_CHAIN

        def fake_run(command, cwd, total, idle):
            model = command[command.index("-m") + 1]
            return (1, "Error") if model == first else (0, "ok\nVerdict: READY")

        errors = ['level=ERROR message="stream error" modelID=minimax-m3.1-flash-free'] * 3
        with (
            mock.patch.object(external_review, "preflight", lambda model, snap: True),
            mock.patch.object(external_review, "run_with_idle_timeout", fake_run),
            mock.patch.object(external_review, "opencode_errors", lambda model, since: errors),
        ):
            model, _ = external_review.review_with_fallback(
                external_review.THIRD_REVIEWER_CHAIN, Path("/snap"), "p", 10, 10
            )
        self.assertEqual(model, second)
        self.assertTrue(external_review.limited_today(first))

    def test_free_runs_are_serialised_by_a_lock(self):
        held = external_review.free_model_lock("opencode/x-free")
        with held:
            other = os.open(Path(self.tmp.name) / "free.lock", os.O_RDWR)
            import fcntl

            with self.assertRaises(BlockingIOError):
                fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)
            os.close(other)
        self.assertIsNone(external_review.free_model_lock("openai/gpt-6.1-sol").__enter__())


if __name__ == "__main__":
    unittest.main()
