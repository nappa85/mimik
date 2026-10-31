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

LICENSES = Path(__file__).resolve().parents[1] / "licenses"
IJG_README_SHA256 = "7c25493a9f64fed34d01445467341bda77bc1cdbeccbe33558659ef173fb9ff2"
IJG_INVENTORY_SHA256 = "efcf41c7664ec021ca36790adc5b1a16a6e0f309ba19e293bfa66a32849fa362"
GPL3_PATH = "mupdf/thirdparty/lcms2/plugins/fast_float/COPYING.GPL3"
GPL3_SHA256 = "c53a65c2fd561c87eaabf1072ef5dcab8653042bc15308465f52413585eb6271"
IJG_CHANGED = {"jidctint.c", "jdarith.c", "jerror.c"}


def pinned(path, digest):
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f"pinned licence/provenance hash mismatch: {path}")
    return data


def verify_gpl(crate):
    """The elected GPLv3 text is explicitly present, not just an FTL pointer."""
    pinned(crate / GPL3_PATH, GPL3_SHA256)


def ijg_notice(crate, licenses=LICENSES):
    readme = pinned(licenses / "ijg-libjpeg-9f-README", IJG_README_SHA256).decode("utf-8")
    inventory = pinned(licenses / "ijg-libjpeg-9f-upstream.sha256", IJG_INVENTORY_SHA256)
    upstream = dict(line.split()[::-1] for line in inventory.decode("ascii").splitlines())
    tree = crate / "mupdf/thirdparty/libjpeg"
    found = {p.relative_to(tree).as_posix() for p in tree.rglob("*") if p.is_file()}
    if len(upstream) != 84 or found != upstream.keys():
        raise ValueError("IJG libjpeg source inventory mismatch")
    changed = {
        name
        for name, digest in upstream.items()
        if hashlib.sha256((tree / name).read_bytes()).hexdigest() != digest
    }
    if changed != IJG_CHANGED:
        raise ValueError(f"unreviewed IJG libjpeg changes: {sorted(changed)}")
    return (
        "\n### IJG libjpeg 9f\n\n"
        "this software is based in part on the work of the Independent JPEG Group\n\n"
        "MuPDF changed these files relative to the pinned upstream jpeg-9f release: "
        + ", ".join(sorted(changed))
        + ". The other 81 shipped files match upstream.\n\n"
        "The vendored tree omits upstream documentation (including the README supplied "
        "below) and build/configuration files.\n\n"
        "Upstream: https://www.ijg.org/files/jpegsrc.v9f.tar.gz; SHA-256 "
        "04705c110cb2469caa79fb71fba3d7bf834914706e9641a4589485c1f832565b.\n\n"
        f"licenses/ijg-libjpeg-9f-README; SHA-256 {IJG_README_SHA256}\n\n"
        f"```text\n{readme}\n```\n"
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
    verify_gpl(crate)
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
        "Only FreeType's LICENSE.TXT pointer text ships in the crate: docs/FTL.TXT, "
        "docs/GPLv2.TXT, BDF/PCF READMEs and src/gzip/zlib.h are absent. "
        f"The full elected GPLv3 text is reproduced below from {GPL3_PATH} "
        f"(verified SHA-256 {GPL3_SHA256}).\n\n"
        "Fonts: the han and noto licence files are reproduced, even though disabled. "
        "The compiled URW base14 fonts have no separate licence file in the crate; "
        "they are covered by MuPDF's COPYING (Artifex).\n\n"
        "Crate source gaps: libjpeg has IJG licence source-header notices but omits the "
        "README they refer to; the pinned upstream README is supplied in full below. "
        "Leptonica is vendored (disabled), has BSD-style source-header "
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
        text = (crate / relative).read_bytes().decode("utf-8")
        start = text.find("/*")
        end = text.find("*/", start)
        if start < 0 or end < 0 or "Copyright" not in text[start:end]:
            raise ValueError(f"missing source-header notice: {relative}")
        out.append(
            f"\n### {relative} (source-header notice)\n\n```text\n{text[start : end + 2]}\n```\n"
        )
    out.append(ijg_notice(crate))
    return "".join(out)


def verify(text, crate):
    """Packaging refuses omitted, modified or stale vendored-tree notices."""
    if notice(crate) not in text:
        raise ValueError(
            "third-party notices omit or alter the MuPDF vendored-tree notices; "
            "run python3 tools/mupdf_notices.py target/THIRD-PARTY-NOTICES.md "
            "after generating cargo-about notices"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("notices", type=Path)
    args = parser.parse_args()
    text = args.notices.read_bytes() + notice(registry_crate()).encode("utf-8")
    args.notices.write_bytes(text)


if __name__ == "__main__":
    main()
