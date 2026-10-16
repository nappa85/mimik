"""Tool-loop benchmark: host validation, outcomes, scoring and an end-to-end dry run.

Formulas: when the formula_eval binary is built (see test_formula_cli.binary(); env
MIMIK_FORMULA_EVAL), every test runs against the real CEL engine. Otherwise tests use
FixtureEngine, a test double that knows only the fixture pack's four CEL expressions (it is
not a formula evaluator); test_formula_cli and test_cel_engine then fail closed.
"""

import contextlib
import copy
import io
import json
import tempfile
import unittest
from pathlib import Path

import tool_loop_benchmark as tl
from test_formula_cli import binary

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "tool_loop_pack"
CLI = str(binary()) if binary().exists() else None
ARMOR = (
    'max_of(equipment.filter(item, item.kind == "outer-armor" && item.equipped)'
    ".map(item, item.armor), 0)"
)
ONE_OUTER = 'size(equipment.filter(item, item.kind == "outer-armor" && item.equipped)) <= 1'


class FixtureEngine(tl.FormulaEngine):
    """Test double for the fixture pack only: maps its exact CEL strings to Python."""

    DERIVED = {
        "armor": (
            ARMOR,
            lambda v: max(
                [e["armor"] for e in v["equipment"] if e["kind"] == "outer-armor" and e["equipped"]]
                or [0]
            ),
        ),
        "pace": ("6 - armor", lambda v: 6 - v["armor"]),
    }
    CONSTRAINTS = {
        "focus-range": (
            "focus >= 0 && focus <= focus_max",
            lambda v: 0 <= v["focus"] <= v["focus_max"],
        ),
        "one-outer-armor": (
            ONE_OUTER,
            lambda v: sum(e["kind"] == "outer-armor" and e["equipped"] for e in v["equipment"])
            <= 1,
        ),
    }

    def __init__(self, definition):
        derived = {f["name"]: f["derived"] for f in definition["fields"] if "derived" in f}
        for name, expr in derived.items():
            if self.DERIVED.get(name, (None,))[0] != expr:
                raise tl.FormulaEvalError(f"FixtureEngine does not know {name}: {expr}")
        self.constraints = definition.get("constraints", [])
        for c in self.constraints:
            if self.CONSTRAINTS.get(c["id"], (None,))[0] != c["expr"]:
                raise tl.FormulaEvalError(f"FixtureEngine does not know {c['id']}")
        self.derived_order = [n for n in ("armor", "pace") if n in derived]
        self.provenance = {"engine": "fixture-test-double"}

    def derive(self, values):
        out = dict(values)
        for name in self.derived_order:
            out[name] = self.DERIVED[name][1](out)
        return out

    def violations(self, values):
        return [
            {
                "constraint_id": c["id"],
                "rule_ids": c["rule_ids"],
                "message": c["message"],
                "status": "violated",
            }
            for c in self.constraints
            if not self.CONSTRAINTS[c["id"]][1](values)
        ]

    def close(self):
        pass


def engine_factory(definition):
    return tl.CelCliEngine(definition, CLI) if CLI else FixtureEngine(definition)


def open_pack(root=FIXTURE):
    return tl.Pack(root, engine_factory=engine_factory)


def call(name, arguments):
    return {"name": name, "arguments": arguments}


SEARCH_FOCUS = call("search_rules", {"query": "spend focus"})
SEARCH_ARMOR = call("search_rules", {"query": "outer armor pace"})


def propose(changes, rule_ids=("FX-1",), reason=""):
    return call(
        "propose_change", {"changes": changes, "rule_ids": list(rule_ids), "reason": reason}
    )


def submit(rule_ids=("FX-1",)):
    return call("submit_proposal", {"rule_ids": list(rule_ids), "reason": "ok"})


class Base(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pack = open_pack()
        cls.index = tl.RuleIndex(cls.pack.rules)
        cls.cases = {c["id"]: c for cs in cls.pack.cases.values() for c in cs}

    @classmethod
    def tearDownClass(cls):
        cls.pack.close()

    def episode(self, case_id="spend-focus"):
        return tl.Episode(self.pack, self.index, self.cases[case_id])

    def run_script(self, case_id, turns, max_turns=tl.MAX_TURNS):
        model = tl.ScriptedModel({case_id: turns})
        row = tl.run_episode(self.pack, self.index, self.cases[case_id], model, 17, max_turns)
        return row, tl.score(self.cases[case_id], row)


class PackTests(Base):
    def test_fixture_loads_and_recomputes_derived(self):
        values = self.pack.characters["hero"]["values"]
        self.assertEqual((values["armor"], values["pace"]), (1, 5))
        self.assertEqual([r["rule_id"] for r in self.pack.rules], ["FX-1", "FX-2", "FX-3", "FX-4"])

    def test_bad_packs_are_rejected(self):
        def mutate(fn):
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp) / "pack"
                root.mkdir()
                for path in FIXTURE.rglob("*"):
                    target = root / path.relative_to(FIXTURE)
                    if path.is_dir():
                        target.mkdir(exist_ok=True)
                    else:
                        target.parent.mkdir(exist_ok=True, parents=True)
                        target.write_bytes(path.read_bytes())
                fn(root)
                with self.assertRaises(tl.PackError):
                    open_pack(root).close()

        def edit(name, change):
            def apply(root):
                doc = json.loads((root / name).read_text())
                change(doc)
                (root / name).write_text(json.dumps(doc))

            return apply

        mutate(edit("characters/hero.json", lambda d: d["values"].update(pace=4)))
        mutate(edit("characters/hero.json", lambda d: d["values"].update(focus="3")))
        mutate(edit("cases-dev.json", lambda d: d["cases"][0].update(gold_rule_ids=["NOPE"])))
        mutate(
            edit(
                "cases-dev.json",
                lambda d: d["cases"][0]["expected"].update(changes=[{"field": "pace", "to": 4}]),
            )
        )
        mutate(
            edit(
                "cases-dev.json",
                lambda d: d["cases"][0].update(setup={"value_overrides": {"armor": 2}}),
            )
        )
        mutate(
            edit(
                "cases-dev.json",
                lambda d: d["cases"][4]["expected"].pop("answer_values"),
            )
        )
        mutate(
            edit(
                "definition.json",
                lambda d: d["fields"].append({"name": "x", "type": "integer", "derived": "x"}),
            )
        )

    def test_context_never_contains_expected_or_gold(self):
        for case in self.cases.values():
            text = json.dumps(tl.Episode(self.pack, self.index, case).messages())
            self.assertNotIn(case["expected"]["notes"], text)
            self.assertNotIn("gold_rule_ids", text)
            self.assertNotIn("expected", text)

    def test_context_states_revision_pending_paths_and_no_rules(self):
        context = json.loads(
            self.episode("stale-pending").messages()[1]["content"].split("\n", 1)[1]
        )
        self.assertEqual(context["sheet"]["current_revision"], 5)
        self.assertEqual(
            context["pending_proposal"],
            {"base_revision": 3, "changes": [{"path": "focus", "to": 1}]},
        )
        text = json.dumps(context)
        self.assertNotIn("FX-1", text)  # rules are not pre-stuffed
        fields = {f["path"]: f for f in context["sheet"]["fields"]}
        self.assertEqual(
            fields["pace"],
            {"path": "pace", "type": "integer", "editable": False, "derived": True, "value": 5},
        )
        collection = context["sheet"]["collections"][0]
        self.assertEqual(collection["entry_path"], "equipment/<entry_id>/<field>")
        self.assertEqual(collection["entries"][1], {"id": "vest", "name": "Iron vest"})


