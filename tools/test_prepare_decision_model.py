"""Artifact hashes, not model execution or network calls."""

import hashlib
import tempfile
import unittest
from pathlib import Path

from prepare_decision_model import readable_public_tree, verify_file


class ArtifactTests(unittest.TestCase):
    def test_generated_public_tree_is_readable_by_the_inference_uid(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            revision = "a" * 40
            tree = output / "hf/models--demo--model/trees" / (revision + ".json")
            tree.parent.mkdir(parents=True)
            tree.write_text("{}")
            tree.chmod(0o600)
            readable_public_tree(output, "demo/model", revision)
            self.assertEqual(tree.stat().st_mode & 0o777, 0o644)

    def test_both_hf_hash_forms_and_wrong_hash_or_size(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture"
            data = b"synthetic non-model bytes"
            path.write_bytes(data)
            common = {"rfilename": "fixture", "size": len(data)}
            lfs = {**common, "lfs": {"sha256": hashlib.sha256(data).hexdigest()}}
            git = {
                **common,
                "blobId": hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest(),
            }
            self.assertEqual(verify_file(path, lfs)["sha256"], verify_file(path, git)["sha256"])
            for bad in (
                {**lfs, "size": 1},
                {**lfs, "lfs": {"sha256": "0" * 64}},
                {**git, "blobId": "0" * 40},
            ):
                with self.assertRaises(ValueError):
                    verify_file(path, bad)


if __name__ == "__main__":
    unittest.main()
