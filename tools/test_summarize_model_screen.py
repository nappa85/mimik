import json
import tempfile
import unittest
from pathlib import Path

from local_rag_benchmark import digest
from summarize_model_screen import finalize, load_contract


class FinalizationTests(unittest.TestCase):
    def test_finished_gold_stratum_and_hash_must_match_frozen_case(self):
        contract = self.contract()
        contract["cases"][0]["stratum"] = "known_development"
        base = {
            **contract["cases"][0],
            "order": contract["orders"][0],
            "event": "finished",
            "status": "completed",
        }
        self.assertEqual(finalize(contract, json.dumps(base))[0]["expected"], "supported")
        for field, value in (
            ("expected", "conflict"),
            ("expected_context", "conflict"),
            ("stratum", "fabricated"),
            ("state_hash", "changed"),
        ):
            with self.subTest(field=field), self.assertRaises(ValueError):
                finalize(contract, json.dumps({**base, field: value}))

    def test_changed_contract_with_old_digest_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "contract.json"
            contract = self.contract()
            envelope = {"contract": contract, "sha256": digest(contract)}
            path.write_text(json.dumps(envelope))
            self.assertEqual(load_contract(path), contract)
            contract["cases"][0]["expected"] = "conflict"
            path.write_text(json.dumps(envelope))
            with self.assertRaises(ValueError):
                load_contract(path)

    def contract(self):
        return {
            "cases": [
                {
                    "case": "one",
                    "state": "{}",
                    "state_hash": "unused",
                    "expected": "supported",
                    "expected_context": "supported",
                }
            ],
            "orders": [["supported", "conflict"], ["conflict", "supported"]],
        }

    def test_outer_cutoff_retains_unstarted_and_interrupted_denominator(self):
        raw = (
            json.dumps(
                {
                    "case": "one",
                    "state": "{}",
                    "order": ["supported", "conflict"],
                    "event": "started",
                }
            )
            + "\n"
        )
        rows = finalize(self.contract(), raw)
        self.assertEqual(len(rows), 2)
        self.assertEqual([r["status"] for r in rows], ["interrupted", "not_run"])

    def test_only_incomplete_final_line_is_tolerated(self):
        rows = finalize(self.contract(), '{"event":')
        self.assertEqual(len(rows), 2)
        with self.assertRaises(ValueError):
            finalize(self.contract(), '{"event":\n{}\n')

    def test_duplicate_finished_and_wrong_case_or_state_rejected(self):
        base = {
            **self.contract()["cases"][0],
            "order": ["supported", "conflict"],
            "event": "finished",
            "status": "completed",
        }
        for raw in (
            json.dumps(base) + "\n" + json.dumps(base),
            json.dumps({**base, "case": "other"}),
            json.dumps({**base, "state": "changed"}),
        ):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                finalize(self.contract(), raw)


if __name__ == "__main__":
    unittest.main()
