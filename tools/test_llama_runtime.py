import hashlib
import io
import os
import re
import stat
import tarfile
import tempfile
import unittest
from pathlib import Path

import llama_runtime
import package_release


def digest(data):
    return hashlib.sha256(data).hexdigest()


SERVER = b"\x7fELF synthetic server"
LIBRARY = b"\x7fELF synthetic library"
LICENSE = b"MIT"


def synthetic():
    """A manifest of the real one's shape over synthetic files."""
    return {
        "manifest": "mimik-llama-runtime-v1",
        "llama_cpp": {
            "tag": "b1",
            "build": 1,
            "commit": "0" * 40,
            "repository": "https://example.invalid/llama.cpp",
        },
        "source": {"name": "llama.cpp-b1-source.tar", "prefix": "llama.cpp-b1/", "tar_sha256": ""},
        "builds": [
            {
                "path": "cpu",
                "directory": "cpu",
                "server": "llama-server",
                "files": [
                    {
                        "name": "llama-server",
                        "from": "llama-b1/llama-server",
                        "sha256": digest(SERVER),
                        "size": len(SERVER),
                        "mode": "0755",
                    },
                    {
                        "name": "libllama.so.0",
                        "from": "llama-b1/libllama.so.0.6.0",
                        "sha256": digest(LIBRARY),
                        "size": len(LIBRARY),
                        "mode": "0755",
                    },
                    {
                        "name": "LICENSE",
                        "from": "llama-b1/LICENSE",
                        "sha256": digest(LICENSE),
                        "size": len(LICENSE),
                        "mode": "0644",
                    },
                ],
            }
        ],
        "test_model": {"name": "m.gguf", "sha256": digest(b"GGUF"), "size": 4},
    }


def tree(root):
    cpu = Path(root) / "llama" / "cpu"
    cpu.mkdir(parents=True)
    for name, content in [
        ("llama-server", SERVER),
        ("libllama.so.0", LIBRARY),
        ("LICENSE", LICENSE),
    ]:
        (cpu / name).write_bytes(content)
        os.chmod(cpu / name, 0o600)
    return Path(root) / "llama"


def asset(path, members):
    with tarfile.open(path, "w:gz") as tar:
        for name, content, kind in members:
            info = tarfile.TarInfo(name)
            if kind == "link":
                info.type = tarfile.SYMTYPE
                info.linkname = "elsewhere"
                tar.addfile(info)
            else:
                info.size = len(content)
                tar.addfile(info, io.BytesIO(content))


