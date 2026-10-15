"""Semantic regressions for multi-book identity, ancestry, access and revisions."""

import copy
import unittest

from evidence_forest import Forest, assemble


def node(ident, parent, start, end, text="original synthetic text"):
    return {
        "id": ident,
        "title": ident,
        "parent_state": "root" if parent is None else "known",
        "parent": parent,
        "start": start,
        "end": end,
        "text": text,
        "references": [],
    }


def fixture():
    books = []
    for ident in ("core", "gm", "bestiary", "expansion"):
        books.append(
            {
                "id": ident,
                "revision": "v1",
                "edition": "demo",
                "pages": 200,
                "nodes": [node("root", None, [1, 0], [200, 999])],
            }
        )
    books[0]["nodes"] += [
        node("spells", "root", [10, 0], [180, 999]),
        node("lists", "spells", [11, 0], [20, 999]),
        node("descriptions", "spells", [21, 0], [180, 999]),
        node("spark", "descriptions", [160, 0], [161, 99]),
    ]
    books[1]["nodes"] += [node("spark", "root", [50, 0], [50, 99], "GM-only synthetic content")]
    return {
        "encoding": 1,
        "revision": 1,
        "policy": {"edition": "demo", "precedence": []},
        "enabled": [{"book": b["id"], "revision": "v1"} for b in books],
        "books": books,
    }


def target(data):
    return data["books"][0]["nodes"][-1]


