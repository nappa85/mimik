"""Decision router SYS-R: policy, records, folds, CV, gate, replay, states, decision file.

docs/decision-router-plan.md revision 3.1 §0, §2-§4, §6. New module: red = module absent; the
paired tests (planted leak, interleave order, planted fold leakage, forged entries) carry their
own negative control.
"""

import ast
import copy
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import decision_router as dr
import router_state_builder as builder
import tool_loop_benchmark as tl

DEMO = Path(__file__).resolve().parents[1] / "examples" / "tool-loop-demo"
FORMULA = os.environ.get("MIMIK_FORMULA_EVAL", "")
HAS_CLI = bool(FORMULA) and Path(FORMULA).exists()


def dist(**p):
    out = {k: 0.0 for k in dr.CANONICAL}
    out.update(p)
    return out


def ok(**p):
    return {"status": "completed", "probabilities": dist(**p)}


def failure(status="timeout"):
    return {"status": status, "error_class": status, "probabilities": None}


def demo_cases():
    return dr.pack_cases(DEMO)


# ---------------------------------------------------------------- question and options


class OptionTests(unittest.TestCase):
    def test_option_ids_canonical_and_from_the_system_prompt(self):
        self.assertEqual(list(dr.CHOICES), list(tl.EXPECTED_OUTCOMES))
        prompt = " ".join(tl.SYSTEM_PROMPT.split())
        for key, text in dr.CHOICES.items():
            body = text.removeprefix("ask_user ") if key == "ask_fact" else text
            self.assertIn(body, prompt, key)
        self.assertTrue(dr.CHOICES["ask_rule"].startswith('ask_user kind "rule"'))
        self.assertTrue(dr.CHOICES["ask_fact"].startswith('ask_user kind "fact"'))

    def test_option_hash_stable_across_orders(self):
        self.assertEqual(
            dr.OPTION_HASH,
            dr.question_hash(dr.QUESTION, dict(reversed(list(dr.CHOICES.items())))),
        )
        self.assertEqual(dr.order_ids("reversed"), list(reversed(dr.CANONICAL)))
        changed = tl.SYSTEM_PROMPT.replace(
            "the rules do not settle the question", "no rule settles it"
        )
        self.assertNotEqual(
            dr.question_hash(dr.QUESTION, dr.outcome_options(changed)), dr.OPTION_HASH
        )


# ---------------------------------------------------------------- policy


class PolicyTests(unittest.TestCase):
    def test_threshold_boundaries(self):
        self.assertEqual(
            dr.route(ok(proposal=0.8, conflict=0.2), 0.8), ("proposal", "supported_threshold")
        )
        below = 0.8 - 1e-9
        self.assertEqual(
            dr.route(ok(proposal=below, conflict=1 - below), 0.8), ("conflict", "at_risk_argmax")
        )

    def test_ties_go_to_the_earlier_canonical_id(self):
        self.assertEqual(dr.argmax(dist(proposal=0.5, answer=0.5), dr.CANONICAL), "proposal")
        # Alphabetical (the legacy rule) would pick ask_fact; canonical picks ask_rule.
        tie = dist(ask_rule=0.5, ask_fact=0.5)
        self.assertEqual(
            dr.route({"status": "completed", "probabilities": tie}, 0.5),
            ("ask_rule", "at_risk_argmax"),
        )
        self.assertEqual(
            dr.route(ok(proposal=0.4, conflict=0.3, stale=0.3), dr.NONE),
            ("conflict", "at_risk_argmax"),
        )

    def test_none_routes_no_supported_kind_even_at_one(self):
        self.assertEqual(dr.route(ok(proposal=1.0), dr.NONE), ("ask_rule", "zero_at_risk_mass"))
        self.assertEqual(dr.route(ok(proposal=1.0), 0.99), ("proposal", "supported_threshold"))
        self.assertEqual(
            dr.route(ok(answer=0.9, refuse=0.1), dr.NONE), ("refuse", "at_risk_argmax")
        )

    def test_zero_at_risk_mass_asks_for_a_rule(self):
        self.assertEqual(
            dr.route(ok(proposal=0.6, answer=0.4), 0.7), ("ask_rule", "zero_at_risk_mass")
        )

    def test_failure_routes_ask_rule_is_never_correct_never_unsafe_and_a_recall_miss(self):
        self.assertEqual(dr.route(failure(), 0.5), ("ask_rule", "fallback_failure"))
        routes = {
            "a": {"route": "ask_rule", "reason": "fallback_failure"},
            "b": {"route": "ask_rule", "reason": "fallback_failure"},
        }
        m = dr.route_metrics(routes, {"a": "ask_rule", "b": "proposal"})
        self.assertEqual(m["correct"]["count"], 0)
        self.assertEqual(m["unsafe_all"]["count"], 0)
        self.assertEqual(m["supported_recall"]["count"], 0)
        self.assertEqual(m["failures"], 2)


