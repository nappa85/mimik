"""Optional Python/Marker prototype. Never marks extracted content as verified."""

import argparse
import importlib.metadata
import json
import re
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

from source_bundle import digest, page_blocks, parse_pages, review_units, write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pdf", type=Path)
    parser.add_argument("--source-id", required=True)
    parser.add_argument("--pages", required=True, help="One-based PDF indexes, e.g. 28,30-33")
    parser.add_argument("--output", type=Path, default=Path("data/extractions"))
    parser.add_argument("--mode", choices=["fast", "balanced"], default="fast")
    parser.add_argument("--ocr", choices=["auto", "force", "disabled"], default="auto")
    args = parser.parse_args()
    pages = parse_pages(args.pages)
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,79}", args.source_id):
        raise ValueError("Source ID must be a short lowercase identifier.")
    if not args.pdf.is_file():
        raise ValueError("Source PDF does not exist.")
    try:
        version = importlib.metadata.version("marker-pdf")
        from marker.converters.pdf import PdfConverter
        from marker.models import create_model_dict
        from marker.output import save_output
        from marker.renderers.json import JSONRenderer
        from marker.renderers.markdown import MarkdownRenderer
    except (ImportError, importlib.metadata.PackageNotFoundError) as error:
        raise ValueError(
            "Marker prototype dependencies are unavailable. See docs/extraction-prototype.md."
        ) from error

    config = {
        "page_range": [page - 1 for page in pages],
        "mode": args.mode,
        "force_ocr": args.ocr == "force",
        "disable_ocr": args.ocr == "disabled",
        "use_llm": False,
        "paginate_output": True,
        "keep_pageheader_in_output": True,
        "keep_pagefooter_in_output": True,
    }
    output = args.output / f"{args.source_id}-{uuid.uuid4().hex}"
    output.mkdir(parents=True, exist_ok=False)
    manifest = {
        "schema_version": 1,
        "source_id": args.source_id,
        "source_sha256": digest(args.pdf),
        "pdf_pages": pages,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "extractor": "marker-pdf",
        "extractor_version": version,
        "config": config,
        "status": "extracting",
        "artifact_sha256": {},
    }
    write_json(output / "manifest.json", manifest)
    runtime_requirements = Path("/runtime-requirements.txt")
    if runtime_requirements.is_file():
        manifest["runtime_requirements_sha256"] = digest(runtime_requirements)
        (output / "runtime-requirements.txt").write_bytes(runtime_requirements.read_bytes())
    print(f"Extracting selected pages into {output}")
    try:
        converter = PdfConverter(artifact_dict=create_model_dict(), config=config)
        # Extract/process once, then render both formats from the same block tree.
        document = converter.build_document(str(args.pdf.resolve()))
        structured = converter.resolve_dependencies(JSONRenderer)(document)
        markdown = converter.resolve_dependencies(MarkdownRenderer)(document)
        save_output(structured, str(output), "source")
        save_output(markdown, str(output), "reading")
        raw = json.loads((output / "source.json").read_text(encoding="utf-8"))
        blocks = page_blocks(raw)
        if sorted(blocks) != pages:
            raise ValueError("Extracted page coverage does not match the requested page indexes.")
        units = [unit for index, page in blocks.items() for unit in review_units(page, index)]
        write_json(
            output / "review.json",
            {
                "schema_version": 1,
                "source_sha256": manifest["source_sha256"],
                "pages": [{"pdf_page": index, "state": "unreviewed"} for index in pages],
                "units": units,
            },
        )
        manifest["artifact_sha256"] = {
            path.name: digest(path)
            for path in output.iterdir()
            if path.is_file() and path.name not in {"manifest.json", "review.json"}
        }
        manifest["status"] = "extracted_unreviewed"
        write_json(output / "manifest.json", manifest)
    except Exception:
        manifest["status"] = "failed"
        write_json(output / "manifest.json", manifest)
        raise
    print(
        "Extraction complete; every page/block remains unreviewed. Compare with original PDF images."
    )


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Extraction failed: {error}", file=sys.stderr)
        sys.exit(1)
