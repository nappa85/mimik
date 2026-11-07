"""The vendored mupdf fork in the source package and in the audit (DEP-29).

`cargo package` cannot carry the fork, found empirically with cargo 1.99.1:

- the normalized manifest drops `[patch.crates-io]`;
- the packaged `Cargo.lock` is rewritten to the registry source (with checksum) for mupdf;
- a directory holding its own `Cargo.toml` is a separate package, so `vendor/mupdf/` is left
  out even though `package.include` lists it (the entry documents intent for the allowlist).

Left alone, the corresponding source would build crates.io mupdf 0.8.0 and not the binary's
renderer. `package` rewrites the package in place: the patch entry is appended to the manifest,
the lock is restored to the repository's path form (checked to differ only in the mupdf
package's `source` and `checksum` lines) and `vendor/mupdf/` is copied from the working tree
after an exact check against the pinned inventory `vendor/mupdf.sha256` (no extra, missing or
changed file). The output is deterministic. `cargo vendor` does not vendor path packages, so
the fork travels in the source package, not in the dependency-sources archive.

RustSec matches advisories to crates.io sources, and the lock's path entry has none, so
`cargo audit` would pass silently on a mupdf advisory. `audit-lock` writes a copy of the lock
with mupdf's registry source and checksum restored for `cargo audit --file`.

    python tools/vendored_fork.py package CRATE      # after cargo package
    python tools/vendored_fork.py audit-lock OUT     # then cargo audit --file OUT
    python tools/vendored_fork.py inventory          # rewrite vendor/mupdf.sha256
"""

import argparse
import gzip
import hashlib
import io
import re
import tarfile
from pathlib import Path

PATCH = '\n[patch.crates-io]\nmupdf = { path = "vendor/mupdf" }\n'
FORK = "vendor/mupdf"
INVENTORY = "vendor/mupdf.sha256"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
# The crates.io mupdf 0.8.0 checksum (the original Cargo.lock entry).
MUPDF_CHECKSUM = "185fb74927a40c569b7152c01c73f4c0206059b8b3d37c7eb007cc3f63453b64"
# The mupdf entry in a Cargo.lock: its registry-only lines are the ones the path form lacks.
_REGISTRY_ENTRY = re.compile(
    r'(\[\[package\]\]\nname = "mupdf"\nversion = "0\.8\.0"\n)'
    r'source = "[^"]*"\nchecksum = "[0-9a-f]{64}"\n'
)
_PATH_ENTRY = re.compile(
    r'\[\[package\]\]\nname = "mupdf"\nversion = "0\.8\.0"\n(?!source|checksum)'
)


def path_form_lock(packaged: str) -> str:
    """The packaged lock with mupdf's registry `source`/`checksum` removed (path package)."""
    restored, count = _REGISTRY_ENTRY.subn(r"\1", packaged)
    if count != 1:
        raise ValueError("packaged Cargo.lock has no single registry mupdf entry")
    return restored


def registry_lock(lock: str) -> str:
    """The repository lock with mupdf's registry `source` and pinned `checksum` restored."""
    if len(_PATH_ENTRY.findall(lock)) != 1 or lock.count('name = "mupdf"\n') != 1:
        raise ValueError("Cargo.lock does not have exactly one path-form mupdf 0.8.0 entry")
    return _PATH_ENTRY.sub(
        lambda m: m.group(0) + f'source = "{REGISTRY}"\nchecksum = "{MUPDF_CHECKSUM}"\n',
        lock,
        count=1,
    )


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def tree_files(root: Path) -> dict[str, bytes]:
    """Every file under the fork directory by path relative to it; links and specials refused."""
    files = {}
    for path in sorted((root / FORK).rglob("*")):
        relative = path.relative_to(root / FORK).as_posix()
        if path.is_symlink():
            raise ValueError(f"symbolic link in the vendored fork: {relative}")
        if path.is_file():
            files[relative] = path.read_bytes()
        elif not path.is_dir():
            raise ValueError(f"special file in the vendored fork: {relative}")
    return files


def inventory_text(files: dict[str, bytes]) -> str:
    return "".join(f"{digest(data)}  {name}\n" for name, data in sorted(files.items()))


