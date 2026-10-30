"""Optional prototype: cross-check an OCR extraction against the PDF's own text layer.

For each unit of a prepared review bundle (`prepare_review.py`'s output, the units Mimik
imports), the text-layer words whose centre lies inside the
unit's box are collected (a shadow duplicate, the same word drawn again within 3 points, is
dropped) and aligned with the unit's OCR words. Two kinds of suggestion result:

- spelling: an OCR word with a near-identical text-layer twin (similarity at least 0.6, not
  equal, same digits and same non-letter characters apart from apostrophes and dashes) takes
  the twin's spelling; the suggested correction is the unit's text with every such word
  replaced, in the OCR's word order.
- difference: a near twin that differs in digits, dice, signs or markers (100/10, d6/d8,
  armatura*/armatura): either reading may be the misprint, so no correction is suggested.
- unread: three or more text-layer words inside the box that the OCR text does not contain,
  a sign of skipped lines (or of overlapping boxes); no correction is suggested.

The report has the shape of `review_extraction.py`'s and is a suggestion only: nothing is
applied, and no review state changes. Runs in the extraction container (pdftext).

usage: crosscheck_text_layer.py BUNDLE --pdf FILE
"""

import argparse
import collections
import difflib
import html
import json
import re
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

from source_bundle import digest, write_json

SIMILAR = 0.6
SHADOW_POINTS = 3
UNREAD_MIN = 3


def page_words(page):
    """(word, box) pairs of one pdftext page, shadow duplicates dropped."""
    words = []
    for block in page["blocks"]:
        for line in block["lines"]:
            for span in line["spans"]:
                current, box = "", None
                for char in span.get("chars", []):
                    c, b = char["char"], char["bbox"]
                    if c.isspace():
                        if current:
                            words.append((current, box))
                        current, box = "", None
                        continue
                    current += c
                    box = (
                        list(b)
                        if box is None
                        else [
                            min(box[0], b[0]),
                            min(box[1], b[1]),
                            max(box[2], b[2]),
                            max(box[3], b[3]),
                        ]
                    )
                if current:
                    words.append((current, box))
    kept = []
    for text, box in words:
        # A shadow repeats the word with the same box shifted by under 3 points.
        shadow = any(
            seen == text
            and all(abs(a - b) < SHADOW_POINTS for a, b in zip(other, box, strict=True))
            for seen, other in kept
        )
        if not shadow:
            kept.append((text, box))
    return kept


def inside(box, rect):
    """Whether a word box's centre lies in a unit's bounding rectangle (1-point margin)."""
    cx, cy = (box[0] + box[2]) / 2, (box[1] + box[3]) / 2
    return rect[0] - 1 <= cx <= rect[2] + 1 and rect[1] - 1 <= cy <= rect[3] + 1


def plain(fragment):
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", fragment))).strip()


def bounding(polygon):
    xs, ys = [point[0] for point in polygon], [point[1] for point in polygon]
    return [min(xs), min(ys), max(xs), max(ys)]


TYPOGRAPHIC = str.maketrans({"’": "'", "‘": "'", "–": "-", "—": "-", "−": "-"})


def significant(word):
    """Digits and non-letter characters, typographic apostrophes and dashes folded."""
    return [c for c in word.translate(TYPOGRAPHIC) if not c.isalpha()]


def bare(word):
    return word.lower().strip(".,;:()*")


