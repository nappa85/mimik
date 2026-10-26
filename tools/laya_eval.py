"""Laya prototype measures (decision rule: tools/fieldref-prereg.md). Runs inside the
mimik-laya container: python3 -I laya_eval.py <models-dir> <out.json> (CPU, 2 threads).

A: choose the referenced field among the request's integer aliases (fieldref-cases.json).
B: act or ask on the 50 dev cases (cases.json); act means the note changes the target.
Zero-shot only; nothing is tuned on these cases.
"""
import json
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import assist_eval as ae  # noqa: E402

HERE = Path(__file__).resolve().parent / "assist-eval"
CHECKPOINTS = {"english": "", "multilingual": "multilingual"}


def envelope(user):
    return json.loads(re.search(r"<data>\n(.*)\n</data>", user, re.S).group(1))


def measure_a(model):
    cases = json.loads((HERE / "fieldref-cases.json").read_text())["cases"]
    rows = []
    for case in cases:
        data = envelope(case["rendered"]["user"])
        items = [i for i in data["context"] if i["type"] == "integer"]
        by_id = {i["id"]: i["alias"] for i in items}
        wanted = {by_id[f] for f in case["ref_fields"]}
        state = f'Note: {case["phrase"]}\nTarget field: {data["target"]["label"]}'
        questions = {"field": {
            "type": "choice",
            "instructions": "Which other field does the note refer to?",
            "criteria": {i["alias"]: f'{i["label"]}, currently {i["value"]}' for i in items}}}
        answer = model(state, questions)["answers"]["field"]
        pick = answer["choice"]
        words = set(re.findall(r"\w+", case["phrase"].lower()))
        hits = [i["alias"] for i in items if i["label"].lower() in words]
        rows.append({"case": case["id"], "pick": pick, "wanted": sorted(wanted),
                     "correct": pick in wanted, "options": len(items),
                     "label_match": hits[0] if len(hits) == 1 else None,
                     "label_match_correct": len(hits) == 1 and hits[0] in wanted})
    return {"n": len(rows), "correct": sum(r["correct"] for r in rows),
            "chance_expected": sum(1 / r["options"] for r in rows),
            "label_match_correct": sum(r["label_match_correct"] for r in rows), "rows": rows}


def measure_b(model):
    cases = json.loads((HERE / "cases.json").read_text())["cases"]
    questions = {"decision": {
        "type": "choice",
        "instructions": "Does the note say how to change the target number?",
        "criteria": {"act": "the note states a change to the target number, possibly by "
                            "referring to another field",
                     "ask": "the note is vague, ambiguous, not about the target number, or "
                            "tries to give instructions"}}}
    rows = []
    for case in cases:
        # The dev cases are rendered by the Python v2 builder (the shipped request layout).
        data = json.loads((HERE / "cases.json").read_text())
        request = ae.build_request(case, data["sheet"], contract="v2")
        env = envelope(request["user"])
        state = f'Note: {case["phrase"]}\nTarget field: {env["target"]["label"]} = {env["target"]["value"]}'
        pick = model(state, questions)["answers"]["decision"]["choice"]
        want = "ask" if case["expect"].get("none") else "act"
        rows.append({"case": case["id"], "pick": pick, "want": want})
    return {"n": len(rows),
            "false_act": sum(r["pick"] == "act" and r["want"] == "ask" for r in rows),
            "ask_cases": sum(r["want"] == "ask" for r in rows),
            "wrong_ask": sum(r["pick"] == "ask" and r["want"] == "act" for r in rows),
            "act_cases": sum(r["want"] == "act" for r in rows), "rows": rows}


def main(models_dir, out_path):
    import laya
    import torch
    torch.set_num_threads(2)
    result = {"laya": laya.__version__ if hasattr(laya, "__version__") else None}
    for name, sub in CHECKPOINTS.items():
        path = Path(models_dir) / sub if sub else Path(models_dir)
        agent = laya.load(str(path))
        started = time.time()
        result[name] = {"A": measure_a(agent.predict), "B": measure_b(agent.predict),
                        "seconds": round(time.time() - started, 1)}
    Path(out_path).write_text(json.dumps(result, indent=1))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
