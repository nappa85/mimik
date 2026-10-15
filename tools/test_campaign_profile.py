"""Both successful freeze and last-row rollback must be measured, never silently omitted."""

import unittest

from campaign_profile import durations


class CampaignProfileTests(unittest.TestCase):
    def test_both_actual_rust_workload_labels_are_required(self):
        output = (
            "StageC actual Rust maximum last-row failure/rollback: 1.911s\n"
            "StageC actual Rust 32 × nearly 1 MiB freeze, revision9→10: 1803.0ms\n"
        )
        self.assertEqual(durations(output), [1911.0, 1803.0])

    def test_missing_malformed_and_extra_samples_fail(self):
        for output in [
            "",
            "StageC actual Rust maximum rollback: panic",
            "StageC actual Rust maximum rollback: 1s\n",
        ]:
            with self.subTest(output=output), self.assertRaises(AssertionError):
                durations(output)


if __name__ == "__main__":
    unittest.main()
