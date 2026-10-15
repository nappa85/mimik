"""Bounded optional development screen, using native readouts and existing IPC."""

import argparse
import hashlib
import importlib.util
import json
import sys
import time
from pathlib import Path

import local_decision_benchmark as bench
from screen_limits import admit_execution


def verify_snapshot(root, backend):
    expected = {
        "mapika": ("Mapika/decider-2b", "ff99089b6810691bd79134ec6febb5d9afa6e619"),
        "tacit": ("morriszjm/Tacit-4B", "af3fa1dc3cd98f937ab2ccedd4df8613fce5f1e5"),
    }
    required = {"config.json", "tokenizer.json", "tokenizer_config.json"}
    required |= (
        {
            "model.safetensors",
            "decider_config.json",
            "decider/__init__.py",
            "decider/infer.py",
            "decider/model.py",
            "decider/prompt.py",
            "decider/temperature.py",
        }
        if backend == "mapika"
        else {
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
            "model.safetensors.index.json",
            "tacit_decision.py",
        }
    )
    manifest = json.loads((root / "mimik-verified.json").read_text())
    if (manifest.get("repo"), manifest.get("revision")) != expected[backend]:
        raise ValueError("Snapshot pin mismatch")
    entries = manifest["verified_files"]
    names = [e["file"] for e in entries]
    if not required.issubset(names) or len(names) != len(set(names)):
        raise ValueError("Incomplete snapshot manifest")
    for entry in entries:
        relative = Path(entry["file"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("Unsafe manifest path")
        path = root / relative
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != entry["sha256"] or path.stat().st_size != entry["bytes"]:
            raise ValueError("Snapshot content changed")
    return {
        "repo": manifest["repo"],
        "revision": manifest["revision"],
        "verified_files": entries,
    }


class ForwardGuard:
    def __init__(self):
        self.count = 0

    def enter(self, *_args):
        if self.count:
            raise RuntimeError("A second native forward is forbidden")
        self.count += 1


def scope_cases():
    common = {
        "game_id": "synthetic",
        "edition_id": "demo",
        "sheet": {"revision": 1, "values": {"focus": 3}},
    }
    cases = [
        (
            "chapter-continues",
            "supported",
            {
                "nodes": {
                    "spells": {"parent": "root", "pages": [10, 180]},
                    "descriptions": {"parent": "spells", "pages": [21, 180]},
                    "spark": {"parent": "descriptions", "pages": [160, 161]},
                }
            },
            "Check the claim that spark is structurally inside the Spells chapter even though it is on page 160.",
        ),
        (
            "list-not-parent",
            "conflict",
            {
                "nodes": {
                    "lists": {"parent": "spells", "references": ["spark"]},
                    "descriptions": {"parent": "spells"},
                    "spark": {"parent": "descriptions"},
                }
            },
            "Check the structural claim that spark's parent is lists because lists references spark. The supplied parent map is authoritative for this check.",
        ),
        (
            "subclass-leak",
            "conflict",
            {
                "nodes": {
                    "ember": {
                        "parent": "class",
                        "rule": "Only members of ember may use flare.",
                    },
                    "tide": {"parent": "class"},
                },
                "member_of": "tide",
                "not_member_of": ["ember"],
            },
            "Check the proposal that this tide member, who is not an ember member, may use flare under the supplied ember-only restriction.",
        ),
        (
            "unresolved-parent",
            "missing_rule",
            {
                "nodes": {"spark": {"parent": None, "status": "unresolved"}},
                "note": "The applicable chapter and its casting cost are unresolved; no casting-cost rule is supplied.",
            },
            "Determine spark's casting cost from its chapter. Do not assume which chapter applies.",
        ),
        (
            "disabled-expansion",
            "missing_rule",
            {
                "enabled_books": ["core@1"],
                "references": [{"book": "expansion@1", "status": "disabled"}],
                "coverage": "No core or enabled-book rule for lunar focus cost is supplied.",
            },
            "Does an enabled source specify lunar focus cost? No rule text from the disabled expansion is supplied.",
        ),
        (
            "cross-book-cost",
            "supported",
            {
                "enabled_books": ["core@1", "expansion@1"],
                "retrieved_evidence": [
                    {
                        "id": "core:casting",
                        "text": "Pay the spell's stated focus cost by subtracting it from current focus.",
                    },
                    {
                        "id": "expansion:lunar",
                        "parent_path": ["expansion", "spells", "lunar"],
                        "text": "Lunar costs 2 focus.",
                    },
                ],
            },
            "Review casting lunar with current focus 3 and proposed focus 1, using the two supplied enabled-book rules.",
        ),
    ]
    return [
        {
            "case": ident,
            "stratum": "scope",
            "state": json.dumps({**common, **data, "user_request": request}, sort_keys=True),
            "expected": expected,
            "expected_context": expected,
        }
        for ident, expected, data, request in cases
    ]


def freeze(demo, destination):
    cases = [
        {**c, "stratum": "known_development"} for c in bench.prepare_cases(demo)
    ] + scope_cases()
    for case in cases:
        case["state_hash"] = bench.digest(case["state"])
    contract = {
        "encoding": 1,
        "cases": cases,
        "question": bench.QUESTION,
        "options": bench.CHOICES,
        "orders": [list(bench.CHOICES), list(reversed(bench.CHOICES))],
        "repeats": 1,
        "request_timeout": 60,
        "physical_evaluations_per_decision": 1,
        "scorer_sha256": hashlib.sha256(Path(bench.__file__).read_bytes()).hexdigest(),
        "screen_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    }
    envelope = {"contract": contract, "sha256": bench.digest(contract)}
    with destination.open("x") as stream:
        json.dump(envelope, stream, indent=2)
    return envelope


def trained_worker(args):
    import torch

    torch.set_num_threads(6)
    torch.set_num_interop_threads(1)
    root = Path(args.kev_checkpoint)
    verified = verify_snapshot(root, args.backend)
    guard = ForwardGuard()
    if args.backend == "mapika":
        sys.path.insert(0, str(root))
        from decider.infer import Decider

        decider = Decider(str(root), device="cpu", dtype=torch.float32, use_graphs=False)
        decider.m.lm.model.register_forward_pre_hook(guard.enter)
        tokenizer = decider.m.tok

        def predict(request):
            options = [f"{k}: {bench.CHOICES[k]}" for k in request["order"]]
            if (
                len(tokenizer.encode("Context:\n" + request["state"], add_special_tokens=False))
                > 3500
            ):
                raise ValueError("No silent native context truncation")
            requests, items = decider._decide_items(
                [
                    (
                        request["state"],
                        [{"question": bench.QUESTION, "options": options}],
                    )
                ],
                max_ctx_tokens=3500,
            )
            started = time.perf_counter()
            result = decider.decide(
                request["state"],
                [{"question": bench.QUESTION, "options": options}],
                max_ctx_tokens=3500,
            )[0]
            return {
                "probabilities": dict(zip(request["order"], result["probs_list"], strict=True)),
                "seconds": time.perf_counter() - started,
                "encoded_token_ids": items[0]["ids"],
                "physical_evaluations": 1,
            }

        metadata = {
            "native": "decider.infer.Decider",
            "name": decider.name,
            "temperature": decider.T,
            "temperature_by_type": decider.T_by_type,
            "dtype": "float32",
            "device": "cpu",
        }
    else:
        from transformers import AutoModelForCausalLM, AutoTokenizer

        spec = importlib.util.spec_from_file_location("pinned_tacit", root / "tacit_decision.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        tokenizer = AutoTokenizer.from_pretrained(
            root, local_files_only=True, trust_remote_code=False
        )
        model = AutoModelForCausalLM.from_pretrained(
            root, dtype=torch.float32, local_files_only=True, trust_remote_code=False
        ).eval()
        model.register_forward_pre_hook(guard.enter)
        decider = module.Tacit(
            module._TransformersEngine(model, tokenizer), tokenizer, adaptive=False
        )

        def predict(request):
            options = [f"{k}: {bench.CHOICES[k]}" for k in request["order"]]
            item = decider._normalise(
                {
                    "state": request["state"],
                    "question": bench.QUESTION,
                    "options": options,
                }
            )
            prompt = decider._render(
                module.ONE_FORWARD_SYSTEM,
                module._one_forward_user(
                    item["state"],
                    item["question"],
                    item["kind"],
                    item["options"],
                    item["labels"],
                ),
                False,
            )
            ids = tokenizer.encode(prompt, add_special_tokens=False)
            if len(ids) > 4096:
                raise ValueError("Screen context budget exceeded")
            started = time.perf_counter()
            result = decider.decide(request["state"], bench.QUESTION, options)
            return {
                "probabilities": {
                    key: result["probs"][option]
                    for key, option in zip(request["order"], options, strict=True)
                },
                "seconds": time.perf_counter() - started,
                "encoded_token_ids": ids,
                "route": result["route"],
                "physical_evaluations": 1,
            }

        metadata = {
            "native": "Tacit._TransformersEngine",
            "adaptive": False,
            "dtype": "float32",
            "device": "cpu",
        }
    native_predict = predict

    def predict(request):
        guard.count = 0
        try:
            result = native_predict(request)
        except Exception as error:
            error.physical_evaluations = guard.count
            raise
        result["physical_evaluations"] = guard.count
        return result

    return (
        predict,
        lambda: None,
        {
            **metadata,
            "checkpoint": str(root),
            "torch": torch.__version__,
            "verified_snapshot": verified,
        },
    )


def run(args):
    frozen = json.loads(args.cases.read_text())
    contract = frozen["contract"]
    if (
        bench.digest(contract) != frozen["sha256"]
        or contract["question"] != bench.QUESTION
        or contract["options"] != bench.CHOICES
    ):
        raise ValueError("Frozen contract mismatch")
    for field, module in (
        ("scorer_sha256", Path(bench.__file__)),
        ("screen_sha256", Path(__file__)),
    ):
        if hashlib.sha256(module.read_bytes()).hexdigest() != contract[field]:
            raise ValueError("Evaluator code changed after freeze")
    for path in (
        args.output,
        args.output.with_suffix(".worker.log"),
        args.output.with_suffix(".provenance.json"),
    ):
        if path.exists():
            raise FileExistsError("Preserve screen evidence")
    runtime = None
    deadline = time.monotonic() + min(args.budget_seconds, args.cutoff_unix - time.time())
    startup_error = None
    rows = []
    reservations = 0
    try:
        try:
            admit_execution(args.cutoff_unix, args.remaining_evaluations, args.free_bytes)
            if args.budget_seconds <= 0:
                raise ValueError("Candidate budget exhausted before setup")
            runtime = bench.Worker(args)
            ready = runtime.receive(
                min(180, args.budget_seconds, max(0.001, args.cutoff_unix - time.time()))
            )
            if "ready" not in ready:
                raise ValueError("No native provenance")
            args.output.with_suffix(".provenance.json").write_text(
                json.dumps({**ready, "contract_hash": frozen["sha256"]}, indent=2)
            )
        except (OSError, RuntimeError, TimeoutError, ValueError) as error:
            startup_error = str(error)
        with args.output.open("x") as stream:
            for case in contract["cases"]:
                for order in contract["orders"]:
                    began = time.monotonic()
                    row = {
                        **case,
                        "order": order,
                        "method": args.backend,
                        "started_at_unix": time.time(),
                        "event": "started",
                    }
                    stream.write(json.dumps(row) + "\n")
                    stream.flush()
                    remaining = deadline - time.monotonic()
                    reserved = 0
                    if (
                        startup_error
                        or remaining <= 0
                        or time.time() >= args.cutoff_unix
                        or reservations >= args.remaining_evaluations
                        or runtime.process.poll() is not None
                    ):
                        result = {
                            "status": "not_run",
                            "error": startup_error or "Budget/worker unavailable",
                            "physical_evaluations": 0,
                        }
                    else:
                        reserved = 1
                        reservations += 1
                        try:
                            result = runtime.predict(
                                {
                                    "state": case["state"],
                                    "order": order,
                                    "method": "raw",
                                    "clear_cache": True,
                                },
                                min(60, remaining),
                            )
                        except (
                            OSError,
                            ValueError,
                            RuntimeError,
                            TimeoutError,
                        ) as error:
                            result = {
                                "status": "failed",
                                "error": str(error),
                                "physical_evaluations": None,
                            }
                    row.update(
                        result,
                        event="finished",
                        reserved_evaluations=reserved,
                        elapsed_seconds=time.monotonic() - began,
                        finished_at_unix=time.time(),
                    )
                    try:
                        row["score"] = bench.score_result(row)
                    except (KeyError, TypeError, ValueError) as error:
                        row.update(
                            status="failed",
                            error=str(error),
                            score={"correct": False, "failed": True},
                        )
                    stream.write(json.dumps(row) + "\n")
                    stream.flush()
                    print(
                        f"{args.backend} {case['case']} {row['status']} {row['score']['correct']}",
                        flush=True,
                    )
                    rows.append(row)
    finally:
        if runtime:
            runtime.close()
    verification = {
        "passed": bool(rows) and all(r["status"] == "completed" for r in rows),
        "scheduled": len(rows),
        "completed": sum(r["status"] == "completed" for r in rows),
        "reserved_evaluations": reservations,
        "startup_error": startup_error,
        "cutoff_unix": args.cutoff_unix,
    }
    args.output.with_suffix(".verification.json").write_text(json.dumps(verification, indent=2))
    if not verification["passed"]:
        raise RuntimeError("Incomplete screen; full scheduled evidence retained")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=("anyjev", "kev", "mapika", "tacit"))
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--freeze", type=Path)
    parser.add_argument("--demo", type=Path)
    parser.add_argument("--cases", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--model")
    parser.add_argument("--model-sha256")
    parser.add_argument("--kev-checkpoint")
    parser.add_argument("--budget-seconds", type=float, default=400)
    parser.add_argument("--cutoff-unix", type=float, default=1792072246.0)
    parser.add_argument("--remaining-evaluations", type=int, default=48)
    parser.add_argument("--free-bytes", type=int, default=0)
    args = parser.parse_args()
    args.raw_only = True
    args.worker_script = __file__
    if args.freeze:
        print(freeze(args.demo, args.freeze)["sha256"])
    elif args.worker:
        factory = (
            bench.anyjev_worker
            if args.backend == "anyjev"
            else bench.kev_worker
            if args.backend == "kev"
            else trained_worker
        )
        bench.worker(args, factory)
    else:
        run(args)


if __name__ == "__main__":
    main()
