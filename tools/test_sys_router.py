"""Live SYS-R in the harness (decision-router plan §4, rev. 3.1): on/off pairs. Failing-first.

Before this change the harness had no --router-decisions flag, no arm_type in the summary key
or the bar, and no router terminal. The byte-identity test pins the v3 C0 dry-run ledger of
the demo dev split (two seeds, expected replay) to the digest produced by the unchanged
pre-router harness merged with master f9d34e0, on the rev3 pack (computed with this file's own normalisation), so the router code must
leave C0 byte-identical when it is off. A pack change legitimately changes the digest: then
recompute it by running c0_ledger_text with the previous commit's tool_loop_benchmark on the new
pack (docs/decision-router-implementation.md), never by copying the current output.
"""

import copy
import hashlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

import decision_router as dr
import router_state_builder as builder
import tool_loop_benchmark as tl
from test_tool_loop_benchmark import FIXTURE, SEARCH_FOCUS, call, open_pack, propose, submit

DEMO = Path(__file__).resolve().parents[1] / "examples" / "tool-loop-demo"
FORMULA = os.environ.get("MIMIK_FORMULA_EVAL", "")
HAS_CLI = bool(FORMULA) and Path(FORMULA).exists()
# Normalised C0 dry-run ledger of the rev3 demo dev split, seeds 17 and 29, expected replay, from
# the harness without the router: harness-v3 before the router (46e93b2) merged with master
# f9d34e0 (tool_loop_benchmark.py 2bc70c30...). Master's own harness (c9725045...) gives the same
# ledger except header "device" (null: master lacks the CPU device mode of 6324fad).
GOLDEN_C0 = "06a7c6cc9ba1d61210aa4f43172389120370e0a334e4fb4c3c1b2a0c85a8ab34"


def c0_ledger_text(tl_module, pack_root, formula_cli, extra=()):
    """The ledger of a deterministic dry run, normalised: no harness sha, no engine path."""
    argv = ["--pack", str(pack_root), "--model-label", "golden", "--dry-run", "--dry-run-expected"]
    argv += ["--output", "/dev/null", "--seeds", "17,29", "--formula-cli", str(formula_cli), *extra]
    args = tl_module.build_parser().parse_args(argv)
    args.server_config_doc = None
    args.case_subset_doc = None
    pack = tl_module.open_pack(args)
    try:
        scripts = {
            c["id"]: tl_module.expected_replay_script(c) for cs in pack.cases.values() for c in cs
        }
        requests = {c["id"]: c["request"] for cs in pack.cases.values() for c in cs}
        model = tl_module.ScriptedModel(scripts, requests)
        out = io.StringIO()
        tick = iter(range(10**9))
        tl_module.run(
            args, model, out, clock=lambda: float(next(tick)), pack=pack, wall=lambda: 1.0e9
        )
    finally:
        pack.close()
    rows = [json.loads(line) for line in out.getvalue().splitlines()]
    rows[0].pop("harness_sha256")
    rows[0].pop("formula_engine")
    return json.dumps(rows, sort_keys=True)


@unittest.skipUnless(HAS_CLI, "needs MIMIK_FORMULA_EVAL (formula CLI)")
class ByteIdentityTests(unittest.TestCase):
    def test_router_off_ledger_is_byte_identical_to_v3_c0(self):
        text = c0_ledger_text(tl, DEMO, FORMULA)
        self.assertEqual(hashlib.sha256(text.encode()).hexdigest(), GOLDEN_C0)
        self.assertNotIn('"arm_type"', text)
        self.assertNotIn('"router', text)


# ---------------------------------------------------------------- fixture decision files


def dist(**p):
    out = {k: 0.0 for k in dr.CANONICAL}
    out.update(p)
    return out