def score_case(expected_kind):
    case = {"id": "c", "gold_rule_ids": [], "expected": {"outcome": expected_kind}}
    if expected_kind == "proposal":
        case["expected"]["changes"] = [{"field": "hp", "to": 5}]
    if expected_kind == "answer":
        case["expected"]["answer_values"] = {"hp": 5}
    return case


def score_row(route_kind, *, values=None, changes=None, violations=None):
    row = {"outcome": route_kind, "retrieved_ids": [], "accepted_proposal": None, "terminal": None}
    if route_kind == "proposal":
        row["accepted_proposal"] = {
            "changes": changes or [{"path": "hp", "to": 5}],
            "rule_ids": [],
            "preview": {"violations": violations or []},
        }
        row["terminal"] = {"tool": "submit_proposal", "rule_ids": []}
    elif route_kind == "answer":
        row["terminal"] = {
            "tool": "answer",
            "rule_ids": [],
            "message": "",
            "values": values or {"hp": 5},
        }
    elif route_kind in ("ask_rule", "ask_fact"):
        row["terminal"] = {"tool": "ask_user", "rule_ids": []}
    else:
        row["terminal"] = {"tool": "respond", "rule_ids": []}
    return row


class UnsafeThroughRealScoreTests(unittest.TestCase):
    def test_all_49_pairs_value_correct(self):
        """Cross-kind unsafe equals score()'s false support when every value is correct."""
        for r in dr.CANONICAL:
            for e in dr.CANONICAL:
                scored = tl.score(score_case(e), score_row(r))
                self.assertEqual(scored["false_supported"], dr.unsafe(r, e), (r, e))

    def test_value_errors_are_false_support_but_not_router_unsafe(self):
        cases = [
            ("answer", score_row("answer", values={"hp": 6}), "wrong_answer"),
            ("answer", score_row("answer", values={"a": 5, "b": 1, "c": 2}), "over_cap"),
            (
                "proposal",
                score_row("proposal", changes=[{"path": "hp", "to": 6}]),
                "wrong_proposal",
            ),
            ("proposal", score_row("proposal", violations=["v"]), "unreported_violation"),
        ]
        for kind, row, reason in cases:
            scored = tl.score(score_case(kind), row)
            self.assertIn(reason, scored["false_supported_reasons"])
            self.assertFalse(dr.unsafe(kind, kind))


# ---------------------------------------------------------------- records


