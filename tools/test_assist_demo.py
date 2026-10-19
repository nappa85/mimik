"""Pure helpers of the assisted-sheet demo launcher (prototype tier)."""

import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import assist_demo


class FreshDirectoryTests(unittest.TestCase):
    def test_creates_missing_and_accepts_empty(self):
        with tempfile.TemporaryDirectory() as temp:
            created = assist_demo.fresh_directory(Path(temp) / "new" / "data")
            self.assertTrue(created.is_dir())
            self.assertEqual(assist_demo.fresh_directory(created), created)

    def test_refuses_used_directory_or_file(self):
        with tempfile.TemporaryDirectory() as temp:
            used = Path(temp) / "used"
            used.mkdir()
            (used / "smoke.sqlite3").write_bytes(b"")
            with self.assertRaises(AssertionError):
                assist_demo.fresh_directory(used)
            with self.assertRaises(AssertionError):
                assist_demo.fresh_directory(used / "smoke.sqlite3")

    def test_data_directory_is_private(self):
        with tempfile.TemporaryDirectory() as temp:
            created = assist_demo.fresh_directory(Path(temp) / "data")
            self.assertEqual(stat.S_IMODE(created.stat().st_mode), 0o700)
            empty = Path(temp) / "empty"
            empty.mkdir(mode=0o755)
            assist_demo.fresh_directory(empty)
            self.assertEqual(stat.S_IMODE(empty.stat().st_mode), 0o700)


class AccountInputTests(unittest.TestCase):
    """Password and invitation files exist only while the CLI reads them."""

    def run_accounts(self, fail_at=None):
        calls = []

        def cli(binary, directory, *arguments, secrets=None):
            private = directory / "account-inputs"
            calls.append(arguments[0])
            self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o700)
            for flag in ("--password-file", "--token-file"):
                if flag in arguments:
                    path = Path(arguments[arguments.index(flag) + 1])
                    self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            if "--token-output" in arguments:
                assist_demo.write_private(
                    Path(arguments[arguments.index("--token-output") + 1]), b"t" * 64
                )
            if arguments[0] == fail_at:
                raise AssertionError("synthetic CLI failure")
            return b"{}"

        with tempfile.TemporaryDirectory() as temp:
            directory = assist_demo.fresh_directory(Path(temp) / "data")
            with patch.object(assist_demo, "registry_cli", cli):
                try:
                    assist_demo.create_accounts(
                        Path("mimik"), directory, assist_demo.demo_passwords()
                    )
                except AssertionError:
                    pass
            remaining = sorted(path.name for path in directory.iterdir())
        return calls, remaining

    def test_inputs_removed_after_setup(self):
        calls, remaining = self.run_accounts()
        self.assertEqual(calls, ["bootstrap-owner", "invite-account", "enroll-account"])
        self.assertEqual(remaining, [])

    def test_inputs_removed_after_a_failure(self):
        calls, remaining = self.run_accounts(fail_at="invite-account")
        self.assertEqual(calls, ["bootstrap-owner", "invite-account"])
        self.assertEqual(remaining, [])


class PrivateFileTests(unittest.TestCase):
    def test_private_and_never_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "p.password"
            assist_demo.write_private(path, b"dummy")
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            with self.assertRaises(FileExistsError):
                assist_demo.write_private(path, b"other")
            self.assertEqual(path.read_bytes(), b"dummy")

    def test_passwords_are_generated_per_run(self):
        first, second = assist_demo.demo_passwords(), assist_demo.demo_passwords()
        self.assertEqual(set(first), {assist_demo.OWNER, assist_demo.PLAYER})
        self.assertNotEqual(first, second)
        self.assertNotEqual(first[assist_demo.OWNER], first[assist_demo.PLAYER])


class SheetFormTests(unittest.TestCase):
    def rendered(self, rows):
        form = {"csrf": "c" * 64, "revision": "0", "name": "", "notes": "", "op": "add-4"}
        for r in range(rows):
            form[f"id-4-{r}"] = ""
        return form

    def test_fills_demo_values_and_rows(self):
        values = assist_demo.sheet_form(self.rendered(2))
        self.assertEqual(
            (values["value-hp"], values["value-max_hp"], values["value-armour"]),
            ("84", "84", "-1"),
        )
        self.assertEqual((values["null-luck"], values["value-luck"]), ("known", "3"))
        self.assertEqual(values["row-4-1-0"], "Torch")
        self.assertEqual(values["set-4-0-3"], "absent")
        self.assertNotIn("op", values)
        self.assertEqual(values["csrf"], "c" * 64)

    def test_requires_the_added_rows(self):
        with self.assertRaises(AssertionError):
            assist_demo.sheet_form(self.rendered(1))


if __name__ == "__main__":
    unittest.main()
