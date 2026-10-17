#!/usr/bin/env python3
"""Decision-router model registry (tools/decision_models.json). Prototype tier, stdlib only.

Separate from tools/models.json (the tool-loop registry), whose file and consumers are
unchanged. Every entry pins a runtime, a local image id, a library revision, and every artefact
file with its path (relative to the model root), size and sha256; the runner hashes each one
before load and the worker hashes again at every start.

Usage:
  decision_models.py [--registry FILE] check [--files]   schema (and on-disk sizes)
  decision_models.py [--registry FILE] list
  decision_models.py [--registry FILE] shell KEY          bash assignments for the runner
Environment: MIMIK_MODEL_ROOT overrides the model root.
"""

import argparse
import json
import os
import re
import shlex
import sys
from pathlib import Path, PurePosixPath

REGISTRY = Path(__file__).resolve().parent / "decision_models.json"
SCHEMA = 1
ROOT_ENV = "MIMIK_MODEL_ROOT"
KEY_RE = re.compile(r"^r[0-9]-[a-z0-9][a-z0-9.-]{0,40}$")
ROUTER_ID_RE = re.compile(r"^R[0-9]$")
SHA_RE = re.compile(r"^[0-9a-f]{64}$")
IMAGE_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
MEMORY_RE = re.compile(r"^[1-9][0-9]{0,3}g$")
ROLE_RE = re.compile(r"^[a-z][a-z0-9_]{0,40}$")
RUNTIMES = ("anyjev-llamacpp", "kev-torch")
DEVICES = ("cpu", "gpu")
KEV_ROLES = ("adapter", "head", "base_weights", "base_tokenizer", "base_config")
COMMON_KEYS = {
    "router_id",
    "label",
    "runtime",
    "image",
    "image_tag",
    "library_revision",
    "files",
    "devices",
    "threads",
    "n_ctx",
    "memory_limit",
    "seconds_per_decision",
    "condition",
    "licence",
}
KEV_KEYS = {"hf_cache", "kev_checkpoint"}


class RegistryError(ValueError):
    pass


def safe_relative(path, where) -> str:
    if not isinstance(path, str) or not path:
        raise RegistryError(f"{where}: path must be text")
    p = PurePosixPath(path)
    if p.is_absolute() or ".." in p.parts or "\\" in path or "\n" in path or "\t" in path:
        raise RegistryError(f"{where}: path must stay under the model root ({path!r})")
    return path


def validate(doc) -> dict:
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA:
        raise RegistryError(f"schema must be {SCHEMA}")
    if set(doc) - {"schema", "description", "model_root", "routers"}:
        raise RegistryError(
            f"unknown top-level keys {sorted(set(doc) - {'schema', 'description', 'model_root', 'routers'})}"
        )
    if not isinstance(doc.get("model_root"), str) or not os.path.isabs(doc["model_root"]):
        raise RegistryError("model_root must be an absolute path")
    routers = doc.get("routers")
    if not isinstance(routers, dict) or not routers:
        raise RegistryError("routers must be a non-empty object")
    ids = set()
    for key, entry in routers.items():
        where = f"routers.{key}"
        if not KEY_RE.match(key):
            raise RegistryError(f"{where}: key must match {KEY_RE.pattern}")
        if not isinstance(entry, dict):
            raise RegistryError(f"{where}: must be an object")
        allowed = COMMON_KEYS | (KEV_KEYS if entry.get("runtime") == "kev-torch" else set())
        missing, unknown = allowed - set(entry), set(entry) - allowed
        if missing or unknown:
            raise RegistryError(f"{where}: missing {sorted(missing)}, unknown {sorted(unknown)}")
        if entry["runtime"] not in RUNTIMES:
            raise RegistryError(f"{where}: runtime must be one of {RUNTIMES}")
        if not ROUTER_ID_RE.match(str(entry["router_id"])) or entry["router_id"] in ids:
            raise RegistryError(f"{where}: router_id must be unique R<n>")
        ids.add(entry["router_id"])
        if not IMAGE_RE.match(str(entry["image"])):
            raise RegistryError(f"{where}: image must be a pinned local image id sha256:<64 hex>")
        if not MEMORY_RE.match(str(entry["memory_limit"])):
            raise RegistryError(f"{where}: memory_limit like 8g")
        if not entry["devices"] or set(entry["devices"]) - set(DEVICES):
            raise RegistryError(f"{where}: devices must be a subset of {DEVICES}")
        for name, lo, hi in (("threads", 1, 64), ("n_ctx", 512, 262144)):
            value = entry[name]
            if isinstance(value, bool) or not isinstance(value, int) or not lo <= value <= hi:
                raise RegistryError(f"{where}: {name} must be an integer {lo}..{hi}")
        spd = entry["seconds_per_decision"]
        if spd is not None and (
            isinstance(spd, bool) or not isinstance(spd, (int, float)) or spd <= 0
        ):
            raise RegistryError(f"{where}: seconds_per_decision must be positive or null")
        files = entry["files"]
        if not isinstance(files, list) or not files:
            raise RegistryError(f"{where}: files must be a non-empty list")
        roles = set()
        for n, item in enumerate(files):
            fw = f"{where}.files[{n}]"
            if not isinstance(item, dict) or set(item) != {"role", "path", "size", "sha256"}:
                raise RegistryError(f"{fw}: needs exactly role, path, size, sha256")
            if not ROLE_RE.match(str(item["role"])) or item["role"] in roles:
                raise RegistryError(f"{fw}: role must be unique")
            roles.add(item["role"])
            safe_relative(item["path"], fw)
            if (
                isinstance(item["size"], bool)
                or not isinstance(item["size"], int)
                or item["size"] <= 0
            ):
                raise RegistryError(f"{fw}: size must be a positive integer")
            if not SHA_RE.match(str(item["sha256"])):
                raise RegistryError(f"{fw}: sha256 must be 64 hex")
        if entry["runtime"] == "anyjev-llamacpp":
            if roles != {"gguf"}:
                raise RegistryError(f"{where}: an AnyJev entry has exactly one gguf file")
        else:
            if set(KEV_ROLES) - roles:
                raise RegistryError(f"{where}: Kev needs roles {KEV_ROLES}")
            cache = safe_relative(entry["hf_cache"], f"{where}.hf_cache")
            ckpt = safe_relative(entry["kev_checkpoint"], f"{where}.kev_checkpoint")
            for item in files:
                if not PurePosixPath(item["path"]).is_relative_to(cache):
                    raise RegistryError(f"{where}: Kev files must live in its hf_cache")
            if not PurePosixPath(ckpt).is_relative_to(cache):
                raise RegistryError(f"{where}: kev_checkpoint must live in its hf_cache")
    return doc