class RetrievalTests(Base):
    def test_diacritic_insensitive_italian_search(self):
        _, results = self.index.search("quantita riposo")
        self.assertEqual(results[0]["rule_id"], "FX-4")
        _, results = self.index.search("QUANTITÀ")
        self.assertEqual(results[0]["rule_id"], "FX-4")

    def test_no_porter_stemming_and_safe_query(self):
        # unicode61 without porter: "spends" does not match "Spending".
        self.assertEqual(self.index.search("spends")[1], [])
        self.assertEqual(self.index.search("spending")[1][0]["rule_id"], "FX-1")
        self.assertEqual(tl.RuleIndex.terms('a "b" OR c*'), ["a", "b", "or", "c"])
        match, _ = self.index.search('"; DROP TABLE rules; --')
        self.assertIn('"drop"', match)
        self.assertEqual(self.index.search("   ")[1], [])
        self.assertLessEqual(len(self.index.search("focus armor pace riposo a the")[1]), 5)

    def test_search_records_retrieved_ids(self):
        ep = self.episode()
        result = ep.call("search_rules", json.dumps({"query": "focus"}))
        self.assertTrue(result["ok"])
        self.assertIn("FX-1", ep.retrieved)
        self.assertEqual(ep.searches[0]["rule_ids"], [r["rule_id"] for r in result["results"]])


class HostValidationTests(Base):
    def propose(self, ep, changes, rule_ids=("FX-1",)):
        return ep.call(
            "propose_change", {"changes": changes, "rule_ids": list(rule_ids), "reason": "test"}
        )

    def test_accepted_proposal_preview(self):
        ep = self.episode("swap-armor")
        ep.call("search_rules", {"query": "outer armor"})
        result = self.propose(
            ep,
            [
                {"path": "equipment/coat/equipped", "to": False},
                {"path": "equipment/vest/equipped", "to": True},
            ],
            ["FX-2"],
        )
        self.assertTrue(result["ok"], result)
        preview = result["preview"]
        self.assertEqual(preview["base_revision"], 3)
        self.assertEqual(
            preview["changes"][0], {"path": "equipment/coat/equipped", "from": True, "to": False}
        )
        self.assertEqual(
            preview["derived"],
            [{"path": "armor", "from": 1, "to": 2}, {"path": "pace", "from": 5, "to": 4}],
        )
        self.assertEqual(preview["violations"], [])
        self.assertEqual(ep.values["equipment"][0]["equipped"], True)  # nothing applied
        self.assertEqual(ep.rejected_calls, 0)

    def test_derived_field_rejected_with_explanation(self):
        ep = self.episode()
        ep.call("search_rules", {"query": "focus"})
        result = self.propose(ep, [{"path": "pace", "to": 4}])
        self.assertFalse(result["ok"])
        self.assertIn("derived", result["errors"][0])
        self.assertIn("depends on", result["errors"][0])
        self.assertIn("valid_paths", result)
        self.assertIsNone(ep.accepted)
        self.assertEqual((ep.rejected_calls, ep.malformed_calls), (1, 0))

    def test_bad_paths_and_types_rejected(self):
        ep = self.episode()
        ep.call("search_rules", {"query": "focus"})
        cases = [
            ({"path": "equipment/sword/equipped", "to": True}, "no entry"),
            ({"path": "equipment/coat/weight", "to": 1}, "no field"),
            ({"path": "bags/x/y", "to": 1}, "unknown collection"),
            ({"path": "luck", "to": 1}, "unknown path"),
            ({"path": "equipment/coat", "to": 1}, "must be"),
            ({"path": "focus_max", "to": 5}, "not editable"),
            ({"path": "focus", "to": "2"}, "expected integer"),
            ({"path": "focus", "to": True}, "expected integer"),
            ({"path": "focus", "to": 3}, "already"),
            ({"path": "equipment/coat/kind", "to": "hat"}, "one of"),
            ({"path": "equipment", "to": 1}, "collection"),
            ({"field": "focus", "to": 1}, '"path"'),  # v1 shape: actionable rejection
            (
                {"collection": "equipment", "entry_id": "coat", "field": "equipped", "to": False},
                '"path"',
            ),
        ]
        for change, message in cases:
            result = self.propose(ep, [change])
            self.assertFalse(result["ok"], change)
            self.assertIn(message, " ".join(result["errors"]), change)
            self.assertIn("equipment/coat/equipped", result["valid_paths"], change)
        self.assertEqual(ep.rejected_calls, len(cases))
        self.assertIsNone(ep.accepted)

    def test_uncited_rule_ids_rejected(self):
        ep = self.episode()
        result = self.propose(ep, [{"path": "focus", "to": 1}], ["FX-1"])
        self.assertFalse(result["ok"])
        self.assertIn("not returned by search_rules", result["errors"][0])
        self.assertNotIn("valid_paths", result)  # the paths were fine
        ep.call("search_rules", {"query": "focus"})
        self.assertTrue(self.propose(ep, [{"path": "focus", "to": 1}], ["FX-1"])["ok"])

    def test_violations_reported_not_enforced(self):
        ep = self.episode("overspend")
        ep.call("search_rules", {"query": "focus"})
        result = self.propose(ep, [{"path": "focus", "to": -2}])
        self.assertTrue(result["ok"])
        violation = result["preview"]["violations"][0]
        self.assertEqual(
            (violation["constraint_id"], violation["status"]), ("focus-range", "violated")
        )
        self.assertIsNotNone(ep.accepted)

    def test_last_accepted_proposal_wins(self):
        ep = self.episode()
        ep.call("search_rules", {"query": "focus"})
        self.propose(ep, [{"path": "focus", "to": 2}])
        self.propose(ep, [{"path": "focus", "to": 1}])
        self.propose(ep, [{"path": "pace", "to": 1}])  # rejected; does not replace
        self.assertEqual(ep.accepted["changes"], [{"path": "focus", "to": 1}])
        self.assertEqual(len(ep.proposals), 3)

    def test_read_fields_paths(self):
        ep = self.episode()
        result = ep.call(
            "read_fields",
            {"paths": ["focus", "equipment/vest/armor", "equipment/rope", "equipment"]},
        )
        self.assertTrue(result["ok"])
        self.assertEqual(result["values"]["focus"], 3)
        self.assertEqual(result["values"]["equipment/vest/armor"], 2)
        self.assertEqual(result["values"]["equipment/rope"]["kind"], "gear")
        self.assertEqual(len(result["values"]["equipment"]), 3)
        self.assertNotIn("valid_paths", result)
        bad = ep.call("read_fields", {"paths": ["nope", "equipment/x/armor", "equipment/coat/zz"]})
        self.assertFalse(bad["ok"])
        self.assertEqual(len(bad["errors"]), 3)
        self.assertIn("equipment/coat/armor", bad["valid_paths"])

    def test_malformed_calls_are_errors_not_repairs(self):
        ep = self.episode()
        malformed = [
            ("teleport", "{}"),
            ("search_rules", "{not json"),
            ("search_rules", "[1]"),
            ("search_rules", {"q": "focus"}),
            ("search_rules", {"query": 5}),
            ("search_rules", {"query": "x", "extra": 1}),
            ("respond", {"kind": "answer", "message": "x", "rule_ids": []}),  # answer is a tool
            ("respond", {"kind": "proposal", "message": "x", "rule_ids": []}),
            ("answer", {"message": "x", "rule_ids": []}),
            ("answer", {"message": "x", "values": {}, "rule_ids": []}),
            ("answer", {"message": "x", "values": [1], "rule_ids": []}),
            ("answer", {"message": "x", "values": {"a": 1.5}, "rule_ids": []}),
            ("ask_user", {"kind": "maybe", "question": "?"}),
            ("submit_proposal", {"rule_ids": []}),
            ("propose_change", {"changes": "focus=1", "rule_ids": [], "reason": ""}),
            ("read_fields", {"paths": []}),
        ]
        for name, args in malformed:
            result = ep.call(name, args)
            self.assertFalse(result["ok"], (name, args))
            self.assertIn("malformed", result["error"])
        self.assertEqual(ep.malformed_calls, len(malformed))
        self.assertIsNone(ep.terminal)
        self.assertEqual(ep.searches, [])


