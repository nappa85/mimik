"""Isolated router state builder (docs/decision-router-plan.md §2). Prototype tier.

It runs in its own process over a derived case file that holds only `key`, `character`,
`request` and `setup` per entry, and over a pack directory that holds no case file at all, so
nothing scoring-related can reach a router prompt. It renders every state through the harness's
C0 context path (`Episode.messages`, BASE_FLAGS) and adds one key, `retrieved_rules`: what
`RuleIndex(pack.rules, 5).search(request)` returns (0 to 5 `{rule_id, title, text}` objects,
full text, ordered by bm25 then rule_id), in the same `json.dumps(..., ensure_ascii=False)`
context object. The retrieval query is the request text only.

Usage: router_state_builder.py --pack DIR --derived FILE --out STATES.jsonl [--formula-cli BIN]
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

import tool_loop_benchmark as tl

DERIVED_KEYS = {"key", "character", "request", "setup"}
ENCODING = 1
STATE_CASE_ID = "router-state"  # the episode's own id; never a pack case id


def text_hash(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def check_derived(doc) -> list:
    """The derived file's schema: exactly the four allowed keys per entry, nothing else."""
    if not isinstance(doc, dict) or set(doc) != {"encoding", "entries"}:
        raise ValueError("derived file: needs exactly {encoding, entries}")
    if doc["encoding"] != ENCODING or not isinstance(doc["entries"], list):
        raise ValueError("derived file: encoding 1 and an entries list")
    keys = set()
    for entry in doc["entries"]:
        if not isinstance(entry, dict) or set(entry) != DERIVED_KEYS:
            extra = sorted(set(entry) - DERIVED_KEYS) if isinstance(entry, dict) else entry
            raise ValueError(f"derived entry: only {sorted(DERIVED_KEYS)} allowed, got {extra}")
        if entry["key"] in keys or not isinstance(entry["key"], str):
            raise ValueError("derived entry: keys must be unique strings")
        keys.add(entry["key"])
        if not isinstance(entry["request"], str) or not isinstance(entry["setup"], dict):
            raise ValueError("derived entry: request text and setup object")
    return doc["entries"]


def render_state(pack, index, entry) -> str:
    """The C0 user-message context object plus retrieved_rules, as one JSON text."""
    case = {
        "id": STATE_CASE_ID,
        "character": entry["character"],
        "request": entry["request"],
        "setup": entry["setup"],
    }
    user = tl.Episode(pack, index, case, tl.BASE_FLAGS).messages()[1]["content"]
    intro, _, payload = user.partition("\n")
    if intro != tl.CONTEXT_INTRO:
        raise ValueError("C0 context intro mismatch (untrusted marking is not C0)")
    context = json.loads(payload)
    _, rules = index.search(entry["request"])
    context["retrieved_rules"] = rules
    return json.dumps(context, ensure_ascii=False)


def request_only_state(entry) -> str:
    """Ablation state: the request alone (plan §2 ablations)."""
    return json.dumps({"request": entry["request"]}, ensure_ascii=False)


def build(pack, entries, top_k=tl.DEFAULT_TOP_K) -> list:
    index = tl.RuleIndex(pack.rules, top_k)
    rows = []
    for entry in entries:
        state = render_state(pack, index, entry)
        request_only = request_only_state(entry)
        rows.append(
            {
                "key": entry["key"],
                "state": state,
                "state_hash": text_hash(state),
                "request_only": request_only,
                "request_only_hash": text_hash(request_only),
                "retrieved_rule_ids": [r["rule_id"] for r in json.loads(state)["retrieved_rules"]],
            }
        )
    return rows


def open_caseless_pack(root: Path, formula_cli=None):
    root = Path(root)
    present = sorted(p.name for p in root.glob("cases-*.json"))
    if present:
        raise ValueError(f"isolated builder: the pack directory holds case files {present}")
    cli = formula_cli or tl.DEFAULT_FORMULA_CLI
    return tl.Pack(root, engine_factory=lambda d: tl.CelCliEngine(d, Path(cli)))


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--pack", type=Path, required=True)
    p.add_argument("--derived", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--formula-cli", type=Path)
    args = p.parse_args(argv)
    entries = check_derived(json.loads(args.derived.read_text(encoding="utf-8")))
    pack = open_caseless_pack(args.pack, args.formula_cli)
    try:
        rows = build(pack, entries)
    finally:
        pack.close()
    with args.out.open("x", encoding="utf-8") as out:
        for row in rows:
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(json.dumps({"states": len(rows), "builder_sha256": tl.file_digest(Path(__file__))}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
