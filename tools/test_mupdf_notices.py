import contextlib
import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import mupdf_notices
import package_release


class MuPdfNoticesTests(unittest.TestCase):
    def test_crlf_licence_survives_generation_and_release_verification(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            crate = root / "target/vendor/mupdf-sys"
            crate.mkdir(parents=True)
            self.fixture(crate)
            licence = b"Copyright synthetic\r\nPermission granted\r\n"
            (crate / "mupdf/thirdparty/lcms2/utils/jpgicc/LICENSE_iccjpeg").write_bytes(licence)
            notices = root / "target/THIRD-PARTY-NOTICES.md"
            notices.write_bytes(b"cargo-about\r\n")
            with (
                patch.object(mupdf_notices, "registry_crate", return_value=crate),
                patch("sys.argv", ["mupdf_notices", str(notices)]),
            ):
                mupdf_notices.main()
            self.assertIn(licence, notices.read_bytes())
            self.assertTrue(notices.read_bytes().startswith(b"cargo-about\r\n"))
            (root / "target/package").mkdir()
            (root / "target/package/mimik-test.crate").write_bytes(b"synthetic")
            (root / "target/release").mkdir()
            (root / "target/release/mimik").write_bytes(b"synthetic")
            (root / "target/vendor-config.toml").write_text(
                '[source.vendored-sources]\ndirectory = "target/vendor"\n'
            )
            with (
                contextlib.chdir(root),
                patch("sys.argv", ["package_release", "--version", "test", "--revision", "test"]),
                patch.object(
                    package_release, "stage_llama", side_effect=RuntimeError("staging reached")
                ),
                self.assertRaisesRegex(RuntimeError, "staging reached"),
            ):
                package_release.main()

    def setUp(self):
        self.real_ijg = mupdf_notices.ijg_notice
        self.real_gpl = mupdf_notices.verify_gpl
        self.external = patch.object(
            mupdf_notices, "ijg_notice", return_value="IJG fixture\n", create=True
        )
        self.external.start()
        self.addCleanup(self.external.stop)
        self.gpl = patch.object(mupdf_notices, "verify_gpl", create=True)
        self.gpl.start()
        self.addCleanup(self.gpl.stop)

    def test_vendor_metadata_does_not_change_notices_and_missing_section_is_actionable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.fixture(root)
            (root / ".cargo-ok").write_text("")
            text = mupdf_notices.notice(root)
            (root / ".cargo-ok").unlink()
            (root / ".cargo-checksum.json").write_text("{}")
            mupdf_notices.verify(text, root)
            with self.assertRaisesRegex(ValueError, "tools/mupdf_notices.py"):
                mupdf_notices.verify("cargo-about only", root)

    def fixture(self, root):
        for relative in mupdf_notices.LICENSE_FILES:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"Synthetic licence for {relative}\n", encoding="utf-8")
        for relative in (
            "mupdf/thirdparty/libjpeg/jpeglib.h",
            "mupdf/thirdparty/leptonica/src/allheaders.h",
        ):
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("/* Copyright synthetic licence */\nint code;\n", encoding="utf-8")

    def test_every_shipped_licence_is_included_and_packaging_checks_full_text(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.fixture(root)
            text = mupdf_notices.notice(root)
            for relative in mupdf_notices.LICENSE_FILES:
                self.assertIn((root / relative).read_text(), text)
                self.assertIn(f"### {relative}\n", text)
            self.assertIn("GPLv2-or-later", text)
            self.assertIn("used here as GPLv3", text)
            self.assertIn("URW base14", text)
            mupdf_notices.verify("cargo-about output\n" + text, root)
            for relative in mupdf_notices.LICENSE_FILES:
                with self.subTest(relative=relative), self.assertRaises(ValueError):
                    mupdf_notices.verify(text.replace(f"### {relative}\n", ""), root)

    def test_new_or_missing_license_fails_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.fixture(root)
            extra = root / "mupdf/thirdparty/NEW-LICENCE"
            extra.write_text("new licence")
            with self.assertRaisesRegex(ValueError, "unreviewed"):
                mupdf_notices.notice(root)
            extra.unlink()
            (root / "mupdf/COPYING").unlink()
            with self.assertRaisesRegex(ValueError, "missing"):
                mupdf_notices.notice(root)

    def test_pinned_readme_and_source_comparison_fail_closed(self):
        # Synthetic upstream files exercise every comparison, including a fourth change.
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            tree = root / "mupdf/thirdparty/libjpeg"
            tree.mkdir(parents=True)
            names = sorted(mupdf_notices.IJG_CHANGED) + [f"file{i}.c" for i in range(81)]
            digest = hashlib.sha256(b"upstream").hexdigest()
            inventory = "".join(f"{digest}  {name}\n" for name in names).encode()
            for name in names:
                (tree / name).write_bytes(
                    b"changed" if name in mupdf_notices.IJG_CHANGED else b"upstream"
                )
            real_ijg = self.real_ijg
            with patch.object(mupdf_notices, "pinned", side_effect=[b"full README", inventory]):
                text = real_ijg(root)
            self.assertIn("full README", text)
            self.assertIn(
                "this software is based in part on the work of the Independent JPEG Group", text
            )
            for name in mupdf_notices.IJG_CHANGED:
                self.assertIn(name, text)
            (tree / "file0.c").write_bytes(b"unexpected")
            with (
                patch.object(mupdf_notices, "pinned", side_effect=[b"full README", inventory]),
                self.assertRaisesRegex(ValueError, "unreviewed"),
            ):
                real_ijg(root)
            readme = root / "README"
            readme.write_bytes(b"altered")
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                mupdf_notices.pinned(readme, mupdf_notices.IJG_README_SHA256)
            readme.unlink()
            with self.assertRaises(FileNotFoundError):
                mupdf_notices.pinned(readme, mupdf_notices.IJG_README_SHA256)

    def test_gpl_election_requires_exact_full_gpl3_text(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / mupdf_notices.GPL3_PATH
            with self.assertRaises(FileNotFoundError):
                self.real_gpl(root)
            path.parent.mkdir(parents=True)
            path.write_text("AGPL or pointer text is not GPLv3")
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                self.real_gpl(root)

    def test_all_font_and_disabled_component_licences_are_in_the_inventory(self):
        for component in ("mujs", "tesseract", "curl", "freeglut", "zint", "zxing-cpp"):
            self.assertTrue(any(f"/{component}/" in path for path in mupdf_notices.LICENSE_FILES))
        self.assertTrue(mupdf_notices.license_name("OFL.txt"))
        self.assertTrue(mupdf_notices.license_name("COPYRIGHT"))


if __name__ == "__main__":
    unittest.main()
