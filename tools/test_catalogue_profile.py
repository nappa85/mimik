"""The catalogue profile draft must sit at the plan's maxima without exceeding any limit."""

import json
import unittest

from catalogue_profile import MAX_DRAFT_BYTES, OWNER_TOTAL, draft, encoded


class DraftShapeTests(unittest.TestCase):
    def test_capacity_drafts_fit_every_limit_and_grow_append_only(self):
        for capacity in (4096, 8192):
            versions = [draft(capacity - 2, extra, capacity=capacity) for extra in (0, 1, 2)]
            last = versions[-1]
            self.assertEqual(len(last["entries"]), capacity)
            self.assertLessEqual(len(encoded(last)), MAX_DRAFT_BYTES)
            owner = sum(
                len(entry["description"]["owner_text"].encode())
                for entry in last["entries"]
                if "owner_text" in entry["description"]
            )
            self.assertLessEqual(owner, OWNER_TOTAL)
            keys = [entry["key"] for entry in last["entries"]]
            self.assertEqual(len(keys), len(set(keys)))
            units = {
                unit["unit_id"]
                for entry in last["entries"]
                for unit in entry["description"].get("units", [])
            }
            self.assertLessEqual(len(units), 16_384)
            # Each version keeps every earlier key with the same type.
            for older, newer in zip(versions, versions[1:], strict=False):
                kept = {e["key"]: e["type"] for e in newer["entries"]}
                for entry in older["entries"]:
                    self.assertEqual(kept[entry["key"]], entry["type"])

    def test_drafts_are_deterministic(self):
        self.assertEqual(json.dumps(draft(64)), json.dumps(draft(64)))

    def test_oversized_drafts_are_refused_before_any_run(self):
        value = draft(8)
        value["title"] = "x" * MAX_DRAFT_BYTES
        with self.assertRaises(AssertionError):
            encoded(value)


if __name__ == "__main__":
    unittest.main()
