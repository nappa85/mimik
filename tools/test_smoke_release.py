"""CLI smoke rejects unrelated failures and keeps captured secret diagnostics private."""

import os
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

from smoke_release import (
    DEFINITION_CONFLICT,
    FormFields,
    corrected_evidence,
    editor_action,
    registry_cli,
    require_secret_free,
    smoke_invalid_flags,
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
                error=DEFINITION_CONFLICT,
            )

    def test_unrelated_failure_is_not_a_stale_ticket_rejection(self):
        for code, stderr in [
            (1, b"Error: Storage(Database(...))\n"),
            (2, b"error: unrecognized subcommand 'approve-definition'\n"),
            (-11, b""),
            (1, b"Error: Configuration\n"),
            (1, b"panic: Conflict\n"),
            (1, b""),
            (0, f"Error: {DEFINITION_CONFLICT}\n".encode()),
        ]:
            with self.subTest(code=code, stderr=stderr), self.assertRaises(AssertionError):
                self.invoke(code, stderr)

    def test_typed_conflict_is_accepted(self):
        self.assertEqual(self.invoke(1, f"Error: {DEFINITION_CONFLICT}\n".encode()), b"")


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
            self.invoke(stderr=b"Error: " + secret, error=DEFINITION_CONFLICT)
        self.assertNotIn(secret.decode(), str(caught.exception))

    def test_exact_static_invalid_diagnostics(self):
        for error in (
            "Owner bootstrap has already occurred.",
            "Credential is unavailable, expired or superseded.",
        ):
            with self.subTest(error=error):
                self.assertEqual(
                    self.invoke(stderr=f"Error: {error}\n".encode(), error=error, secrets=()),
                    b"",
                )
                for code, stdout, stderr in (
                    (2, b"", f"Error: {error}\n".encode()),
                    (1, b"unexpected", f"Error: {error}\n".encode()),
                    (1, b"", f"Error: {DEFINITION_CONFLICT}\n".encode()),
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
    def test_removed_or_invalid_flags_must_be_refused(self):
        accepted = subprocess.CompletedProcess([], 0, b"", b"")
        with patch("smoke_release.subprocess.run", return_value=accepted):
            with self.assertRaisesRegex(AssertionError, "Invalid CLI flags accessed database"):
                smoke_invalid_flags(Path("/fake/mimik"), Path("/fake/work"))
        refused = subprocess.CompletedProcess([], 2, b"", b"")
        with patch("smoke_release.subprocess.run", return_value=refused):
            smoke_invalid_flags(Path("/fake/mimik"), Path("/fake/work"))

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

    def test_a_crlf_split_across_two_feeds_is_one_newline(self):
        parser = FormFields()
        parser.feed('<form action="/s"><textarea name="a">\r')
        parser.feed('\nplain</textarea><textarea name="b">x\r')
        parser.feed("</textarea></form>")
        parser.close()
        self.assertEqual(parser.forms["/s"], {"a": "plain", "b": "x\n"})

    def test_literal_crlf_and_cr_are_normalised_before_the_initial_newline_rule(self):
        parser = FormFields()
        parser.feed(
            '<form action="/s"><textarea name="a">\r\nplain</textarea>'
            '<textarea name="b">\rplain</textarea><textarea name="c">\r\n\r\nkept\r\n</textarea>'
            '<textarea name="d">&#13;ref</textarea></form>'
        )
        self.assertEqual(
            parser.forms["/s"], {"a": "plain", "b": "plain", "c": "\nkept\n", "d": "\rref"}
        )


class FormFieldsLifecycleTests(unittest.TestCase):
    def test_reset_discards_a_held_back_cr(self):
        parser = FormFields()
        parser.feed('<form action="/s"><textarea name="a">x\r')
        parser.reset()
        self.assertEqual(parser.pending_cr, "", "reset() discards the held-back CR")
        parser.feed('<form action="/s"><textarea name="a">\nplain</textarea></form>')
        parser.close()
        self.assertEqual(parser.forms["/s"], {"a": "plain"})


class FormListTests(unittest.TestCase):
    def test_the_first_form_per_action_wins_and_every_form_is_kept(self):
        # A saved runtime sheet (assisted sheet slice 1.5a): unit forms, then two list forms
        # posting to the sheet itself.
        parser = FormFields()
        parser.feed(
            '<form action="/private-sheets/1/meta/name"><input name="value" value="N"></form>'
            '<form action="/private-sheets/1"><input name="list" value="1"></form>'
            '<form action="/private-sheets/1"><input name="list" value="2"></form>'
        )
        parser.close()
        self.assertEqual(parser.forms["/private-sheets/1"], {"list": "1"})
        self.assertEqual(parser.forms["/private-sheets/1/meta/name"], {"value": "N"})
        self.assertEqual(
            [action for action, _ in parser.form_list],
            ["/private-sheets/1/meta/name", "/private-sheets/1", "/private-sheets/1"],
        )
        self.assertEqual(parser.form_list[2][1], {"list": "2"})


class CorrectedEvidenceTests(unittest.TestCase):
    def test_the_search_check_requires_the_stored_leading_newline(self):
        self.assertTrue(
            corrected_evidence('<pre class="evidence-text">\n\nVerified fixture correction</pre>')
        )
        for lost in (
            '<pre class="evidence-text">\nVerified fixture correction</pre>',
            '<pre class="evidence-text">Verified fixture correction</pre>',
        ):
            self.assertFalse(corrected_evidence(lost), lost)


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


class EditorActionTests(unittest.TestCase):
    def test_the_header_sign_out_form_is_not_the_editor_form(self):
        parser = FormFields()
        parser.feed(
            '<header><form method="post" action="/auth/logout"><input name="csrf" value="x">'
            '</form></header><main><form method="post" action="/characters/1/edit">'
            '<input name="name" value="A"></form></main>'
        )
        self.assertEqual(editor_action(parser.forms), "/characters/1/edit")

    def test_two_page_forms_are_still_refused(self):
        with self.assertRaises(AssertionError):
            editor_action({"/a": {}, "/b": {}, "/auth/logout": {}})


if __name__ == "__main__":
    unittest.main()
