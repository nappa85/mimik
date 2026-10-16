#!/usr/bin/env python3
"""Model registry for the tool-loop runner and queue (tools/models.json). Prototype tier.

Stdlib only. A new model is a new registry entry, never a code change. `validate` is a strict
schema check (unknown and missing keys are refused); `server_args` builds the llama.cpp
server vector; `shell` prints bash assignments for tools/run_tool_loop_benchmark.sh.

Usage:
  model_registry.py [--registry FILE] check
  model_registry.py [--registry FILE] list
  model_registry.py [--registry FILE] get KEY
  model_registry.py [--registry FILE] shell KEY [--ctx N] [--n-cpu-moe N]
Environment: MIMIK_MODEL_ROOT overrides the registry's model_root.
"""

import argparse
import json
import os
import re
import shlex
import sys
from pathlib import Path, PurePosixPath

REGISTRY = Path(__file__).resolve().parent / "models.json"
SCHEMA = 1
ROOT_ENV = "MIMIK_MODEL_ROOT"
KEY_RE = re.compile(r"^[a-z0-9][a-z0-9.-]{0,63}$")
LABEL_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
SHA_RE = re.compile(r"^[0-9a-f]{64}$")
REVISION_RE = re.compile(r"^[0-9a-f]{12,40}$")
IMAGE_RE = re.compile(r"^[a-z0-9][a-z0-9./_-]*@sha256:[0-9a-f]{64}$")
MEMORY_RE = re.compile(r"^[1-9][0-9]{0,4}[mg]$")
ARG_RE = re.compile(r"^[A-Za-z0-9_.,:=+/-]{1,128}$")
CACHE_TYPES = (None, "f16", "bf16", "f32", "q8_0", "q5_1", "q5_0", "q4_1", "q4_0", "iq4_nl")
FLASH_ATTN = (None, "on", "off", "auto")
TOOLS = ("handler", "unverified", "none")
THINKING = ("toggle", "always", "never")
READOUT = ("verified", "untested", "unsupported")
# Flags the runner sets itself (hardening, endpoint, identity); extra_args may not override them.
MANAGED_FLAGS = {
    "-m",
    "--model",
    "-a",
    "--alias",
    "--host",
    "--port",
    "--offline",
    "--api-key",
    "--api-key-file",
    "--webui",
    "--no-webui",
    "--slots",
    "--no-slots",
    "--mmproj",
    "--no-mmproj",
    "--jinja",
    "-ngl",
    "--n-gpu-layers",
    "--fit",
    "-t",
    "--threads",
    "-c",
    "--ctx-size",
    "-np",
    "--parallel",
    "--no-warmup",
    "--cache-ram",
    "--n-cpu-moe",
    "-ctk",
    "--cache-type-k",
    "-ctv",
    "--cache-type-v",
    "-fa",
    "--flash-attn",
    "--path",
    "--chat-template-file",
    "--model-url",
    "-mu",
    "-hf",
    "--hf-repo",
}
TOP_KEYS = {"schema", "model_root", "path_convention", "models"}
MODEL_KEYS = {
    "label",
    "downloaded",
    "path",
    "sha256",
    "size",
    "source",
    "server_image",
    "server",
    "capabilities",
    "recommended_sampling",
    "memory_limit",
    "prewarm",
    "health_wait_seconds",
    "request_timeout_seconds",
    "seconds_per_episode",
    "seconds_per_episode_source",
    "licence",
    "notes",
}
SERVER_KEYS = {
    "ctx",
    "n_gpu_layers",
    "n_cpu_moe",
    "n_cpu_moe_required",
    "threads",
    "cache_ram",
    "cache_type_k",
    "cache_type_v",
    "flash_attn",
    "extra_args",
}
CAPABILITY_KEYS = {"tools", "thinking", "thinking_evidence", "decision_readout"}
SOURCE_KEYS = {"repo", "revision", "file"}
CTX_RANGE = (512, 262144)


class RegistryError(ValueError):
    pass


