"""tools/decision_models.json schema and hashes (decision-router plan §5, §6 "Ops"). New module."""

import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import decision_models as dm
import model_registry

DOC = json.loads(dm.REGISTRY.read_text(encoding="utf-8"))


class RegistryTests(unittest.TestCase):
    def test_registry_is_valid_and_has_the_three_candidates(self):
        doc = dm.load()
        self.assertEqual({e["router_id"] for e in doc["routers"].values()}, {"R1", "R3", "R4"})
        kev = doc["routers"]["r4-kev-0.8b"]
        self.assertEqual(kev["runtime"], "kev-torch")
        self.assertEqual(kev["devices"], ["cpu"])

    def test_refusals(self):
        def bad(change):
            doc = copy.deepcopy(DOC)
            change(doc)
            with self.assertRaises(dm.RegistryError):
                dm.validate(doc)

        bad(lambda d: d["routers"]["r1-qwen3-4b"].update(extra=1))
        bad(lambda d: d["routers"]["r1-qwen3-4b"]["files"][0].update(path="/abs/x.gguf"))
        bad(lambda d: d["routers"]["r1-qwen3-4b"]["files"][0].update(path="../x.gguf"))
        bad(lambda d: d["routers"]["r1-qwen3-4b"]["files"][0].update(sha256="abc"))
        bad(lambda d: d["routers"]["r1-qwen3-4b"].update(image="mimik-decision-anyjev:20261015"))
        bad(
            lambda d: d["routers"]["r1-qwen3-4b"]["files"].append(
                dict(d["routers"]["r1-qwen3-4b"]["files"][0], role="second")
            )
        )
        bad(
            lambda d: d["routers"]["r4-kev-0.8b"].update(
                files=[f for f in d["routers"]["r4-kev-0.8b"]["files"] if f["role"] != "head"]
            )
        )
        bad(lambda d: d["routers"]["r4-kev-0.8b"].update(kev_checkpoint="elsewhere/ckpt"))
        bad(lambda d: d["routers"]["r1-qwen3-4b"].update(devices=["tpu"]))
        bad(lambda d: d["routers"]["r3-qwen35-9b"].update(router_id="R1"))
        bad(lambda d: d["routers"]["r1-qwen3-4b"].update(seconds_per_decision=0))
        bad(lambda d: d.update(model_root="relative"))

    def test_gguf_hashes_agree_with_the_tool_loop_registry(self):
        tool_loop = model_registry.load()["models"]
        for key, tl_key in (("r1-qwen3-4b", "qwen3-4b-2507"), ("r3-qwen35-9b", "qwen35-9b")):
            gguf = DOC["routers"][key]["files"][0]
            self.assertEqual(gguf["sha256"], tool_loop[tl_key]["sha256"])
            self.assertEqual(gguf["size"], tool_loop[tl_key]["size"])

    def test_kev_hashes_equal_the_worker_literals(self):
        import local_decision_benchmark as bench

        files = {f["role"]: f["sha256"] for f in DOC["routers"]["r4-kev-0.8b"]["files"]}
        self.assertEqual(files["adapter"], bench.KEV_ARTEFACT_SHA256["adapter_model.safetensors"])
        self.assertEqual(files["head"], bench.KEV_ARTEFACT_SHA256["head.pt"])
        self.assertEqual(files["base_weights"], bench.KEV_BASE_WEIGHTS_SHA256)

    def test_models_json_consumers_untouched(self):
        source = Path(model_registry.__file__).read_text(encoding="utf-8")
        self.assertNotIn("decision_models", source)
        self.assertNotIn("routers", json.loads(model_registry.REGISTRY.read_text())["models"])

    def test_shell_assignments_evaluate_in_bash(self):
        text = dm.shell(dm.load(), "r4-kev-0.8b", env={})
        out = subprocess.run(
            [
                "bash",
                "-c",
                text + '\nprintf "%s|%s|%s\\n" "$dm_runtime" "${#dm_files[@]}" "${dm_files[0]}"',
            ],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        runtime, count, first = out.split("|")
        self.assertEqual(runtime, "kev-torch")
        self.assertEqual(int(count), len(DOC["routers"]["r4-kev-0.8b"]["files"]))
        self.assertEqual(first.split("\t")[0], "adapter")

    def test_check_files_reports_missing_and_size(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = dm.check_files(DOC, "r1-qwen3-4b", env={dm.ROOT_ENV: tmp})
            self.assertTrue(problems and "missing" in problems[0])
            path = Path(tmp) / DOC["routers"]["r1-qwen3-4b"]["files"][0]["path"]
            path.parent.mkdir(parents=True)
            path.write_bytes(b"x")
            self.assertIn(
                "size differs", dm.check_files(DOC, "r1-qwen3-4b", env={dm.ROOT_ENV: tmp})[0]
            )


if __name__ == "__main__":
    unittest.main()