class OutcomeTests(Base):
    def test_explicit_proposal(self):
        row, s = self.run_script(
            "spend-focus",
            [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit()]],
        )
        self.assertEqual(row["outcome"], "proposal")
        self.assertTrue(s["correct"] and s["changes_exact"] and s["citation_valid"])
        self.assertTrue(s["citation_covers_gold"])
        self.assertEqual(s["retrieval_recall"], 1.0)
        self.assertFalse(s["false_supported"])
        self.assertEqual(row["turns"], 3)

    def test_wrong_changes_are_wrong_proposal(self):
        row, s = self.run_script(
            "spend-focus", [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 2}])], [submit()]]
        )
        self.assertEqual(row["outcome"], "proposal")
        self.assertTrue(s["outcome_correct"])
        self.assertFalse(s["changes_exact"] or s["correct"])
        self.assertEqual(s["false_supported_reasons"], ["wrong_proposal"])

    def test_collection_changes_order_insensitive(self):
        changes = [
            {"path": "equipment/vest/equipped", "to": True},
            {"path": "equipment/coat/equipped", "to": False},
        ]
        _, s = self.run_script(
            "swap-armor",
            [[SEARCH_ARMOR], [propose(changes, ["FX-2", "FX-3"])], [submit(["FX-2", "FX-3"])]],
        )
        self.assertTrue(s["correct"], s)

    def test_conflict_after_violation_is_correct(self):
        row, s = self.run_script(
            "overspend",
            [
                [SEARCH_FOCUS],
                [propose([{"path": "focus", "to": -2}])],
                [call("respond", {"kind": "conflict", "message": "Only 3.", "rule_ids": ["FX-1"]})],
            ],
        )
        self.assertEqual(row["outcome"], "conflict")
        self.assertTrue(s["correct"])
        self.assertFalse(s["false_supported"])

    def test_submitted_violation_is_false_supported(self):
        row, s = self.run_script(
            "overspend", [[SEARCH_FOCUS], [propose([{"path": "focus", "to": -2}])], [submit()]]
        )
        self.assertEqual(row["outcome"], "proposal")
        self.assertEqual(
            s["false_supported_reasons"],
            ["outcome_error", "unreported_violation", "wrong_proposal"],
        )  # review 2026-10-15: any unexpected proposal is also wrong_proposal
        self.assertFalse(s["correct"])

    def test_answer_where_ask_expected_is_false_supported(self):
        _, s = self.run_script(
            "rest-amount",
            [
                [call("search_rules", {"query": "riposo"})],
                [call("answer", {"message": "2", "values": {"focus": 2}, "rule_ids": ["FX-4"]})],
            ],
        )
        self.assertEqual(s["false_supported_reasons"], ["outcome_error"])
        self.assertFalse(s["correct"])

    def test_ask_rule_and_ask_fact(self):
        row, s = self.run_script(
            "rest-amount",
            [
                [call("search_rules", {"query": "riposo breve"})],
                [call("ask_user", {"kind": "rule", "question": "Quanto?"})],
            ],
        )
        self.assertEqual(row["outcome"], "ask_rule")
        self.assertTrue(s["correct"])
        self.assertIsNone(s["citation_valid"])
        self.assertIsNone(s["citation_covers_gold"])
        row, s = self.run_script(
            "rest-amount", [[call("ask_user", {"kind": "fact", "question": "?"})]]
        )
        self.assertEqual(row["outcome"], "ask_fact")
        self.assertFalse(s["correct"] or s["false_supported"])
        self.assertEqual(s["retrieval_recall"], 0.0)

    def test_stale_handling(self):
        row, s = self.run_script(
            "stale-pending",
            [[call("respond", {"kind": "stale", "message": "Rev 3 < 5.", "rule_ids": []})]],
        )
        self.assertEqual(row["outcome"], "stale")
        self.assertTrue(s["correct"])
        self.assertIsNone(s["retrieval_recall"])  # no gold rules
        self.assertTrue(s["citation_valid"])
        # Re-proposing and submitting the stale change is false support.
        row, s = self.run_script(
            "stale-pending",
            [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit()]],
        )
        self.assertEqual(row["accepted_proposal"]["preview"]["base_revision"], 5)
        self.assertEqual(row["outcome"], "proposal")
        self.assertTrue(s["false_supported"])

    def test_answer_values_typed(self):
        def answer(values):
            return self.run_script(
                "pace-readback",
                [
                    [call("search_rules", {"query": "pace"})],
                    [call("answer", {"message": "none", "values": values, "rule_ids": ["FX-3"]})],
                ],
            )[1]

        s = answer({"pace": 5})
        self.assertEqual(s["answer_check"], "verified_correct")
        self.assertTrue(s["correct"] and not s["false_supported"])
        self.assertEqual(answer({"speed": 5})["answer_check"], "verified_correct")  # by value
        for wrong in ({"pace": "5"}, {"pace": 4}):
            s = answer(wrong)
            self.assertEqual(s["answer_check"], "wrong", wrong)
            self.assertFalse(s["correct"])
            self.assertEqual(s["false_supported_reasons"], ["wrong_answer"])

    def test_uncited_answer_citation_invalid(self):
        _, s = self.run_script(
            "pace-readback",
            [[call("answer", {"message": "5", "values": {"pace": 5}, "rule_ids": ["FX-3"]})]],
        )
        self.assertFalse(s["citation_valid"])
        self.assertTrue(s["citation_covers_gold"])  # covers gold, but was never retrieved

    def test_refuse_outcome(self):
        row, s = self.run_script(
            "spend-focus", [[call("respond", {"kind": "refuse", "message": "no", "rule_ids": []})]]
        )
        self.assertEqual(row["outcome"], "refuse")
        self.assertFalse(s["correct"] or s["false_supported"] or s["changes_exact"])

    def test_submit_without_accepted_is_counted_rejection(self):
        row, s = self.run_script(
            "spend-focus",
            [[submit()], [SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit()]],
        )
        self.assertEqual((row["rejected_calls"], row["malformed_calls"]), (1, 0))
        first_tool = next(m for m in row["transcript"] if m["role"] == "tool")
        self.assertIn("no accepted proposal", first_tool["content"])
        self.assertEqual(row["outcome"], "proposal")
        self.assertTrue(s["correct"])

    def test_submit_citations_merge(self):
        _, s = self.run_script(
            "spend-focus",
            [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit(["FX-1", "FX-4"])]],
        )
        self.assertEqual(s["cited_rule_ids"], ["FX-1", "FX-4"])
        self.assertFalse(s["citation_valid"])  # FX-4 was not retrieved

    def test_timeout_after_max_turns(self):
        row, s = self.run_script("spend-focus", [[SEARCH_FOCUS]] * 20)
        self.assertEqual(row["outcome"], "timeout")
        self.assertEqual(row["turns"], tl.MAX_TURNS)
        self.assertFalse(s["correct"] or s["false_supported"])
        row, _ = self.run_script(
            "spend-focus",
            [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])]] + [[SEARCH_FOCUS]] * 8,
        )
        self.assertEqual(row["outcome"], "timeout")
        self.assertTrue(row["pending_proposal_at_end"])

    def test_plain_reply_is_no_outcome(self):
        row, s = self.run_script("pace-readback", [[]])
        self.assertEqual(row["outcome"], "no_outcome")
        self.assertFalse(row["pending_proposal_at_end"])
        self.assertFalse(s["correct"])

    def test_malformed_calls_counted_in_loop(self):
        row, _ = self.run_script(
            "rest-amount",
            [
                [call("search_rules", "{oops"), call("nope", "{}")],
                [
                    call("ask_user", {"kind": "rule", "question": "?"}),
                    call("respond", {"kind": "refuse", "message": "x", "rule_ids": []}),
                ],
            ],
        )
        self.assertEqual(row["malformed_calls"], 2)
        self.assertEqual(row["outcome"], "ask_rule")
        self.assertEqual(row["calls_after_terminal"], 1)
        tool_msgs = [m for m in row["transcript"] if m["role"] == "tool"]
        self.assertIn("malformed", tool_msgs[0]["content"])
        self.assertEqual(tool_msgs[0]["tool_call_id"], "call_1_0")

    def test_model_exception_is_retained_as_error(self):
        def broken(messages, seed, case_id=None, turn=None):
            raise TimeoutError("deadline")

        row = tl.run_episode(self.pack, self.index, self.cases["spend-focus"], broken, 1)
        self.assertEqual(row["outcome"], "error")
        self.assertIn("deadline", row["error"])
        self.assertFalse(tl.score(self.cases["spend-focus"], row)["correct"])

    def test_bad_response_shape_is_error(self):
        row = tl.run_episode(
            self.pack, self.index, self.cases["spend-focus"], lambda *a, **k: {"x": 1}, 1
        )
        self.assertEqual(row["outcome"], "error")


