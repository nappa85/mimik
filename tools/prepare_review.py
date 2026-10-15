"""Prepare fixed-name original page images and strict leaf units for Rust source import."""

import argparse
import json
import subprocess
import tempfile
import uuid
from pathlib import Path

from source_bundle import digest, page_blocks, verified_snapshots, write_json

KINDS = set(
    json.loads(
        (Path(__file__).resolve().parents[1] / "schemas/source-kinds.json").read_text(
            encoding="utf-8"
        )
    )
)


def units(node):
    result = []
    children = node.get("children") or []
    if node["block_type"] in KINDS:
        if children or "<content-ref" in node["html"]:
            raise ValueError("Nested review content is not supported by prepared contract v1")
        result.append(
            {
                "id": node["id"],
                "kind": node["block_type"],
                "polygon": node["polygon"],
                "html": node["html"],
            }
        )
    for child in children:
        result.extend(units(child))
    return result


def render_pages(pdf_snapshot, pages, output):
    import pypdfium2
    from PIL import Image

    prepared = []
    with pypdfium2.PdfDocument(pdf_snapshot) as pdf:
        for index, page in pages.items():
            if not 1 <= index <= len(pdf):
                raise ValueError("Original PDF page index out of bounds")
            original = pdf[index - 1]
            try:
                if original.get_rotation() != 0 or original.get_bbox()[:2] != (0, 0):
                    raise ValueError(
                        "Rotation/nonzero crop origin unsupported in first import contract"
                    )
                width, height = page["polygon"][2]
                actual_width, actual_height = original.get_size()
                if abs(width - actual_width) > 2 or abs(height - actual_height) > 2:
                    raise ValueError("Marker page dimensions differ from original")
                image = output / f"page-{index}.png"
                subprocess.run(
                    [
                        "mutool",
                        "draw",
                        "-q",
                        "-r",
                        "110",
                        "-o",
                        str(image),
                        str(pdf_snapshot),
                        str(index),
                    ],
                    check=True,
                    capture_output=True,
                    timeout=60,
                )
                with Image.open(image) as png:
                    png.verify()
                prepared.append(
                    {
                        "pdf_page": index,
                        "width_points": width,
                        "height_points": height,
                        "image_sha256": digest(image),
                        "units": units(page),
                    }
                )
            finally:
                original.close()
    return prepared


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("extraction", type=Path)
    parser.add_argument("--pdf", required=True, type=Path)
    parser.add_argument("--title", required=True)
    parser.add_argument("--system", required=True)
    parser.add_argument("--edition", required=True)
    parser.add_argument("--language", required=True)
    parser.add_argument("--output", type=Path, default=Path("/output"))
    args = parser.parse_args()
    manifest, pdf_bytes, raw_bytes = verified_snapshots(args.extraction, args.pdf)
    pages = page_blocks(json.loads(raw_bytes))
    if sorted(pages) != manifest["pdf_pages"]:
        raise ValueError("Raw page coverage differs from extraction manifest")
    output = args.output / f"review-ready-{uuid.uuid4().hex}"
    output.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="mimik-verified-") as temp:
        pdf_snapshot = Path(temp) / "original.pdf"
        pdf_snapshot.write_bytes(pdf_bytes)
        prepared = render_pages(pdf_snapshot, pages, output)
    (output / "source.json").write_bytes(raw_bytes)
    version = subprocess.run(
        ["mutool", "-v"], capture_output=True, text=True, check=True
    ).stderr.strip()
    bundle = {
        "contract_version": 1,
        "source_id": manifest["source_id"],
        "title": args.title,
        "system_id": args.system,
        "edition": args.edition,
        "language": args.language,
        "source_sha256": manifest["source_sha256"],
        "extraction_sha256": manifest["artifact_sha256"]["source.json"],
        "extractor": {
            "name": manifest["extractor"],
            "version": manifest["extractor_version"],
            "settings": manifest["config"],
        },
        "renderer": {
            "name": "mutool",
            "version": version,
            "settings": {"dpi": 110, "original_pdf_indexes": True},
        },
        "pages": prepared,
    }
    write_json(output / "bundle.json", bundle)
    print(f"Prepared unreviewed bundle: {output}")


if __name__ == "__main__":
    main()
