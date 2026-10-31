"""Complete MuPDF vendored-tree notices from the pinned crate, never copied licence assets."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

# Exact inventory of the 0.8.0 crate, including disabled code and non-base14 fonts.
# Discovery below fails closed if an update adds/removes a licence document.
LICENSE_FILES = frozenset(
    """mupdf/COPYING
mupdf/resources/fonts/han/LICENSE.txt
mupdf/resources/fonts/noto/COPYING
mupdf/thirdparty/brotli/LICENSE
mupdf/thirdparty/curl/COPYING
mupdf/thirdparty/curl/docs/LICENSE-MIXING.md
mupdf/thirdparty/curl/tests/python_dependencies/impacket/LICENSE
mupdf/thirdparty/extract/COPYING
mupdf/thirdparty/freeglut/COPYING
mupdf/thirdparty/freetype/LICENSE.TXT
mupdf/thirdparty/freetype/subprojects/dlg/LICENSE
mupdf/thirdparty/gumbo-parser/COPYING
mupdf/thirdparty/harfbuzz/COPYING
mupdf/thirdparty/harfbuzz/src/ms-use/COPYING
mupdf/thirdparty/jbig2dec/COPYING
mupdf/thirdparty/jbig2dec/LICENSE
mupdf/thirdparty/lcms2/LICENSE
mupdf/thirdparty/lcms2/plugins/fast_float/COPYING.GPL3
mupdf/thirdparty/lcms2/plugins/threaded/COPYING.GPL3
mupdf/thirdparty/lcms2/utils/jpgicc/LICENSE_iccjpeg
mupdf/thirdparty/mujs/COPYING
mupdf/thirdparty/openjpeg/LICENSE
mupdf/thirdparty/tesseract/LICENSE
mupdf/thirdparty/zint/backend_qt/COPYING
mupdf/thirdparty/zint/frontend/COPYING
mupdf/thirdparty/zint/frontend_qt/COPYING
mupdf/thirdparty/zint/frontend_qt/res/LICENSE_feathericons
mupdf/thirdparty/zint/frontend_qt/res/LICENSE_lucide
mupdf/thirdparty/zint/LICENSE
mupdf/thirdparty/zlib/contrib/dotzlib/LICENSE_1_0.txt
mupdf/thirdparty/zlib/LICENSE
mupdf/thirdparty/zxing-cpp/LICENSE""".splitlines()
)


def license_name(name):
    """Upstream uses both licence spellings; include notices and font OFLs too."""
    lowered = name.lower()
    return (
        "license" in lowered
        or "licence" in lowered
        or lowered.startswith(("copying", "copyright", "ofl", "notice"))
    )


def registry_crate():
    """Cargo resolves the pinned crate, including when CARGO_HOME differs in CI."""
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--offline", "--locked", "--format-version", "1"],
            text=True,
        )
    )
    matches = [p for p in metadata["packages"] if p["name"] == "mupdf-sys"]
    if len(matches) != 1 or matches[0]["version"] != "0.8.0":
        raise ValueError("review the MuPDF vendored licence inventory for this crate version")
    return Path(matches[0]["manifest_path"]).parent


def notice(crate):
    """Every shipped licence, byte-content preserved (decoded to text), compiled or not."""
    crate = Path(crate)
    found = {
        p.relative_to(crate).as_posix()
        for p in crate.rglob("*")
        if p.is_file() and license_name(p.name)
    }
    if found != LICENSE_FILES:
        raise ValueError(
            f"MuPDF licence inventory mismatch: missing={sorted(LICENSE_FILES - found)}, "
            f"unreviewed={sorted(found - LICENSE_FILES)}"
        )
    out = [
        "\n## MuPDF 1.27.2 vendored tree (mupdf-sys 0.8.0)\n\n"
        "All licence documents shipped in the crate follow, including disabled components. "
        "MuPDF, extract and jbig2dec: AGPL-3.0. HarfBuzz, lcms2, brotli, mujs, curl: MIT-style; "
        "OpenJPEG: BSD-2-Clause; gumbo and tesseract: Apache-2.0; zlib: Zlib. "
        "zint, zxing-cpp and freeglut have their respective licence documents below.\n\n"
        "FreeType: Mimik elects GPLv2-or-later, as LICENSE.TXT states that "
        '"any later version can be used also"; used here as GPLv3, compatible with AGPL-3.0. '
        "The FTL alternative is reproduced as shipped but is not Mimik's selected option.\n\n"
        "Fonts: the han and noto licence files are reproduced, even though disabled. "
        "The compiled URW base14 fonts have no separate licence file in the crate; "
        "they are covered by MuPDF's COPYING (Artifex).\n\n"
        "Crate source gaps: libjpeg has IJG licence source-header notices but omits the "
        "README they refer to. Leptonica is vendored (disabled), has BSD-style source-header "
        "notices, and ships no separate licence document. These header notices follow too.\n"
    ]
    for relative in sorted(found):
        data = (crate / relative).read_bytes()
        text = data.decode("utf-8")
        if not text.strip():
            raise ValueError(f"empty MuPDF licence: {relative}")
        out.append(
            f"\n### {relative}\n\nSHA-256: {hashlib.sha256(data).hexdigest()}\n\n"
            f"```text\n{text}\n```\n"
        )
    # The two trees with missing standalone licences still ship their copyright/licence
    # in C headers. Include the initial comment verbatim, fail if it is absent or changed
    # to a format this extractor cannot preserve.
    for relative in (
        "mupdf/thirdparty/libjpeg/jpeglib.h",
        "mupdf/thirdparty/leptonica/src/allheaders.h",
    ):
        text = (crate / relative).read_text(encoding="utf-8")
        start = text.find("/*")
        end = text.find("*/", start)
        if start < 0 or end < 0 or "Copyright" not in text[start:end]:
            raise ValueError(f"missing source-header notice: {relative}")
        out.append(
            f"\n### {relative} (source-header notice)\n\n```text\n{text[start : end + 2]}\n```\n"
        )
    return "".join(out)


def verify(text, crate):
    """Packaging refuses omitted, modified or stale vendored-tree notices."""
    if notice(crate) not in text:
        raise ValueError("third-party notices omit or alter the MuPDF vendored-tree notices")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("notices", type=Path)
    args = parser.parse_args()
    text = args.notices.read_text(encoding="utf-8") + notice(registry_crate())
    args.notices.write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
