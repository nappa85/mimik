"""Shared, dependency-free helpers for extraction/review prototypes."""

import hashlib
import json
import re
from pathlib import Path


def digest(path):
    hasher = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def parse_pages(value):
    """User-facing page indexes are one-based; Marker indexes are zero-based."""
    pages = set()
    for part in value.split(","):
        if not re.fullmatch(r"[0-9]+(?:-[0-9]+)?", part.strip()):
            raise ValueError("Pages must be comma-separated positive indexes/ranges.")
        bounds = [int(v) for v in part.strip().split("-")]
        first, last = bounds[0], bounds[-1]
        if first < 1 or last < first or last - first > 10_000:
            raise ValueError("Invalid page range.")
        pages.update(range(first, last + 1))
    return sorted(pages)


def page_blocks(raw):
    pages = raw if isinstance(raw, list) else raw.get("children", [])
    result = {}
    for page in pages:
        match = re.fullmatch(r"/page/(\d+)/Page/\d+", str(page.get("id", "")))
        if page.get("block_type") != "Page" or not match:
            raise ValueError("Unsupported Marker page schema: no reliable page index.")
        index = int(match[1]) + 1
        if index in result:
            raise ValueError("Duplicate PDF page index in extraction.")
        result[index] = page
    return result


def review_units(page, pdf_page):
    """Preserve structural blocks, including tables/figures, not plain-text chunks."""
    units = []

    def visit(block):
        children = block.get("children") or []
        if block.get("block_type") != "Page":
            html = block.get("html", "")
            flags = []
            if "\ufffd" in html:
                flags.append("replacement_character")
            if not block.get("polygon"):
                flags.append("missing_region")
            if block.get("block_type") in {"Picture", "Figure", "Table", "Form"}:
                flags.append("visual_semantics_check_required")
            units.append(
                {
                    "block_id": block["id"],
                    "pdf_page": pdf_page,
                    "printed_page": None,
                    "block_type": block.get("block_type"),
                    "polygon": block.get("polygon"),
                    "flags": flags,
                    "state": "unreviewed",
                    "corrections": [],
                }
            )
        for child in children:
            visit(child)

    visit(page)
    return units


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def verify_bundle(bundle, pdf):
    from jsonschema import Draft202012Validator

    bundle = Path(bundle)
    manifest = json.loads((bundle / "manifest.json").read_text(encoding="utf-8"))
    schema = {
        "type": "object",
        "required": ["schema_version", "status", "source_sha256", "artifact_sha256"],
        "properties": {
            "schema_version": {"const": 1},
            "status": {"const": "extracted_unreviewed"},
            "source_sha256": {"type": "string", "pattern": "^[a-f0-9]{64}$"},
            "artifact_sha256": {
                "type": "object",
                "required": ["source.json"],
                "propertyNames": {"pattern": r"^[^/\\]+$"},
                "additionalProperties": {"type": "string", "pattern": "^[a-f0-9]{64}$"},
            },
        },
    }
    if not Draft202012Validator(schema).is_valid(manifest):
        raise ValueError(
            "Invalid or incomplete extraction manifest; source.json must be hash-bound."
        )
    if manifest.get("status") != "extracted_unreviewed":
        raise ValueError("Extraction is incomplete or not in a supported state.")
    if digest(pdf) != manifest["source_sha256"]:
        raise ValueError("PDF hash differs from the extracted source.")
    for name, expected in manifest["artifact_sha256"].items():
        path = bundle / name
        if path.parent != bundle or digest(path) != expected:
            raise ValueError("Extraction artifact changed; create a new bundle before review.")
    return manifest


def verified_snapshots(bundle, pdf):
    """Freeze and recheck exactly the PDF/JSON bytes used by preparation/rendering."""
    manifest = verify_bundle(bundle, pdf)

    def read(path, limit):
        with Path(path).open("rb") as stream:
            value = stream.read(limit + 1)
        if len(value) > limit:
            raise ValueError("Source snapshot exceeds the documented limit")
        return value

    pdf_bytes = read(pdf, 128 * 1024 * 1024)
    raw_bytes = read(Path(bundle) / "source.json", 32 * 1024 * 1024)
    if (
        hashlib.sha256(pdf_bytes).hexdigest() != manifest["source_sha256"]
        or hashlib.sha256(raw_bytes).hexdigest() != manifest["artifact_sha256"]["source.json"]
    ):
        raise ValueError("Source changed after verification; no replacement identity adopted")
    return manifest, pdf_bytes, raw_bytes
