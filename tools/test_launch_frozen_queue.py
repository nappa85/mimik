"""tools/launch_frozen_queue.sh (second review round N1): frozen detached worktree.

Failing-first: run before the script existed. Only --print is exercised (nothing starts).
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
SCRIPT = "launch_frozen_queue.sh"


@unittest.skipUnless(shutil.which("git"), "needs git")
class FrozenLaunchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "main"
        (self.repo / "tools" / "queues").mkdir(parents=True)
        shutil.copy2(TOOLS / SCRIPT, self.repo / "tools" / SCRIPT)
        (self.repo / "tools" / "run_queue.py").write_text("# stand-in\n")
        (self.repo / "tools" / "queues" / "a.json").write_text("{}\n")
        (self.repo / "tools" / "queues" / "b.json").write_text("{}\n")
        self.git("init", "-q")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "t")
        self.head = self.git("rev-parse", "HEAD").strip()
        self.base = self.tmp / "wts"
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text(
            '#!/bin/sh\n[ "$1 $2" = "context show" ] && echo "${FAKE_CONTEXT:-default}"\n'
        )
        docker.chmod(0o755)

    def git(self, *args, cwd=None):
        cmd = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
        cmd += ["-C", str(cwd or self.repo), *args]
        return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout

    def launch(self, *args, **env):
        base = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.tmp),
            "FROZEN_BASE": str(self.base),
        }
        base.update(env)
        return subprocess.run(
            ["bash", str(self.repo / "tools" / SCRIPT), *args],
            env=base,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_print_creates_a_detached_worktree_at_the_commit(self):
        proc = self.launch("--print", "HEAD", "tools/queues/a.json", "tools/queues/b.json")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        wt = self.base / f"run-{self.head[:12]}"
        self.assertEqual(self.git("rev-parse", "HEAD", cwd=wt).strip(), self.head)
        out = proc.stdout
        self.assertIn(f"frozen_worktree={wt} commit={self.head}", out)
        self.assertIn(f"{wt}/tools/run_queue.py run {wt}/tools/queues/a.json;", out)
        self.assertLess(out.index("a.json"), out.index("b.json"))
        # The worktree shares the main checkout's git dir, so the locks stay shared.
        common = self.git("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=wt)
        self.assertEqual(Path(common.strip()).parent, self.repo.resolve())
        again = self.launch("--print", self.head, "tools/queues/a.json")
        self.assertEqual(again.returncode, 0, again.stderr)

    def test_session_end_passed_to_every_queue(self):
        proc = self.launch(
            "--print",
            "--session-end",
            "2099-10-18 22:00",
            "HEAD",
            "tools/queues/a.json",
            "tools/queues/b.json",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.count("--session-end 2099-10-18\\ 22:00"), 2, proc.stdout)
        for bad in ("Sunday", "2099-13-40 22:00", "2001-01-01 00:00"):
            with self.subTest(bad=bad):
                proc = self.launch("--print", "--session-end", bad, "HEAD", "tools/queues/a.json")
                self.assertEqual(proc.returncode, 2, proc.stdout)

    def test_round_three_refusals(self):
        cases = {
            "model root": {"MIMIK_MODEL_ROOT": "/home/x"},
            "docker config": {"DOCKER_CONFIG": "/tmp/c"},
            "context": {"FAKE_CONTEXT": "remote"},
        }
        for name, env in cases.items():
            with self.subTest(name=name):
                proc = self.launch("--print", "HEAD", "tools/queues/a.json", **env)
                self.assertEqual(proc.returncode, 2, proc.stdout)
                self.assertIn("refusing", proc.stderr)

    def test_refuses_while_a_queue_or_watchdog_runs(self):
        fake = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)", "run_watchdog.py"]
        )
        self.addCleanup(fake.kill)
        proc = self.launch("--print", "HEAD", "tools/queues/a.json")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("already running", proc.stderr)

    def test_prunes_a_stale_worktree_entry(self):
        self.assertEqual(self.launch("--print", "HEAD", "tools/queues/a.json").returncode, 0)
        shutil.rmtree(self.base / f"run-{self.head[:12]}")  # deleted, still registered
        proc = self.launch("--print", "HEAD", "tools/queues/a.json")
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_refusals(self):
        self.assertEqual(self.launch("--print", "HEAD", "tools/queues/a.json").returncode, 0)
        wt = self.base / f"run-{self.head[:12]}"
        (wt / "tools" / "run_queue.py").write_text("# edited\n")
        cases = {
            "dirty frozen copy": (["HEAD", "tools/queues/a.json"], {}),
            "missing queue": (["HEAD", "tools/queues/nope.json"], {}),
            "absolute queue": (["HEAD", "/etc/passwd"], {}),
            "unknown commit": (["no-such-rev", "tools/queues/a.json"], {}),
            "endpoint": (["HEAD", "tools/queues/a.json"], {"DOCKER_HOST": "tcp://x"}),
        }
        for name, (args, env) in cases.items():
            with self.subTest(name=name):
                proc = self.launch("--print", *args, **env)
                self.assertNotEqual(proc.returncode, 0, proc.stdout)


if __name__ == "__main__":
    unittest.main()
