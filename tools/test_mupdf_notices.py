import tempfile
import unittest
from pathlib import Path

import mupdf_notices


class MuPdfNoticesTests(unittest.TestCase):
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

    def test_all_font_and_disabled_component_licences_are_in_the_inventory(self):
        for component in ("mujs", "tesseract", "curl", "freeglut", "zint", "zxing-cpp"):
            self.assertTrue(any(f"/{component}/" in path for path in mupdf_notices.LICENSE_FILES))
        self.assertTrue(mupdf_notices.license_name("OFL.txt"))
        self.assertTrue(mupdf_notices.license_name("COPYRIGHT"))


if __name__ == "__main__":
    unittest.main()