def compare(ocr_words, layer_words):
    """(OCR words with text-layer spellings, [[ocr, layer], ...], unread layer words,
    [[ocr, layer], ...] differences in digits, dice, signs or markers)."""
    out, repairs, unread, differences = [], [], [], []
    # OCR words the alignment left unpaired: they may still be read, in another order.
    spare = collections.Counter()
    matcher = difflib.SequenceMatcher(None, ocr_words, layer_words, autojunk=False)
    for op, a0, a1, b0, b1 in matcher.get_opcodes():
        if op == "equal":
            out += ocr_words[a0:a1]
        elif op == "replace" and a1 - a0 == b1 - b0:
            for ocr, layer in zip(ocr_words[a0:a1], layer_words[b0:b1], strict=True):
                out.append(ocr)
                if difflib.SequenceMatcher(None, ocr.lower(), layer.lower()).ratio() < SIMILAR:
                    spare[bare(ocr)] += 1
                    unread.append(layer)
                elif significant(ocr) != significant(layer):
                    differences.append([ocr, layer])
                else:
                    out[-1] = layer
                    repairs.append([ocr, layer])
        else:
            out += ocr_words[a0:a1]
            spare.update(bare(word) for word in ocr_words[a0:a1])
            unread += layer_words[b0:b1]
    missing = []
    for word in unread:
        if spare[bare(word)] > 0:
            spare[bare(word)] -= 1
        else:
            missing.append(word)
    return out, repairs, missing, differences


def issues_for(units, words):
    issues = []
    for unit in units:
        text = plain(unit["html"])
        rect = bounding(unit["polygon"])
        layer = [word for word, box in words if inside(box, rect)]
        fixed, repairs, unread, differences = compare(text.split(), layer)
        if repairs:
            changes = ", ".join(f"{ocr!r} -> {layer!r}" for ocr, layer in repairs)
            issues.append(
                {
                    "block_id": unit["id"],
                    "kind": "text-layer-spelling",
                    "description": f"The PDF text layer inside this block spells: {changes}.",
                    "suggested_correction": " ".join(fixed),
                }
            )
        if differences:
            changes = ", ".join(f"OCR {ocr!r}, text layer {layer!r}" for ocr, layer in differences)
            issues.append(
                {
                    "block_id": unit["id"],
                    "kind": "text-layer-difference",
                    "description": "Digits, dice, signs or markers differ; check the page image: "
                    + changes
                    + ".",
                    "suggested_correction": None,
                }
            )
        if len(unread) >= UNREAD_MIN:
            issues.append(
                {
                    "block_id": unit["id"],
                    "kind": "text-layer-unread",
                    "description": "Text-layer words inside this block that its text lacks: "
                    + " ".join(unread),
                    "suggested_correction": None,
                }
            )
    return issues


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--pdf", required=True, type=Path)
    args = parser.parse_args()
    bundle_path = args.bundle / "bundle.json"
    bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
    if digest(args.pdf) != bundle["source_sha256"]:
        raise ValueError("The PDF does not match the bundle's source hash.")
    from pdftext.extraction import dictionary_output

    pages = bundle["pages"]
    layer = dictionary_output(
        str(args.pdf), page_range=[page["pdf_page"] - 1 for page in pages], keep_chars=True
    )
    report = {
        "schema_version": 1,
        "state": "tool_suggestions_not_verified",
        "tool": "crosscheck_text_layer",
        "source_sha256": bundle["source_sha256"],
        "bundle_sha256": digest(bundle_path),
        "created_at": datetime.now(timezone.utc).isoformat(),
        "pages": [],
    }
    for page, text_layer in zip(pages, layer, strict=True):
        issues = issues_for(page["units"], page_words(text_layer))
        report["pages"].append(
            {
                "pdf_page": page["pdf_page"],
                "state": "tool_suggestions_not_verified",
                "findings": {
                    "summary": f"{len(issues)} text-layer suggestion(s) for {len(page['units'])} block(s).",
                    "issues": issues,
                    "needs_visual_review": True,
                },
            }
        )
    output = args.bundle / f"text-layer-check-{uuid.uuid4().hex}"
    output.mkdir(exist_ok=False)
    write_json(output / "report.json", report)
    total = sum(len(page["findings"]["issues"]) for page in report["pages"])
    print(
        f"Text-layer report: {output / 'report.json'} ({total} suggestions). Nothing was applied."
    )


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Cross-check failed: {error}", file=sys.stderr)
        sys.exit(1)
