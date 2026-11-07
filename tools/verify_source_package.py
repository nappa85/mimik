"""Validate actual corresponding-source archive members, never extract private paths."""

import argparse
import tarfile
import tomllib
from pathlib import Path, PurePosixPath

import vendored_fork
from pathspec import PathSpec

REQUIRED = {
    "licenses/ijg-libjpeg-9f-README",
    "licenses/ijg-libjpeg-9f-upstream.sha256",
    "Cargo.toml",
    "Cargo.lock",
    "vendor/mupdf.sha256",
    "vendor/mupdf/Cargo.toml",
    "vendor/mupdf/LICENSE",
    "vendor/mupdf/MIMIK-VENDOR.md",
    "vendor/mupdf/src/lib.rs",
    "vendor/mupdf/src/text_page.rs",
    "LICENSE",
    "logo.png",
    "rust-toolchain.toml",
    "src/main.rs",
    "src/lib.rs",
    "schemas/llama-runtime-v1.json",
    "tools/llama_runtime.py",
    "tools/mupdf_notices.py",
    "tools/test_mupdf_notices.py",
    "examples/fake_llama_server.rs",
    "templates/base.html",
    "assets/htmx.min.js",
    "assets/htmx.source.js",
    "assets/response-targets.source.js",
    "assets/response-targets-LICENSE",
    "assets/htmx-LICENSE",
    "assets/hakoniwa-LICENSE",
    "migrations/001_characters.sql",
    "tools/smoke_release.py",
    "tools/evidence_forest.py",
    "tools/test_evidence_forest.py",
    "tools/local_model_screen.py",
    "tools/test_local_model_screen.py",
    "tools/run_model_screen.py",
    "tools/screen_limits.py",
    "tools/summarize_model_screen.py",
    "tools/test_summarize_model_screen.py",
    "examples/sheet-harness-demo/README.md",
    "examples/sheet-harness-demo/rules.md",
    "examples/sheet-harness-demo/sheet.html",
    "examples/sheet-harness-demo/character.json",
    "examples/sheet-harness-demo/review-cases.json",
    "examples/sheet-harness-demo/decision-cases.json",
    "tools/tool_loop_benchmark.py",
    "tools/test_tool_loop_benchmark.py",
    "tools/test_cel_engine.py",
    "tools/run_tool_loop_benchmark.sh",
    "tools/fixtures/tool_loop_pack/rules.md",
    "tools/fixtures/tool_loop_pack/definition.json",
    "tools/fixtures/tool_loop_pack/cases-dev.json",
    "tools/fixtures/tool_loop_pack/cases-heldout.json",
    "tools/fixtures/tool_loop_pack/dry-run-script.json",
    "tools/fixtures/tool_loop_pack/characters/hero.json",
    "examples/tool-loop-demo/README.md",
    "examples/tool-loop-demo/rules.md",
    "examples/tool-loop-demo/definition.json",
    "examples/tool-loop-demo/cases-dev.json",
    "examples/tool-loop-demo/cases-heldout.json",
    ".github/workflows/ci.yml",
}


def verify_members(names):
    if len(names) != len(set(names)):
        raise ValueError("Duplicate source archive member")
    manifest = tomllib.loads(
        (Path(__file__).resolve().parents[1] / "Cargo.toml").read_bytes().decode("utf-8")
    )
    policy = PathSpec.from_lines("gitwildmatch", manifest["package"]["include"])
    paths = [PurePosixPath(name) for name in names]
    roots = {path.parts[0] for path in paths if path.parts}
    if len(roots) != 1:
        raise ValueError("Source archive must have a single package root")
    relative = set()
    for path in paths:
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("Invalid archive path")
        parts = path.parts[1:]
        if not parts:
            continue
        if any(
            part in {"data", "target", "__pycache__", ".ruff_cache", ".git"}
            or part.startswith(".venv")
            for part in parts
        ):
            raise ValueError("Source archive contains private data/cache paths")
        if any(part.startswith(".env") and part != ".env.example" for part in parts):
            raise ValueError("Source archive contains a secret-file path")
        if (
            path.suffix.lower() in {".pdf", ".sqlite3", ".png", ".jpeg", ".jpg"}
            and "/".join(parts) != "logo.png"
        ):
            raise ValueError("Source archive contains source corpus/database/image material")
        name = "/".join(parts)
        if name not in {"Cargo.toml.orig", ".cargo_vcs_info.json"} and not policy.match_file(name):
            raise ValueError("Unapproved source archive member")
        relative.add(name)
    if not REQUIRED.issubset(relative):
        raise ValueError("Corresponding source is missing required build/test inputs")


def verify_fork(manifest_text, lock_text, text_page, listing=None, files=None):
    """DEP-29: the package must build the vendored fork, not crates.io mupdf. `cargo package`
    drops the patch entry and rewrites the lock, so `tools/vendored_fork.py` restores both."""
    patch = tomllib.loads(manifest_text).get("patch", {}).get("crates-io", {})
    if patch != {"mupdf": {"path": "vendor/mupdf"}}:
        raise ValueError("Source package manifest does not patch mupdf to vendor/mupdf")
    packages = [p for p in tomllib.loads(lock_text)["package"] if p["name"] == "mupdf"]
    if len(packages) != 1 or "source" in packages[0] or "checksum" in packages[0]:
        raise ValueError("Source package lock does not resolve mupdf to the vendored path")
    if "pub fn is_rtl" not in text_page:
        raise ValueError("Vendored mupdf is not the Mimik fork")
    if listing is not None:
        vendored_fork.check_inventory(listing, files)


def verify_archive(path):
    with tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        if any(not member.isfile() and not member.isdir() for member in members):
            raise ValueError("Unexpected special archive member")
        names = [member.name for member in members]
        verify_members(names)
        root = names[0].split("/", 1)[0]

        def read(name):
            return archive.extractfile(f"{root}/{name}").read().decode("utf-8")

        prefix = f"{root}/{vendored_fork.FORK}/"
        files = {
            member.name.removeprefix(prefix): archive.extractfile(member).read()
            for member in members
            if member.isfile() and member.name.startswith(prefix)
        }
        verify_fork(
            read("Cargo.toml"),
            read("Cargo.lock"),
            read("vendor/mupdf/src/text_page.rs"),
            read(vendored_fork.INVENTORY),
            files,
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    verify_archive(args.archive)
    print("Corresponding-source member allowlist verified.")


if __name__ == "__main__":
    main()
