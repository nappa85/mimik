"""Evaluator integrity: reject tempting false positives; never query private data."""

import copy
import json
import unittest
from pathlib import Path
from unittest.mock import patch

import jsonschema
import local_rag_benchmark as bench

DEMO = Path(__file__).resolve().parents[1] / "examples" / "sheet-harness-demo"


class LocalRagTests(unittest.TestCase):
    def setUp(self):
        self.character = json.loads((DEMO / "character.json").read_text())
        self.cases = {
            c["id"]: c for c in json.loads((DEMO / "review-cases.json").read_text())["cases"]
        }

    def answer(self, name):
        case = self.cases[name]
        result = {
            "kind": case["expected"]["kind"],
            "message": "",
            "questions": [],
            "citations": case["rule_ids"],
            "changes": copy.deepcopy(case["expected"].get("changes", [])),
            "readback": [],
        }
        for change in result["changes"]:
            change.setdefault("collection", "")
            change.setdefault("entry_id", "")
        return result

    def test_invalid_schema_is_not_accepted_by_mechanical_scoring(self):
        case = self.cases["spend-focus"]
        good = self.answer("spend-focus")
        bads = [
            None,
            [],
            dict(good, extra=True),
            dict(good, kind="committed"),
            dict(good, questions="none"),
        ]
        for key in good:
            bad = copy.deepcopy(good)
            bad.pop(key)
            bads.append(bad)
        bad = copy.deepcopy(good)
        bad["changes"][0].pop("entry_id")
        bads.append(bad)
        for bad in bads:
            with self.assertRaises(jsonschema.ValidationError):
                bench.score(case, bad, case["rule_ids"])

    def test_runtime_failure_retains_raw_response(self):
        with patch.object(bench.http.client, "HTTPConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status = 503
            response.read.return_value = b"fixture unavailable"
            result = bench.generate("http://localhost:8080", {}, 17)
            self.assertEqual(result["http_status"], 503)
            self.assertEqual(result["response_body"], "fixture unavailable")

    def test_thinking_mode_is_explicit_and_baseline_defaults_unchanged(self):
        with patch.object(bench.http.client, "HTTPConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status = 200
            response.read.return_value = b"{}"
            bench.generate("http://localhost", {}, 17)
            baseline = json.loads(connection.return_value.request.call_args.args[2])
            self.assertEqual(baseline["max_tokens"], 768)
            self.assertFalse(baseline["chat_template_kwargs"]["enable_thinking"])
            bench.generate("http://localhost", {}, 17, thinking=True, max_tokens=1536)
            thinking = json.loads(connection.return_value.request.call_args.args[2])
            self.assertEqual(thinking["max_tokens"], 1536)
            self.assertTrue(thinking["chat_template_kwargs"]["enable_thinking"])

    def test_plain_has_no_schema_constraint_or_automatic_quality_score(self):
        with patch.object(bench.http.client, "HTTPConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status = 200
            response.read.return_value = b"{}"
            bench.generate("http://localhost", {}, 17, plain=True, system=bench.PLAIN_SYSTEM)
            payload = json.loads(connection.return_value.request.call_args.args[2])
            self.assertNotIn("response_format", payload)
            self.assertEqual(payload["messages"][0]["content"], bench.PLAIN_SYSTEM)
        body = json.dumps(
            {
                "choices": [
                    {
                        "message": {"content": "Focus would go from 3 to 1 [LR-2]."},
                        "finish_reason": "stop",
                    }
                ]
            }
        )
        row = bench.perform_attempt(
            "http://localhost",
            {},
            self.cases["spend-focus"],
            17,
            ["LR-2"],
            plain=True,
            generator=lambda *_a, **_k: {"http_status": 200, "response_body": body},
        )
        self.assertEqual(row["status"], "completed")
        self.assertIn("answer_text", row)
        self.assertNotIn("score", row)
        self.assertIsNone(bench.summarize([row], 1)["final_correct"])

    def test_bad_envelopes_and_transport_failures_stay_attempts(self):
        case = self.cases["spend-focus"]
        answer = self.answer("spend-focus")
        good = {"choices": [{"message": {"content": json.dumps(answer)}, "finish_reason": "stop"}]}
        envelopes = [
            [],
            None,
            {},
            {"choices": []},
            {"choices": [None]},
            {"choices": [{"message": {"content": None}}]},
            {"choices": [{"message": {"content": "not JSON"}}]},
        ]
        for envelope in envelopes:
            result = bench.perform_attempt(
                "http://localhost",
                {},
                case,
                17,
                ["LR-2"],
                generator=lambda *_a, envelope=envelope, **_k: {
                    "http_status": 200,
                    "response_body": json.dumps(envelope),
                },
            )
            self.assertEqual(result["status"], "failed")
            self.assertIn("seconds", result)
            self.assertIn("response_body", result)
        for body in (
            "not JSON",
            json.dumps(dict(good, choices=[dict(good["choices"][0], finish_reason="length")])),
        ):
            result = bench.perform_attempt(
                "http://localhost",
                {},
                case,
                17,
                ["LR-2"],
                generator=lambda *_a, body=body, **_k: {"http_status": 200, "response_body": body},
            )
            self.assertEqual(result["status"], "failed")
            self.assertEqual(result["response_body"], body)

        def timeout(*_a, **_k):
            raise TimeoutError("fixture")

        failed = bench.perform_attempt(
            "http://localhost", {}, case, 17, ["LR-2"], generator=timeout
        )
        valid = bench.perform_attempt(
            "http://localhost",
            {},
            case,
            17,
            ["LR-2"],
            generator=lambda *_a, **_k: {"http_status": 200, "response_body": json.dumps(good)},
        )
        summary = bench.summarize([failed, valid], 2)
        self.assertEqual((failed["status"], valid["status"]), ("failed", "completed"))
        self.assertEqual(summary["attempts"], 2)
        self.assertIsNone(summary["final_correct"])

    def test_deadline_stops_dispatch_and_mechanical_results_are_not_quality(self):
        now = [0.0]
        events = []

        def run(_job, _timeout):
            now[0] += 2
            return {"status": "completed", "score": {"mechanical_pass": True}}

        rows = bench.dispatch(
            [{"case": "a"}, {"case": "b"}], run, events.append, 1, clock=lambda: now[0]
        )
        self.assertEqual(len(rows), 1)
        self.assertEqual([e["event"] for e in events], ["started", "finished"])
        summary = bench.summarize(rows, 2)
        self.assertFalse(summary["complete"])
        self.assertIsNone(summary["final_correct"])
        # Contradictory prose can pass a mechanical signal: adjudication must gate quality.
        case = self.cases["excess-vitality"]
        answer = self.answer("excess-vitality")
        answer["message"] = "12 does not exceed 10; this edit is legal."
        row = {"score": bench.score(case, answer, ["LR-1"])}
        self.assertIsNone(bench.summarize([row], 1)["final_correct"])

    def test_complete_armor_only_and_wrong_or_extra_mutations_fail(self):
        case = self.cases["replace-armor"]
        answer = self.answer("replace-armor")
        self.assertTrue(bench.score(case, answer, case["rule_ids"])["mechanical_pass"])
        for mutation in ("missing", "wrong-id", "extra", "wrong-type"):
            bad = copy.deepcopy(answer)
            if mutation == "missing":
                bad["changes"].pop()
            elif mutation == "wrong-id":
                bad["changes"][0]["entry_id"] = "vest-1"
            elif mutation == "extra":
                bad["changes"].append(
                    {"collection": "", "entry_id": "", "field": "focus", "from": 3, "to": 1}
                )
            else:
                bad["changes"][0]["to"] = 0
            self.assertFalse(bench.score(case, bad, case["rule_ids"])["mechanical_pass"])

    def test_kind_alone_and_unsupported_citations_do_not_pass(self):
        case = self.cases["excess-vitality"]
        answer = self.answer("excess-vitality")
        answer["message"] = "That edit is fine."
        self.assertFalse(bench.score(case, answer, case["rule_ids"])["mechanical_pass"])
        answer["message"] = "12 exceeds the maximum of 10."
        self.assertTrue(bench.score(case, answer, case["rule_ids"])["mechanical_pass"])
        answer["citations"] = ["LR-999"]
        self.assertFalse(bench.score(case, answer, case["rule_ids"])["mechanical_pass"])

    def test_retrieval_is_scoped_safe_and_has_no_padding(self):
        chunks = [
            {"edition": "demo-1", "id": "a", "text": "focus cost"},
            {"edition": "other", "id": "wrong", "text": "focus focus focus focus"},
        ]
        retrieval = bench.Retrieval(chunks)
        self.assertEqual([r["id"] for r in retrieval.search("focus")[1]], ["a"])
        self.assertEqual(retrieval.search("zzzznothing")[1], [])
        self.assertEqual(retrieval.search('" OR ( - ) * focus')[1][0]["id"], "a")
        self.assertEqual(retrieval.search("the and my")[1], [])

    def test_prompt_allowlist_keeps_expectations_out_and_stale_metadata_in(self):
        case = self.cases["stale-proposal"]
        context = bench.model_input(
            self.character,
            [],
            case["request"],
            [],
            state_override=case["current_state_override"],
            proposal_base=case["proposal_base_revision"],
        )
        self.assertEqual(context["sheet"]["revision"], 2)
        self.assertEqual(context["sheet"]["values"]["focus"], 4)
        self.assertEqual(context["pending_proposal"]["base_revision"], 1)
        self.assertEqual(
            context["pending_proposal"]["changes"], [{"field": "focus", "from": 3, "to": 1}]
        )
        self.assertFalse({"expected", "forbidden", "rule_ids", "id"} & set(context))
        self.assertEqual(self.character["revision"], 1)

    def test_typed_schema_constrains_only_state_and_bindings_not_rules(self):
        fields = bench.FieldCatalog()
        fields.feed((DEMO / "sheet.html").read_text())
        context = bench.model_input(self.character, fields.fields, "request", [])
        schema = bench.typed_schema(context)
        answer = self.answer("spend-focus")
        # Any structurally valid integer is representable, including semantically invalid999.
        answer["changes"][0]["to"] = 999
        jsonschema.validate(answer, schema)
        for mutation in ("field", "from", "type", "entry"):
            bad = copy.deepcopy(answer)
            if mutation == "field":
                bad["changes"][0]["field"] = "invented"
            elif mutation == "from":
                bad["changes"][0]["from"] = 4
            elif mutation == "type":
                bad["changes"][0]["to"] = "999"
            else:
                bad["changes"][0]["entry_id"] = "example-mira"
            with self.assertRaises(jsonschema.ValidationError):
                jsonschema.validate(bad, schema)
        answer["changes"] = []
        jsonschema.validate(answer, schema)


if __name__ == "__main__":
    unittest.main()