def _keys(obj, expected, where, optional=()):
    if not isinstance(obj, dict):
        raise RegistryError(f"{where}: must be an object")
    unknown = sorted(set(obj) - expected - set(optional))
    missing = sorted(expected - set(obj))
    if unknown:
        raise RegistryError(f"{where}: unknown keys {unknown}")
    if missing:
        raise RegistryError(f"{where}: missing keys {missing}")


def _int(value, where, low, high=None, allow_none=False):
    if value is None and allow_none:
        return
    if not isinstance(value, int) or isinstance(value, bool) or value < low:
        raise RegistryError(f"{where}: must be an integer >= {low}")
    if high is not None and value > high:
        raise RegistryError(f"{where}: must be <= {high}")


def _number(value, where, low_exclusive=0.0):
    if not isinstance(value, (int, float)) or isinstance(value, bool) or value <= low_exclusive:
        raise RegistryError(f"{where}: must be a number > {low_exclusive}")


def _text(value, where):
    if not isinstance(value, str) or not value.strip():
        raise RegistryError(f"{where}: must be a non-empty string")


def _bool(value, where):
    if not isinstance(value, bool):
        raise RegistryError(f"{where}: must be true or false")


def _choice(value, choices, where):
    if value not in choices or isinstance(value, bool):
        raise RegistryError(f"{where}: must be one of {list(choices)}")


def _flag_name(arg: str) -> str:
    return arg.split("=", 1)[0]


def _validate_server(server, where):
    _keys(server, SERVER_KEYS, where)
    _int(server["ctx"], f"{where}.ctx", *CTX_RANGE)
    _int(server["n_gpu_layers"], f"{where}.n_gpu_layers", 0, 999)
    _int(server["n_cpu_moe"], f"{where}.n_cpu_moe", 0, 999, allow_none=True)
    _bool(server["n_cpu_moe_required"], f"{where}.n_cpu_moe_required")
    _int(server["threads"], f"{where}.threads", 1, 64)
    _int(server["cache_ram"], f"{where}.cache_ram", 0)
    for name in ("cache_type_k", "cache_type_v"):
        _choice(server[name], CACHE_TYPES, f"{where}.{name}")
    _choice(server["flash_attn"], FLASH_ATTN, f"{where}.flash_attn")
    if server["cache_type_v"] not in (None, "f16") and server["flash_attn"] != "on":
        raise RegistryError(f"{where}.flash_attn: a quantised V cache needs flash_attn 'on'")
    extra = server["extra_args"]
    if not isinstance(extra, list) or not all(
        isinstance(a, str) and ARG_RE.match(a) for a in extra
    ):
        raise RegistryError(f"{where}.extra_args: must be a list of plain argument strings")
    managed = [a for a in extra if _flag_name(a) in MANAGED_FLAGS]
    if managed:
        raise RegistryError(f"{where}.extra_args: runner-managed flags {managed}")


