import io
import shutil
import subprocess
import tarfile
import tempfile
import time
import unittest
from pathlib import Path

import package_release
import vendored_fork
from verify_source_package import REQUIRED, verify_fork, verify_members

LOCK_REGISTRY = (
    '[[package]]\nname = "mupdf"\nversion = "0.8.0"\n'
    'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
    f'checksum = "{"a" * 64}"\ndependencies = [\n "bitflags",\n]\n'
)
LOCK_PATH = '[[package]]\nname = "mupdf"\nversion = "0.8.0"\ndependencies = [\n "bitflags",\n]\n'
PATCHED = '[package]\nname = "mimik"\n[patch.crates-io]\nmupdf = { path = "vendor/mupdf" }\n'


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

    def test_hakoniwa_notices_carry_the_linking_exception_text(self):
        """DEP-24: cargo-about prints only the LGPL for `LGPL-3.0-only WITH
        LGPL-3.0-linking-exception`; when the notices name hakoniwa the exception must follow."""
        project = Path(__file__).resolve().parents[1]
        license_path = project / "assets" / "hakoniwa-LICENSE"
        text = package_release.hakoniwa_notice(
            "## GNU Lesser General Public License v3.0 only\n\n- hakoniwa 1.8.0\n",
            license_path,
        )
        self.assertIn("As a special exception to the GNU Lesser General Public License", text)
        self.assertIn("GNU LESSER GENERAL PUBLIC LICENSE", text)
        # Without hakoniwa in the build there is nothing to add.
        self.assertEqual(package_release.hakoniwa_notice("## MIT\n- serde 1\n", license_path), "")
        # A copy that lost the exception is refused.
        with tempfile.TemporaryDirectory() as tmp:
            stripped = Path(tmp) / "LICENSE"
            stripped.write_text("GNU LESSER GENERAL PUBLIC LICENSE\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                package_release.hakoniwa_notice("- hakoniwa 1.8.0", stripped)
        self.assertIn("assets/hakoniwa-LICENSE", REQUIRED)

    def test_the_release_and_its_notices_are_built_with_the_same_features(self):
        """DEP-24: the release binary and the generated notices must name the same features. The
        sandbox is in the default build (Hakoniwa, ASSIST-51): the release has no `--features`
        and `hakoniwa_notice` appends the exception text to its notices."""
        ci = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text()
        lines = [line.strip() for line in ci.splitlines()]
        release = [line for line in lines if line.startswith("cargo build --release")]
        notices = [line for line in lines if line.startswith("cargo about generate")]
        self.assertEqual(len(release), 1, release)
        self.assertEqual(len(notices), 1, notices)

        def features(line):
            return sorted(
                word
                for word in line.split()
                if word.startswith(("--features", "--all-features", "--no-default-features"))
            )

        self.assertEqual(features(release[0]), features(notices[0]))
        self.assertEqual(features(release[0]), [], "the release gained features: see DEP-24")

    def test_ci_runs_the_whole_live_suite_with_the_releases_own_runner(self):
        """The live run must use the release's Mimik binary as the runner (the single-threaded
        guard active), skip the GPU-only tests and count its tests; no bubblewrap is installed."""
        ci = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text()
        step = ci[ci.index("Start-time probe and the whole live suite") :]
        step = step[: step.index("tee target/confinement.log")]
        self.assertIn("MIMIK_LIVE_RUNNER: ${{ github.workspace }}/target/rel/mimik", step)
        self.assertIn("assistant::runtime::launch::live_tests::", step)
        self.assertIn("--skip live_gpu_", step)
        self.assertIn("--include-ignored", step)
        self.assertNotIn("bubblewrap", ci.lower())
        self.assertNotIn("sandbox-hakoniwa", ci)

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

    def test_vendored_fork_is_a_required_source_member(self):
        """DEP-29: a package without the vendored mupdf fork would build crates.io mupdf."""
        members = [f"mimik/{name}" for name in REQUIRED]
        for name in ("vendor/mupdf/Cargo.toml", "vendor/mupdf/src/text_page.rs"):
            self.assertIn(name, REQUIRED)
            with self.subTest(name=name), self.assertRaises(ValueError):
                verify_members([m for m in members if m != "mimik/" + name])

    def test_dependency_sources_do_not_unpack_over_the_fork(self):
        """The crates unpack into the package root beside `vendor/mupdf`; a `vendor` directory
        source would try to read the fork as a vendored crate and fail the offline rebuild."""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "target/vendor/serde").mkdir(parents=True)
            (root / "target/vendor/serde/lib.rs").write_text("", encoding="utf-8")
            archive = root / "deps.tar.gz"
            package_release.write_dependency_sources(
                archive,
                root / "target/vendor",
                '[source.vendored-sources]\ndirectory = "target/vendor"\n',
            )
            with tarfile.open(archive) as tar:
                names = tar.getnames()
                config = tar.extractfile(".cargo/config.toml").read().decode()
        self.assertIn("vendored-crates/serde/lib.rs", names)
        self.assertEqual([n for n in names if n.startswith("vendor/")], [])
        self.assertIn('directory = "vendored-crates"', config)

    def test_ci_audits_the_lock_with_mupdf_restored_to_its_registry_source(self):
        ci = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text()
        lines = [line.strip() for line in ci.splitlines()]
        first = lines.index("cargo audit --deny warnings")
        self.assertEqual(
            lines[first + 1 : first + 3],
            [
                "python tools/vendored_fork.py audit-lock target/Cargo.audit.lock",
                "cargo audit --deny warnings --file target/Cargo.audit.lock",
            ],
        )

    def test_ci_adds_the_fork_between_cargo_package_and_its_verification(self):
        ci = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text()
        lines = [line.strip() for line in ci.splitlines()]
        steps = [
            lines.index("cargo package --locked --allow-dirty --no-verify"),
            lines.index("python tools/vendored_fork.py package target/package/mimik-0.1.0.crate"),
            lines.index("python tools/verify_source_package.py target/package/mimik-0.1.0.crate"),
        ]
        self.assertEqual(steps, sorted(steps))

    def test_release_packaging_refuses_a_source_package_without_the_fork(self):
        import contextlib
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "target/package").mkdir(parents=True)
            (root / "target/release").mkdir()
            (root / "target/release/mimik").write_bytes(b"x")
            (root / "target/THIRD-PARTY-NOTICES.md").write_text("x", encoding="utf-8")
            # Every required member is present, but this is cargo package's own output: no
            # patch entry, registry lock.
            with tarfile.open(root / "target/package/mimik-test.crate", "w:gz") as tar:
                for name in sorted(REQUIRED):
                    data = {
                        "Cargo.toml": b'[package]\nname = "mimik"\n',
                        "Cargo.lock": LOCK_REGISTRY.encode(),
                    }.get(name, b"x")
                    info = tarfile.TarInfo(f"mimik-test/{name}")
                    info.size = len(data)
                    tar.addfile(info, io.BytesIO(data))
            with (
                contextlib.chdir(root),
                patch("sys.argv", ["package_release", "--version", "test", "--revision", "t"]),
                self.assertRaisesRegex(ValueError, "does not patch mupdf"),
            ):
                package_release.main()

    def test_package_must_patch_mupdf_to_the_fork(self):
        fork = "pub fn is_rtl(&self) -> bool {}"
        verify_fork(PATCHED, LOCK_PATH, fork)
        for manifest, lock, text in (
            ('[package]\nname = "mimik"\n', LOCK_PATH, fork),  # cargo package drops [patch]
            (PATCHED.replace("vendor/mupdf", "vendor/other"), LOCK_PATH, fork),
            (PATCHED, LOCK_REGISTRY, fork),  # cargo package rewrites the lock to the registry
            (PATCHED, LOCK_PATH, "pub fn char(&self) {}"),  # unmodified upstream source
        ):
            with self.assertRaises(ValueError):
                verify_fork(manifest, lock, text)

    def fork_root(self, root):
        (root / "vendor/mupdf/src").mkdir(parents=True)
        (root / "vendor/mupdf/Cargo.toml").write_text("[package]\n", encoding="utf-8")
        (root / "vendor/mupdf/src/text_page.rs").write_text("fork", encoding="utf-8")
        (root / "Cargo.lock").write_text(LOCK_PATH, encoding="utf-8")
        vendored_fork.main_inventory(root)
        crate = root / "mimik-0.1.0.crate"
        self.write_crate(crate, '[package]\nname = "mimik"\n', LOCK_REGISTRY)
        return crate

    def write_crate(self, crate, manifest, lock, inventory=None):
        with tarfile.open(crate, "w:gz") as tar:
            members = [("Cargo.toml", manifest), ("Cargo.lock", lock)]
            if inventory is not None:
                members.append(("vendor/mupdf.sha256", inventory))
            for name, text in members:
                data = text.encode()
                info = tarfile.TarInfo(f"mimik-0.1.0/{name}")
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))

    def test_fork_tool_restores_what_cargo_package_drops_and_fails_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            crate = self.fork_root(root)
            vendored_fork.augment(crate, root)
            with tarfile.open(crate) as tar:
                names = tar.getnames()
                manifest = tar.extractfile("mimik-0.1.0/Cargo.toml").read().decode()
                lock = tar.extractfile("mimik-0.1.0/Cargo.lock").read().decode()
            self.assertIn("mimik-0.1.0/vendor/mupdf/src/text_page.rs", names)
            self.assertIn("mimik-0.1.0/vendor/mupdf.sha256", names)
            verify_fork(manifest, lock, "pub fn is_rtl")
            with self.assertRaises(ValueError):
                vendored_fork.augment(crate, root)  # not twice
            # A lock that drifted from the repository's is refused.
            self.write_crate(crate, "", LOCK_REGISTRY)
            (root / "Cargo.lock").write_text(LOCK_PATH + "# drift\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                vendored_fork.augment(crate, root)

    def test_cargo_packaged_inventory_is_accepted_only_when_it_is_the_pinned_one(self):
        """cargo package itself ships the plain-file inventory (it is not a nested package)."""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            crate = self.fork_root(root)
            listing = (root / "vendor/mupdf.sha256").read_text(encoding="utf-8")
            self.write_crate(crate, "", LOCK_REGISTRY, listing + "# edited\n")
            with self.assertRaises(ValueError):
                vendored_fork.augment(crate, root)
            self.write_crate(crate, "", LOCK_REGISTRY, listing)
            vendored_fork.augment(crate, root)
            with tarfile.open(crate) as tar:
                self.assertEqual(tar.getnames().count("mimik-0.1.0/vendor/mupdf.sha256"), 1)

    def test_fork_output_is_deterministic(self):
        outputs = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                crate = self.fork_root(root)
                vendored_fork.augment(crate, root)
                outputs.append(crate.read_bytes())
                if len(outputs) == 1:
                    time.sleep(1.1)  # a gzip header timestamp would differ
        self.assertEqual(outputs[0], outputs[1])

    def test_fork_must_match_the_pinned_inventory_exactly(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            crate = self.fork_root(root)
            fork = root / "vendor/mupdf"
            original = (fork / "src/text_page.rs").read_text(encoding="utf-8")
            for name, change, undo in (
                (
                    "extra",
                    lambda: (fork / "Cargo.toml.rej").write_text("x", encoding="utf-8"),
                    lambda: (fork / "Cargo.toml.rej").unlink(),
                ),
                (
                    "missing",
                    lambda: (fork / "src/text_page.rs").unlink(),
                    lambda: (fork / "src/text_page.rs").write_text(original, encoding="utf-8"),
                ),
                (
                    "modified",
                    lambda: (fork / "src/text_page.rs").write_text("fork!", encoding="utf-8"),
                    lambda: (fork / "src/text_page.rs").write_text(original, encoding="utf-8"),
                ),
            ):
                with self.subTest(name):
                    change()
                    with self.assertRaises(ValueError):
                        vendored_fork.augment(crate, root)
                    listing = (root / "vendor/mupdf.sha256").read_text(encoding="utf-8")
                    files = {
                        p.relative_to(fork).as_posix(): p.read_bytes()
                        for p in fork.rglob("*")
                        if p.is_file()
                    }
                    with self.assertRaises(ValueError):
                        verify_fork(PATCHED, LOCK_PATH, "pub fn is_rtl", listing, files)
                    undo()
            vendored_fork.augment(crate, root)

    def test_real_fork_matches_its_pinned_inventory(self):
        project = Path(__file__).resolve().parents[1]
        listing = (project / "vendor/mupdf.sha256").read_text(encoding="utf-8")
        vendored_fork.check_inventory(listing, vendored_fork.tree_files(project))

    def test_audit_lock_restores_mupdf_registry_source_and_checksum(self):
        restored = vendored_fork.registry_lock(LOCK_PATH + "# tail\n")
        self.assertIn(
            'name = "mupdf"\nversion = "0.8.0"\nsource = "registry+https://github.com/'
            f'rust-lang/crates.io-index"\nchecksum = "{vendored_fork.MUPDF_CHECKSUM}"\n',
            restored,
        )
        self.assertEqual(vendored_fork.path_form_lock(restored), LOCK_PATH + "# tail\n")
        for lock in (
            LOCK_REGISTRY,  # already a registry entry
            '[[package]]\nname = "other"\nversion = "1.0.0"\n',
            LOCK_PATH.replace("0.8.0", "0.9.0"),
            LOCK_PATH + LOCK_PATH,
        ):
            with self.assertRaises(ValueError):
                vendored_fork.registry_lock(lock)
        project = Path(__file__).resolve().parents[1]
        real = vendored_fork.registry_lock((project / "Cargo.lock").read_text(encoding="utf-8"))
        self.assertIn(vendored_fork.MUPDF_CHECKSUM, real)

    def test_actual_cargo_include_excludes_private_files_and_nested_cache(self):
        project = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in ["Cargo.toml", "Cargo.lock", "LICENSE", "rust-toolchain.toml", "README.md"]:
                shutil.copy2(project / name, root / name)
            # The manifest patches mupdf to this path, which cargo reads even for --list.
            shutil.copytree(project / "vendor/mupdf", root / "vendor/mupdf")
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