class RecordTests(unittest.TestCase):
    def test_success_and_failure_schemas(self):
        dr.check_record(ok(proposal=1.0))
        dr.check_record(failure("over_context"))
        with self.assertRaises(dr.RouterError):
            dr.check_record(
                {"status": "timeout", "error_class": "timeout", "probabilities": dist(proposal=1.0)}
            )
        with self.assertRaises(dr.RouterError):
            dr.check_record({"status": "completed", "probabilities": None})
        with self.assertRaises(dr.RouterError):
            dr.check_record({"status": "failed", "probabilities": None})  # no error_class
        with self.assertRaises(dr.IncompleteUnit):
            dr.check_record({"status": "not_run"})

    def test_invalid_distribution_never_becomes_a_prediction(self):
        for probs in (
            dist(proposal=0.5),
            dist(proposal=1.0 + 2e-6),
            {**dist(proposal=1.0), "extra": 0.0},
            dist(proposal=float("nan")),
            dist(proposal=1.1, answer=-0.1),
            dist(proposal=True),
        ):
            rec = dr.normalise_worker_result(
                {"status": "completed", "probabilities": probs}, list(dr.CANONICAL)
            )
            self.assertEqual(rec["status"], "invalid_distribution")
            self.assertIsNone(rec["probabilities"])
        within = dr.normalise_worker_result(
            {"status": "completed", "probabilities": dist(proposal=1.0 + 5e-7)}, list(dr.CANONICAL)
        )
        self.assertEqual(within["status"], "completed")
        mapped = dr.normalise_worker_result(
            {"status": "failed", "error_class": "invalid_distribution"}, []
        )
        self.assertEqual(mapped["status"], "invalid_distribution")
        self.assertEqual(
            dr.normalise_worker_result({"status": "failed", "error_class": "over_context"}, [])[
                "status"
            ],
            "over_context",
        )


# ---------------------------------------------------------------- folds


class FoldTests(unittest.TestCase):
    def test_fold_allocation_reproducible_and_groups_never_split(self):
        cases = demo_cases()
        a, b = dr.allocate_folds(cases), dr.allocate_folds(copy.deepcopy(cases))
        self.assertEqual(a, b)
        self.assertEqual(set(a), {c["id"] for c in cases})
        for _, ids in dr.groups_of(cases):
            self.assertEqual(len({a[i] for i in ids}), 1, ids)
        doc = dr.fold_document(cases)
        self.assertEqual(sum(doc["sizes"]), 174)
        self.assertEqual(doc["sizes"], [sum(1 for f in a.values() if f == i) for i in range(5)])
        for kind in dr.CANONICAL:
            n = sum(1 for c in cases if c["expected"]["outcome"] == kind)
            self.assertEqual(sum(doc["held_per_kind"][kind]), n)
        self.assertEqual(doc["sha256"], dr.fold_document(cases)["sha256"])
        self.assertNotEqual(dr.allocate_folds(cases, seed=1), a)

    def test_groups_use_non_text_metadata_only(self):
        cases = demo_cases()
        mutated = copy.deepcopy(cases)
        for c in mutated:
            c["request"] = "changed text"
            c["language"] = "en"
        self.assertEqual(dr.allocate_folds(cases), dr.allocate_folds(mutated))


def synthetic(n_per_kind=10):
    cases, expected = [], {}
    for kind in dr.CANONICAL:
        for i in range(n_per_kind):
            cid = f"{kind}-{i:02d}"
            cases.append(
                {
                    "id": cid,
                    "character": f"ch{i}",
                    "gold_rule_ids": [cid],
                    "expected": {"outcome": kind},
                }
            )
            expected[cid] = kind
    return cases, expected


def perfect_records(expected, confidence=0.9):
    records = {}
    for cid, kind in expected.items():
        other = "conflict" if kind != "conflict" else "stale"
        records[cid] = ok(**{kind: confidence, other: 1 - confidence})
    return records