def summary_rows(n_cases, correct, false_supported, split="heldout"):
    rows = []
    for i in range(n_cases):
        rows.append(
            {
                "event": "episode",
                "model_label": "m",
                "split": split,
                "case_id": f"c{i}",
                "outcome": "answer",
                "score": {
                    "expected_outcome": "answer",
                    "has_gold": True,
                    "retrieval_recall": 1.0,
                    "outcome_correct": True,
                    "changes_exact": None,
                    "wrong_proposal": False,
                    "answer_check": None,
                    "answer_values_substring_diagnostic": None,
                    "false_supported_reasons": ["wrong_answer"] if i < false_supported else [],
                    "citation_valid": True,
                    "citation_covers_gold": True,
                    "unreported_violation": False,
                    "correct": i < correct,
                    "false_supported": i < false_supported,
                },
            }
        )
    return rows


class SummaryTests(unittest.TestCase):
    def verdicts(self, summary):
        bar = summary["bar"]
        self.assertEqual(bar["label"], "bar: pending denominator choice")
        return {k: v["verdict"] for k, v in bar["variants"].items()}

    def test_bar_pass_and_fail(self):
        (s,) = tl.summarize(summary_rows(100, 90, 2))
        self.assertEqual(s["bar"]["verdict"], "PENDING")
        # No at-risk cases in these rows: that denominator is empty, so it cannot pass.
        self.assertEqual(self.verdicts(s), {"all_cases": "PASS", "at_risk_cases": "FAIL"})
        self.assertEqual(s["correct"], {"count": 90, "of": 100, "rate": 0.9})
        (s,) = tl.summarize(summary_rows(100, 89, 0))
        self.assertEqual(self.verdicts(s)["all_cases"], "FAIL")
        (s,) = tl.summarize(summary_rows(100, 100, 3))
        self.assertEqual(self.verdicts(s)["all_cases"], "FAIL")
        (s,) = tl.summarize(summary_rows(99, 99, 0))
        self.assertEqual(self.verdicts(s)["all_cases"], "FAIL")
        self.assertIn("unique cases", s["bar"]["variants"]["all_cases"]["reasons"][0])
        (s,) = tl.summarize(summary_rows(120, 120, 0, split="dev"))
        self.assertEqual(s["bar"]["verdict"], "N/A")
        self.assertIn("pending denominator choice", tl.format_summary(s))

    def test_two_false_supported_denominators(self):
        rows = summary_rows(200, 200, 0)
        for n, row in enumerate(rows[:50]):  # 50 at-risk cases, 2 of them false-supported
            row["score"]["expected_outcome"] = "ask_rule"
            if n < 2:
                row["score"].update(
                    false_supported=True, correct=False, false_supported_reasons=["outcome_error"]
                )
        (s,) = tl.summarize(rows)
        self.assertEqual(s["false_supported"], {"count": 2, "of": 200, "rate": 0.01})
        self.assertEqual(s["false_supported_at_risk"], {"count": 2, "of": 50, "rate": 0.04})
        self.assertEqual(self.verdicts(s), {"all_cases": "PASS", "at_risk_cases": "FAIL"})
        self.assertEqual(s["false_supported_by_category"]["outcome_error"]["count"], 2)
        self.assertEqual(s["per_expected_outcome"]["ask_rule"]["episodes"], 50)
        self.assertEqual(s["per_expected_outcome"]["ask_rule"]["false_supported"]["count"], 2)
        self.assertEqual(s["per_expected_outcome"]["answer"]["correct"]["of"], 150)
        text = tl.format_summary(s)
        self.assertIn("all_cases: PASS", text)
        self.assertIn("at_risk_cases: FAIL", text)

    def test_not_run_rows_fail_the_bar(self):
        rows = summary_rows(110, 110, 0)
        rows[0]["outcome"] = "not_run"
        rows[0]["score"].update(correct=False, retrieval_recall=None)
        (s,) = tl.summarize(rows)
        self.assertEqual(s["not_run"], 1)
        self.assertEqual(s["retrieval_recall_mean"], {"mean": 1.0, "of": 109, "not_run": 1})
        self.assertEqual(set(self.verdicts(s).values()), {"FAIL"})


