import hashlib
import io
import os
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

    def test_packaging_stages_exactly_the_verified_files_and_a_deterministic_source(self):
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
            out = Path(temp) / "out"
            out.mkdir()
            first = package_release.compress_source(fetched, data, out).read_bytes()
            second = package_release.compress_source(fetched, data, out).read_bytes()
            self.assertEqual(first, second)
            self.assertIn("ggml", package_release.llama_notice(data))
            self.assertIn("notices", llama_runtime.manifest()["source"])
            (fetched / "llama/cpu/extra").write_bytes(b"x")
            with self.assertRaises(ValueError):
                package_release.stage_llama(fetched, Path(temp) / "stage2", data)
            tar.write_bytes(b"another tar")
            with self.assertRaises(ValueError):
                package_release.compress_source(fetched, data, out)


class NoticeTests(unittest.TestCase):
    def test_every_compiled_in_component_gets_its_text_from_the_source(self):
        data = synthetic()
        data["source"]["notices"] = [
            {"component": "llama.cpp", "licence": "MIT", "path": "LICENSE"},
            {"component": "header", "licence": "Unlicense", "path": "h.h", "lines": [2, 3]},
        ]
        with tempfile.TemporaryDirectory() as temp:
            tar = Path(temp) / "source.tar"
            with tarfile.open(tar, "w") as out:
                for name, content in [
                    ("LICENSE", b"MIT text"),
                    ("h.h", b"code\nlicence line one\nlicence line two\nmore code\n"),
                ]:
                    info = tarfile.TarInfo(data["source"]["prefix"] + name)
                    info.size = len(content)
                    out.addfile(info, io.BytesIO(content))
            notice = package_release.llama_notice(data, tar)
            self.assertIn("MIT text", notice)
            self.assertIn("licence line one\nlicence line two", notice)
            self.assertNotIn("more code", notice)
            data["source"]["notices"].append(
                {"component": "absent", "licence": "MIT", "path": "missing"}
            )
            with self.assertRaises(KeyError):
                package_release.llama_notice(data, tar)


if __name__ == "__main__":
    unittest.main()


class InventoryTests(unittest.TestCase):
    NAMES = [
        "mimik-0.1.0-linux-x86_64.tar.gz",
        "mimik-0.1.0-source.crate",
        "mimik-0.1.0-dependency-sources.tar.gz",
        "llama.cpp-b11609-source.tar.gz",
    ]

    def test_the_release_scope_lists_every_archive_and_their_count(self):
        readme = Path(__file__).resolve().parents[1] / "docs" / "release-scope.md"
        package_release.check_inventory(readme.read_text(encoding="utf-8"), self.NAMES)

    def test_a_missing_archive_or_a_wrong_count_is_refused(self):
        listed = "".join(f"- `{name}`: x\n" for name in self.NAMES[:3])
        with self.assertRaisesRegex(ValueError, "llama.cpp-b11609-source.tar.gz"):
            package_release.check_inventory(
                listed + "- `SHA256SUMS`: checksums of the four archives.\n", self.NAMES
            )
        listed += f"- `{self.NAMES[3]}`: x\n"
        with self.assertRaisesRegex(ValueError, "four"):
            package_release.check_inventory(
                listed + "- `SHA256SUMS`: checksums of the three archives.\n", self.NAMES
            )
        package_release.check_inventory(
            listed + "- `SHA256SUMS`: checksums of the four archives.\n", self.NAMES
        )