class SelectionTests(unittest.TestCase):
    def test_nested_cv_never_sees_the_held_fold(self):
        cases, expected = synthetic()
        folds = dr.allocate_folds(cases)
        records = perfect_records(expected)
        seen = []
        real = dr.select_tau

        def spy(records_, expected_, ids):
            seen.append(set(ids))
            return real(records_, expected_, ids)

        with patch.object(dr, "select_tau", spy):
            dr.cross_validate(records, expected, folds)
        for i, ids in enumerate(seen):
            held = {c for c, f in folds.items() if f == i}
            self.assertFalse(ids & held, i)

        # Pair: a leaky variant that selects on every case is detected by the same check.
        def leaky(records_, expected_, folds_):
            return [set(folds_)] * 5

        self.assertTrue(
            any(
                ids & {c for c, f in folds.items() if f == i}
                for i, ids in enumerate(leaky(records, expected, folds))
            )
        )

    def test_folds_with_different_tau_pool_correctly(self):
        cases, expected = synthetic()
        folds = dr.allocate_folds(cases)
        records = perfect_records(expected)
        # One at-risk case in fold 0 looks like a confident proposal: the inner sets of the
        # other folds contain it and need a higher tau; fold 0's inner set does not.
        bad = next(c for c, f in folds.items() if f == 0 and expected[c] == "conflict")
        records[bad] = ok(proposal=0.75, conflict=0.25)
        cv = dr.cross_validate(records, expected, folds)
        taus = [f["tau"] for f in cv["folds"]]
        self.assertEqual(taus[0], 0.5)
        self.assertTrue(all(t == 0.8 for t in taus[1:]))
        self.assertEqual(cv["routes"][bad]["route"], "proposal")  # out-of-fold, unsafe
        self.assertEqual(len(cv["routes"]), len(expected))
        oof = dr.route_metrics(cv["routes"], expected)
        self.assertEqual(oof["unsafe_all"]["count"], 1)

    def test_infeasible_fold_gets_none_and_the_gate_fails_on_recall(self):
        cases, expected = synthetic()
        folds = dr.allocate_folds(cases)
        records = perfect_records(expected)
        for cid in [c for c in expected if expected[c] == "conflict"][:2]:
            records[cid] = ok(proposal=1.0)  # unsafe at every tau
        cv = dr.cross_validate(records, expected, folds)
        self.assertTrue(all(f["tau"] == dr.NONE for f in cv["folds"] if f["inner_unsafe"]["0.99"]))
        oof = dr.route_metrics(cv["routes"], expected)
        g = dr.gate(True, oof, n_supported=20, n_cases=70)
        self.assertFalse(g["passed"])
        self.assertTrue(any("recall" in r for r in g["reasons"]))

    def test_gate_needs_completeness_zero_unsafe_recall_and_coverage(self):
        cases, expected = synthetic()
        records = perfect_records(expected)
        oof = dr.route_metrics(
            dr.cross_validate(records, expected, dr.allocate_folds(cases))["routes"], expected
        )
        self.assertTrue(dr.gate(True, oof, 20, 70)["passed"])
        self.assertFalse(dr.gate(False, oof, 20, 70)["passed"])
        records2 = dict(records)
        for cid in list(expected)[:5]:
            records2[cid] = failure()
        oof2 = dr.route_metrics(
            dr.cross_validate(records2, expected, dr.allocate_folds(cases))["routes"], expected
        )
        g = dr.gate(True, oof2, 20, 70)
        self.assertFalse(g["passed"])
        self.assertTrue(any("failed decisions" in r for r in g["reasons"]))
        self.assertEqual(dr.gate(True, oof, 105, 174)["failures_allowed"], 8)
        self.assertEqual(dr.gate(True, oof, 105, 174)["recall_needed"], 84)

    def test_refit_uses_all_cases_and_none_is_null(self):
        cases, expected = synthetic()
        records = perfect_records(expected)
        r = dr.refit(records, expected)
        self.assertEqual(r["tau_star"], 0.5)
        self.assertEqual(len(r["routes"]), len(expected))
        records[next(c for c in expected if expected[c] == "stale")] = ok(answer=1.0)
        self.assertTrue(dr.refit(records, expected)["null"])

    def test_router_choice_tie_rules(self):
        def rep(router, recall, seconds):
            return {
                "router": router,
                "gate": {"passed": True},
                "refit": {"null": False},
                "cv": {"out_of_fold": {"supported_recall": {"count": recall}}},
                "median_seconds_per_decision": seconds,
            }

        self.assertEqual(dr.choose([rep("R4", 90, 1), rep("R1", 91, 9)])["choice"], "R1")
        self.assertEqual(dr.choose([rep("R1", 90, 9), rep("R4", 90, 1)])["choice"], "R4")
        self.assertEqual(dr.choose([rep("R3", 90, 5), rep("R4", 90, 5)])["choice"], "R4")
        self.assertEqual(dr.choose([rep("R4", 90, 5), rep("R1", 90, 5)])["choice"], "R1")
        failing = rep("R1", 99, 1)
        failing["gate"]["passed"] = False
        self.assertEqual(dr.choose([failing])["result"], "null")

    def test_baselines_and_falsifier(self):
        cases, expected = synthetic()
        b = dr.route_metrics(dr.baseline_routes("conflict", expected), expected)
        self.assertEqual(b["correct"]["count"], 10)
        self.assertEqual(b["unsafe_all"]["count"], 0)
        maj = dr.route_metrics(dr.baseline_routes("proposal", expected), expected)
        self.assertEqual(maj["unsafe_all"]["count"], 60)
        good = dr.falsifier(perfect_records(expected), expected)
        self.assertFalse(good["null"])
        always_conflict = {c: ok(conflict=1.0) for c in expected}
        self.assertTrue(dr.falsifier(always_conflict, expected)["null"])

    def test_ece_and_brier_known_values(self):
        expected = {"a": "proposal", "b": "proposal"}
        records = {"a": ok(proposal=1.0), "b": ok(answer=1.0)}
        cal = dr.calibration(records, expected)
        self.assertAlmostEqual(cal["ece"], 0.5)
        self.assertAlmostEqual(cal["brier"], 1.0)
        self.assertEqual(cal["risk_coverage"][-1], {"coverage": 1.0, "risk": 0.5})
        half = {"a": ok(proposal=0.5, conflict=0.5)}
        self.assertAlmostEqual(dr.calibration(half, {"a": "proposal"})["brier"], 0.5)


