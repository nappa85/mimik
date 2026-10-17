"""CLI smoke rejects unrelated failures and keeps captured secret diagnostics private."""

import os
import subprocess
import unittest
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import MagicMock, patch

from smoke_release import (
    FormFields,
    registry_cli,
    require_secret_free,
    smoke_owner_development,
    version_link,
)


class PublicationFailureTests(unittest.TestCase):
    def invoke(self, code, stderr):
        result = subprocess.CompletedProcess([], code, stdout=b"", stderr=stderr)
        with patch("smoke_release.subprocess.run", return_value=result):
            return registry_cli(
                Path("/fake/mimik"),
                Path("/fake/work"),
                "approve-definition",
                success=False,
                error="Conflict",
            )

    def test_unrelated_failure_is_not_a_stale_ticket_rejection(self):
        for code, stderr in [
            (1, b"Error: Storage(Database(...))\n"),
            (2, b"error: unrecognized subcommand 'approve-definition'\n"),
            (-11, b""),
            (1, b"Error: Configuration\n"),
            (1, b"panic: Conflict\n"),
            (1, b""),
            (0, b"Error: Conflict\n"),
        ]:
            with self.subTest(code=code, stderr=stderr), self.assertRaises(AssertionError):
                self.invoke(code, stderr)

    def test_typed_conflict_is_accepted(self):
        self.assertEqual(self.invoke(1, b"Error: Conflict\n"), b"")


class AccountFailureTests(unittest.TestCase):
    def test_account_subprocess_environment_excludes_credentials(self):
        result = subprocess.CompletedProcess([], 0, stdout=b"{}", stderr=b"")
        with patch.dict(
            os.environ,
            {
                "API_KEY": "dummy",
                "access_token": "dummy",
                "DATABASE_PASSWORD": "dummy",
                "CLIENT_SECRET": "dummy",
                "AUTH_CREDENTIAL": "dummy",
                "LD_PRELOAD": "/fake/library",
                "MIMIK_DATABASE": "/fake/unrelated.sqlite3",
                "PATH": "/usr/bin",
            },
            clear=True,
        ):
            with patch("smoke_release.subprocess.run", return_value=result) as run:
                registry_cli(Path("/fake/mimik"), Path("/fake/work"), "list-accounts", secrets=())
                env = run.call_args.kwargs.get("env", dict(os.environ))
                self.assertNotIn("API_KEY", env)
                self.assertNotIn("access_token", env)
                for name in (
                    "DATABASE_PASSWORD",
                    "CLIENT_SECRET",
                    "AUTH_CREDENTIAL",
                    "LD_PRELOAD",
                ):
                    self.assertNotIn(name, env)
                self.assertEqual(env["PATH"], "/usr/bin")
                self.assertEqual(env["MIMIK_DATABASE"], "/fake/work/smoke.sqlite3")

    def invoke(self, code=1, stdout=b"", stderr=b"", error=None, secrets=None):
        result = subprocess.CompletedProcess([], code, stdout=stdout, stderr=stderr)
        with patch("smoke_release.subprocess.run", return_value=result) as run:
            output = registry_cli(
                Path("/fake/mimik"),
                Path("/fake/work"),
                "enroll-account",
                success=error is None,
                error=error,
                secrets=secrets,
            )
            self.assertEqual(run.call_args.kwargs["timeout"], 30)
            return output

    def test_unexpected_diagnostics_are_redacted_even_without_secret_guard(self):
        secret = b"synthetic unexpected secret"
        with self.assertRaises(AssertionError) as caught:
            self.invoke(stderr=b"Error: " + secret, error="Conflict")
        self.assertNotIn(secret.decode(), str(caught.exception))

    def test_exact_static_invalid_diagnostics(self):
        for error in (
            'Invalid("Owner bootstrap has already occurred.")',
            'Invalid("Credential is unavailable, expired or superseded.")',
        ):
            with self.subTest(error=error):
                self.assertEqual(
                    self.invoke(stderr=f"Error: {error}\n".encode(), error=error, secrets=()),
                    b"",
                )
                for code, stdout, stderr in (
                    (2, b"", f"Error: {error}\n".encode()),
                    (1, b"unexpected", f"Error: {error}\n".encode()),
                    (1, b"", b"Error: Conflict\n"),
                    (1, b"", b"Error: Storage(Database(...))\n"),
                    (-11, b"", b""),
                ):
                    with self.subTest(code=code, stdout=stdout, stderr=stderr):
                        with self.assertRaises(AssertionError):
                            self.invoke(code, stdout, stderr, error=error, secrets=())

    def test_secret_output_rejected_on_success_and_failure(self):
        password = b"Dummy password for helper checks!"
        for secret in (password, b"abcdef0123456789" * 4, b"$argon2id$v=19$m=19456,t=2,p=1"):
            for code, stdout, stderr, error in (
                (0, secret, b"", None),
                (0, b"{}", secret, None),
                (1, b"", secret, "Conflict"),
            ):
                with self.subTest(code=code, secret_kind=len(secret)):
                    with self.assertRaises(AssertionError) as caught:
                        self.invoke(code, stdout, stderr, error=error, secrets=(password,))
                    self.assertNotIn(secret.decode(), str(caught.exception))

    def test_success_with_unexpected_stderr_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.invoke(0, b"{}", b"warning: unexpected\n", secrets=())

    def test_nonsecret_projection_is_accepted(self):
        output = b'{"id":1,"login":"synthetic","epoch":2,"owner":false}'
        require_secret_free(output, (b"Dummy password for helper checks!",))
        self.assertEqual(self.invoke(0, output, secrets=()), output)


