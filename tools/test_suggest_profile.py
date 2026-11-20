"""The suggestion profile's synthetic draft stays within the catalogue limits and its helpers
compute what they claim (prototype tier self-test)."""

import re
import unittest
from pathlib import Path

import suggest_profile as profile

ROOT = Path(__file__).resolve().parent.parent


class DraftTests(unittest.TestCase):
    def setUp(self):
        self.draft = profile.draft()

    def test_it_sits_at_the_entry_cap_the_rust_decoder_declares(self):
        text = (ROOT / "src" / "catalogues" / "decode.rs").read_text()
        cap = int(
            re.search(r"const MAX_ENTRIES: usize = ([0-9_]+);", text).group(1).replace("_", "")
        )
        self.assertEqual(len(self.draft["entries"]), cap)
        self.assertEqual(profile.ENTRIES, cap)

    def test_keys_names_and_types_are_admissible(self):
        keys = [entry["key"] for entry in self.draft["entries"]]
        self.assertEqual(len(set(keys)), len(keys))
        for entry in self.draft["entries"]:
            self.assertRegex(entry["key"], r"^[a-z][a-z0-9-]{0,63}$")
            self.assertTrue(1 <= len(entry["name"]) <= profile.MAX_NAME, entry["name"])
            self.assertIn(entry["type"], profile.TYPES)
        self.assertLessEqual(len(profile.TYPES), 32)
        self.assertLess(len(profile.encoded(self.draft)), 4 * 1024 * 1024)

    def test_names_mix_scripts_share_prefixes_run_long_and_repeat(self):
        names = [entry["name"] for entry in self.draft["entries"]]
        self.assertTrue(any(ord(ch) > 0x3000 for text in names for ch in text), "CJK")
        self.assertTrue(any("Ѐ" <= ch <= "ӿ" for text in names for ch in text))
        self.assertGreater(sum(text.startswith("La") for text in names), 1000)
        self.assertGreater(max(len(text) for text in names), 90)
        self.assertGreater(len(names) - len(set(names)), 50, "duplicates exist")
        self.assertGreater(len({entry["type"] for entry in self.draft["entries"]}), 3)


class MixTests(unittest.TestCase):
    def test_queries_respect_the_route_bounds(self):
        for mix, query in profile.QUERIES.items():
            scalars = len(query["q"])
            self.assertTrue(2 <= scalars <= 64, mix)
            if query.get("typo"):
                self.assertGreaterEqual(scalars, 3)
        self.assertEqual(profile.QUERIES["typo"]["typo"], "1")

    def test_many_norms_fall_within_two_scalars_of_the_typo_query(self):
        query = len(profile.QUERIES["typo"]["q"])
        near = [
            entry
            for entry in profile.draft()["entries"]
            if abs(len(" ".join(entry["name"].lower().split())) - query) <= 2
        ]
        self.assertGreater(len(near), 1000)

    def test_the_typo_query_matches_no_name_so_every_comparison_runs(self):
        names = [entry["name"].lower() for entry in profile.draft()["entries"]]
        self.assertFalse(any(profile.QUERIES["typo"]["q"] in text for text in names))


class StatisticsTests(unittest.TestCase):
    def test_nearest_rank_percentiles(self):
        samples = list(range(1, 101))
        self.assertEqual(profile.percentile(samples, 0.95), 95)
        self.assertEqual(profile.percentile(samples, 0.5), 50)
        self.assertEqual(profile.percentile(samples, 1), 100)
        self.assertEqual(profile.percentile([7], 0.95), 7)
        with self.assertRaises(ValueError):
            profile.percentile([], 0.5)
        with self.assertRaises(ValueError):
            profile.percentile(samples, 0)

    def test_the_gate_names_each_mix_over_budget_and_an_empty_run(self):
        report = {
            "prefix": profile.summary(9.0, [1.0] * 99 + [2.0]),
            "typo": profile.summary(9.0, [10.0] * 90 + [80.0] * 10),
        }
        self.assertEqual(report["prefix"]["p95_ms"], 1.0)
        self.assertEqual(profile.failures({"prefix": report["prefix"]}, 50), [])
        problems = profile.failures(report, 50)
        self.assertEqual(len(problems), 1)
        self.assertIn("typo", problems[0])
        self.assertEqual(profile.failures({}, 50), ["no mix was measured"])

    def test_cold_loads_are_reported_apart_from_the_warm_percentiles(self):
        row = profile.summary(123.0, [1.0, 2.0, 3.0])
        self.assertEqual((row["cold_ms"], row["warm_count"]), (123.0, 3))
        self.assertEqual(row["p50_ms"], 2.0)


if __name__ == "__main__":
    unittest.main()
