"""The pinned llama.cpp runtime (ASSIST-33, ASSIST-42): fetch, verify and stage exactly the
files of `schemas/llama-runtime-v1.json`, the manifest Mimik embeds.

  python tools/llama_runtime.py fetch --out target/llama-fetch
      downloads each pinned release asset, the external licence notices, the test model and
      the pinned source (a shallow fetch of the commit, then `git archive`), verifies every
      hash, and writes <out>/llama/<build dir>/<file>, <out>/notices/<notice>, <out>/<source
      tar> and <out>/models/<test model>.
  python tools/llama_runtime.py verify --dir <llama dir> [--models <dir>]
      re-hashes every file against the manifest (an extra, missing or changed file fails),
      then reapplies the manifest's modes (artifact hand-offs drop them).

Fails closed: any download, hash or layout mismatch exits non-zero.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

MANIFEST = Path(__file__).resolve().parents[1] / "schemas" / "llama-runtime-v1.json"
BLOCK = 1 << 20


def manifest(path=MANIFEST):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(BLOCK):
            digest.update(chunk)
    return digest.hexdigest()


def download(url, dest, expected, size):
    """Downloads `url` to `dest`, refusing anything but `size` bytes with SHA-256 `expected`."""
    digest = hashlib.sha256()
    total = 0
    with urllib.request.urlopen(url, timeout=120) as response, open(dest, "wb") as out:  # noqa: S310 (pinned https URLs from the manifest)
        while chunk := response.read(BLOCK):
            total += len(chunk)
            if total > size:
                raise ValueError(f"{url}: larger than the pinned {size} bytes")
            digest.update(chunk)
            out.write(chunk)
    if total != size or digest.hexdigest() != expected:
        raise ValueError(f"{url}: does not match the pinned size and SHA-256")


def extract(asset, build, dest):
    """Writes the build's files from the asset tarball under their shipped names. Only
    regular files named by the manifest are read; links and paths outside are refused."""
    wanted = {entry["from"]: entry for entry in build["files"]}
    dest.mkdir(parents=True, exist_ok=False)
    seen = set()
    with tarfile.open(asset, "r:gz") as tar:
        for member in tar:
            if member.name not in wanted:
                continue
            if not member.isfile():
                raise ValueError(f"{member.name}: not a regular file in the asset")
            entry = wanted[member.name]
            source = tar.extractfile(member)
            target = dest / entry["name"]
            with open(target, "wb") as out:
                while chunk := source.read(BLOCK):
                    out.write(chunk)
            seen.add(member.name)
    missing = set(wanted) - seen
    if missing:
        raise ValueError(f"the asset lacks {sorted(missing)}")


def verify(llama_dir, data=None):
    """The exact file set, sizes and hashes of every build; then the manifest's modes."""
    data = data or manifest()
    llama_dir = Path(llama_dir)
    present = {p.name for p in llama_dir.iterdir()}
    expected = {build["directory"] for build in data["builds"]}
    if present != expected:
        raise ValueError(f"{llama_dir}: holds {sorted(present)}, not {sorted(expected)}")
    for build in data["builds"]:
        directory = llama_dir / build["directory"]
        names = {entry["name"] for entry in build["files"]}
        found = set()
        for path in directory.iterdir():
            if path.is_symlink() or not path.is_file():
                raise ValueError(f"{path}: not a regular file")
            found.add(path.name)
        if found != names:
            raise ValueError(
                f"{directory}: extra {sorted(found - names)}, missing {sorted(names - found)}"
            )
        for entry in build["files"]:
            path = directory / entry["name"]
            if path.stat().st_size != entry["size"] or sha256(path) != entry["sha256"]:
                raise ValueError(f"{path}: differs from the pinned file")
            os.chmod(path, int(entry["mode"], 8))
        os.chmod(directory, 0o755)
    os.chmod(llama_dir, 0o755)


def verify_model(models_dir, data=None):
    data = data or manifest()
    model = data["test_model"]
    path = Path(models_dir) / model["name"]
    if path.stat().st_size != model["size"] or sha256(path) != model["sha256"]:
        raise ValueError(f"{path}: differs from the pinned test model")
    os.chmod(path, 0o644)
    os.chmod(models_dir, 0o700)


def fetch_notices(out, data=None):
    """Downloads each external licence text (components compiled into a build whose text is
    not in the source archive) to <out>/notices/<name>, size and SHA-256 checked."""
    data = data or manifest()
    notices = Path(out) / "notices"
    notices.mkdir(parents=True, exist_ok=False)
    for notice in data.get("external_notices", []):
        download(notice["url"], notices / notice["name"], notice["sha256"], notice["size"])
    verify_notices(notices, data)


def verify_notices(notices, data=None):
    """Exactly the pinned notice files, each matching its size and SHA-256."""
    data = data or manifest()
    notices = Path(notices)
    pinned = {notice["name"]: notice for notice in data.get("external_notices", [])}
    present = {path.name for path in notices.iterdir()} if notices.exists() else set()
    if present != set(pinned):
        raise ValueError(f"{notices}: holds {sorted(present)}, not {sorted(pinned)}")
    for name, notice in pinned.items():
        path = notices / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"{path}: not a regular file")
        if path.stat().st_size != notice["size"] or sha256(path) != notice["sha256"]:
            raise ValueError(f"{path}: differs from the pinned notice")
        os.chmod(path, 0o644)


def verify_source_tar(path, data=None):
    """The pinned source tar's size and SHA-256 (checked before any text is taken from it)."""
    data = data or manifest()
    source = data["source"]
    path = Path(path)
    if path.stat().st_size != source.get("tar_size") or sha256(path) != source["tar_sha256"]:
        raise ValueError(f"{path}: the source archive differs from the pinned one")


def source_archive(data, out):
    """`git archive` of the pinned commit from a shallow fetch; the tar's SHA-256 is pinned. It is
    a build-time input only (licence texts for the notices); it is never shipped."""
    source = data["source"]
    commit = data["llama_cpp"]["commit"]
    target = out / source["name"]
    with tempfile.TemporaryDirectory() as temp:
        git = ["git", "-C", temp]
        subprocess.run(["git", "init", "-q", temp], check=True)
        subprocess.run(
            [*git, "fetch", "-q", "--depth", "1", data["llama_cpp"]["repository"], commit],
            check=True,
            timeout=600,
        )
        with open(target, "wb") as handle:
            subprocess.run(
                [*git, "archive", "--format=tar", f"--prefix={source['prefix']}", commit],
                check=True,
                stdout=handle,
                timeout=600,
            )
    verify_source_tar(target, data)
    return target


def fetch(out, data=None):
    data = data or manifest()
    out = Path(out)
    out.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory() as temp:
        for build in data["builds"]:
            asset = Path(temp) / build["asset"]
            download(build["url"], asset, build["sha256"], build["size"])
            extract(asset, build, out / "llama" / build["directory"])
    verify(out / "llama", data)
    fetch_notices(out, data)
    model = data["test_model"]
    (out / "models").mkdir(mode=0o700)
    download(model["url"], out / "models" / model["name"], model["sha256"], model["size"])
    verify_model(out / "models", data)
    source_archive(data, out)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    fetching = commands.add_parser("fetch")
    fetching.add_argument("--out", type=Path, required=True)
    verifying = commands.add_parser("verify")
    verifying.add_argument("--dir", type=Path, required=True)
    verifying.add_argument("--models", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "fetch":
            fetch(args.out)
        else:
            verify(args.dir)
            if args.models:
                verify_model(args.models)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"llama runtime: {error}", file=sys.stderr)
        return 1
    print("llama runtime: verified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
