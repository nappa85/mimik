"""Finalize interrupted experiment ledgers without modifying raw observations."""

import argparse
import json
import math
import statistics
from pathlib import Path

from local_decision_benchmark import score_result
from local_rag_benchmark import digest


def load_contract(path):
    envelope = json.loads(path.read_text())
    if digest(envelope["contract"]) != envelope["sha256"]:
        raise ValueError("Frozen contract envelope digest mismatch")
    return envelope["contract"]


def finalize(contract, raw):
    expected = {
        (case["case"], tuple(order)): {**case, "order": order}
        for case in contract["cases"]
        for order in contract["orders"]
    }
    if len(expected) != len(contract["cases"]) * len(contract["orders"]):
        raise ValueError("Duplicate scheduled observations")
    started, finished = {}, {}
    lines = raw.splitlines()
    for index, line in enumerate(lines):
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            if index == len(lines) - 1 and not raw.endswith("\n"):
                break
            raise ValueError("Corrupt nonterminal ledger line") from error
        key = (row["case"], tuple(row["order"]))
        if key not in expected or row["state"] != expected[key]["state"]:
            raise ValueError("Unscheduled or changed model input")
        if row["event"] == "finished" and any(
            row.get(field) != value for field, value in expected[key].items()
        ):
            raise ValueError("Finished outcome changed frozen case metadata")
        target = (
            started
            if row["event"] == "started"
            else finished
            if row["event"] == "finished"
            else None
        )
        if target is None or key in target:
            raise ValueError("Duplicate or unknown ledger event")
        target[key] = row
    output = []
    for key, case in expected.items():
        if key in finished:
            output.append({**finished[key], "finalization": "native_finished"})
        else:
            output.append(
                {
                    **case,
                    "event": "finished",
                    "status": "interrupted" if key in started else "not_run",
                    "physical_evaluations": None if key in started else 0,
                    "reserved_evaluations": int(key in started),
                    "error": "Outer cutoff/startup prevented a complete native observation",
                    "finalization": "derived_missing_outcome",
                }
            )
    return output


def metrics(rows):
    complete = [row for row in rows if row["status"] == "completed"]
    durations = sorted(row["seconds"] for row in complete if "seconds" in row)
    scores = [(row, score_result(row)) for row in rows]
    supported = [(row, score) for row, score in scores if score.get("choice") == "supported"]
    result = {
        "scheduled": len(rows),
        "unique_cases": len({r["case"] for r in rows}),
        "completed": len(complete),
        "correct": sum(s["correct"] for _, s in scores),
        "not_run": sum(r["status"] == "not_run" for r in rows),
        "interrupted": sum(r["status"] == "interrupted" for r in rows),
        "supported_predictions": len(supported),
        "false_supported": sum(not score["context_correct"] for _, score in supported),
        "observed_physical_evaluations": sum(r.get("physical_evaluations") or 0 for r in rows),
        "unknown_physical_counts": sum(r.get("physical_evaluations") is None for r in rows),
        "median_seconds": statistics.median(durations) if durations else None,
        "p95_seconds": durations[math.ceil(0.95 * len(durations)) - 1] if durations else None,
    }
    result["strata"] = {
        stratum: {
            "scheduled": sum(r.get("stratum") == stratum for r in rows),
            "completed": sum(
                r.get("stratum") == stratum and r["status"] == "completed" for r in rows
            ),
            "correct": sum(r.get("stratum") == stratum and s["correct"] for r, s in scores),
        }
        for stratum in sorted({r.get("stratum", "unspecified") for r in rows})
    }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    contract = load_contract(args.directory / "cases.json")
    summary = {}
    for name in ("instruct4", "qwen8", "qwen4", "kev08"):
        path = args.directory / (name + ".jsonl")
        rows = finalize(contract, path.read_text() if path.exists() else "")
        destination = args.directory / (name + ".finalized.jsonl")
        with destination.open("x") as stream:
            for row in rows:
                row["score"] = score_result(row)
                stream.write(json.dumps(row) + "\n")
        summary[name] = metrics(rows)
    with (args.directory / "summary.json").open("x") as stream:
        json.dump(summary, stream, indent=2)
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
