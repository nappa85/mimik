"""Unit tests of the fake model in tools/assist_model_browser.py (no browser, no server)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import assist_model_browser as browser  # noqa: E402


class ReminderReplyTests(unittest.TestCase):
    def test_the_reminder_prime_without_fields_gets_a_valid_reply(self):
        # Mimik primes the reminder prefix before Ready with a decrease and no fields (3c-1c);
        # the fake model raised StopIteration, so the prime failed and reminders stayed off.
        envelope = {
            "decrease": {"id": "hp", "label": "Hit points", "before": 2, "after": 1, "phrase": ""},
            "fields": [],
            "rows": [],
        }
        reply = browser.reminder_reply(envelope)
        self.assertEqual(reply, {"relevant": False, "row": None, "hint": "-", "option": None})

    def test_a_decrease_with_another_integer_field_gets_the_two_changes(self):
        envelope = {
            "decrease": {"id": "hp", "label": "HP", "before": 9, "after": 5, "phrase": "lost 4"},
            "fields": [{"id": "gold", "label": "Gold", "type": "integer", "value": 3}],
            "rows": [],
        }
        reply = browser.reminder_reply(envelope)
        self.assertTrue(reply["relevant"])
        self.assertEqual([c["ref"] for c in reply["option"]["changes"]], ["max_hp", "gold"])


class StreamUsageTests(unittest.TestCase):
    def test_a_streamed_reply_reports_its_system_message_as_cached(self):
        # Mimik's cache check after a reminder needs a cached-token figure of at least 75 % of
        # the system message's tokens (as the fake's /tokenize counts them); without one it
        # switched reminders off after the first reminder.
        system = "s" * 3000
        usage = browser.stream_usage(system)
        cached = usage["prompt_tokens_details"]["cached_tokens"]
        self.assertGreaterEqual(cached, 0.75 * (len(system) // 3 + 1))


if __name__ == "__main__":
    unittest.main()