class LlamaRuntimeTests(unittest.TestCase):
    def test_the_real_manifest_names_every_file_once_with_a_mode(self):
        data = llama_runtime.manifest()
        for build in data["builds"]:
            names = [entry["name"] for entry in build["files"]]
            self.assertEqual(len(names), len(set(names)))
            self.assertIn(build["server"], names)
            for entry in build["files"]:
                self.assertEqual(int(entry["mode"], 8) & 0o022, 0)
        self.assertEqual(len(data["source"]["tar_sha256"]), 64)

    def test_the_real_manifest_pins_both_ci_models_to_a_commit_with_size_and_hash(self):
        data = llama_runtime.manifest()
        models = llama_runtime.pinned_models(data)
        self.assertEqual(
            [model["name"] for model in models],
            ["stories260K.gguf", "SmolLM2-135M-Instruct-Q4_K_M.gguf"],
        )
        for model in models:
            self.assertRegex(model["revision"], r"^[0-9a-f]{40}$")
            self.assertIn(f"/resolve/{model['revision']}/", model["url"])
            self.assertRegex(model["sha256"], r"^[0-9a-f]{64}$")
            self.assertGreater(model["size"], 0)
            self.assertTrue(model["licence"])

    def test_verify_model_checks_every_pinned_model_and_tolerates_none_extra(self):
        data = synthetic()
        data["smoke_model"] = {"name": "s.gguf", "sha256": digest(b"SMOKE"), "size": 5}
        with tempfile.TemporaryDirectory() as temp:
            models = Path(temp) / "models"
            models.mkdir()
            (models / "m.gguf").write_bytes(b"GGUF")
            with self.assertRaises(OSError):
                llama_runtime.verify_model(models, data)
            (models / "s.gguf").write_bytes(b"SMOKE")
            llama_runtime.verify_model(models, data)
            (models / "s.gguf").write_bytes(b"OTHER")
            with self.assertRaises(ValueError):
                llama_runtime.verify_model(models, data)
            # A manifest with only the confinement model still verifies.
            llama_runtime.verify_model(models, synthetic())

    def test_verify_reapplies_modes_and_refuses_extra_missing_changed_or_linked_files(self):
        data = synthetic()
        with tempfile.TemporaryDirectory() as temp:
            llama = tree(temp)
            llama_runtime.verify(llama, data)
            self.assertEqual(stat.S_IMODE((llama / "cpu/llama-server").stat().st_mode), 0o755)
            self.assertEqual(stat.S_IMODE((llama / "cpu/LICENSE").stat().st_mode), 0o644)
            (llama / "cpu/extra.so").write_bytes(b"x")
            with self.assertRaises(ValueError):
                llama_runtime.verify(llama, data)
            (llama / "cpu/extra.so").unlink()
            (llama / "cpu/LICENSE").write_bytes(b"BSD")
            with self.assertRaises(ValueError):
                llama_runtime.verify(llama, data)
            (llama / "cpu/LICENSE").unlink()
            with self.assertRaises(ValueError):
                llama_runtime.verify(llama, data)
            (llama / "cpu/LICENSE").symlink_to(llama / "cpu/llama-server")
            with self.assertRaises(ValueError):
                llama_runtime.verify(llama, data)
            (llama / "cpu/LICENSE").unlink()
            (llama / "cpu/LICENSE").write_bytes(LICENSE)
            (llama / "vulkan").mkdir()
            with self.assertRaises(ValueError):
                llama_runtime.verify(llama, data)

    def test_extract_writes_shipped_names_and_refuses_links_and_missing_members(self):
        data = synthetic()
        build = data["builds"][0]
        with tempfile.TemporaryDirectory() as temp:
            good = Path(temp) / "good.tar.gz"
            asset(
                good,
                [
                    ("llama-b1/llama-server", SERVER, "file"),
                    ("llama-b1/libllama.so.0.6.0", LIBRARY, "file"),
                    ("llama-b1/libllama.so.0", b"", "link"),
                    ("llama-b1/LICENSE", LICENSE, "file"),
                    ("llama-b1/llama-cli", b"not shipped", "file"),
                ],
            )
            llama_runtime.extract(good, build, Path(temp) / "out/llama/cpu")
            llama_runtime.verify(Path(temp) / "out/llama", data)
            linked = Path(temp) / "linked.tar.gz"
            asset(
                linked,
                [
                    ("llama-b1/llama-server", b"", "link"),
                    ("llama-b1/libllama.so.0.6.0", LIBRARY, "file"),
                    ("llama-b1/LICENSE", LICENSE, "file"),
                ],
            )
            with self.assertRaises(ValueError):
                llama_runtime.extract(linked, build, Path(temp) / "linked/llama/cpu")
            short = Path(temp) / "short.tar.gz"
            asset(short, [("llama-b1/llama-server", SERVER, "file")])
            with self.assertRaises(ValueError):
                llama_runtime.extract(short, build, Path(temp) / "short/llama/cpu")

    def test_downloads_are_refused_unless_size_and_hash_match(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "asset"
            source.write_bytes(b"pinned bytes")
            url = source.as_uri()
            llama_runtime.download(url, Path(temp) / "ok", digest(b"pinned bytes"), 12)
            for expected, size in [(digest(b"other"), 12), (digest(b"pinned bytes"), 11)]:
                with self.assertRaises(ValueError):
                    llama_runtime.download(url, Path(temp) / "bad", expected, size)

    def test_packaging_stages_exactly_the_verified_files(self):
        data = synthetic()
        with tempfile.TemporaryDirectory() as temp:
            fetched = Path(temp) / "fetched"
            tree(fetched)
            tar = fetched / data["source"]["name"]
            tar.write_bytes(b"synthetic source tar")
            data["source"]["tar_sha256"] = digest(b"synthetic source tar")
            stage = Path(temp) / "stage"
            stage.mkdir()
            package_release.stage_llama(fetched, stage, data)
            self.assertEqual(
                sorted(p.name for p in (stage / "llama/cpu").iterdir()),
                ["LICENSE", "libllama.so.0", "llama-server"],
            )
            self.assertEqual(stat.S_IMODE((stage / "llama/cpu/llama-server").stat().st_mode), 0o755)
            self.assertIn("ggml", package_release.llama_notice(data))
            self.assertIn("notices", llama_runtime.manifest()["source"])
            (fetched / "llama/cpu/extra").write_bytes(b"x")
            with self.assertRaises(ValueError):
                package_release.stage_llama(fetched, Path(temp) / "stage2", data)


class GpuBuildTests(unittest.TestCase):
    def test_the_vulkan_build_is_the_cpu_build_plus_the_vulkan_backend(self):
        data = llama_runtime.manifest()
        builds = {build["path"]: build for build in data["builds"]}
        self.assertEqual(sorted(builds), ["cpu", "vulkan"])
        cpu, vulkan = builds["cpu"], builds["vulkan"]
        self.assertEqual(vulkan["directory"], "vulkan")
        self.assertEqual(vulkan["asset"], "llama-b12110-bin-ubuntu-vulkan-x64.tar.gz")
        self.assertEqual(
            vulkan["sha256"], "a5d741940956ec62e8783aee7e5a4c7c761af867d6f027c3277a51e1a3ac59b0"
        )
        shared = {entry["name"]: entry for entry in cpu["files"]}
        extra = [entry for entry in vulkan["files"] if entry["name"] not in shared]
        self.assertEqual([entry["name"] for entry in extra], ["libggml-vulkan.so"])
        for entry in vulkan["files"]:
            if entry["name"] in shared:
                self.assertEqual(entry, shared[entry["name"]])
        self.assertIn("libvulkan.so.1", vulkan["system_libraries"])
        names = [notice["name"] for notice in data["external_notices"]]
        self.assertIn("Vulkan-Headers-MIT.txt", names)


class ExternalNoticeTests(unittest.TestCase):
    TEXT = b"MIT License\n\nCopyright (c) Example\n"

    def data(self, url):
        data = synthetic()
        data["external_notices"] = [
            {
                "component": "Headers",
                "licence": "MIT",
                "name": "Headers-MIT.txt",
                "url": url,
                "sha256": digest(self.TEXT),
                "size": len(self.TEXT),
            }
        ]
        return data

    def test_fetched_notices_are_hash_checked_and_exactly_the_pinned_set(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "upstream.txt"
            source.write_bytes(self.TEXT)
            data = self.data(source.as_uri())
            out = Path(temp) / "out"
            out.mkdir()
            llama_runtime.fetch_notices(out, data)
            llama_runtime.verify_notices(out / "notices", data)
            (out / "notices/extra.txt").write_bytes(b"x")
            with self.assertRaises(ValueError):
                llama_runtime.verify_notices(out / "notices", data)
            (out / "notices/extra.txt").unlink()
            (out / "notices/Headers-MIT.txt").write_bytes(b"changed")
            with self.assertRaises(ValueError):
                llama_runtime.verify_notices(out / "notices", data)
            source.write_bytes(b"another text")
            with self.assertRaises(ValueError):
                llama_runtime.fetch_notices(Path(temp) / "again", data)

    def test_the_package_notice_carries_each_external_text(self):
        with tempfile.TemporaryDirectory() as temp:
            data = self.data("file:///unused")
            notices = Path(temp) / "notices"
            notices.mkdir()
            (notices / "Headers-MIT.txt").write_bytes(self.TEXT)
            text = package_release.llama_notice(data, None, notices)
            self.assertIn("### Headers", text)
            self.assertIn("Copyright (c) Example", text)
            # The provenance claim is exact: the files are the release's binaries, and what
            # the source archive does not hold is named as coming from its build environment.
            self.assertNotIn("is built from it", text)
            self.assertIn("release's own binaries", text)
            self.assertIn("build environment", text)
            (notices / "Headers-MIT.txt").write_bytes(b"changed")
            with self.assertRaises(ValueError):
                package_release.llama_notice(data, None, notices)


class NoticeTests(unittest.TestCase):
    FILES = [
        ("LICENSE", b"MIT text"),
        ("h.h", b"code\nlicence line one\nlicence line two\nmore code\n"),
    ]

    def write_tar(self, path, data, files):
        with tarfile.open(path, "w") as out:
            for name, content in files:
                info = tarfile.TarInfo(data["source"]["prefix"] + name)
                info.size = len(content)
                out.addfile(info, io.BytesIO(content))

    def pinned(self, path, data):
        data["source"]["tar_sha256"] = llama_runtime.sha256(path)
        data["source"]["tar_size"] = Path(path).stat().st_size

    def manifest(self):
        data = synthetic()
        data["source"]["notices"] = [
            {"component": "llama.cpp", "licence": "MIT", "path": "LICENSE"},
            {"component": "header", "licence": "Unlicense", "path": "h.h", "lines": [2, 3]},
        ]
        return data

    def test_every_compiled_in_component_gets_its_text_from_the_source(self):
        data = self.manifest()
        with tempfile.TemporaryDirectory() as temp:
            tar = Path(temp) / "source.tar"
            self.write_tar(tar, data, self.FILES)
            self.pinned(tar, data)
            notice = package_release.llama_notice(data, tar)
            self.assertIn("MIT text", notice)
            self.assertIn("licence line one\nlicence line two", notice)
            self.assertNotIn("more code", notice)
            data["source"]["notices"].append(
                {"component": "absent", "licence": "MIT", "path": "missing"}
            )
            with self.assertRaises(KeyError):
                package_release.llama_notice(data, tar)

    def test_the_source_tar_is_verified_before_any_text_is_taken_from_it(self):
        data = self.manifest()
        with tempfile.TemporaryDirectory() as temp:
            tar = Path(temp) / "source.tar"
            self.write_tar(tar, data, self.FILES)
            self.pinned(tar, data)
            # A licence text replaced inside an otherwise valid tar: of another length, then of
            # the same length (the size check alone cannot catch it, the hash does).
            for forged_text in (b"Forged text", b"MIT TEXT"):
                self.write_tar(tar, data, [("LICENSE", forged_text), self.FILES[1]])
                if len(forged_text) == len(b"MIT text"):
                    self.assertEqual(tar.stat().st_size, data["source"]["tar_size"])
                with self.assertRaisesRegex(ValueError, "differs from the pinned"):
                    package_release.llama_notice(data, tar)
            # The pinned size is checked too.
            data["source"]["tar_sha256"] = llama_runtime.sha256(tar)
            data["source"]["tar_size"] = tar.stat().st_size + 1
            with self.assertRaisesRegex(ValueError, "differs from the pinned"):
                package_release.llama_notice(data, tar)


class InventoryTests(unittest.TestCase):
    NAMES = [
        "mimik-0.1.0-linux-x86_64.tar.gz",
        "mimik-0.1.0-source.crate",
        "mimik-0.1.0-dependency-sources.tar.gz",
    ]

    def test_the_release_scope_lists_every_archive_and_their_count(self):
        readme = Path(__file__).resolve().parents[1] / "docs" / "release-scope.md"
        package_release.check_inventory(readme.read_text(encoding="utf-8"), self.NAMES)

    def test_a_missing_archive_or_a_wrong_count_is_refused(self):
        listed = "".join(f"- `{name}`: x\n" for name in self.NAMES[:2])
        with self.assertRaisesRegex(ValueError, "dependency-sources"):
            package_release.check_inventory(
                listed + "- `SHA256SUMS`: checksums of the three archives.\n", self.NAMES
            )
        listed += f"- `{self.NAMES[2]}`: x\n"
        with self.assertRaisesRegex(ValueError, "three"):
            package_release.check_inventory(
                listed + "- `SHA256SUMS`: checksums of the four archives.\n", self.NAMES
            )
        package_release.check_inventory(
            listed + "- `SHA256SUMS`: checksums of the three archives.\n", self.NAMES
        )

    def test_the_release_scope_no_longer_lists_the_llama_source_archive(self):
        readme = Path(__file__).resolve().parents[1] / "docs" / "release-scope.md"
        self.assertIsNone(re.search(r"llama\.cpp-b\d+-source", readme.read_text(encoding="utf-8")))

    def test_ci_expects_three_archives_and_uploads_no_llama_source(self):
        ci = (Path(__file__).resolve().parents[1] / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("-eq 3", ci)
        self.assertNotIn("-source.tar.gz", ci)

    def test_llama_cpp_is_pointed_to_never_shipped_as_source(self):
        self.assertFalse(hasattr(package_release, "compress_source"))
        data = llama_runtime.manifest()
        text = package_release.llama_pointer(data)
        self.assertIn(data["llama_cpp"]["repository"], text)
        self.assertIn(data["llama_cpp"]["tag"], text)
        self.assertIn(data["llama_cpp"]["commit"], text)
        for build in data["builds"]:
            self.assertIn(build["asset"], text)
            self.assertIn(build["sha256"], text)
        self.assertNotIn("accompanying", text)
        self.assertIn("not shipped", text)


if __name__ == "__main__":
    unittest.main()