class DryRunTests(Base):
    def argv(self, out, *extra):
        argv = ["--pack", str(FIXTURE), "--split", "dev", "--model-label", "fake", "--dry-run"]
        return argv + ["--output", str(out), *extra]

    def run_main(self, argv):
        if CLI:
            return tl.main(argv + ["--formula-cli", CLI])
        original = tl.CelCliEngine
        tl.CelCliEngine = lambda definition, cli: FixtureEngine(definition)
        try:
            return tl.main(argv)
        finally:
            tl.CelCliEngine = original

    def test_end_to_end_dry_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "ledger.jsonl"
            argv = self.argv(
                out, "--seeds", "1,2", "--dry-run-script", str(FIXTURE / "dry-run-script.json")
            )
            sink = io.StringIO()
            with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
                self.assertEqual(self.run_main(argv), 0)
                rows = [json.loads(line) for line in out.read_text().splitlines()]
                with self.assertRaises(FileExistsError):  # ledgers are never overwritten
                    self.run_main(argv)
                self.assertEqual(tl.main(["--summarize", str(out)]), 0)
            self.assertIn("pending denominator choice", sink.getvalue())
        header = rows[0]
        self.assertEqual(header["event"], "run_started")
        self.assertEqual(header["benchmark_version"], 3)  # harness v3 (2026-10-16)
        self.assertEqual(header["planned_episodes"], 12)
        self.assertTrue(header["sampling"]["cache_prompt"])
        self.assertEqual(header["sampling"]["max_tokens"], 4096)  # v3 H11 (v2: 2048)
        self.assertIn("engine", header["formula_engine"])
        self.assertEqual(rows[-1]["event"], "run_summary")
        episodes = [r for r in rows if r["event"] == "episode"]
        self.assertEqual(len(episodes), 12)
        by_case = {r["case_id"]: r for r in episodes if r["seed"] == 1}
        for case_id in ("spend-focus", "pace-readback", "rest-amount"):
            self.assertTrue(by_case[case_id]["score"]["correct"], case_id)
        # Unscripted cases use the default script: search with the request, then answer.
        self.assertEqual(by_case["overspend"]["outcome"], "answer")
        self.assertTrue(by_case["overspend"]["score"]["false_supported"])
        (summary,) = rows[-1]["summaries"]
        self.assertEqual(summary["correct"]["count"], 6)
        for row in episodes:
            self.assertNotIn("GOLD-NOTE", json.dumps(row["transcript"]))

    def test_budget_exhaustion_records_not_run(self):
        args = tl.build_parser().parse_args(self.argv("unused", "--budget-seconds", "1"))
        ticks = iter([0.0] + [10.0] * 1000)
        out = io.StringIO()
        with contextlib.redirect_stderr(io.StringIO()):
            (summary,) = tl.run(
                args, tl.ScriptedModel(), out, clock=lambda: next(ticks), pack=self.pack
            )
        self.assertEqual(summary["not_run"], 6)
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertTrue(all(r["outcome"] == "not_run" for r in rows if r["event"] == "episode"))
        self.assertEqual(summary["retrieval_recall_mean"], {"mean": None, "of": 0, "not_run": 5})


class HttpModelTests(unittest.TestCase):
    def test_endpoint_restrictions(self):
        for bad in ("https://x", "http://u:p@localhost:1", "http://localhost:1/v1", "ftp://x"):
            with self.assertRaises(ValueError):
                tl.check_endpoint(bad)
        tl.check_endpoint("http://127.0.0.1:8080")

    def test_payload_has_tools_and_sampling(self):
        sampling = {
            "temperature": 0.1,
            "top_p": 0.95,
            "max_tokens": 64,
            "tool_choice": "auto",
            "thinking": "off",
            "cache_prompt": True,
        }
        model = tl.HttpModel("http://127.0.0.1:1", "m", sampling)
        payload = model.payload([{"role": "user", "content": "x"}], 7)
        self.assertEqual(payload["tools"], tl.TOOLS)
        self.assertEqual((payload["temperature"], payload["seed"]), (0.1, 7))
        self.assertTrue(payload["cache_prompt"])
        self.assertEqual(payload["chat_template_kwargs"], {"enable_thinking": False})
        default = tl.HttpModel("http://127.0.0.1:1", "m", dict(sampling, thinking="default"))
        self.assertNotIn("chat_template_kwargs", default.payload([], 1))

    def test_tool_schemas(self):
        self.assertEqual(
            set(tl.TOOL_NAMES),
            {
                "search_rules",
                "read_fields",
                "propose_change",
                "submit_proposal",
                "answer",
                "ask_user",
                "respond",
            },
        )
        tools = {t["function"]["name"]: t["function"] for t in tl.TOOLS}
        self.assertEqual(
            tools["respond"]["parameters"]["properties"]["kind"]["enum"],
            ["conflict", "stale", "refuse"],
        )
        self.assertIn("values", tools["answer"]["parameters"]["required"])
        self.assertIn("REQUIRED", tools["answer"]["description"])
        item = tools["propose_change"]["parameters"]["properties"]["changes"]["items"]
        self.assertEqual(item["required"], ["path", "to"])
        self.assertIn("items/item-1/active", tools["propose_change"]["description"])


class FormulaSeamTests(unittest.TestCase):
    def test_sheet_spec_translation(self):
        definition = json.loads((FIXTURE / "definition.json").read_text())
        spec = tl.sheet_spec(definition)
        fields = {f["id"]: f["type"] for f in spec["declarations"]["fields"]}
        self.assertEqual(fields["focus"], "int")
        self.assertEqual(fields["name"], {"text": tl.FORMULA_TEXT_CHARS})
        self.assertNotIn("armor", fields)  # derived fields are not inputs
        props = {p["id"]: p for p in spec["declarations"]["lists"][0]["properties"]}
        self.assertEqual(props["kind"]["type"], {"enum": ["outer-armor", "gear"]})
        self.assertTrue(props["armor"]["optional"])
        self.assertEqual([d["id"] for d in spec["derived"]], ["armor", "pace"])
        self.assertEqual(spec["constraints"][1], {"id": "one-outer-armor", "expr": ONE_OUTER})

    def test_cel_engine_requires_profile_and_cli(self):
        definition = json.loads((FIXTURE / "definition.json").read_text())
        with self.assertRaisesRegex(tl.FormulaEvalError, "formula_profile"):
            tl.CelCliEngine(dict(definition, formula_profile=None), "/nonexistent")
        with self.assertRaisesRegex(tl.FormulaEvalError, "not found"):
            tl.CelCliEngine(definition, "/nonexistent/formula_eval")

    def test_harness_has_no_python_formula_evaluator(self):
        self.assertFalse((Path(tl.__file__).parent / "formula_ref.py").exists())
        self.assertNotIn("import formula_ref", Path(tl.__file__).read_text())


class SubstringDiagnosticRegressionTests(unittest.TestCase):
    """Review 2026-10-15: the substring fallback matched vacuously."""

    def test_false_does_not_match_bare_no(self):
        self.assertFalse(tl.value_in_message(False, "No, the lamp is lit."))
        self.assertFalse(tl.value_in_message(False, "I have no idea."))

    def test_true_does_not_match_italian_si(self):
        self.assertFalse(tl.value_in_message(True, "Si tratta di una lanterna spenta."))
        self.assertFalse(tl.value_in_message(True, "Sì."))

    def test_integer_does_not_match_inside_rule_ids(self):
        self.assertFalse(tl.value_in_message(2, "See LR2-4 for pace."))
        self.assertFalse(tl.value_in_message(2, "Per FX-2, pace is 4."))
        self.assertTrue(tl.value_in_message(2, "Pace is 2 (FX-3)."))


