import logging
import unittest
from unittest.mock import patch

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
    def test_shadow_duplicates_are_dropped_and_distant_repeats_kept(self):
        page = layer_page(
            char_span("Spada Corta", 10, 10),
            char_span("Spada Corta", 11.5, 11.5),
            char_span("Spada", 10, 200),
        )
        words = [word for word, _ in page_words(page)]
        self.assertEqual(words, ["Spada", "Corta", "Spada"])

    def test_near_twins_take_the_text_layer_spelling_in_ocr_order(self):
        fixed, repairs, unread = compare(
            ["3.", "Spada", "Conta", "(d4)"], ["3.", "Spada", "Corta", "(d4)"]
        )
        self.assertEqual(fixed, ["3.", "Spada", "Corta", "(d4)"])
        self.assertEqual(repairs, [["Conta", "Corta"]])
        self.assertEqual(unread, [])

    def test_unlike_words_are_not_replaced_but_reported_unread(self):
        fixed, repairs, unread = compare(["Bevuta", "1a"], ["Notte", "1a"])
        self.assertEqual(fixed, ["Bevuta", "1a"])
        self.assertEqual(repairs, [])
        self.assertEqual(unread, ["Notte"])

    def test_skipped_lines_are_unread_and_order_differences_are_not(self):
        _, _, unread = compare(
            ["Grado", "da", "1", "a", "2", "25a"],
            ["Grado", "da", "1", "a", "2", "25a", "Non", "è", "possibile", "portare"],
        )
        self.assertEqual(unread, ["Non", "è", "possibile", "portare"])
        _, _, unread = compare(["(d6)", "Martello", "d'Arme"], ["Martello", "d'Arme", "(d6)"])
        self.assertEqual(unread, [])

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
        self.assertEqual(ocr["served_model"], ["datalab-to/surya-ocr-2"])
        self.assertEqual(ocr["model_path"], "/m/surya-2.gguf")
        self.assertEqual(ocr["build_info"], "b11609")
        self.assertEqual(ocr["surya_version"], "0.22.1")

    def test_ocr_failure_warnings_are_counted(self):
        failures = extract_manual.OcrFailures()
        logger = logging.getLogger("marker-test")
        logger.addHandler(failures)
        try:
            logger.warning("OCR failed for a SectionHeader block on page 38")
            logger.warning("Table processing stats")
        finally:
            logger.removeHandler(failures)
        self.assertEqual(failures.count, 1)


if __name__ == "__main__":
    unittest.main()