def check_inventory(listing: str, files: dict[str, bytes]) -> None:
    """The fork's files must be exactly the pinned inventory: none extra, missing or changed."""
    pinned = {}
    for line in listing.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (\S.*)", line)
        if not match or match.group(2) in pinned:
            raise ValueError(f"invalid vendored fork inventory line: {line!r}")
        pinned[match.group(2)] = match.group(1)
    actual = {name: digest(data) for name, data in files.items()}
    if extra := sorted(set(actual) - set(pinned)):
        raise ValueError(f"vendored fork has files outside the inventory: {extra}")
    if missing := sorted(set(pinned) - set(actual)):
        raise ValueError(f"vendored fork lacks inventoried files: {missing}")
    if changed := sorted(name for name in pinned if pinned[name] != actual[name]):
        raise ValueError(f"vendored fork files differ from the inventory: {changed}")


def augment(crate: Path, root: Path) -> None:
    if not (root / FORK / "Cargo.toml").is_file():
        raise ValueError(f"{FORK}/Cargo.toml is missing from the working tree")
    files = tree_files(root)
    listing = (root / INVENTORY).read_text(encoding="utf-8")
    check_inventory(listing, files)
    with tarfile.open(crate, "r:gz") as archive:
        members = [(m, archive.extractfile(m).read() if m.isfile() else None) for m in archive]
    names = [m.name for m, _ in members]
    prefix = names[0].split("/", 1)[0]
    if any(name.startswith(f"{prefix}/{FORK}/") for name in names):
        raise ValueError("the package already holds the vendored fork")
    repo_lock = (root / "Cargo.lock").read_text(encoding="utf-8")
    listing_packaged = False
    out = io.BytesIO()
    # Fixed gzip timestamp and GNU headers (cargo's own format): identical input, identical bytes.
    with (
        gzip.GzipFile(fileobj=out, mode="wb", mtime=0) as compressed,
        tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as target,
    ):
        for member, data in members:
            if member.name == f"{prefix}/{INVENTORY}":
                # cargo packages this plain file itself; it must be the pinned one.
                if data.decode("utf-8") != listing:
                    raise ValueError("the packaged inventory differs from the repository's")
                listing_packaged = True
            if member.name == f"{prefix}/Cargo.toml":
                text = data.decode("utf-8")
                if "[patch" in text:
                    raise ValueError("the packaged manifest already has a patch section")
                data = (text.rstrip("\n") + "\n" + PATCH).encode("utf-8")
            elif member.name == f"{prefix}/Cargo.lock":
                if path_form_lock(data.decode("utf-8")) != repo_lock:
                    raise ValueError("packaged Cargo.lock differs from the repository's")
                data = repo_lock.encode("utf-8")
            if data is not None:
                member.size = len(data)
            target.addfile(member, io.BytesIO(data) if data is not None else None)
        template = next(m for m, _ in members if m.name == f"{prefix}/Cargo.toml")
        extra = {} if listing_packaged else {INVENTORY: listing.encode("utf-8")}
        extra |= {f"{FORK}/{name}": data for name, data in files.items()}
        for relative, content in sorted(extra.items()):
            info = tarfile.TarInfo(f"{prefix}/{relative}")
            info.mtime = template.mtime
            info.size = len(content)
            info.mode = 0o644
            target.addfile(info, io.BytesIO(content))
    crate.write_bytes(out.getvalue())


def main_inventory(root: Path) -> None:
    (root / INVENTORY).write_text(inventory_text(tree_files(root)), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("package").add_argument("crate", type=Path)
    commands.add_parser("audit-lock").add_argument("output", type=Path)
    commands.add_parser("inventory")
    args = parser.parse_args()
    if args.command == "package":
        augment(args.crate, args.root)
        print("Vendored fork added to the source package.")
    elif args.command == "audit-lock":
        lock = (args.root / "Cargo.lock").read_text(encoding="utf-8")
        args.output.write_text(registry_lock(lock), encoding="utf-8")
        print(f"Audit lock written to {args.output}.")
    else:
        main_inventory(args.root)
        print(f"{INVENTORY} rewritten.")


if __name__ == "__main__":
    main()
