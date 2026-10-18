"""The catalogue profile draft must sit at the plan's maxima without exceeding any limit."""

import json
import unittest

from catalogue_profile import (
    MAX_DRAFT_BYTES,
    MAX_UNIT_REFERENCES,
    OWNER_TOTAL,
    draft,
    encoded,
    fanout,
    hold_bounds,
    lock_intervals,
)


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
            references = sum(
                len(entry["description"].get("units", [])) for entry in last["entries"]
            )
            self.assertLessEqual(references, MAX_UNIT_REFERENCES)
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

    def test_fanout_sits_at_the_reference_property_and_byte_caps(self):
        first, second = fanout(8192, 0), fanout(8192, 1)
        for value in (first, second):
            self.assertEqual(len(value["entries"]), 8192)
            self.assertLessEqual(len(encoded(value)), MAX_DRAFT_BYTES)
            self.assertGreater(len(encoded(value)), MAX_DRAFT_BYTES - 64 * 1024)
            references = [
                (unit["page_id"], unit["unit_id"])
                for entry in value["entries"]
                for unit in entry["description"].get("units", [])
            ]
            self.assertEqual(len(references), MAX_UNIT_REFERENCES)
            self.assertEqual(len(set(references)), 16_384)
            self.assertTrue(all((unit - 1) // 16 + 1 == page for page, unit in references))
            self.assertTrue(all(len(t["properties"]) == 16 for t in value["entry_types"]))
            owner = sum(
                len(entry["description"].get("owner_text", "").encode())
                for entry in value["entries"]
            )
            self.assertLessEqual(owner, OWNER_TOTAL)
        # The update keeps every key and type and changes content.
        self.assertEqual(
            [(e["key"], e["type"]) for e in first["entries"]],
            [(e["key"], e["type"]) for e in second["entries"]],
        )
        self.assertNotEqual(encoded(first), encoded(second))


class LockProbeTests(unittest.TestCase):
    def test_intervals_bound_acquisition_and_release(self):
        polls = [(0.0, True), (1.0, True), (2.0, False), (3.0, False), (4.0, True), (5.0, False)]
        intervals = lock_intervals(polls)
        self.assertEqual(intervals, [(1.0, 2.0, 3.0, 4.0), (4.0, 5.0, 5.0, None)])
        bounds = hold_bounds(intervals[0], 0.5)
        self.assertEqual(bounds["acquired_after_ms"], [500.0, 1500.0])
        self.assertEqual(bounds["released_after_ms"], [2500.0, 3500.0])
        self.assertEqual((bounds["hold_ms_lower"], bounds["hold_ms_upper"]), (1000.0, 3000.0))
        self.assertIsNone(hold_bounds(intervals[1], 0.0)["hold_ms_upper"])

    def test_polls_after_the_contender_acquires_are_not_attributed(self):
        polls = [(0.0, True), (1.0, False), (2.0, False), (3.0, False), (4.0, True)]
        self.assertEqual(lock_intervals(polls, cutoff=2.5), [(0.0, 1.0, 2.0, 2.5)])


if __name__ == "__main__":
    unittest.main()
