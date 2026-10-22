"""Package the Linux runtime and corresponding source using explicit allowlists."""

import argparse
import gzip
import hashlib
import io
import shutil
import tarfile
import tomllib
from pathlib import Path

import llama_runtime


def toolchain(path=Path("rust-toolchain.toml")):
    """The pinned Rust channel; BUILD.txt and SOURCE.txt state it, so it is read, not repeated."""
    return tomllib.loads(path.read_text(encoding="utf-8"))["toolchain"]["channel"]


def stage_llama(fetched, stage, data=None):
    """Copies llama-fetch's verified `llama/` into the stage, holding exactly the manifest's
    files (anything else refuses the package), re-hashed and with the manifest's modes."""
    data = data or llama_runtime.manifest()
    llama_runtime.verify(fetched / "llama", data)
    shutil.copytree(fetched / "llama", stage / "llama")
    llama_runtime.verify(stage / "llama", data)
    return data


def llama_notice(data):
    return (
        "\n## llama.cpp and ggml (llama/)\n\n"
        f"The official llama.cpp release {data['llama_cpp']['tag']} "
        f"({data['llama_cpp']['repository']}), every file of `llama/` built from it: MIT "
        "License, Copyright (c) 2023-2026 The ggml authors; the full text is "
        "`llama/cpu/LICENSE`.\n"
    )


def compress_source(fetched, data, output):
    """The pinned source tar, verified, gzip-compressed deterministically (no name, no time)."""
    tar = fetched / data["source"]["name"]
    if llama_runtime.sha256(tar) != data["source"]["tar_sha256"]:
        raise ValueError("the llama.cpp source archive differs from the pinned one")
    target = output / (tar.name + ".gz")
    with open(tar, "rb") as source, open(target, "wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as out:
            shutil.copyfileobj(source, out)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output", type=Path, default=Path("target/dist"))
    parser.add_argument(
        "--llama",
        type=Path,
        default=Path("target/llama-fetch"),
        help="llama-fetch's verified output: llama/ and the pinned source archive",
    )
    args = parser.parse_args()
    source = Path("target/package") / f"mimik-{args.version}.crate"
    binary = Path("target/release/mimik")
    notices = Path("target/THIRD-PARTY-NOTICES.md")
    if not all(path.is_file() for path in (source, binary, notices)):
        raise ValueError("Build, cargo package, and license notices must exist before packaging.")
    vendor = Path("target/vendor")
    vendor_config = Path("target/vendor-config.toml").read_text(encoding="utf-8")
    if (
        not vendor.is_dir()
        or tomllib.loads(vendor_config)["source"]["vendored-sources"]["directory"]
        != "target/vendor"
    ):
        raise ValueError("Run cargo vendor --locked target/vendor before packaging")
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"mimik-{args.version}-linux-x86_64"
    stage = args.output / name
    stage.mkdir(exist_ok=False)
    llama = stage_llama(args.llama, stage)
    for original, filename in [
        (binary, "mimik"),
        (Path("LICENSE"), "LICENSE"),
        (Path("assets/htmx-LICENSE"), "htmx-LICENSE"),
        (Path("assets/response-targets-LICENSE"), "response-targets-LICENSE"),
        (notices, "THIRD-PARTY-NOTICES.md"),
        (Path("docs/release-scope.md"), "README.md"),
    ]:
        shutil.copy2(original, stage / filename)
    with open(stage / "THIRD-PARTY-NOTICES.md", "a", encoding="utf-8") as out:
        out.write(llama_notice(llama))
    (stage / "BUILD.txt").write_text(
        f"Version: {args.version}\nRevision: {args.revision}\nRust: {toolchain()}\n"
        f"llama.cpp: {llama['llama_cpp']['tag']} (build {llama['llama_cpp']['build']}, "
        f"commit {llama['llama_cpp']['commit']})\n",
        encoding="utf-8",
    )
    (stage / "SQLITE-NOTICE.txt").write_text(
        "SQLite is bundled via libsqlite3-sys. SQLite is in the public domain.\nhttps://www.sqlite.org/copyright.html\n",
        encoding="utf-8",
    )
    source_name = f"mimik-{args.version}-source.crate"
    shutil.copy2(source, args.output / source_name)
    dependency_name = f"mimik-{args.version}-dependency-sources.tar.gz"
    with tarfile.open(args.output / dependency_name, "w:gz") as tar:
        tar.add(vendor, arcname="vendor")
        config_bytes = vendor_config.replace(
            'directory = "target/vendor"', 'directory = "vendor"'
        ).encode()
        entry = tarfile.TarInfo(".cargo/config.toml")
        entry.size = len(config_bytes)
        entry.mode = 0o644
        tar.addfile(entry, io.BytesIO(config_bytes))
    llama_source = compress_source(args.llama, llama, args.output)
    (stage / "SOURCE.txt").write_text(
        f"Corresponding source: accompanying {source_name} and {dependency_name}\nRevision: {args.revision}\nUnpack the .crate, then unpack dependency sources into that package root.\nBuild: cargo build --release --offline --locked (Rust {toolchain()}).\n"
        f"llama.cpp (llama/): accompanying {llama_source.name}, `git archive` of commit "
        f"{llama['llama_cpp']['commit']} (tar SHA-256 {llama['source']['tar_sha256']}); built by "
        "the official llama.cpp release workflow at that commit.\n",
        encoding="utf-8",
    )
    archive = args.output / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(stage, arcname=name)
    artifacts = [archive, args.output / source_name, args.output / dependency_name, llama_source]
    checksums = []
    for path in artifacts:
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
    (args.output / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")
    print(f"Release archives created in {args.output}")


if __name__ == "__main__":
    main()
