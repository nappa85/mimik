import json
import logging
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

import crosscheck_text_layer
import extract_manual
from crosscheck_text_layer import compare, inside, issues_for, page_words


def char_span(text, x, y, width=5.0):
    chars = []
    for offset, char in enumerate(text):
        left = x + offset * width
        chars.append({"char": char, "bbox": [left, y, left + width, y + 10]})
    return {"chars": chars}


def layer_page(*spans):
    return {"blocks": [{"lines": [{"spans": list(spans)}]}]}


class CrosscheckTests(unittest.TestCase):
    def test_invalid_findings_fail_page_without_truncation(self):
        from source_bundle import digest, write_json

        for field, value in (
            ("description", ""),
            ("description", "x" * 5001),
            ("suggested_correction", "x" * 20001),
            ("block_id", "é" * 257),
        ):
            with (
                self.subTest(field=field, length=len(value)),
                tempfile.TemporaryDirectory() as temp,
            ):
                root = Path(temp)
                pdf = root / "fixture.pdf"
                pdf.write_bytes(b"synthetic pdf")
                write_json(
                    root / "bundle.json",
                    {"source_sha256": digest(pdf), "pages": [{"pdf_page": 1, "units": []}]},
                )
                issue = {
                    "action": "correction",
                    "kind": "text",
                    "block_id": "block",
                    "description": "Read image",
                    "suggested_correction": "Advice",
                }
                issue[field] = value
                extraction = types.ModuleType("pdftext.extraction")
                extraction.dictionary_output = lambda *args, **kwargs: [layer_page()]
                with (
                    patch.dict(
                        sys.modules,
                        {"pdftext": types.ModuleType("pdftext"), "pdftext.extraction": extraction},
                    ),
                    patch.object(sys, "argv", ["crosscheck", str(root), "--pdf", str(pdf)]),
                    patch.object(crosscheck_text_layer, "issues_for", return_value=[issue]),
                ):
                    with self.assertRaisesRegex(ValueError, "failed"):
                        crosscheck_text_layer.main()
                reports = list(root.glob("text-layer-check-*/report.json"))
                self.assertEqual(len(reports), 1)
                page = json.loads(reports[0].read_text())["pages"][0]
                self.assertEqual(page["state"], "failed")
                self.assertNotIn("findings", page)
                self.assertIn(field, page["error"])

    def test_shadow_duplicates_are_dropped_and_distant_repeats_kept(self):
        page = layer_page(
            char_span("Spada Corta", 10, 10),
            char_span("Spada Corta", 11.5, 11.5),
            char_span("Spada", 10, 200),
        )
        words = [word for word, _ in page_words(page)]
        self.assertEqual(words, ["Spada", "Corta", "Spada"])

    def test_a_close_repeat_of_another_size_is_not_a_shadow(self):
        page = layer_page(char_span("d4", 10, 10), char_span("d4", 11, 11, width=12.0))
        self.assertEqual([word for word, _ in page_words(page)], ["d4", "d4"])

    def test_near_twins_take_the_text_layer_spelling_in_ocr_order(self):
        fixed, repairs, unread, _ = compare(
            ["3.", "Spada", "Conta", "(d4)"], ["3.", "Spada", "Corta", "(d4)"]
        )
        self.assertEqual(fixed, ["3.", "Spada", "Corta", "(d4)"])
        self.assertEqual(repairs, [["Conta", "Corta"]])
        self.assertEqual(unread, [])

    def test_unlike_words_are_not_replaced_but_reported_unread(self):
        fixed, repairs, unread, _ = compare(["Bevuta", "1a"], ["Notte", "1a"])
        self.assertEqual(fixed, ["Bevuta", "1a"])
        self.assertEqual(repairs, [])
        self.assertEqual(unread, ["Notte"])

    def test_skipped_lines_are_unread_and_order_differences_are_not(self):
        _, _, unread, _ = compare(
            ["Grado", "da", "1", "a", "2", "25a"],
            ["Grado", "da", "1", "a", "2", "25a", "Non", "è", "possibile", "portare"],
        )
        self.assertEqual(unread, ["Non", "è", "possibile", "portare"])
        _, _, unread, _ = compare(["(d6)", "Martello", "d'Arme"], ["Martello", "d'Arme", "(d6)"])
        self.assertEqual(unread, [])

    def test_repeated_words_count_each_occurrence(self):
        _, _, unread, _ = compare(["ha"], ["ha", "ha", "ha", "ha"])
        self.assertEqual(unread, ["ha", "ha", "ha"])

    def test_digits_dice_signs_and_markers_are_differences_not_spellings(self):
        for ocr, layer in (
            ("100a", "10a"),
            ("(d6)", "(d8)"),
            ("armatura*", "armatura"),
            ("-d4", "+d4"),
            ("ARMI*", "ARMI"),
        ):
            with self.subTest(ocr=ocr, layer=layer):
                fixed, repairs, _, differences = compare(["x", ocr], ["x", layer])
                self.assertEqual(fixed, ["x", ocr])
                self.assertEqual(repairs, [])
                self.assertEqual(differences, [[ocr, layer]])
        fixed, repairs, _, _ = compare(["un'arma", "1-2"], ["un’arma", "1–2"])
        self.assertEqual(fixed, ["un’arma", "1–2"])
        self.assertEqual(len(repairs), 2)

    def test_word_centres_decide_membership(self):
        self.assertTrue(inside([10, 10, 20, 20], [0, 0, 15, 15]))
        self.assertFalse(inside([10, 10, 40, 40], [0, 0, 15, 15]))

    def test_issues_cite_existing_blocks_and_never_apply(self):
        unit = {
            "id": "/page/24/Text/3",
            "polygon": [[0, 0], [200, 0], [200, 30], [0, 30]],
            "html": "<p>3. Spada Conta (d4)</p>",
        }
        words = page_words(layer_page(char_span("3. Spada Corta (d4)", 5, 5)))
        issues = issues_for([unit], words)
        self.assertEqual(len(issues), 1)
        self.assertEqual(issues[0]["block_id"], unit["id"])
        self.assertEqual(issues[0]["kind"], "text-layer-spelling")
        self.assertEqual(issues[0]["action"], "correction")
        self.assertEqual(issues[0]["suggested_correction"], "3. Spada Corta (d4)")
        self.assertEqual(unit["html"], "<p>3. Spada Conta (d4)</p>")

    def test_three_unread_words_make_a_suggestion_without_correction(self):
        unit = {
            "id": "/page/29/Table/2",
            "polygon": [[0, 0], [400, 0], [400, 30], [0, 30]],
            "html": "<p>Grado</p>",
        }
        words = page_words(layer_page(char_span("Grado Non è possibile", 5, 5)))
        issues = issues_for([unit], words)
        self.assertEqual([issue["kind"] for issue in issues], ["text-layer-unread"])
        self.assertIsNone(issues[0]["suggested_correction"])
        self.assertEqual(issues[0]["action"], "note")

    def test_crosscheck_v2_report_matches_closed_schema(self):
        from jsonschema import Draft202012Validator

        schema = json.loads(
            (
                Path(__file__).resolve().parents[1] / "schemas" / "suggestion-report.v2.json"
            ).read_text()
        )
        unit = {
            "id": "/page/0/Text/1",
            "html": "<p>armatura*</p>",
            "polygon": [[0, 0], [100, 0], [100, 30], [0, 30]],
        }
        issues = issues_for([unit], [("armatura", [1, 1, 30, 10])])
        self.assertEqual(issues[0]["action"], "note")
        report = {
            "schema_version": 2,
            "state": "tool_suggestions_not_verified",
            "tool": "crosscheck_text_layer",
            "source_sha256": "a" * 64,
            "bundle_sha256": "b" * 64,
            "created_at": "2026-10-30T00:00:00Z",
            "pages": [
                {
                    "pdf_page": 1,
                    "state": "tool_suggestions_not_verified",
                    "findings": {
                        "summary": "fixture",
                        "needs_visual_review": True,
                        "issues": issues,
                    },
                }
            ],
        }
        validator = Draft202012Validator(schema)
        validator.validate(report)
        report["pages"][0]["findings"]["issues"][0]["region"] = None
        self.assertFalse(validator.is_valid(report))