def _validate_model(key, model):
    where = f"models.{key}"
    if not KEY_RE.match(key):
        raise RegistryError(f"{where}: key must match {KEY_RE.pattern}")
    _keys(model, MODEL_KEYS - {"notes"}, where, optional=("notes",))
    if not isinstance(model["label"], str) or not LABEL_RE.match(model["label"]):
        raise RegistryError(f"{where}.label: must match {LABEL_RE.pattern}")
    _bool(model["downloaded"], f"{where}.downloaded")
    path = model["path"]
    pure = PurePosixPath(path) if isinstance(path, str) else None
    if (
        pure is None
        or pure.is_absolute()
        or ".." in pure.parts
        or pure.suffix != ".gguf"
        or "\\" in path
    ):
        raise RegistryError(f"{where}.path: must be a relative .gguf path without '..'")
    if not isinstance(model["sha256"], str) or not SHA_RE.match(model["sha256"]):
        raise RegistryError(f"{where}.sha256: must be 64 lowercase hex digits")
    _int(model["size"], f"{where}.size", 1)
    source = model["source"]
    _keys(source, SOURCE_KEYS, f"{where}.source")
    _text(source["repo"], f"{where}.source.repo")
    _text(source["file"], f"{where}.source.file")
    revision = source["revision"]
    if not isinstance(revision, str) or not REVISION_RE.match(revision):
        raise RegistryError(f"{where}.source.revision: must be 12-40 lowercase hex digits")
    if model["downloaded"] and len(revision) != 40:
        raise RegistryError(f"{where}.source.revision: a downloaded model needs the full commit")
    if not isinstance(model["server_image"], str) or not IMAGE_RE.match(model["server_image"]):
        raise RegistryError(f"{where}.server_image: must be pinned as name@sha256:<digest>")
    _validate_server(model["server"], f"{where}.server")
    caps = model["capabilities"]
    _keys(caps, CAPABILITY_KEYS, f"{where}.capabilities")
    _choice(caps["tools"], TOOLS, f"{where}.capabilities.tools")
    _choice(caps["thinking"], THINKING, f"{where}.capabilities.thinking")
    _text(caps["thinking_evidence"], f"{where}.capabilities.thinking_evidence")
    _choice(caps["decision_readout"], READOUT, f"{where}.capabilities.decision_readout")
    sampling = model["recommended_sampling"]
    if sampling is not None:
        _keys(
            sampling,
            {"temperature", "top_p", "source"},
            f"{where}.recommended_sampling",
            optional=("top_k", "min_p"),
        )
        _number(sampling["temperature"], f"{where}.recommended_sampling.temperature", -1e-9)
        _number(sampling["top_p"], f"{where}.recommended_sampling.top_p")
        _text(sampling["source"], f"{where}.recommended_sampling.source")
    if not isinstance(model["memory_limit"], str) or not MEMORY_RE.match(model["memory_limit"]):
        raise RegistryError(f"{where}.memory_limit: must look like 14g or 512m")
    _bool(model["prewarm"], f"{where}.prewarm")
    _int(model["health_wait_seconds"], f"{where}.health_wait_seconds", 10, 7200)
    _int(model["request_timeout_seconds"], f"{where}.request_timeout_seconds", 1, 3600)
    _number(model["seconds_per_episode"], f"{where}.seconds_per_episode")
    _text(model["seconds_per_episode_source"], f"{where}.seconds_per_episode_source")
    _text(model["licence"], f"{where}.licence")
    if "notes" in model:
        _text(model["notes"], f"{where}.notes")


def validate(doc: dict) -> dict:
    """Strict schema check; returns the document unchanged."""
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA:
        raise RegistryError(f"registry: schema must be {SCHEMA}")
    _keys(doc, TOP_KEYS, "registry")
    root = doc["model_root"]
    if not isinstance(root, str) or not os.path.isabs(root):
        raise RegistryError("registry.model_root: must be an absolute path")
    _text(doc["path_convention"], "registry.path_convention")
    models = doc["models"]
    if not isinstance(models, dict) or not models:
        raise RegistryError("registry.models: must be a non-empty object")
    labels = {}
    for key, model in models.items():
        _validate_model(key, model)
        if model["label"] in labels:
            raise RegistryError(f"models.{key}.label: also used by {labels[model['label']]}")
        labels[model["label"]] = key
    return doc


def load(path: Path = REGISTRY) -> dict:
    with open(path, encoding="utf-8") as handle:
        return validate(json.load(handle))


def model(doc: dict, key: str) -> dict:
    if key not in doc["models"]:
        raise RegistryError(f"unknown model {key!r}; registered: {sorted(doc['models'])}")
    return doc["models"][key]


def model_root(doc: dict, env=None) -> Path:
    env = os.environ if env is None else env
    root = env.get(ROOT_ENV) or doc["model_root"]
    if not os.path.isabs(root):
        raise RegistryError(f"{ROOT_ENV} must be an absolute path")
    return Path(root)


def model_file(doc: dict, key: str, env=None) -> Path:
    """The registered path under the model root (not resolved; the runner resolves symlinks)."""
    return model_root(doc, env) / model(doc, key)["path"]


