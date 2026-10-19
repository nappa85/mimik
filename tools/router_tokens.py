"""Vocab-only prompt token measurement (docs/evaluation-tooling.md). Prototype tier.

Runs in a decision image. Loads tokenizers only, never weights, and runs no inference:
- GGUF backbones (AnyJev): `llama_cpp.Llama(model_path, vocab_only=True, n_gpu_layers=0)`,
  the GGUF's own chat template through AnyJev's `GGUFTokenizer`, `build_prompt` and
  `render_chat` with the raw decider's default system prompt, exactly as the worker renders;
- Kev: the pinned base tokenizer (`kev.model.load_tokenizer`) and the native record encoding
  (`kev.api.to_record`, `kev.model.encode`, strict), with every weight loader patched to raise.
It counts every state the units will send (full state in both option orders, and the two
ablation variants), reports min, median, p90 and max and the count over n_ctx, and with
--sentinel-states scans every rendered prompt of a sentinel pack for sentinels.

Usage: router_tokens.py --backend anyjev --model M.gguf --states S.jsonl [--sentinel-states X]
       router_tokens.py --backend kev --states S.jsonl [--sentinel-states X]
"""

import argparse
import contextlib
import json
import math
import statistics
import sys
from pathlib import Path

import decision_router as dr
import local_decision_benchmark as bench

VARIANTS = (
    ("full", "canonical"),
    ("full", "reversed"),
    ("request_only", "canonical"),
    ("empty", "canonical"),
)


class WeightLoadRefused(RuntimeError):
    pass


def stats(values) -> dict:
    values = sorted(values)
    if not values:
        return {"n": 0}
    n = len(values)
    return {
        "n": n,
        "min": values[0],
        "median": statistics.median(values),
        "p90": values[max(0, math.ceil(0.9 * n) - 1)],
        "max": values[-1],
    }


def open_vocab(path, llama_module):
    """The only way this tool opens a GGUF: vocabulary only, no layer on any GPU."""
    return llama_module.Llama(model_path=str(path), vocab_only=True, n_gpu_layers=0, verbose=False)


@contextlib.contextmanager
def refuse_weight_loads(modules=None):
    """Patch every weight loader present to raise (Kev: tokenizer only)."""
    modules = sys.modules if modules is None else modules
    targets = [
        ("torch", "load"),
        ("safetensors", "safe_open"),
        ("safetensors.torch", "load_file"),
        ("safetensors.torch", "load"),
    ]
    saved = []

    def refuse(*_a, **_k):
        raise WeightLoadRefused("weight load refused: token measurement is vocabulary-only")

    for mod_name, attr in targets:
        mod = modules.get(mod_name)
        if mod is not None and hasattr(mod, attr):
            saved.append((mod, attr, getattr(mod, attr)))
            setattr(mod, attr, refuse)
    transformers = modules.get("transformers")
    pretrained = getattr(transformers, "PreTrainedModel", None) if transformers else None
    if pretrained is not None:
        saved.append((pretrained, "from_pretrained", pretrained.__dict__.get("from_pretrained")))
        pretrained.from_pretrained = classmethod(lambda cls, *a, **k: refuse())
    try:
        yield
    finally:
        for mod, attr, value in reversed(saved):
            setattr(mod, attr, value)


def anyjev_renderer(model_path):
    import llama_cpp
    from anyjev import Question
    from anyjev.backends.llamacpp import GGUFTokenizer
    from anyjev.readout import DEFAULT_SYSTEM, build_prompt, render_chat

    llm = open_vocab(model_path, llama_cpp)
    tok = GGUFTokenizer(llm)

    def render(state, order):
        q = Question.choice(dr.QUESTION, bench.option_lines(dr.CHOICES, order), name="review")
        text = render_chat(tok, build_prompt(state, q, list(range(len(order))), DEFAULT_SYSTEM))
        return text, len(llm.tokenize(text.encode("utf-8"), add_bos=False, special=True))

    provenance = {
        "tokenizer": "GGUF vocabulary (vocab_only=True)",
        "chat_template_sha256": dr.text_hash(tok.chat_template or ""),
        "n_vocab": llm.n_vocab(),
        "binding": llama_cpp.__version__,
    }
    return render, provenance


def kev_renderer():
    import kev.model as km
    from kev.api import SystemOneRequest, to_record

    tok = km.load_tokenizer(bench.KEV_BASE, revision=bench.KEV_BASE_REVISION)

    def render(state, order):
        req = SystemOneRequest(
            state=state,
            questions={
                "review": {
                    "type": "choice",
                    "instructions": dr.QUESTION,
                    "criteria": {k: dr.CHOICES[k] for k in order},
                }
            },
        )
        record, _ = to_record(req)
        enc = km.encode(
            tok, record, max_state=km.SERVE_MAX_STATE, max_branch=km.SERVE_MAX_BRANCH, strict=True
        )
        return tok.decode(enc["ids"]), len(enc["ids"])

    provenance = {"tokenizer": f"{bench.KEV_BASE}@{bench.KEV_BASE_REVISION} (load_tokenizer)"}
    return render, provenance


def measure(states, render, n_ctx) -> dict:
    out = {}
    per_key = {}
    for variant, order in VARIANTS:
        counts = {}
        for key, row in states.items():
            _, n = render(dr.state_text(row, variant), dr.order_ids(order))
            counts[key] = n
        name = f"{variant}/{order}"
        out[name] = {
            **stats(counts.values()),
            "over_n_ctx": sum(n > n_ctx for n in counts.values()),
        }
        if name == "full/canonical":
            per_key = counts
    return {"n_ctx": n_ctx, "distributions": out, "per_key_full_canonical": per_key}


def sentinel_scan(states, render) -> dict:
    hits, rendered = [], 0
    for key, row in states.items():
        for variant, order in (*VARIANTS, ("request_only", "reversed"), ("empty", "reversed")):
            text, _ = render(dr.state_text(row, variant), dr.order_ids(order))
            rendered += 1
            if dr.SENTINEL in text:
                hits.append(f"{key}:{variant}/{order}")
    return {"rendered_prompts": rendered, "sentinel_hits": hits}


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--backend", choices=("anyjev", "kev"), required=True)
    p.add_argument("--model", type=Path)
    p.add_argument("--states", type=Path, required=True)
    p.add_argument("--sentinel-states", type=Path)
    p.add_argument("--n-ctx", type=int, default=8192)
    p.add_argument("--label", required=True)
    args = p.parse_args(argv)
    states = dr.read_states(args.states)
    with refuse_weight_loads():
        if args.backend == "anyjev":
            render, provenance = anyjev_renderer(args.model)
        else:
            render, provenance = kev_renderer()
        report = {"label": args.label, "option_hash": dr.OPTION_HASH, **provenance}
        report.update(measure(states, render, args.n_ctx))
        if args.sentinel_states:
            report["sentinel_scan"] = sentinel_scan(dr.read_states(args.sentinel_states), render)
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
