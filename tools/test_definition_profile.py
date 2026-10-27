"""Profile outcomes must identify the intended validation failure, not any failed process."""

import contextlib
import io
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from definition_profile import measured


class RejectionMeasurementTests(unittest.TestCase):
    def invoke(self, code, stderr):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "sample.json"
            source.write_text("{}", encoding="utf-8")
            output = io.StringIO()
            self.last_output = output
            result = subprocess.CompletedProcess([], code, stdout=b"", stderr=stderr)
            with (
                patch("definition_profile.subprocess.run", return_value=result),
                contextlib.redirect_stdout(output),
            ):
                measured(Path("/fake/mimik"), Path(directory) / "db", source, False)
            return output.getvalue()

    def test_unrelated_nonzero_and_signal_exits_cannot_claim_validation_rejection(self):
        for code, stderr in [
            (1, b"Error: Storage(Database(...))\n"),
            (1, b"Error: Configuration\n"),
            (1, b"panic: unrelated failure\n"),
            (-11, b""),
            (2, b"Usage: bad argument\n"),
            (1, b""),
        ]:
            with self.subTest(code=code, stderr=stderr), self.assertRaises(AssertionError):
                self.invoke(code, stderr)
            self.assertEqual(self.last_output.getvalue(), "")

    def test_wrong_or_disconnected_diagnostic_cannot_claim_nullable_type_rejection(self):
        for diagnostics in [
            b"/fields/1/nullable: type; ",
            b"/fields/0/nullable: required; ",
            b"/fields/0/nullable: required; /fields/1/nullable: type; ",
        ]:
            stderr = b"Error: Definition rejected: " + diagnostics + b"\n"
            with self.subTest(diagnostics=diagnostics), self.assertRaises(AssertionError):
                self.invoke(1, stderr)
            self.assertEqual(self.last_output.getvalue(), "")

    def test_expected_top_level_report_is_measured(self):
        stderr = b"Error: Definition rejected: /fields/0/nullable: type; \n"
        self.assertIn('"accepted": false', self.invoke(1, stderr))


if __name__ == "__main__":
    unittest.main()
