import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_agent_worktree as check  # noqa: E402

ENV = {
    "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@example.invalid",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@example.invalid",
}


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, env=ENV, check=True, capture_output=True)


class WorktreeCheckTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        base = Path(self.tmp.name).resolve()
        self.main = base / "main"
        self.main.mkdir()
        git(self.main, "init", "-q")
        (self.main / ".gitignore").write_text("/target/\n__pycache__/\n*.pdf\n")
        (self.main / "src.txt").write_text("source\n")
        git(self.main, "add", ".")
        git(self.main, "commit", "-q", "-m", "init")
        self.wt = base / "wt"
        git(self.main, "worktree", "add", "-q", str(self.wt))
        self.marker = base / "PWNED"

    def tearDown(self):
        self.tmp.cleanup()

    def run_check(self, *flags):
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            code = check.main(["--main", str(self.main), str(self.wt), *flags])
        return code, errors.getvalue()

    def test_a_clean_worktree_passes(self):
        (self.wt / "new.txt").write_text("agent work\n")
        self.assertEqual(self.run_check(), (0, ""))

    def test_a_nested_repository_is_refused_before_git_runs(self):
        sub = self.wt / "sub"
        sub.mkdir()
        git(sub, "init", "-q")
        git(sub, "config", "core.fsmonitor", f"touch {self.marker} #")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("nested VCS entry: sub/.git", errors)
        self.assertFalse(self.marker.exists())

    def test_a_nested_git_file_and_jj_directory_are_refused(self):
        (self.wt / "a").mkdir()
        (self.wt / "a" / ".git").write_text("gitdir: ../planted\n")
        (self.wt / ".jj").mkdir()
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("a/.git", errors)
        self.assertIn(".jj", errors)

    def test_a_retargeted_root_git_file_is_refused(self):
        (self.wt / ".git").write_text("gitdir: /tmp/planted\n")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn(".git points outside", errors)

    def test_links_and_fifos_are_refused(self):
        (self.wt / "link").symlink_to("../wt/.git")
        os.mkfifo(self.wt / "pipe")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("symbolic link: link", errors)
        self.assertIn("special file: pipe", errors)

    def test_ignored_paths_are_refused_and_cleaned(self):
        (self.wt / "target" / "debug").mkdir(parents=True)
        (self.wt / "target" / "debug" / "x").write_text("x")
        (self.wt / "tools" / "__pycache__").mkdir(parents=True)
        (self.wt / "tools" / "__pycache__" / "m.cpython-312.pyc").write_bytes(b"\0")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("ignored path (delete with --clean): target", errors)
        self.assertIn("tools/__pycache__", errors)
        code, _ = self.run_check("--clean")
        self.assertEqual(code, 0)
        self.assertFalse((self.wt / "target").exists())
        self.assertFalse((self.wt / "tools" / "__pycache__").exists())

    def test_a_staged_gitlink_is_refused(self):
        git(self.wt, "update-index", "--add", "--cacheinfo", f"160000,{'1' * 40},sub")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("gitlink in the index: sub", errors)

    def test_review_mode_refuses_material_a_snapshot_would_send(self):
        (self.wt / "data").mkdir()
        (self.wt / "nested" / "data").mkdir(parents=True)
        (self.wt / "nested" / "data" / "page.png").write_bytes(b"\0")
        (self.wt / "copy.sqlite3.bak").write_bytes(b"\0")
        (self.wt / "model.gguf").write_bytes(b"\0")
        (self.wt / ".env.example").write_text("KEY=dummy\n")
        code, errors = self.run_check("--review")
        self.assertEqual(code, 1)
        self.assertIn("nested/data/page.png", errors)
        self.assertIn("copy.sqlite3.bak", errors)
        self.assertIn("model.gguf", errors)
        self.assertNotIn(".env.example", errors)


if __name__ == "__main__":
    unittest.main()
