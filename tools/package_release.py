"""Package the Linux runtime and corresponding source using explicit allowlists."""

import argparse
import hashlib
import io
import shutil
import tarfile
import tomllib
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output", type=Path, default=Path("target/dist"))
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
    for original, filename in [
        (binary, "mimik"),
        (Path("LICENSE"), "LICENSE"),
        (Path("assets/htmx-LICENSE"), "htmx-LICENSE"),
        (Path("assets/response-targets-LICENSE"), "response-targets-LICENSE"),
        (notices, "THIRD-PARTY-NOTICES.md"),
        (Path("docs/release-scope.md"), "README.md"),
    ]:
        shutil.copy2(original, stage / filename)
    (stage / "BUILD.txt").write_text(
        f"Version: {args.version}\nRevision: {args.revision}\nRust: 1.95.0\n", encoding="utf-8"
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
        f"Corresponding source: accompanying {source_name} and {dependency_name}\nRevision: {args.revision}\nUnpack the .crate, then unpack dependency sources into that package root.\nBuild: cargo build --release --offline --locked (Rust 1.95.0).\n",
        encoding="utf-8",
    )
    archive = args.output / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(stage, arcname=name)
    artifacts = [archive, args.output / source_name, args.output / dependency_name]
    checksums = []
    for path in artifacts:
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
    (args.output / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")
    print(f"Release archives created in {args.output}")


if __name__ == "__main__":
    main()