# ---------------------------------------------------------------- replay (§0, §4)


def episode(cid, outcome, correct, fs, seed=17):
    return {
        "case_id": cid,
        "seed": seed,
        "outcome": outcome,
        "score": {"correct": correct, "false_supported": fs},
    }


class ReplayTests(unittest.TestCase):
    cases = {
        "p1": {"expected": {"outcome": "proposal"}},
        "p2": {"expected": {"outcome": "proposal"}},
        "a1": {"expected": {"outcome": "answer"}},
        "c1": {"expected": {"outcome": "conflict"}},
        "r1": {"expected": {"outcome": "ask_rule"}},
    }

    def test_oracle_replay_on_a_synthetic_ledger(self):
        eps = [
            episode("p1", "proposal", True, False),
            episode("p2", "proposal", False, True),  # value error: resolved false support
            episode("a1", "proposal", False, True),  # other supported kind: unresolved
            episode("c1", "answer", False, True),  # cross-kind: removed by gold routing
            episode("r1", "answer", False, True),
        ]
        cases = {k: dict(v, id=k) for k, v in self.cases.items()}
        t = dr.replay(eps, cases, dr.oracle_routes(cases.values()))["totals"]
        self.assertEqual(t["resolved_correct"], 3)
        self.assertEqual(t["resolved_false_support"], 1)
        self.assertEqual(t["replay_unresolved"], 1)
        self.assertEqual(t["worst_case_false_support_bound"], 2)
        self.assertIn("worst-case bound", t["bound_line"])

    def test_unresolved_never_enters_false_support_and_failures_have_a_column(self):
        eps = [episode("a1", "proposal", False, True), episode("r1", "ask_rule", True, False)]
        routes = {"a1": {"route": "answer"}, "r1": {"route": "ask_rule", "failure": True}}
        res = dr.replay(eps, self.cases, routes)
        t = res["totals"]
        self.assertEqual(t["resolved_false_support"], 0)
        self.assertEqual(t["replay_unresolved"], 1)
        self.assertEqual(t["router_failure"], 1)
        # A6: a router failure is never correct, even on an ask_rule case.
        self.assertFalse(res["rows"][1]["correct"])
        self.assertEqual(t["resolved_correct"], 0)


# ---------------------------------------------------------------- isolated states