def server_args(entry: dict, ctx: int, n_cpu_moe=None) -> list[str]:
    """The llama.cpp server vector. Fixed hardening flags first, in the runner's v2 order."""
    _int(ctx, "ctx", *CTX_RANGE)
    server = entry["server"]
    moe = server["n_cpu_moe"] if n_cpu_moe is None else n_cpu_moe
    if moe is None and server["n_cpu_moe_required"]:
        raise RegistryError(f"{entry['label']} needs n_cpu_moe (N_CPU_MOE, llama.cpp --n-cpu-moe)")
    _int(moe, "n_cpu_moe", 0, 999, allow_none=True)
    args = ["--model", "/models/model.gguf", "--alias", entry["label"], "--host", "0.0.0.0"]
    args += ["--port", "8080", "--offline", "--no-webui", "--no-slots", "--no-mmproj", "--jinja"]
    args += ["--n-gpu-layers", str(server["n_gpu_layers"]), "--fit", "off"]
    args += ["--threads", str(server["threads"]), "--ctx-size", str(ctx), "--parallel", "1"]
    args += ["--no-warmup", "--cache-ram", str(server["cache_ram"])]
    if moe is not None:
        args += ["--n-cpu-moe", str(moe)]
    if server["cache_type_k"] is not None:
        args += ["--cache-type-k", server["cache_type_k"]]
    if server["cache_type_v"] is not None:
        args += ["--cache-type-v", server["cache_type_v"]]
    if server["flash_attn"] is not None:
        args += ["--flash-attn", server["flash_attn"]]
    return args + list(server["extra_args"])


def shell_exports(doc: dict, key: str, ctx=None, n_cpu_moe=None, env=None) -> str:
    entry = model(doc, key)
    if not entry["downloaded"]:
        raise RegistryError(f"{key} is registered but not downloaded (downloaded: false)")
    if entry["capabilities"]["tools"] == "none":
        raise RegistryError(f"{key} has no tool-call support (capabilities.tools: none)")
    ctx = entry["server"]["ctx"] if ctx is None else ctx
    args = server_args(entry, ctx, n_cpu_moe)
    moe = entry["server"]["n_cpu_moe"] if n_cpu_moe is None else n_cpu_moe
    values = {
        "reg_label": entry["label"],
        "reg_path": str(model_file(doc, key, env)),
        "reg_sha256": entry["sha256"],
        "reg_size": entry["size"],
        "reg_image": entry["server_image"],
        "reg_ctx": ctx,
        "reg_n_cpu_moe": "" if moe is None else moe,
        "reg_memory": entry["memory_limit"],
        "reg_prewarm": int(entry["prewarm"]),
        "reg_health_wait": entry["health_wait_seconds"],
        "reg_request_timeout": entry["request_timeout_seconds"],
        "reg_thinking": entry["capabilities"]["thinking"],
        "reg_tools": entry["capabilities"]["tools"],
        "reg_server_config": json.dumps(
            {"image": entry["server_image"], "args": args}, separators=(",", ":")
        ),
    }
    lines = [f"{name}={shlex.quote(str(value))}" for name, value in values.items()]
    lines.append("reg_server_args=(" + " ".join(shlex.quote(a) for a in args) + ")")
    return "\n".join(lines) + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--registry", type=Path, default=REGISTRY)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("check")
    sub.add_parser("list")
    get = sub.add_parser("get")
    get.add_argument("key")
    shell = sub.add_parser("shell")
    shell.add_argument("key")
    shell.add_argument("--ctx", type=int)
    shell.add_argument("--n-cpu-moe", type=int)
    args = parser.parse_args(argv)
    try:
        doc = load(args.registry)
        if args.command == "check":
            print(f"registry ok: {len(doc['models'])} models")
        elif args.command == "list":
            for key, entry in doc["models"].items():
                state = "downloaded" if entry["downloaded"] else "not downloaded"
                print(f"{key}\t{entry['label']}\t{state}")
        elif args.command == "get":
            print(json.dumps(model(doc, args.key), indent=2))
        else:
            sys.stdout.write(shell_exports(doc, args.key, args.ctx, args.n_cpu_moe))
    except (RegistryError, OSError, json.JSONDecodeError) as error:
        print(f"model registry: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