def load(path=REGISTRY) -> dict:
    return validate(json.loads(Path(path).read_text(encoding="utf-8")))


def model_root(doc, env=None) -> Path:
    env = os.environ if env is None else env
    return Path(env.get(ROOT_ENV) or doc["model_root"])


def under_root(path: Path, root: Path) -> bool:
    """The resolved path stays beneath the resolved model root (symlinks followed; review
    Sol 6). Hugging Face snapshot -> blob links inside the root are fine."""
    return Path(path).resolve().is_relative_to(Path(root).resolve())


def check_files(doc, key, env=None) -> list:
    """Sizes and containment on disk (hashes are the runner's job). Problems, empty if fine."""
    root = model_root(doc, env)
    entry = doc["routers"][key]
    problems = []
    for item in entry["files"]:
        path = root / item["path"]
        if not path.exists():
            problems.append(f"{item['role']}: missing {path}")
        elif not under_root(path, root):
            problems.append(f"{item['role']}: resolves outside the model root ({path})")
        elif path.resolve().stat().st_size != item["size"]:
            problems.append(f"{item['role']}: size differs ({path})")
    if entry.get("hf_cache") and (root / entry["hf_cache"]).exists():
        if not under_root(root / entry["hf_cache"], root):
            problems.append("hf_cache: resolves outside the model root")
    return problems


def summary_extra(doc, key, disk_rate=None) -> dict:
    """Run identity the runner hands the driver (summary.json): what the run actually used,
    carried into the selection report and the decision file (review Sol B1, m9)."""
    e = doc["routers"][key]
    return {
        "router_key": key,
        "router_id": e["router_id"],
        "runtime_kind": e["runtime"],
        "image": e["image"],
        "library_revision": e["library_revision"],
        "artefacts": [{k: f[k] for k in ("role", "size", "sha256")} for f in e["files"]],
        "threads": e["threads"],
        "n_ctx": e["n_ctx"],
        "total_bytes": sum(f["size"] for f in e["files"]),
        "disk_rate": disk_rate,
    }


def shell(doc, key, env=None) -> str:
    if key not in doc["routers"]:
        raise RegistryError(f"unknown router {key!r}")
    e = doc["routers"][key]
    root = model_root(doc, env)
    values = {
        "dm_router_id": e["router_id"],
        "dm_label": e["label"],
        "dm_runtime": e["runtime"],
        "dm_image": e["image"],
        "dm_library_revision": e["library_revision"],
        "dm_threads": str(e["threads"]),
        "dm_n_ctx": str(e["n_ctx"]),
        "dm_memory": e["memory_limit"],
        "dm_root": str(root),
        "dm_total_bytes": str(sum(f["size"] for f in e["files"])),
        "dm_hf_cache": e.get("hf_cache", ""),
        "dm_kev_checkpoint": e.get("kev_checkpoint", ""),
        "dm_gguf": next((f["path"] for f in e["files"] if f["role"] == "gguf"), ""),
        "dm_gguf_sha256": next((f["sha256"] for f in e["files"] if f["role"] == "gguf"), ""),
    }
    lines = [f"{k}={shlex.quote(v)}" for k, v in values.items()]
    triples = " ".join(
        shlex.quote(f"{f['role']}\t{f['path']}\t{f['size']}\t{f['sha256']}") for f in e["files"]
    )
    lines.append(f"dm_files=({triples})")
    return "\n".join(lines) + "\n"


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--registry", type=Path, default=REGISTRY)
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check")
    c.add_argument("--files", action="store_true")
    sub.add_parser("list")
    s = sub.add_parser("shell")
    s.add_argument("key")
    x = sub.add_parser("summary-extra")
    x.add_argument("key")
    x.add_argument("--disk-rate", type=int)
    args = p.parse_args(argv)
    try:
        doc = load(args.registry)
        if args.cmd == "check":
            problems = [
                f"{k}: {x}"
                for k in doc["routers"]
                for x in (check_files(doc, k) if args.files else [])
            ]
            for line in problems:
                print(line, file=sys.stderr)
            print(
                f"{len(doc['routers'])} routers{'' if not args.files else f', {len(problems)} file problems'}"
            )
            return 1 if problems else 0
        if args.cmd == "list":
            for key, e in doc["routers"].items():
                print(f"{key} {e['router_id']} {e['runtime']} {e['label']}")
            return 0
        if args.cmd == "summary-extra":
            print(json.dumps(summary_extra(doc, args.key, args.disk_rate)))
            return 0
        sys.stdout.write(shell(doc, args.key))
        return 0
    except RegistryError as error:
        print(f"decision registry: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
