import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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

    def test_an_ignored_path_that_cannot_be_deleted_is_a_clean_refusal(self):
        cache = self.wt / "tools" / "__pycache__"
        cache.mkdir(parents=True)
        (cache / "m.pyc").write_bytes(b"\0")

        def refuse(path, *args, **kwargs):
            raise PermissionError(13, "Permission denied", str(path))

        # Patched rather than chmod: CI runs the helper tests as root, which ignores modes.
        with mock.patch.object(check.shutil, "rmtree", refuse):
            code, errors = self.run_check("--clean")
        self.assertEqual(code, 1)
        self.assertIn("cannot delete ignored path tools/__pycache__", errors)
        self.assertNotIn("Traceback", errors)

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

    def test_an_unreadable_directory_is_refused_rather_than_skipped(self):
        hidden = self.wt / "x"
        (hidden / "sub").mkdir(parents=True)
        git(hidden / "sub", "init", "-q")
        hidden.chmod(0)
        try:
            code, errors = self.run_check()
        finally:
            hidden.chmod(0o755)
        self.assertEqual(code, 1)
        self.assertIn("x", errors)
        self.assertTrue("cannot list x" in errors or "not readable by its owner: x" in errors)

    def test_a_bare_repository_is_refused(self):
        git(self.wt, "init", "-q", "--bare", "planted")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("bare repository: planted", errors)

    def test_a_repository_split_through_commondir_is_refused(self):
        (self.wt / "sub").mkdir()
        (self.wt / "sub" / "HEAD").write_text("ref: refs/heads/main\n")
        (self.wt / "sub" / "commondir").write_text("../store\n")
        for part in ("objects", "refs"):
            (self.wt / "store" / part).mkdir(parents=True)
        (self.wt / "store" / "config").write_text(f"[core]\n\tpager = touch {self.marker}\n")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("bare repository: sub", errors)
        self.assertIn("bare repository: store", errors)

    def test_case_variants_of_vcs_names_are_refused(self):
        (self.wt / "a").mkdir()
        (self.wt / "a" / ".GIT").mkdir()
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("nested VCS entry: a/.GIT", errors)

    def test_an_added_or_changed_gitignore_is_refused(self):
        (self.wt / "tools" / "__pycache__").mkdir(parents=True)
        (self.wt / "tools" / "__pycache__" / "m.pyc").write_bytes(b"\0")
        (self.wt / "tools" / ".gitignore").write_text("!__pycache__/\n")
        code, errors = self.run_check("--clean")
        self.assertEqual(code, 1)
        self.assertIn("ignore rules changed by the run: tools/.gitignore", errors)
        (self.wt / "tools" / ".gitignore").unlink()
        (self.wt / ".gitignore").write_text("*\n")
        code, errors = self.run_check("--clean")
        self.assertEqual(code, 1)
        self.assertIn("ignore rules changed by the run: .gitignore", errors)
        self.assertTrue((self.wt / "src.txt").exists())

    def test_a_damaged_git_file_is_a_clean_refusal(self):
        (self.wt / ".git").write_text(f"gitdir: {self.main}/.git/worktrees/..\n")
        code, errors = self.run_check()
        self.assertEqual(code, 1)
        self.assertIn("refused: .git", errors)
        self.assertNotIn("Traceback", errors)

    def test_the_main_checkout_takes_review_only(self):
        (self.main / "manual.pdf").write_bytes(b"\0")
        git(self.main, "add", "-f", "manual.pdf")
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            code = check.main(["--main", str(self.main), str(self.main), "--review"])
        self.assertEqual(code, 1)
        self.assertIn("manual.pdf", errors.getvalue())
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            check.main(["--main", str(self.main), str(self.main), "--clean"])


if __name__ == "__main__":
    unittest.main()