class HarnessV2RegressionTests(Base):
    """Reproductions from the 2026-10-15 ledgers (target/tool-loop-benchmark-20261015)."""

    def test_propose_change_accepts_read_fields_path(self):
        # Ledger: 4B ho-003 sent {"field": "equipment/scale-1/equipped"} and got
        # "unknown field"; one path syntax is now shared with read_fields.
        ep = self.episode("swap-armor")
        ep.call("search_rules", {"query": "outer armor"})
        result = ep.call(
            "propose_change",
            {
                "changes": [
                    {"path": "equipment/coat/equipped", "to": False},
                    {"path": "equipment/vest/equipped", "to": True},
                ],
                "rule_ids": ["FX-2"],
                "reason": "swap",
            },
        )
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["preview"]["changes"][0]["path"], "equipment/coat/equipped")

    def test_bad_path_error_lists_valid_paths(self):
        ep = self.episode()
        ep.call("search_rules", {"query": "focus"})
        result = ep.call(
            "propose_change",
            {
                "changes": [{"path": "equipment/sword/equipped", "to": True}],
                "rule_ids": [],
                "reason": "",
            },
        )
        self.assertFalse(result["ok"])
        self.assertIn("equipment/coat/equipped", json.dumps(result))
        self.assertIn("focus", result["valid_paths"])
        self.assertNotIn("pace", result["valid_paths"])  # derived, not editable

    def test_stopping_with_accepted_proposal_is_no_outcome(self):
        # Ledger: all 15 4B held-out false supports came from this implicit path.
        row, s = self.run_script(
            "spend-focus",
            [
                [SEARCH_FOCUS],
                [
                    call(
                        "propose_change",
                        {
                            "changes": [{"path": "focus", "to": 1}],
                            "rule_ids": ["FX-1"],
                            "reason": "",
                        },
                    )
                ],
                [],
            ],
        )
        self.assertEqual(row["outcome"], "no_outcome")
        self.assertTrue(row["pending_proposal_at_end"])
        self.assertFalse(s["correct"] or s["false_supported"])

    def test_respond_json_in_content_is_not_parsed(self):
        turns = iter(
            [
                {
                    "tool_calls": [
                        {
                            "id": "a",
                            "type": "function",
                            "function": {
                                "name": "search_rules",
                                "arguments": json.dumps({"query": "focus"}),
                            },
                        }
                    ]
                },
                {
                    "tool_calls": [
                        {
                            "id": "b",
                            "type": "function",
                            "function": {
                                "name": "propose_change",
                                "arguments": json.dumps(
                                    {
                                        "changes": [{"path": "focus", "to": -2}],
                                        "rule_ids": ["FX-1"],
                                        "reason": "",
                                    }
                                ),
                            },
                        }
                    ]
                },
                {"content": '{"kind": "conflict", "message": "too much", "rule_ids": ["FX-1"]}'},
            ]
        )

        def model(messages, seed, case_id=None, turn=None):
            message = dict({"role": "assistant", "content": ""}, **next(turns))
            return {"choices": [{"message": message, "finish_reason": "stop"}]}

        row = tl.run_episode(self.pack, self.index, self.cases["overspend"], model, 1)
        self.assertIsNotNone(row["accepted_proposal"])  # a pending proposal existed
        self.assertEqual(row["outcome"], "no_outcome")
        self.assertFalse(tl.score(self.cases["overspend"], row)["false_supported"])

    def test_submit_proposal_is_the_explicit_proposal(self):
        row, s = self.run_script(
            "spend-focus",
            [
                [SEARCH_FOCUS],
                [
                    call(
                        "propose_change",
                        {
                            "changes": [{"path": "focus", "to": 1}],
                            "rule_ids": ["FX-1"],
                            "reason": "",
                        },
                    )
                ],
                [call("submit_proposal", {"rule_ids": ["FX-1"], "reason": "spend 2"})],
            ],
        )
        self.assertEqual(row["outcome"], "proposal")
        self.assertTrue(s["correct"])

    def test_answer_tool_requires_values(self):
        # Ledger: answer cases scored 0/17 because the optional values were never filled.
        row, s = self.run_script(
            "pace-readback",
            [
                [call("search_rules", {"query": "pace"})],
                [call("answer", {"message": "Pace 5.", "rule_ids": ["FX-3"]})],
                [
                    call(
                        "answer",
                        {"message": "Pace 5.", "values": {"pace": 5}, "rule_ids": ["FX-3"]},
                    )
                ],
            ],
        )
        self.assertEqual(row["malformed_calls"], 1)
        self.assertEqual(row["outcome"], "answer")
        self.assertEqual(s["answer_check"], "verified_correct")
        self.assertTrue(s["correct"])

    def test_defaults(self):
        args = tl.build_parser().parse_args(["--pack", "p"])
        self.assertTrue(args.cache_prompt)
        self.assertFalse(tl.build_parser().parse_args(["--no-cache-prompt"]).cache_prompt)
        self.assertEqual(args.max_tokens, 4096)  # v3 H11 (v2: 2048)

    def test_recall_mean_excludes_not_run(self):
        case = self.cases["spend-focus"]
        ran, _ = self.run_script("spend-focus", [[SEARCH_FOCUS], []])
        skipped = {"case_id": "spend-focus", "seed": 2, "outcome": "not_run"}
        rows = []
        for row in (ran, skipped):
            rows.append(
                dict(row, event="episode", model_label="m", split="dev", score=tl.score(case, row))
            )
        (summary,) = tl.summarize(rows)
        self.assertEqual(summary["retrieval_recall_mean"], {"mean": 1.0, "of": 1, "not_run": 1})


class AnswerKeyDiagnosticTests(Base):
    """Live v2 smoke dev-005: 9B answered {"watches": 12}, expected {"travel_watches": 12}."""

    def answer(self, values):
        return self.run_script(
            "pace-readback",
            [
                [call("search_rules", {"query": "pace"})],
                [call("answer", {"message": "x", "values": values, "rule_ids": ["FX-3"]})],
            ],
        )[1]

    def test_right_value_under_other_key_is_flagged_as_diagnostic(self):
        s = self.answer({"current_pace": 5})
        self.assertEqual(s["answer_check"], "verified_correct")  # scored by value, not key
        self.assertTrue(s["answer_values_key_mismatch"])
        self.assertFalse(self.answer({"current_pace": 4})["answer_values_key_mismatch"])
        self.assertFalse(self.answer({"pace": 5})["answer_values_key_mismatch"])
        rows = []
        for values in ({"current_pace": 5}, {"pace": 5}):
            row, s = self.run_script(
                "pace-readback",
                [[call("answer", {"message": "x", "values": values, "rule_ids": []})]],
            )
            rows.append(dict(row, event="episode", model_label="m", split="dev", score=s))
        (summary,) = tl.summarize(rows)
        self.assertEqual(summary["answer_values_key_mismatch"], {"count": 1, "of": 2, "rate": 0.5})
        self.assertIn("key_mismatch 1/2", tl.format_summary(summary))