class WorkspaceFormTests(unittest.TestCase):
    def test_development_preservation_cannot_pass_with_empty_history(self):
        db = MagicMock()
        db.execute.return_value.fetchone.return_value = ("synthetic snapshot",)
        db.execute.return_value.fetchall.return_value = []
        connection = MagicMock()
        connection.__enter__.return_value = db
        with ExitStack() as stack:
            for name, value in {
                "subprocess.run": subprocess.CompletedProcess([], 2, b"", b""),
                "start": (None, None),
                "stop": None,
                "auth_request": (200, {}, "Development"),
                "auth_cookie": "dummy-cookie",
                "auth_fields": {},
                "auth_login": "dummy-cookie",
                "workspace_form": ("/dummy", {}, ""),
                "workspace_submit": "/private-sheets/1",
                "auth_error": None,
                "sqlite3.connect": connection,
            }.items():
                stack.enter_context(patch(f"smoke_release.{name}", return_value=value))
            with self.assertRaisesRegex(
                AssertionError, "Development sheet/history baseline missing"
            ):
                smoke_owner_development(Path("/fake/mimik"), Path("/fake/work"), (b"dummy",))

    def test_select_values_preserve_explicit_false_unknown_and_enum(self):
        parser = FormFields()
        parser.feed(
            '<form action="/private-sheets/1">'
            '<select name="null-flag"><option value="known">Known</option>'
            '<option value="unknown" selected>Unknown</option></select>'
            '<select name="value-flag"><option value="true">True</option>'
            '<option value="false" selected>False</option></select>'
            '<select name="value-choice"><option value="a">A</option>'
            '<option value="b" selected>B</option></select></form>'
        )
        self.assertEqual(
            parser.forms["/private-sheets/1"],
            {"null-flag": "unknown", "value-flag": "false", "value-choice": "b"},
        )


class TextareaNewlineTests(unittest.TestCase):
    def test_one_leading_newline_after_the_textarea_tag_is_dropped_like_a_browser(self):
        parser = FormFields()
        parser.feed(
            '<form action="/s"><textarea name="a">\nplain</textarea>'
            '<textarea name="b">\n\nkept</textarea><textarea name="c">x</textarea></form>'
        )
        self.assertEqual(parser.forms["/s"], {"a": "plain", "b": "\nkept", "c": "x"})
        self.assertEqual(parser.values["b"], "\nkept")


class VersionSelectionTests(unittest.TestCase):
    PAGE = (
        '<a href="/campaigns/4/sheets/new?version=1">Generic v1</a>'
        '<a href="/campaigns/4/sheets/new?version=2">Generic v2</a>'
        '<a href="/campaigns/4/sheets/new?version=3">List definition</a>'
    )

    def test_create_link_is_chosen_by_version_id_not_position(self):
        self.assertEqual(
            version_link(self.PAGE, "/campaigns/4", 2), "/campaigns/4/sheets/new?version=2"
        )
        self.assertEqual(
            version_link(self.PAGE, "/campaigns/4", 3), "/campaigns/4/sheets/new?version=3"
        )

    def test_missing_or_prefix_version_is_refused(self):
        for version in (4, 0):
            with self.assertRaisesRegex(AssertionError, "Published runtime version"):
                version_link(self.PAGE, "/campaigns/4", version)
        with self.assertRaisesRegex(AssertionError, "Published runtime version"):
            version_link('<a href="/campaigns/4/sheets/new?version=21">', "/campaigns/4", 2)


if __name__ == "__main__":
    unittest.main()
