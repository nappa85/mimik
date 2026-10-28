import importlib.util
import sys
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "external_review", Path(__file__).with_name("external_review.py")
)
review = importlib.util.module_from_spec(SPEC)
sys.modules["external_review"] = review
SPEC.loader.exec_module(review)

LONG = "Finding text. " * 400


class Classify(unittest.TestCase):
    def test_a_review_that_states_changes_required_without_the_word_verdict_is_ok(self):
        text = "# Review\n\n**Changes required**\n\n### Major — Cancel races\n" + LONG
        self.assertEqual(review.classify(0, text), "ok")

    def test_a_review_that_approves_without_the_word_verdict_is_ok(self):
        self.assertEqual(review.classify(0, "Approve with minor.\n" + LONG), "ok")

    def test_a_tool_trace_without_a_verdict_is_incomplete(self):
        self.assertEqual(review.classify(0, "→ Read src/main.rs\n→ Read src/lib.rs\n"), "incomplete")


class Fallback(unittest.TestCase):
    def test_an_incomplete_output_is_kept_for_the_caller(self):
        review.preflight = lambda model, snap: True
        review.run_with_idle_timeout = lambda command, snap, total, idle: (0, "→ Read a.rs\n")
        review.opencode_command = lambda model, snap, prompt: []
        kept = []
        model, _ = review.review_with_fallback(["paid/x"], Path("."), "p", 1, 1, kept)
        self.assertIsNone(model)
        self.assertEqual(kept, [("paid/x", "→ Read a.rs\n")])


if __name__ == "__main__":
    unittest.main()
