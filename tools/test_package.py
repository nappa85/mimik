import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import package_release
from verify_source_package import REQUIRED, verify_members


class PackageTests(unittest.TestCase):
    def test_release_notes_state_the_pinned_toolchain(self):
        """Review of the 1.99.1 upgrade: BUILD.txt and SOURCE.txt repeated the Rust version as
        literals, so a later bump could leave the source offer naming another toolchain."""
        project = Path(__file__).resolve().parents[1]
        pinned = (project / "rust-toolchain.toml").read_text(encoding="utf-8")
        self.assertEqual(
            package_release.toolchain(project / "rust-toolchain.toml"),
            pinned.split('channel = "', 1)[1].split('"', 1)[0],
        )
        script = (project / "tools" / "package_release.py").read_text(encoding="utf-8")
        self.assertNotIn("Rust: 1.", script)
        self.assertNotIn("(Rust 1.", script)

    def test_hierarchy_and_screen_regressions_are_required_source_members(self):
        members = [f"mimik/{name}" for name in REQUIRED]
        for name in (
            "tools/evidence_forest.py",
            "tools/test_evidence_forest.py",
            "tools/local_model_screen.py",
            "tools/test_local_model_screen.py",
            "tools/run_model_screen.py",
            "tools/screen_limits.py",
            "tools/summarize_model_screen.py",
            "tools/test_summarize_model_screen.py",
            "tools/tool_loop_benchmark.py",
            "tools/test_tool_loop_benchmark.py",
            "tools/test_cel_engine.py",
            "tools/run_tool_loop_benchmark.sh",
            "tools/fixtures/tool_loop_pack/definition.json",
            "tools/fixtures/tool_loop_pack/characters/hero.json",
            "examples/tool-loop-demo/definition.json",
            "examples/tool-loop-demo/cases-heldout.json",
        ):
            with self.subTest(name=name), self.assertRaises(ValueError):
                verify_members([m for m in members if m != "mimik/" + name])

    def test_actual_cargo_include_excludes_private_files_and_nested_cache(self):
        project = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in ["Cargo.toml", "Cargo.lock", "LICENSE", "rust-toolchain.toml", "README.md"]:
                shutil.copy2(project / name, root / name)
            # cargo package normalizes auto-discovered README metadata explicitly.
            # Exercise that delivered-source form even when testing the working tree.
            manifest = root / "Cargo.toml"
            content = manifest.read_text()
            if "\nreadme = " not in content:
                manifest.write_text(
                    content.replace("[package]\n", '[package]\nreadme = "README.md"\n', 1)
                )
            (root / "src").mkdir()
            (root / "src/main.rs").write_text("fn main() {}", encoding="utf-8")
            forbidden = [
                ".env",
                "data/manual.pdf",
                ".ruff_cache/.gitignore",
                "tools/__pycache__/cache.pyc",
                "data/local.sqlite3",
            ]
            for name in forbidden:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("synthetic private fixture", encoding="utf-8")
            result = subprocess.run(
                ["cargo", "package", "--list", "--locked", "--allow-dirty"],
                cwd=root,
                capture_output=True,
                text=True,
                check=True,
                timeout=60,
            )
            included = set(result.stdout.splitlines())
            self.assertTrue(
                {"Cargo.toml", "Cargo.lock", "LICENSE", "src/main.rs"}.issubset(included)
            )
            self.assertTrue(included.isdisjoint(forbidden), included.intersection(forbidden))

    def test_archive_checker_rejects_prohibited_members(self):
        for name in [
            "mimik/data/manual.pdf",
            "mimik/.ruff_cache/.gitignore",
            "mimik/.env",
            "mimik/../secret",
        ]:
            with self.assertRaises(ValueError):
                verify_members([f"mimik/{entry}" for entry in REQUIRED] + [name])

    def test_otherwise_valid_archive_rejects_unapproved_content(self):
        members = [f"mimik/{name}" for name in REQUIRED]
        verify_members(members)
        for bad in ["unapproved/corpus.txt", "data/manual.pdf", ".ruff_cache/.gitignore", ".env"]:
            with self.assertRaises(ValueError):
                verify_members(members + [f"mimik/{bad}"])
        with self.assertRaises(ValueError):
            verify_members(members + [members[0]])

    def test_preferred_form_htmx_source_is_required_even_without_compile_dependency(self):
        preferred = "mimik/assets/htmx.source.js"
        members = sorted({f"mimik/{name}" for name in REQUIRED} | {preferred})
        verify_members(members)
        with self.assertRaises(ValueError):
            verify_members([name for name in members if name != preferred])

    def test_logo_is_the_only_reviewed_png_exception(self):
        members = [f"mimik/{name}" for name in REQUIRED]
        verify_members(members)
        with self.assertRaises(ValueError):
            verify_members([name for name in members if name != "mimik/logo.png"])
        for name in ["assets/logo.png", "Logo.png", "data/logo.png", "unapproved/owner.png"]:
            with self.assertRaises(ValueError):
                verify_members(members + [f"mimik/{name}"])
