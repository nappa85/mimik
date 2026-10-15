import contextlib
import hashlib
import json
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import prepare_review
from source_bundle import verify_bundle, write_json


class PrepareSnapshotTests(unittest.TestCase):
    def test_verified_inputs_cannot_be_replaced_before_preparation(self):
        for replaced in ["source.json", "source.pdf"]:
            with self.subTest(replaced=replaced), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                bundle = root / "extracted"
                bundle.mkdir()
                pdf = root / "source.pdf"
                pdf.write_bytes(b"verified PDF snapshot")
                raw = {
                    "children": [
                        {
                            "id": "/page/0/Page/0",
                            "block_type": "Page",
                            "polygon": [[0, 0], [200, 0], [200, 300], [0, 300]],
                            "html": "",
                            "children": [],
                        }
                    ]
                }
                write_json(bundle / "source.json", raw)

                def sha(path):
                    return hashlib.sha256(path.read_bytes()).hexdigest()

                write_json(
                    bundle / "manifest.json",
                    {
                        "schema_version": 1,
                        "status": "extracted_unreviewed",
                        "source_id": "fixture",
                        "source_sha256": sha(pdf),
                        "artifact_sha256": {"source.json": sha(bundle / "source.json")},
                        "pdf_pages": [1],
                        "extractor": "fixture",
                        "extractor_version": "1",
                        "config": {},
                    },
                )

                def replace_after_verify(
                    directory, source, pdf=pdf, replaced=replaced, bundle=bundle
                ):
                    manifest = verify_bundle(directory, source)
                    target = pdf if replaced == "source.pdf" else bundle / "source.json"
                    if replaced == "source.pdf":
                        target.write_bytes(b"different PDF")
                    else:
                        modified = json.loads(target.read_text())
                        modified["children"][0]["html"] = "different extraction"
                        write_json(target, modified)
                    return manifest

                page = Mock()
                page.get_rotation.return_value = 0
                page.get_bbox.return_value = (0, 0, 200, 300)
                page.get_size.return_value = (200, 300)
                document = Mock()
                document.__len__ = Mock(return_value=1)
                document.__getitem__ = Mock(return_value=page)
                pdf_module = types.SimpleNamespace(
                    PdfDocument=lambda path, document=document: document
                )
                pillow = types.SimpleNamespace(
                    Image=types.SimpleNamespace(open=lambda path: contextlib.nullcontext(Mock()))
                )

                def render(arguments, **kwargs):
                    if "-o" in arguments:
                        Path(arguments[arguments.index("-o") + 1]).write_bytes(
                            b"fixture rendered image"
                        )
                    return types.SimpleNamespace(stderr="fixture renderer", stdout="", returncode=0)

                argv = [
                    "prepare_review.py",
                    str(bundle),
                    "--pdf",
                    str(pdf),
                    "--title",
                    "Fixture",
                    "--system",
                    "mork-borg",
                    "--edition",
                    "fixture",
                    "--language",
                    "en",
                    "--output",
                    str(root / "output"),
                ]
                with (
                    patch.dict("sys.modules", {"pypdfium2": pdf_module, "PIL": pillow}),
                    patch("sys.argv", argv),
                    patch.object(
                        prepare_review,
                        "verify_bundle",
                        side_effect=replace_after_verify,
                        create=True,
                    ),
                    patch("source_bundle.verify_bundle", side_effect=replace_after_verify),
                    patch("prepare_review.subprocess.run", side_effect=render),
                ):
                    with self.assertRaises(ValueError):
                        prepare_review.main()
