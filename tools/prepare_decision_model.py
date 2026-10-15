"""Container-only public HF snapshot downloader/verifier for optional local evaluation."""

import argparse
import fnmatch
import hashlib
import json
import re
import shutil
import urllib.request
from pathlib import Path

PATTERNS = ("*.json", "*.safetensors", "*.pt", "*.txt", "*.jinja", "LICENSE", "README.md")


def readable_public_tree(output, repo, revision):
    """HF makes this public-only derived metadata0600; the inference UID must read it."""
    path = output / "hf" / ("models--" + repo.replace("/", "--")) / "trees" / (revision + ".json")
    if path.exists():
        if path.is_symlink() or not path.resolve().is_relative_to(output.resolve()):
            raise ValueError("Unexpected public cache-tree path")
        path.chmod(0o644)


def verify_file(path, entry):
    if path.stat().st_size != entry["size"]:
        raise ValueError(f"Wrong byte count: {entry['rfilename']}")
    with path.open("rb") as stream:
        sha256 = hashlib.file_digest(stream, "sha256").hexdigest()
    if entry.get("lfs"):
        if sha256 != entry["lfs"]["sha256"]:
            raise ValueError(f"Wrong LFS hash: {entry['rfilename']}")
    else:
        git = hashlib.sha1(f"blob {entry['size']}\0".encode())
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                git.update(chunk)
        if git.hexdigest() != entry["blobId"]:
            raise ValueError(f"Wrong Git blob hash: {entry['rfilename']}")
    return {"file": entry["rfilename"], "bytes": entry["size"], "sha256": sha256}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", args.repo) or not re.fullmatch(
        r"[0-9a-f]{40}", args.revision
    ):
        raise ValueError("Use a public repository and immutable full revision.")
    if not args.output.is_dir():
        raise ValueError("Create the intended output directory first.")
    from huggingface_hub import snapshot_download

    url = f"https://huggingface.co/api/models/{args.repo}/revision/{args.revision}?blobs=true"
    with urllib.request.urlopen(url, timeout=30) as response:
        metadata = json.load(response)
    if metadata["sha"] != args.revision or metadata.get("private") or metadata.get("gated"):
        raise ValueError("Unexpected model provenance/access scope.")
    selected = [
        e for e in metadata["siblings"] if any(fnmatch.fnmatch(e["rfilename"], p) for p in PATTERNS)
    ]
    if sum(e["size"] for e in selected) + 3 * 1024**3 > shutil.disk_usage(args.output).free:
        raise ValueError("Insufficient disk reserve for the pinned snapshot.")
    snapshot = Path(
        snapshot_download(
            args.repo,
            revision=args.revision,
            cache_dir=args.output / "hf",
            token=False,
            allow_patterns=list(PATTERNS),
            max_workers=4,
        )
    )
    files = [verify_file(snapshot / entry["rfilename"], entry) for entry in selected]
    readable_public_tree(args.output, args.repo, args.revision)
    record = {
        "repo": args.repo,
        "revision": args.revision,
        "snapshot_relative": str(snapshot.relative_to(args.output)),
        "metadata": metadata,
        "verified_files": files,
    }
    destination = args.output / (args.repo.replace("/", "--") + "-verified.json")
    destination.write_text(json.dumps(record, indent=2))
    print(
        json.dumps(
            {
                "repo": args.repo,
                "revision": args.revision,
                "verified_files": len(files),
                "snapshot_relative": record["snapshot_relative"],
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