class OcrOptionTests(unittest.TestCase):
    def fetch(self, url):
        if url.endswith("/props"):
            return {"model_path": "/m/surya-2.gguf", "build_info": "b11609"}
        return {"data": [{"id": "datalab-to/surya-ocr-2"}]}

    def test_ocr_needs_a_server_and_disabled_ocr_refuses_one(self):
        with self.assertRaises(ValueError):
            extract_manual.ocr_provenance("force", None)
        with self.assertRaises(ValueError):
            extract_manual.ocr_provenance("disabled", "http://ocr:8080")
        self.assertIsNone(extract_manual.ocr_provenance("disabled", None))
        for bad in ("ftp://ocr:8080", "http://", "http://ocr:8080/v1"):
            with self.assertRaises(ValueError):
                extract_manual.ocr_provenance("force", bad, self.fetch)

    def test_provenance_records_the_served_model_and_build(self):
        with patch.object(extract_manual.importlib.metadata, "version", return_value="0.22.1"):
            ocr = extract_manual.ocr_provenance("force", "http://ocr:8080/", self.fetch)
        self.assertEqual(ocr["server"], "http://ocr:8080")
        reported = ocr["server_reported"]
        self.assertEqual(reported["served_model"], ["datalab-to/surya-ocr-2"])
        self.assertEqual(reported["model_path"], "/m/surya-2.gguf")
        self.assertEqual(reported["build_info"], "b11609")
        self.assertEqual(ocr["surya_version"], "0.22.1")

    def test_an_incomplete_or_foreign_server_report_is_refused(self):
        def fetch_with(props, models):
            return lambda url: props if url.endswith("/props") else {"data": models}

        good = {"model_path": "/m/surya-2.gguf", "build_info": "b11609"}
        for props, models in (
            (good, []),
            (good, [{"id": "other-model"}]),
            (good, [{"id": "datalab-to/surya-ocr-2"}, {"id": "other-model"}]),
            ({"build_info": "b11609"}, [{"id": "datalab-to/surya-ocr-2"}]),
            ({"model_path": "/m/surya-2.gguf"}, [{"id": "datalab-to/surya-ocr-2"}]),
        ):
            with self.subTest(props=props, models=models), self.assertRaises(ValueError):
                extract_manual.ocr_provenance("force", "http://ocr:8080", fetch_with(props, models))

    def test_ocr_failure_warnings_from_marker_reach_the_root_handler(self):
        # As in main(): the handler is on the root logger; Marker logs to "marker" (propagates).
        failures = extract_manual.OcrFailures()
        logging.getLogger().addHandler(failures)
        try:
            marker = logging.getLogger("marker")
            marker.warning("OCR failed for a SectionHeader block on page 38")
            marker.warning("Table OCR failed for block /page/1/Table/2")
            marker.warning("Table processing stats")
        finally:
            logging.getLogger().removeHandler(failures)
        self.assertEqual(failures.count, 2)


if __name__ == "__main__":
    unittest.main()