class ForestTests(unittest.TestCase):
    def test_public_indexes_cannot_mutate_authority(self):
        forest = Forest(fixture())
        before = forest.materialize("core", "spark", {"core"})
        forest.nodes["core"]["spark"]["text"] = "modified without revision"
        forest.enabled.pop("core")
        self.assertEqual(forest.read_materialized(before, {"core"}), before["context"])

    def test_reference_consumer_stale_after_target_repair(self):
        data = fixture()
        target(data)["references"] = [{"book": "gm", "revision": "v1", "node": "spark"}]
        original = Forest(data)
        artifact = original.materialize("core", "spark", {"core", "gm"})
        repaired = original.revise("gm", "spark", {"text": "corrected target"})
        with self.assertRaises(ValueError):
            repaired.read_materialized(artifact, {"core", "gm"})

    def test_reference_missing_and_wrong_revision_remain_explicit(self):
        data = fixture()
        target(data)["references"] = [
            {"book": "gm", "revision": "v2", "node": "spark"},
            {"book": "gm", "revision": "v1", "node": "absent"},
        ]
        refs = Forest(data).context("core", "spark", {"core", "gm"})["references"]
        self.assertEqual([r["status"] for r in refs], ["revision_mismatch", "unresolved"])

    def test_subclasses_keep_structural_ancestry_after_repair(self):
        data = fixture()
        data["books"][0]["nodes"] += [
            node("class", "root", [181, 0], [199, 0]),
            node("ember", "class", [182, 0], [185, 0]),
            node("tide", "class", [186, 0], [190, 0]),
        ]
        forest = Forest(data).revise("core", "ember", {"title": "Ember revised"})
        self.assertEqual(
            [n["id"] for n in forest.lineage("core", "tide")], ["root", "class", "tide"]
        )

    def test_hundred_page_chapter_and_sibling_list_not_ancestor(self):
        forest = Forest(fixture())
        self.assertEqual(
            [n["id"] for n in forest.lineage("core", "spark")],
            ["root", "spells", "descriptions", "spark"],
        )

    def test_batch_continuation_preserves_parent_and_deduplicates_overlap(self):
        data = fixture()
        tail = data["books"][0]["nodes"].pop()
        batches = [
            {"book": "core", "revision": "v1", "nodes": [tail]},
            {"book": "core", "revision": "v1", "nodes": [tail]},
        ]
        forest = assemble(data, batches)
        self.assertEqual(len(forest.payload["books"][0]["nodes"]), 5)
        self.assertEqual(forest.lineage("core", "spark")[-2]["id"], "descriptions")

    def test_conflicting_overlap_and_wrong_batch_pin_rejected(self):
        data = fixture()
        changed = copy.deepcopy(target(data))
        changed["title"] = "different"
        with self.assertRaises(ValueError):
            assemble(data, [{"book": "core", "revision": "v1", "nodes": [changed]}])
        with self.assertRaises(ValueError):
            assemble(data, [{"book": "core", "revision": "v2", "nodes": []}])

    def test_cycles_dangling_and_cross_book_parent_rejected(self):
        for parent in ("spark", "missing", "gm:root"):
            with self.subTest(parent=parent):
                data = fixture()
                target(data)["parent"] = parent
                with self.assertRaises(ValueError):
                    Forest(data)

    def test_invalid_known_endpoints_not_excused_by_incomplete(self):
        for start, end in (
            ([0, 0], None),
            ([201, 0], None),
            ([161, 0], [160, 0]),
            ([True, 0], None),
        ):
            with self.subTest(start=start):
                data = fixture()
                target(data).update(start=start, end=end, reason="boundary unresolved")
                with self.assertRaises(ValueError):
                    Forest(data)

    def test_parent_containment_checked(self):
        data = fixture()
        target(data)["end"] = [190, 0]
        with self.assertRaises(ValueError):
            Forest(data)

    def test_unresolved_parent_is_explicit_not_dangling_or_usable(self):
        data = fixture()
        target(data).update(parent=None, parent_state="unresolved", reason="source heading unclear")
        forest = Forest(data)
        with self.assertRaises(ValueError):
            forest.context("core", "spark", {"core"})

    def test_same_title_other_book_does_not_merge_or_grant_access(self):
        forest = Forest(fixture())
        self.assertNotEqual(
            forest.context("core", "spark", {"core"})["text"],
            forest.context("gm", "spark", {"gm"})["text"],
        )
        with self.assertRaises(PermissionError):
            forest.context("gm", "spark", {"core"})

    def test_cross_book_reference_enabled_pin_and_access(self):
        data = fixture()
        target(data)["references"] = [{"book": "gm", "revision": "v1", "node": "spark"}]
        forest = Forest(data)
        hidden = forest.context("core", "spark", {"core"})
        self.assertNotIn("GM-only", str(hidden))
        self.assertEqual(hidden["references"][0]["status"], "inaccessible")
        visible = forest.context("core", "spark", {"core", "gm"})
        self.assertEqual(visible["references"][0]["status"], "resolved")
        data["enabled"] = [pin for pin in data["enabled"] if pin["book"] != "gm"]
        hidden = Forest(data).context("core", "spark", {"core", "gm"})
        self.assertEqual(hidden["references"][0]["status"], "disabled")
        self.assertNotIn("GM-only", str(hidden))

    def test_edition_and_duplicate_source_pins_rejected(self):
        for mode in ("edition", "duplicate", "revision"):
            data = fixture()
            if mode == "edition":
                data["books"][1]["edition"] = "other"
            elif mode == "duplicate":
                data["enabled"].append(data["enabled"][0])
            else:
                data["enabled"][0]["revision"] = "v2"
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                Forest(data)

    def test_repair_keeps_ids_original_and_invalidates_all_dependent_artifacts(self):
        original = Forest(fixture())
        artifact = original.materialize("core", "spark", {"core"})
        repaired = original.revise("core", "descriptions", {"title": "corrected heading"})
        self.assertEqual(original.payload["books"][0]["nodes"][3]["title"], "descriptions")
        self.assertEqual(repaired.lineage("core", "spark")[-1]["id"], "spark")
        with self.assertRaises(ValueError):
            repaired.read_materialized(artifact, {"core"})
        with self.assertRaises(ValueError):
            original.revise("core", "spark", {"id": "renamed-id"})

    def test_cached_context_never_grants_current_access(self):
        forest = Forest(fixture())
        artifact = forest.materialize("gm", "spark", {"gm"})
        with self.assertRaises(PermissionError):
            forest.read_materialized(artifact, {"core"})

    def test_source_set_change_invalidates_materialized_context(self):
        data = fixture()
        forest = Forest(data)
        artifact = forest.materialize("core", "spark", {"core"})
        data["enabled"].pop()
        with self.assertRaises(ValueError):
            Forest(data).read_materialized(artifact, {"core"})


if __name__ == "__main__":
    unittest.main()
