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


if __name__ == "__main__":
    unittest.main()
