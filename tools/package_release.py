"""Package the Linux runtime and corresponding source using explicit allowlists."""

import argparse
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


def llama_notice(data, tar_path=None, notices_dir=None):
    """The notice for `llama/`: every component compiled into it, its text copied from the
    pinned (hash-verified) source tar, or, for a component the source does not hold, from
    the pinned external notice llama-fetch verified (re-checked here)."""
    out = [
        "\n## llama.cpp and ggml (llama/)\n\n"
        f"The official llama.cpp release {data['llama_cpp']['tag']} "
        f"({data['llama_cpp']['repository']}): every file of `llama/` is one of the release's own binaries, "
        "built by upstream from llama.cpp's source (not shipped here: `SOURCE.txt` points to the "
        "exact commit) together with components of the release builder's build environment "
        "(listed below). MIT "
        "License, Copyright (c) 2023-2026 The ggml authors (`llama/cpu/LICENSE`). Components "
        "compiled into it, with their licences:\n"
    ]
    notices = data["source"].get("notices", [])
    for notice in [*notices, *data.get("external_notices", [])]:
        out.append(f"\n- {notice['component']}: {notice['licence']}")
    out.append("\n")
    if tar_path is not None:
        llama_runtime.verify_source_tar(tar_path, data)
        prefix = data["source"]["prefix"]
        with tarfile.open(tar_path) as tar:
            for notice in notices:
                member = tar.extractfile(prefix + notice["path"])
                if member is None:
                    raise ValueError(f"{notice['path']}: not in the source archive")
                text = member.read().decode("utf-8")
                if "lines" in notice:
                    first, last = notice["lines"]
                    text = "".join(text.splitlines(keepends=True)[first - 1 : last])
                if not text.strip():
                    raise ValueError(f"{notice['path']}: no licence text")
                out.append(f"\n### {notice['component']}\n\n```\n{text.rstrip()}\n```\n")
    external = data.get("external_notices", [])
    if notices_dir is not None and external:
        llama_runtime.verify_notices(notices_dir, data)
        for notice in external:
            text = (Path(notices_dir) / notice["name"]).read_text(encoding="utf-8")
            out.append(
                f"\n### {notice['component']}\n\n{notice['licence']}.\n\n"
                f"```\n{text.rstrip()}\n```\n"
            )
    return "".join(out)


def llama_pointer(data):
    """Where llama.cpp's source is: upstream, by exact tag, commit and asset hashes. It is a
    separately executed, pinned build dependency, credited and pointed to, not shipped as
    source (a distribution package would depend on the distribution's llama.cpp)."""
    pinned = data["llama_cpp"]
    lines = [
        f"llama.cpp (llama/): the official release {pinned['tag']} ({pinned['release']}), source "
        f"at {pinned['repository']} commit {pinned['commit']}; not shipped as source, a "
        "separately executed build dependency built by upstream's release workflow. The "
        "shipped files are the release assets, hash-pinned:",
    ]
    lines += [f"  {build['asset']} SHA-256 {build['sha256']}" for build in data["builds"]]
    return "\n".join(lines) + "\n"


COUNTS = {1: "one", 2: "two", 3: "three", 4: "four", 5: "five", 6: "six"}


def check_inventory(readme, names):
    """The release scope (shipped as README.md) lists every archive and says how many
    SHA256SUMS covers."""
    missing = [name for name in names if f"`{name}`" not in readme]
    if missing:
        raise ValueError(f"the release scope does not list: {', '.join(missing)}")
    expected = f"checksums of the {COUNTS[len(names)]} archives"
    if expected not in readme:
        raise ValueError(f"the release scope must say: {expected}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output", type=Path, default=Path("target/dist"))
    parser.add_argument(
        "--llama",
        type=Path,
        default=Path("target/llama-fetch"),
        help="llama-fetch's verified output: llama/, the notices and the pinned source archive (read for licence texts only, never shipped)",
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
        out.write(llama_notice(llama, args.llama / llama["source"]["name"], args.llama / "notices"))
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
    (stage / "SOURCE.txt").write_text(
        f"Corresponding source: accompanying {source_name} and {dependency_name}\nRevision: {args.revision}\nUnpack the .crate, then unpack dependency sources into that package root.\nBuild: cargo build --release --offline --locked (Rust {toolchain()}).\n"
        + llama_pointer(llama),
        encoding="utf-8",
    )
    archive = args.output / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(stage, arcname=name)
    artifacts = [archive, args.output / source_name, args.output / dependency_name]
    check_inventory(
        Path("docs/release-scope.md").read_text(encoding="utf-8"),
        [path.name for path in artifacts],
    )
    checksums = []
    for path in artifacts:
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
    (args.output / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")
    print(f"Release archives created in {args.output}")


if __name__ == "__main__":
    main()
