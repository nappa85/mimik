import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from review_extraction import resolve_api_key, v2_findings, validate_findings
from source_bundle import digest, page_blocks, parse_pages, review_units, verify_bundle, write_json


class SourceReviewTests(unittest.TestCase):
    def setUp(self):
        self.page = {
            "id": "/page/29/Page/0",
            "block_type": "Page",
            "children": [
                {
                    "id": "/page/29/Table/1",
                    "block_type": "Table",
                    "polygon": [[0, 0], [1, 0], [1, 1], [0, 1]],
                    "html": "<table><tr><th>Die</th><th>Value</th></tr><tr><td>d6</td><td>−2</td></tr></table>",
                    "children": [],
                }
            ],
        }

    def test_page_numbering_and_table_regions_remain_unreviewed(self):
        self.assertEqual(parse_pages("28,30-33"), [28, 30, 31, 32, 33])
        pages = page_blocks({"children": [self.page]})
        self.assertEqual(list(pages), [30])
        units = review_units(self.page, 30)
        self.assertEqual(units[0]["block_type"], "Table")
        self.assertEqual(units[0]["state"], "unreviewed")
        self.assertIsNone(units[0]["printed_page"])
        self.assertTrue(units[0]["polygon"])
        for value in ["0", "5-2", "", "-1", "1,x"]:
            with self.assertRaises(ValueError):
                parse_pages(value)

    def test_modified_pdf_or_extraction_cannot_reuse_bundle_identity(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            pdf = root / "test.pdf"
            pdf.write_bytes(b"fixture pdf identity")
            bundle = root / "bundle"
            bundle.mkdir()
            write_json(bundle / "source.json", {"children": [self.page]})
            write_json(
                bundle / "manifest.json",
                {
                    "schema_version": 1,
                    "status": "extracted_unreviewed",
                    "source_sha256": digest(pdf),
                    "artifact_sha256": {"source.json": digest(bundle / "source.json")},
                },
            )
            verify_bundle(bundle, pdf)
            (bundle / "source.json").write_text("changed", encoding="utf-8")
            with self.assertRaises(ValueError):
                verify_bundle(bundle, pdf)

    def test_model_cannot_cite_invented_blocks_or_return_untyped_claims(self):
        result = {
            "summary": "Check the sign",
            "needs_visual_review": True,
            "issues": [
                {
                    "block_id": "/page/29/Table/1",
                    "kind": "numeric",
                    "description": "Potential sign error",
                    "suggested_correction": None,
                }
            ],
        }
        validate_findings(result, self.page)
        invalid = json.loads(json.dumps(result))
        invalid["issues"][0]["block_id"] = "invented"
        with self.assertRaises(ValueError):
            validate_findings(invalid, self.page)
        invalid = dict(result, needs_visual_review="false")
        with self.assertRaises(ValueError):
            validate_findings(invalid, self.page)

    def test_review_schema_requires_nullable_keys_and_rejects_extra_fields(self):
        good = {
            "summary": "Check source",
            "needs_visual_review": True,
            "issues": [
                {
                    "block_id": None,
                    "kind": "missing",
                    "description": "Region absent",
                    "suggested_correction": None,
                }
            ],
        }
        validate_findings(good, self.page)
        for field in ["block_id", "suggested_correction"]:
            invalid = json.loads(json.dumps(good))
            del invalid["issues"][0][field]
            with self.assertRaises(ValueError):
                validate_findings(invalid, self.page)
        with self.assertRaises(ValueError):
            validate_findings(dict(good, unexpected="not allowed"), self.page)
        invalid = json.loads(json.dumps(good))
        invalid["issues"][0]["unexpected"] = "not allowed"
        with self.assertRaises(ValueError):
            validate_findings(invalid, self.page)

    def test_v2_actions_follow_prepared_unit_shape_not_model_kind(self):
        page = json.loads(json.dumps(self.page))
        page["children"].append(
            {"id": "/page/29/Section/1", "block_type": "Section", "children": []}
        )
        findings = {
            "summary": "Fixture",
            "needs_visual_review": True,
            "issues": [
                {
                    "block_id": block,
                    "kind": "reject",
                    "description": "Read the page",
                    "suggested_correction": text,
                }
                for block, text in [
                    ("/page/29/Table/1", "Advice"),
                    ("/page/29/Table/1", None),
                    ("/page/29/Section/1", "Quoted text"),
                    (None, "Unlocated"),
                ]
            ],
        }
        converted = v2_findings(findings, page)
        self.assertEqual(
            [i["action"] for i in converted["issues"]], ["correction", "note", "note", "note"]
        )
        self.assertNotIn("action", findings["issues"][0])
        report = {
            "schema_version": 2,
            "state": "model_suggestions_not_verified",
            "source_sha256": "a" * 64,
            "bundle_sha256": "b" * 64,
            "endpoint": "https://example.invalid",
            "requested_model": "fixture",
            "created_at": "2026-10-30T00:00:00Z",
            "pages": [
                {"pdf_page": 30, "state": "model_suggestions_not_verified", "findings": converted},
                {
                    "pdf_page": 31,
                    "state": "failed",
                    "error": "Fixture failure",
                    "render_sha256": "c" * 64,
                    "returned_model": None,
                    "usage": {"tokens": 1},
                },
            ],
        }
        from jsonschema import Draft202012Validator

        schemas = Path(__file__).resolve().parents[1] / "schemas"
        v2 = Draft202012Validator(json.loads((schemas / "suggestion-report.v2.json").read_text()))
        v2.validate(report)
        for field in ["bundle_sha256", "action"]:
            bad = json.loads(json.dumps(report))
            if field == "action":
                del bad["pages"][0]["findings"]["issues"][0][field]
            else:
                del bad[field]
            self.assertFalse(v2.is_valid(bad))
        report["schema_version"] = 1
        del report["bundle_sha256"]
        report["artifact_sha256"] = {"source.json": "d" * 64, "reading.md": "e" * 64}
        for issue in report["pages"][0]["findings"]["issues"]:
            del issue["action"]
        v1 = Draft202012Validator(json.loads((schemas / "suggestion-report.v1.json").read_text()))
        v1.validate(report)
        report["pages"][0]["unexpected"] = True
        self.assertFalse(v1.is_valid(report))

    def test_changed_pdf_and_unsupported_manifest_identity_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            pdf = root / "source.pdf"
            pdf.write_bytes(b"source fixture")
            bundle = root / "bundle"
            bundle.mkdir()
            write_json(bundle / "source.json", {"children": [self.page]})
            good = {
                "schema_version": 1,
                "status": "extracted_unreviewed",
                "source_sha256": digest(pdf),
                "artifact_sha256": {"source.json": digest(bundle / "source.json")},
            }
            for field, value in [
                ("schema_version", 2),
                ("status", "reviewed"),
                ("source_sha256", "invalid"),
            ]:
                write_json(bundle / "manifest.json", dict(good, **{field: value}))
                with self.assertRaises(ValueError):
                    verify_bundle(bundle, pdf)
            write_json(bundle / "manifest.json", good)
            pdf.write_bytes(b"changed source fixture")
            with self.assertRaises(ValueError):
                verify_bundle(bundle, pdf)

    def test_bundle_must_hash_the_structured_artifact_the_reviewer_consumes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            pdf = root / "source.pdf"
            pdf.write_bytes(b"dummy source")
            bundle = root / "bundle"
            bundle.mkdir()
            write_json(bundle / "source.json", {"children": [self.page]})
            for hashes in [{}, {"reading.md": "0" * 64}]:
                write_json(
                    bundle / "manifest.json",
                    {
                        "schema_version": 1,
                        "status": "extracted_unreviewed",
                        "source_sha256": digest(pdf),
                        "artifact_sha256": hashes,
                    },
                )
                with self.assertRaises(ValueError):
                    verify_bundle(bundle, pdf)

    def test_dotenv_uses_dummy_keys_without_shell_execution_or_env_mutation(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {}, clear=True):
            path = Path(temp) / ".env"
            path.write_text(
                'OPENAI_API_KEY="dummy-file-key"\nMIMIK_REVIEW_API_KEY=\n', encoding="utf-8"
            )
            self.assertEqual(resolve_api_key(path), "dummy-file-key")
            self.assertNotIn("OPENAI_API_KEY", os.environ)
            os.environ["OPENAI_API_KEY"] = "dummy-process-key"
            path.write_text('MIMIK_REVIEW_API_KEY="dummy-specific-key"\n', encoding="utf-8")
            self.assertEqual(resolve_api_key(path), "dummy-process-key")
            del os.environ["OPENAI_API_KEY"]
            self.assertEqual(resolve_api_key(path), "dummy-specific-key")
            path.write_text('OPENAI_API_KEY="$(do-not-execute)"\n', encoding="utf-8")
            self.assertEqual(resolve_api_key(path), "$(do-not-execute)")


if __name__ == "__main__":
    unittest.main()