class BuilderIsolationTests(unittest.TestCase):
    def test_builder_has_no_expected_import(self):
        source = (Path(dr.TOOLS) / "router_state_builder.py").read_text(encoding="utf-8")
        tree = ast.parse(source)
        imports = {a.name for n in ast.walk(tree) if isinstance(n, ast.Import) for a in n.names}
        imports |= {n.module for n in ast.walk(tree) if isinstance(n, ast.ImportFrom)}
        self.assertEqual(
            imports, {"argparse", "hashlib", "json", "sys", "pathlib", "tool_loop_benchmark"}
        )
        for word in (
            "expected",
            "gold",
            "tags",
            "language",
            "interleave",
            "cases-dev",
            "public_case",
        ):
            self.assertNotIn(word, source.lower(), word)

    def test_derived_schema_refuses_any_extra_key(self):
        good = {
            "encoding": 1,
            "entries": [{"key": "k0", "character": "c", "request": "r", "setup": {}}],
        }
        builder.check_derived(good)
        for extra in ("id", "expected", "gold_rule_ids", "tags", "language"):
            bad = copy.deepcopy(good)
            bad["entries"][0][extra] = "x"
            with self.assertRaises(ValueError):
                builder.check_derived(bad)

    def test_caseless_pack_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "cases-dev.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "case files"):
                builder.open_caseless_pack(Path(tmp))

    def test_order_shuffled_seeded_and_interleave_refused(self):
        cases = demo_cases()
        derived, mapping = dr.derive(cases)
        again, mapping2 = dr.derive(copy.deepcopy(cases))
        self.assertEqual(mapping, mapping2)
        order = list(mapping.values())
        self.assertNotEqual(order, [c["id"] for c in cases])
        self.assertNotEqual(order, [c["id"] for c in tl.interleave_by_outcome(cases)])
        for entry in derived["entries"]:
            self.assertEqual(set(entry), builder.DERIVED_KEYS)
        with self.assertRaisesRegex(dr.RouterError, "interleave"):
            dr.derive(cases, order_fn=lambda cs, seed: tl.interleave_by_outcome(cs))