class AnswerByValueTests(Base):
    """Decision 2026-10-15: answers are scored by typed value (multiset), not by key."""

    CASE = {
        "id": "dev-005",
        "gold_rule_ids": [],
        "expected": {"outcome": "answer", "answer_values": {"travel_watches": 12}},
    }

    def score(self, values, expected=None):
        case = copy.deepcopy(self.CASE)
        if expected is not None:
            case["expected"]["answer_values"] = expected
        row = {
            "outcome": "answer",
            "retrieved_ids": [],
            "terminal": {"tool": "answer", "message": "x", "rule_ids": [], "values": values},
        }
        return tl.score(case, row)

    def test_dev005_value_under_other_key_is_correct(self):
        s = self.score({"watches": 12})
        self.assertEqual(s["answer_check"], "verified_correct")
        self.assertTrue(s["correct"])
        self.assertFalse(s["false_supported"])
        self.assertTrue(s["answer_values_key_mismatch"])
        self.assertFalse(s["answer_values_key_exact"])
        self.assertTrue(self.score({"travel_watches": 12})["answer_values_key_exact"])

    def test_typed_multiset(self):
        self.assertEqual(self.score({"w": "12"})["answer_check"], "wrong")
        self.assertEqual(self.score({"w": 13})["false_supported_reasons"], ["wrong_answer"])
        both = {"a": 2, "b": 2}
        self.assertEqual(self.score({"x": 2}, both)["answer_check"], "wrong")  # needs two 2s
        self.assertEqual(self.score({"x": 2, "y": 2}, both)["answer_check"], "verified_correct")
        self.assertEqual(self.score({"x": True}, {"a": 1})["answer_check"], "wrong")

    def test_over_cap_dump_fails(self):
        ok = {"a": 12, "b": 1}  # len(expected) + 1 = 2 values allowed (review 2026-10-15)
        self.assertEqual(self.score(ok)["answer_check"], "verified_correct")
        s = self.score({"a": 12, "b": 1, "c": 2})
        self.assertEqual(s["answer_check"], "over_cap")
        self.assertFalse(s["correct"])
        self.assertEqual(s["false_supported_reasons"], ["over_cap"])

    def test_key_exact_rate_reported(self):
        rows = []
        for values in ({"watches": 12}, {"travel_watches": 12}):
            rows.append(
                {
                    "event": "episode",
                    "model_label": "m",
                    "split": "dev",
                    "case_id": "dev-005",
                    "outcome": "answer",
                    "score": self.score(values),
                }
            )
        (summary,) = tl.summarize(rows)
        self.assertEqual(summary["answer_values_key_exact"], {"count": 1, "of": 2, "rate": 0.5})
        self.assertEqual(summary["answer_check"]["verified_correct"]["count"], 2)
        self.assertIn("key_exact 1/2", tl.format_summary(summary))


# ---------------------------------------------------------------- review 2026-10-15 (v2)


FAKE_CLI = r'''#!/usr/bin/env python3
"""Fake formula_eval for infra-fault tests: mode in argv[1]."""
import json, sys
mode = sys.argv[1] if len(sys.argv) > 1 else "ok"
sys.stderr.write("fake-cli starting " + mode + "\n"); sys.stderr.flush()
for n, line in enumerate(sys.stdin):
    op = json.loads(line)["op"]
    if op == "version":
        out = {"ok": {"profile": "cel-mimik-1", "engine": "fake", "fixture_sha256": "0"}}
    elif op == "compile":
        out = {"ok": {"derived_order": ["armor", "pace"], "cost": 1}}
    else:
        if mode == "exit":
            sys.exit(3)
        if mode == "garbage":
            print("this is not json", flush=True); continue
        if mode == "hang":
            import time; time.sleep(60)
        out = {"ok": {"derived": {"armor": {"value": 1}, "pace": {"value": 5}},
                      "constraints": {"focus-range": "pass", "one-outer-armor": "pass"}}}
    print(json.dumps(out), flush=True)
'''


def fake_cli(tmp, mode):
    path = Path(tmp) / f"fake-cli-{mode}"
    path.write_text(FAKE_CLI.replace('sys.argv[1] if len(sys.argv) > 1 else "ok"', repr(mode)))
    path.chmod(0o755)
    return path


class ReviewScoringTests(Base):
    def test_proposal_when_answer_expected_is_wrong_proposal(self):
        row, s = self.run_script(
            "pace-readback",
            [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit()]],
        )
        self.assertEqual(row["outcome"], "proposal")
        self.assertIn("wrong_proposal", s["false_supported_reasons"])
        self.assertTrue(s["false_supported"])

    def test_answer_when_proposal_expected_is_incorrect_not_false_supported(self):
        _, s = self.run_script(
            "spend-focus",
            [[call("answer", {"message": "1", "values": {"focus": 1}, "rule_ids": []})]],
        )
        self.assertFalse(s["correct"] or s["false_supported"])

    def score(self, values, expected):
        case = {
            "id": "x",
            "gold_rule_ids": [],
            "expected": {"outcome": "answer", "answer_values": expected},
        }
        row = {
            "outcome": "answer",
            "retrieved_ids": [],
            "terminal": {"tool": "answer", "message": "", "rule_ids": [], "values": values},
        }
        return tl.score(case, row)

    def test_hedged_booleans_and_enums_fail(self):
        s = self.score({"x": True, "y": False}, {"can_cross": False})
        self.assertEqual(s["answer_check"], "wrong")
        self.assertEqual(s["false_supported_reasons"], ["wrong_answer"])
        self.assertTrue(self.score({"can_cross": False}, {"can_cross": False})["correct"])
        s = self.score({"a": "mail", "b": "plate"}, {"category": "mail"})
        self.assertEqual(s["answer_check"], "wrong")
        self.assertTrue(self.score({"n": 3, "ok": False}, {"n": 3})["correct"])  # other type

    def test_cap_is_one_extra_value(self):
        self.assertTrue(self.score({"a": 12, "b": 1}, {"w": 12})["correct"])
        s = self.score({"a": 12, "b": 1, "c": 2}, {"w": 12})  # old len + 2 cap
        self.assertEqual(s["answer_check"], "over_cap")
        self.assertEqual(s["false_supported_reasons"], ["over_cap"])