RECORDS = {
    "spend-focus": {"status": "completed", "probabilities": dist(proposal=0.9, conflict=0.1)},
    "swap-armor": {"status": "completed", "probabilities": dist(answer=0.9, conflict=0.1)},
    "overspend": {"status": "completed", "probabilities": dist(conflict=0.9, proposal=0.1)},
    "rest-amount": {"status": "timeout", "error_class": "timeout", "probabilities": None},
    "pace-readback": {"status": "completed", "probabilities": dist(answer=0.8, refuse=0.2)},
    "stale-pending": {"status": "completed", "probabilities": dist(stale=0.7, proposal=0.3)},
}


def state_hash(pack, case):
    index = tl.RuleIndex(pack.rules, tl.DEFAULT_TOP_K)
    entry = {
        "key": "k",
        "character": case["character"],
        "request": case["request"],
        "setup": case.get("setup", {}),
    }
    return builder.text_hash(builder.render_state(pack, index, entry))


def decision_file(pack, tau=0.5, records=RECORDS):
    entries = []
    for case in pack.cases["dev"]:
        record = records[case["id"]]
        routed, reason = dr.route(record, tau)
        entries.append(
            {
                "case_id": case["id"],
                "state_hash": state_hash(pack, case),
                "record": record,
                "route": routed,
                "reason": reason,
            }
        )
    header = {
        **{
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
        },
        "encoding": 1,
        "kind": "sys-r-decisions",
        "split": "dev",
        "router": "R-test",
        "policy": dr.policy_document(tau),
        "policy_hash": tl.digest(dr.policy_document(tau)),
        "question_hash": dr.OPTION_HASH,
    }
    return {"header": header, "entries": entries}


class CountingModel(tl.ScriptedModel):
    def __init__(self, scripts):
        super().__init__(scripts)
        self.calls = []

    def __call__(self, messages, seed, case_id=None, turn=None, **kw):
        self.calls.append((case_id, turn, kw.get("tools")))
        return super().__call__(messages, seed, case_id=case_id, turn=turn)


SCRIPTS = {
    "spend-focus": [[SEARCH_FOCUS], [propose([{"path": "focus", "to": 1}])], [submit()]],
    "swap-armor": [
        [SEARCH_FOCUS],
        [call("answer", {"message": "m", "values": {"x": 1}, "rule_ids": []})],
    ],
    "pace-readback": [[call("answer", {"message": "m", "values": {"pace": 5}, "rule_ids": []})]],
}


class SysRouterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pack = open_pack()

    @classmethod
    def tearDownClass(cls):
        cls.pack.close()

    def write(self, doc):
        tmp = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        self.addCleanup(os.unlink, tmp.name)
        json.dump(doc, tmp)
        tmp.close()
        return Path(tmp.name), tl.file_digest(Path(tmp.name))

    def args(self, label="SYS-R-test", router=None, sha=None, split="dev"):
        argv = [
            "--pack",
            str(FIXTURE),
            "--model-label",
            label,
            "--dry-run",
            "--output",
            "/dev/null",
            "--split",
            split,
            "--seeds",
            "17",
        ]
        if router:
            argv += ["--router-decisions", str(router), "--router-decisions-sha256", sha]
        args = tl.build_parser().parse_args(argv)
        args.server_config_doc = None
        args.case_subset_doc = None
        return args

    def run_rows(self, args, model):
        out = io.StringIO()
        tick = iter(range(10**9))
        tl.prepare_router(args, self.pack)
        tl.run(
            args, model, out, clock=lambda: float(next(tick)), pack=self.pack, wall=lambda: 1.0e9
        )
        return [json.loads(line) for line in out.getvalue().splitlines()]

    def test_router_off_calls_the_model_as_before(self):
        model = CountingModel(SCRIPTS)
        rows = self.run_rows(self.args(label="m"), model)
        self.assertTrue(all(tools is None for _, _, tools in model.calls))
        self.assertNotIn("arm_type", rows[0])
        self.assertTrue(all("router" not in r for r in rows if r["event"] == "episode"))

    def test_router_on_changes_only_routed_cases(self):
        path, sha = self.write(decision_file(self.pack))
        off = {
            r["case_id"]: r
            for r in self.run_rows(self.args(label="m"), CountingModel(SCRIPTS))
            if r["event"] == "episode"
        }
        model = CountingModel(SCRIPTS)
        rows = self.run_rows(self.args(router=path, sha=sha), model)
        header, eps = rows[0], {r["case_id"]: r for r in rows if r["event"] == "episode"}
        self.assertEqual(header["arm_type"], "system")
        self.assertEqual(header["router_decisions_sha256"], sha)
        # spend-focus: routed proposal, the model used the allowed finisher: same episode.
        same = {
            k: v
            for k, v in eps["spend-focus"].items()
            if k not in ("router", "arm_type", "model_label")
        }
        ref = {k: v for k, v in off["spend-focus"].items() if k != "model_label"}
        self.assertEqual(same, ref)
        self.assertEqual(eps["overspend"]["terminal"]["tool"], "router")
        self.assertEqual(eps["overspend"]["outcome"], "conflict")
        self.assertEqual(eps["stale-pending"]["outcome"], "stale")

    def test_router_terminal_makes_no_model_call(self):
        path, sha = self.write(decision_file(self.pack))
        model = CountingModel(SCRIPTS)
        self.run_rows(self.args(router=path, sha=sha), model)
        called = {c for c, _, _ in model.calls}
        self.assertNotIn("overspend", called)
        self.assertNotIn("stale-pending", called)
        self.assertNotIn("rest-amount", called)

    def test_removed_finisher_is_rejected_through_real_dispatch(self):
        path, sha = self.write(decision_file(self.pack))
        model = CountingModel(SCRIPTS)
        rows = self.run_rows(self.args(router=path, sha=sha), model)
        swap = next(r for r in rows if r.get("case_id") == "swap-armor" and r["event"] == "episode")
        # swap-armor is routed answer: submit_proposal is removed; spend-focus (proposal): answer removed.
        tools = {c: t for c, _, t in model.calls}
        self.assertNotIn("submit_proposal", [t["function"]["name"] for t in tools["swap-armor"]])
        self.assertNotIn("answer", [t["function"]["name"] for t in tools["spend-focus"]])
        self.assertEqual(swap["outcome"], "answer")
        # A routed-proposal case whose model calls answer is refused by dispatch.
        scripts = dict(
            SCRIPTS,
            **{
                "spend-focus": [
                    [call("answer", {"message": "m", "values": {"x": 1}, "rule_ids": []})]
                ]
            },
        )
        rows = self.run_rows(self.args(router=path, sha=sha), CountingModel(scripts))
        spend = next(
            r for r in rows if r.get("case_id") == "spend-focus" and r["event"] == "episode"
        )
        self.assertNotEqual(spend["outcome"], "answer")
        self.assertEqual(spend["rejected_calls"], 1)
        result = json.loads(spend["transcript"][3]["content"])
        self.assertFalse(result["ok"])
        self.assertIn("not available", result["error"])

    def test_failure_record_routes_ask_rule_flagged_and_never_correct(self):
        path, sha = self.write(decision_file(self.pack))
        rows = self.run_rows(self.args(router=path, sha=sha), CountingModel(SCRIPTS))
        rest = next(
            r for r in rows if r.get("case_id") == "rest-amount" and r["event"] == "episode"
        )
        self.assertEqual(rest["outcome"], "ask_rule")
        self.assertTrue(rest["router"]["failure"])
        self.assertEqual(rest["score"]["expected_outcome"], "ask_rule")
        self.assertFalse(rest["score"]["correct"])
        self.assertTrue(rest["score"]["router_failure"])

    def test_decision_file_refusals(self):
        doc = decision_file(self.pack)
        path, sha = self.write(doc)
        with self.assertRaisesRegex(SystemExit, "sha256"):
            self.run_rows(self.args(router=path, sha="0" * 64), CountingModel(SCRIPTS))
        missing = copy.deepcopy(doc)
        missing["entries"].pop()
        p2, s2 = self.write(missing)
        with self.assertRaisesRegex(SystemExit, "missing"):
            self.run_rows(self.args(router=p2, sha=s2), CountingModel(SCRIPTS))
        wrong = copy.deepcopy(doc)
        wrong["entries"][0]["state_hash"] = "0" * 64
        p3, s3 = self.write(wrong)
        with self.assertRaisesRegex(SystemExit, "state hash"):
            self.run_rows(self.args(router=p3, sha=s3), CountingModel(SCRIPTS))
        forged = copy.deepcopy(doc)
        forged["entries"][2]["route"] = "proposal"
        p4, s4 = self.write(forged)
        with self.assertRaisesRegex(SystemExit, "route or reason"):
            self.run_rows(self.args(router=p4, sha=s4), CountingModel(SCRIPTS))

    def test_router_needs_a_system_label_and_the_dev_split(self):
        path, sha = self.write(decision_file(self.pack))
        with self.assertRaisesRegex(SystemExit, "SYS-"):
            self.run_rows(self.args(label="model", router=path, sha=sha), CountingModel(SCRIPTS))
        with self.assertRaisesRegex(SystemExit, "dev"):
            tl.prepare_router(self.args(router=path, sha=sha, split="heldout"), self.pack)
        with self.assertRaisesRegex(SystemExit, "SYS-"):
            self.run_rows(self.args(label="SYS-model"), CountingModel(SCRIPTS))

    def system_ledger(self):
        path, sha = self.write(decision_file(self.pack))
        rows = self.run_rows(self.args(router=path, sha=sha), CountingModel(SCRIPTS))
        return rows

    def test_summary_key_and_bar_isolation(self):
        sys_rows = [r for r in self.system_ledger() if r["event"] == "episode"]
        model_rows = [
            dict(r, model_label="SYS-R-test")
            for r in self.run_rows(self.args(label="m"), CountingModel(SCRIPTS))
            if r["event"] == "episode"
        ]
        summaries = tl.summarize(sys_rows + model_rows)
        self.assertEqual(sorted(s.get("arm_type", "model") for s in summaries), ["model", "system"])
        system = next(s for s in summaries if s.get("arm_type") == "system")
        self.assertEqual(system["bar"]["verdict"], "N/A (system arm)")
        held = dict(system, split="heldout")
        self.assertEqual(tl.bar_verdict(held)["verdict"], "N/A (system arm)")
        model = next(s for s in summaries if s.get("arm_type") is None)
        self.assertEqual(
            tl.bar_verdict(dict(model, split="heldout", unique_cases=200))["verdict"], "PENDING"
        )
        self.assertEqual(system["router_failures"], 1)

    def ledger_file(self, rows):
        tmp = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
        self.addCleanup(os.unlink, tmp.name)
        tmp.write("".join(json.dumps(r) + "\n" for r in rows))
        tmp.close()
        return tmp.name

    def test_forged_relabel_refused(self):
        rows = self.system_ledger()
        tl.load_ledgers([self.ledger_file(rows)])  # the genuine system ledger loads
        forged = copy.deepcopy(rows)
        forged[0].pop("arm_type")
        for r in forged:
            r.pop("arm_type", None)
            r["model_label"] = "relabelled"
        forged[0]["model_label"] = "relabelled"
        with self.assertRaisesRegex(SystemExit, "router"):
            tl.load_ledgers([self.ledger_file(forged)])
        unlabelled = copy.deepcopy(rows)
        for r in unlabelled:
            if "model_label" in r:
                r["model_label"] = "plain"
        with self.assertRaisesRegex(SystemExit, "SYS-"):
            tl.load_ledgers([self.ledger_file(unlabelled)])


if __name__ == "__main__":
    unittest.main()