@unittest.skipUnless(HAS_CLI, "needs MIMIK_FORMULA_EVAL (formula CLI)")
class StateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        root = Path(cls.tmp.name)
        cls.pack = dr.caseless_copy(DEMO, root / "pack")
        cls.open = builder.open_caseless_pack(cls.pack, FORMULA)
        cls.index = tl.RuleIndex(cls.open.rules, 5)

    @classmethod
    def tearDownClass(cls):
        cls.open.close()
        cls.tmp.cleanup()

    def entry(self, case):
        return {
            "key": "k",
            "character": case["character"],
            "request": case["request"],
            "setup": case.get("setup", {}),
        }

    def test_state_is_the_c0_context_plus_retrieved_rules(self):
        case = next(c for c in demo_cases() if c.get("setup", {}).get("pending_proposal"))
        state = json.loads(builder.render_state(self.open, self.index, self.entry(case)))
        episode = tl.Episode(self.open, self.index, {**self.entry(case), "id": "x"}, tl.BASE_FLAGS)
        c0 = json.loads(episode.messages()[1]["content"].split("\n", 1)[1])
        self.assertEqual({k: v for k, v in state.items() if k != "retrieved_rules"}, c0)
        self.assertIn("pending_proposal", state)

    def test_retrieval_query_is_request_only(self):
        case = demo_cases()[0]
        calls = []
        real = tl.RuleIndex.search

        def spy(index, query):
            calls.append(query)
            return real(index, query)

        with patch.object(tl.RuleIndex, "search", spy):
            builder.render_state(self.open, self.index, self.entry(case))
        self.assertEqual(calls, [case["request"]])

    def test_retrieved_rules_format_order_and_count(self):
        texts = {r["rule_id"]: r for r in self.open.rules}
        counts = set()
        for case in demo_cases()[:40]:
            rules = json.loads(builder.render_state(self.open, self.index, self.entry(case)))[
                "retrieved_rules"
            ]
            counts.add(len(rules))
            self.assertLessEqual(len(rules), 5)
            self.assertEqual(rules, self.index.search(case["request"])[1])
            for rule in rules:
                self.assertEqual(set(rule), {"rule_id", "title", "text"})
                self.assertEqual(rule["text"], texts[rule["rule_id"]]["text"])
        empty = json.loads(
            builder.render_state(
                self.open, self.index, {**self.entry(demo_cases()[0]), "request": "!!!"}
            )
        )
        self.assertEqual(empty["retrieved_rules"], [])

    def test_sentinels_absent_from_every_prompt(self):
        cases = dr.sentinel_cases(demo_cases())
        derived, _ = dr.derive(cases)
        rows = builder.build(self.open, builder.check_derived(derived))
        texts = []
        for row in rows:
            for variant in ("full", "request_only", "empty"):
                for order in ("canonical", "reversed"):
                    texts.append(
                        dr.reference_prompt(dr.state_text(row, variant), dr.order_ids(order))
                    )
        self.assertEqual(len(texts), 174 * 6)
        self.assertEqual(dr.scan_for_sentinels(texts), [])
        # Pair: a planted leak (the case id in the state) is caught.
        leaked = [dr.reference_prompt(rows[0]["state"] + cases[0]["id"], list(dr.CANONICAL))]
        self.assertEqual(len(dr.scan_for_sentinels(leaked)), 1)

    def test_prepare_isolated_process_writes_states_plans_and_host_mapping(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = dr.prepare(DEMO, Path(tmp), FORMULA)
            prepared, host = Path(tmp) / "prepared", Path(tmp) / "host"
            states = dr.read_states(prepared / "states.jsonl")
            mapping = json.loads((host / "mapping.json").read_text())
            self.assertEqual(set(states), set(mapping))
            self.assertEqual(len(states), 174)
            screen = json.loads((prepared / "plan-screen.json").read_text())["decisions"]
            self.assertEqual(len(screen), 214)
            ablation = json.loads((prepared / "plan-ablation.json").read_text())["decisions"]
            self.assertEqual(len(ablation), 80)
            self.assertFalse(
                any(cid in (prepared / "states.jsonl").read_text() for cid in mapping.values())
            )
            self.assertFalse((prepared / "mapping.json").exists())
            self.assertEqual(manifest["option_hash"], dr.OPTION_HASH)


# ---------------------------------------------------------------- decision file


PROVENANCE = {
    "router": "R-test",
    "runtime": "anyjev-llamacpp",
    "image": "sha256:" + "0" * 64,
    "library_revision": "x",
    "artefacts": [],
    "device": "cpu",
    "device_provenance": {"device": "cpu"},
    "threads": 6,
    "n_ctx": 8192,
    "retrieval": {"top_k": 5, "query": "request"},
    "state_builder_sha256": "0" * 64,
    "fold_seed": 20261016,
    "fold_sha256": "0" * 64,
    "gate": {"passed": True},
    "pack_hashes": {},
}


def decision_doc(expected, tau=0.5, records=None):
    records = records or perfect_records(expected)
    entries = []
    for cid in sorted(expected):
        routed, reason = dr.route(records[cid], tau)
        entries.append(
            {
                "case_id": cid,
                "state_hash": f"h-{cid}",
                "record": records[cid],
                "route": routed,
                "reason": reason,
            }
        )
    header = {
        **PROVENANCE,
        "encoding": 1,
        "kind": "sys-r-decisions",
        "policy": dr.policy_document(tau),
        "policy_hash": tl.digest(dr.policy_document(tau)),
        "question_hash": dr.OPTION_HASH,
    }
    return {"header": header, "entries": entries}


class DecisionFileTests(unittest.TestCase):
    def setUp(self):
        _, self.expected = synthetic(2)
        self.ids = sorted(self.expected)

    def verify(self, doc):
        return dr.verify_decision_file(doc, self.ids, lambda cid: f"h-{cid}")

    def test_valid_file_verifies(self):
        self.assertEqual(len(self.verify(decision_doc(self.expected))), len(self.ids))

    def test_refusals(self):
        def forge(change):
            doc = decision_doc(self.expected)
            change(doc)
            with self.assertRaises(dr.RouterError):
                self.verify(doc)

        forge(lambda d: d["entries"].pop())  # missing
        forge(lambda d: d["entries"].append(copy.deepcopy(d["entries"][0])))  # duplicate
        forge(lambda d: d["entries"][0]["record"].update(status="not_run"))
        forge(lambda d: d["entries"][0]["record"].update(probabilities=dist(proposal=0.5)))
        forge(
            lambda d: d["entries"][0].update(
                record={
                    "status": "timeout",
                    "error_class": "t",
                    "probabilities": dist(proposal=1.0),
                }
            )
        )
        forge(
            lambda d: d["entries"][0].update(record={"status": "completed", "probabilities": None})
        )
        forge(lambda d: d["entries"][0].update(route="proposal"))
        forge(lambda d: d["entries"][0].update(reason="at_risk_argmax"))
        forge(lambda d: d["entries"][0].update(state_hash="other"))
        forge(lambda d: d["header"]["policy"].update(tau_star=0.6))  # policy hash mismatch
        forge(lambda d: d["header"].update(question_hash="0" * 64))
        forge(lambda d: d["header"].pop("device_provenance"))  # missing provenance
        forge(lambda d: d["header"].update(gate={"passed": False}))

    def test_failure_entry_routes_ask_rule(self):
        records = perfect_records(self.expected)
        records[self.ids[0]] = failure()
        doc = decision_doc(self.expected, records=records)
        seen = self.verify(doc)
        self.assertEqual(
            (seen[self.ids[0]]["route"], seen[self.ids[0]]["reason"]),
            ("ask_rule", "fallback_failure"),
        )


if __name__ == "__main__":
    unittest.main()


class HostSideTests(unittest.TestCase):
    def test_option_hash_equals_the_worker_function(self):
        import local_decision_benchmark as bench

        self.assertEqual(dr.OPTION_HASH, bench.question_hash(dr.QUESTION, dr.CHOICES))

    def test_host_module_imports_no_neural_or_jsonschema_dependency(self):
        tree = ast.parse((Path(dr.TOOLS) / "decision_router.py").read_text(encoding="utf-8"))
        imports = {a.name for n in ast.walk(tree) if isinstance(n, ast.Import) for a in n.names}
        self.assertNotIn("local_decision_benchmark", imports)
        self.assertNotIn("local_rag_benchmark", imports)

    def test_unit_timeouts_need_a_complete_probe(self):
        probe = {
            "unit": "probe",
            "complete": True,
            "planned": 3,
            "statuses": {"completed": 3},
            "decision_seconds": {"max": 120.0},
            "disk_rate": 12e6,
        }
        out = dr.unit_timeouts("screen", 2_500_000_000, probe)
        self.assertEqual(out["decision_timeout"], 360)
        self.assertAlmostEqual(out["ready_timeout"], 2_500_000_000 / 12e6 + 600)
        for bad in (
            None,
            dict(probe, complete=False),
            dict(probe, statuses={"completed": 2}),
            dict(probe, unit="screen"),
        ):
            with self.assertRaises(dr.RouterError):
                dr.unit_timeouts("screen", 1, bad)
        self.assertEqual(dr.unit_timeouts("probe", 5_000_000)["decision_timeout"], 1800)


class RuntimeHeaderTests(unittest.TestCase):
    def test_runtime_header_from_registry_and_worker_record(self):
        import decision_models

        entry = decision_models.load()["routers"]["r1-qwen3-4b"]
        summary = {
            "starts": [
                {"ok": False},
                {
                    "ok": True,
                    "ready": {"device_provenance": {"device": "cpu", "offloaded_layers": 0}},
                },
            ]
        }
        header = dr.runtime_header(entry, summary)
        self.assertEqual(header["device_provenance"]["offloaded_layers"], 0)
        self.assertEqual(header["artefacts"][0]["sha256"], entry["files"][0]["sha256"])
        self.assertTrue(set(header) <= set(dr.REQUIRED_HEADER))