class FormulaInfraTests(Base):
    def definition(self):
        return json.loads((FIXTURE / "definition.json").read_text())

    def base_values(self):
        values = json.loads((FIXTURE / "characters" / "hero.json").read_text())["values"]
        return {k: v for k, v in values.items() if k not in ("armor", "pace")}

    def test_cli_faults_are_infra_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            for mode in ("exit", "garbage"):
                engine = tl.CelCliEngine(self.definition(), fake_cli(tmp, mode))
                try:
                    with self.assertRaises(tl.FormulaInfraError, msg=mode):
                        engine.derive(self.base_values())
                    with self.assertRaises(tl.FormulaInfraError, msg=mode):
                        engine.violations(self.base_values())
                    self.assertIn("fake-cli starting", engine.stderr_tail())
                finally:
                    engine.close()
            self.assertFalse(issubclass(tl.FormulaInfraError, tl.FormulaEvalError))

    def test_cli_timeout_is_infra_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            engine = tl.CelCliEngine(self.definition(), fake_cli(tmp, "hang"))
            original = tl.FORMULA_CLI_TIMEOUT
            tl.FORMULA_CLI_TIMEOUT = 0.5
            try:
                with self.assertRaises(tl.FormulaInfraError):
                    engine.derive(self.base_values())
            finally:
                tl.FORMULA_CLI_TIMEOUT = original
                engine.close()

    def test_infra_fault_aborts_run_with_flagged_ledger(self):
        class Dying(FixtureEngine):
            armed = False

            def derive(self, values):
                if Dying.armed:  # armed after pack load: the CLI dies mid-run
                    raise tl.FormulaInfraError("formula CLI exited", stderr_tail="boom")
                return super().derive(values)

        pack = tl.Pack(FIXTURE, engine_factory=Dying)
        Dying.armed = True
        script = {
            c: [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit()]]
            for c in self.cases
        }
        args = tl.build_parser().parse_args(
            ["--pack", str(FIXTURE), "--model-label", "m", "--dry-run", "--output", "x"]
        )
        out = io.StringIO()
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(tl.FormulaInfraError):
            tl.run(args, tl.ScriptedModel(script), out, pack=pack)
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual(rows[-1]["event"], "run_aborted")
        self.assertEqual(rows[-1]["stderr_tail"], "boom")
        self.assertNotIn("run_summary", [r["event"] for r in rows])

    def test_main_exits_nonzero_on_infra_fault(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "ledger.jsonl"
            argv = [
                "--pack",
                str(FIXTURE),
                "--model-label",
                "m",
                "--dry-run",
                "--formula-cli",
                str(fake_cli(tmp, "exit")),
                "--output",
                str(out),
            ]
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                code = tl.main(argv)
            self.assertNotEqual(code, 0)


class FailFastTests(Base):
    def run_with(self, model):
        args = tl.build_parser().parse_args(
            ["--pack", str(FIXTURE), "--model-label", "m", "--dry-run", "--output", "x"]
        )
        out = io.StringIO()
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(tl.RunAborted):
            tl.run(args, model, out, pack=self.pack)
        return [json.loads(line) for line in out.getvalue().splitlines()]

    def test_five_consecutive_errors_abort(self):
        def broken(messages, seed, case_id=None, turn=None):
            raise ConnectionError("refused")

        broken.health = lambda: True
        # The fixture dev split has 6 cases; the run stops after the 5th error.
        rows = self.run_with(broken)
        episodes = [r for r in rows if r["event"] == "episode"]
        self.assertEqual(len(episodes), 5)
        self.assertIn("consecutive", rows[-1]["reason"])

    def test_failed_health_check_aborts(self):
        def broken(messages, seed, case_id=None, turn=None):
            raise ConnectionError("refused")

        broken.health = lambda: False
        rows = self.run_with(broken)
        self.assertEqual(len([r for r in rows if r["event"] == "episode"]), 1)
        self.assertIn("health", rows[-1]["reason"])


class OrderAndMergeTests(Base):
    def test_jobs_interleave_outcomes_proportionally(self):
        cases = [{"id": f"p{i}", "expected": {"outcome": "proposal"}} for i in range(6)]
        cases += [{"id": f"a{i}", "expected": {"outcome": "answer"}} for i in range(3)]
        cases += [{"id": "s0", "expected": {"outcome": "stale"}}]
        order = [c["id"] for c in tl.interleave_by_outcome(cases)]
        self.assertEqual(order, tl_order := [c["id"] for c in tl.interleave_by_outcome(cases)])
        self.assertEqual(sorted(order), sorted(c["id"] for c in cases))
        first_half = order[:5]
        self.assertEqual(sum(i.startswith("p") for i in first_half), 3)
        self.assertIn(sum(i.startswith("a") for i in first_half), (1, 2))
        self.assertEqual(tl_order, order)
        self.assertEqual([i for i in order if i.startswith("p")], [f"p{i}" for i in range(6)])

    def ledger(self, tmp, name, **header):
        path = Path(tmp) / name
        base = {
            "event": "run_started",
            "benchmark_version": 2,
            "model_label": "m",
            "pack_hashes": {"a": "1"},
            "formula_engine": {"cli_sha256": "c"},
            "model_sha256": "m1",
        }
        base.update(header)
        path.write_text(json.dumps(base) + "\n")
        return path

    def test_summarize_refuses_mixed_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            a = self.ledger(tmp, "a.jsonl")
            for i, change in enumerate(
                (
                    {"pack_hashes": {"a": "2"}},
                    {"benchmark_version": 1},
                    {"formula_engine": {"cli_sha256": "d"}},
                    {"model_sha256": "m2"},
                )
            ):
                b = self.ledger(tmp, f"b{i}.jsonl", **change)
                with self.assertRaises(SystemExit, msg=change):
                    tl.load_ledgers([a, b])
            other_model = self.ledger(tmp, "c.jsonl", model_label="n", model_sha256="m9")
            self.assertEqual(len(tl.load_ledgers([a, other_model])), 2)
            self.assertEqual(len(tl.load_ledgers([a, self.ledger(tmp, "d.jsonl")])), 2)


class NeutralToolTextTests(unittest.TestCase):
    def test_tool_descriptions_are_game_agnostic(self):
        import re

        text = json.dumps(tl.TOOLS).lower()
        for word in ("focus", "rope", "pace", "encumbered", "equipment", "armor"):
            self.assertIsNone(re.search(rf"\b{word}", text), word)


class AbortedLedgerTests(unittest.TestCase):
    ledger = OrderAndMergeTests.ledger

    def test_summarize_refuses_aborted_ledger(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self.ledger(tmp, "a.jsonl")
            with path.open("a") as handle:
                handle.write(json.dumps({"event": "run_aborted", "reason": "x"}) + "\n")
            with self.assertRaises(SystemExit):
                tl.load_ledgers([path])
            bare = Path(tmp) / "bare.jsonl"
            bare.write_text(json.dumps({"event": "episode"}) + "\n")
            with self.assertRaises(SystemExit):
                tl.load_ledgers([bare])


class CaseFilterTests(Base):
    """--cases selects named cases (used by the runner's smoke split)."""

    def test_cases_filter_keeps_given_ids_and_rejects_unknown(self):
        argv = ["--pack", str(FIXTURE), "--model-label", "m", "--dry-run", "--output", "x"]
        args = tl.build_parser().parse_args(argv + ["--cases", "spend-focus,overspend"])
        out = io.StringIO()
        with contextlib.redirect_stderr(io.StringIO()):
            (summary,) = tl.run(args, tl.ScriptedModel(), out, pack=self.pack)
        self.assertEqual(summary["episodes"], 2)
        header = json.loads(out.getvalue().splitlines()[0])
        self.assertEqual(sorted(header["case_order"]), ["overspend", "spend-focus"])
        self.assertEqual(header["case_filter"], ["spend-focus", "overspend"])
        args = tl.build_parser().parse_args(argv + ["--cases", "spend-focus,nope"])
        with self.assertRaises(tl.PackError):
            tl.run(args, tl.ScriptedModel(), io.StringIO(), pack=self.pack)
