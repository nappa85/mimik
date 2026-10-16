"""Model registry (tools/models.json): schema check, root resolution, server args.

Failing-first: run before tools/model_registry.py existed (ModuleNotFoundError).
"""

import copy
import json
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import model_registry as mr

TOOLS = Path(__file__).resolve().parent
# The vector tools/run_tool_loop_benchmark.sh built at 2566040 for the 9B at ctx 16384.
LEGACY_9B_ARGS = (
    "--model /models/model.gguf --alias qwen3.5-9b-q4_k_m --host 0.0.0.0 --port 8080 --offline "
    "--no-webui --no-slots --no-mmproj --jinja --n-gpu-layers 99 --fit off --threads 6 "
    "--ctx-size 16384 --parallel 1 --no-warmup --cache-ram 0"
).split()
LEGACY = {
    "qwen35-9b": (
        "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8",
        "qwen3.5-9b-q4_k_m",
        "target/model-fetch-20261015/Qwen3.5-9B-Q4_K_M.gguf",
        "14g",
    ),
    "qwen3-4b-2507": (
        "2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e",
        "qwen3-4b-instruct-2507-q4_k_m",
        "target/local-rag-benchmark-20261015/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        "14g",
    ),
    "qwen36-35b-a3b": (
        "671e47e0ec53c665d048b98c3ecbfd5236b5ca9c3e02ed19fc8f81f7b85140c7",
        "qwen3.6-35b-a3b-q4_k_m",
        "target/model-fetch-20261015/Qwen3.6-35B-A3B-Q4_K_M.gguf",
        "26g",
    ),
}
CANDIDATES = {
    "minicpm5-2b": "ec2d5801640099e97d8d7e8003ad4d81f336e757811f03a26173dddf386602fd",
    "lfm25-2.6b": "02a8b7e17487d326e46d68ce0ba24211e1b80a14c4cd0597fa73c1cd697f52ed",
    "lfm25-8b-a1b": "4923ec14f06b968b74d663e5949867d2d9c3bf13a20b8be1a9f9af39989b2bb0",
    "granite42-3b": "e0406663965846ae22a403456eb826ccce5f450840491f71952f18a7cb78e7d5",
    "granite42-8b": "16a9369d0805f80b7377d25d87f937a90c05dc04ad79173a52001e42c9aab311",
    "llama31-8b": "7b064f5842bf9532c91456deda288a1b672397a54fa729aa665952863033557c",
    "ministral3-8b": "33e7a72cf5e6e2cfc2f2847075acc013d68bba023e35310cef86b5cf8fdca761",
    "gemma4-e4b": "a555b900214b477d8880e7832e0b8925e139b0159640036b09fe472b6f2097f2",
}


def registry():
    return json.loads((TOOLS / "models.json").read_text(encoding="utf-8"))


class RegistryFileTests(unittest.TestCase):
    def test_committed_registry_validates(self):
        doc = mr.validate(registry())
        self.assertEqual(set(doc["models"]), set(LEGACY) | set(CANDIDATES))

    def test_legacy_models_keep_the_runner_hashes_and_settings(self):
        """Same sha256, label, path, memory, ctx, request timeout and health wait as the
        hard-coded case statement of tools/run_tool_loop_benchmark.sh at 2566040."""
        models = registry()["models"]
        for key, (sha, label, path, memory) in LEGACY.items():
            with self.subTest(key=key):
                m = models[key]
                self.assertEqual((m["sha256"], m["label"], m["path"]), (sha, label, path))
                self.assertEqual(m["memory_limit"], memory)
                self.assertEqual(m["server"]["ctx"], 16384)
                self.assertEqual(
                    (m["request_timeout_seconds"], m["health_wait_seconds"]), (180, 900)
                )
                self.assertTrue(m["downloaded"])
        self.assertTrue(models["qwen36-35b-a3b"]["server"]["n_cpu_moe_required"])

    def test_candidates_are_registered_and_download_state_is_pinned(self):
        models = registry()["models"]
        for key, sha in CANDIDATES.items():
            with self.subTest(key=key):
                self.assertEqual(models[key]["sha256"], sha)
                self.assertTrue(models[key]["path"].startswith(f"{key}/"))
                revision = models[key]["source"]["revision"]
                if models[key]["downloaded"]:
                    # A downloaded model is pinned to a full commit, never a prefix.
                    self.assertRegex(revision, r"^[0-9a-f]{40}$")
                else:
                    self.assertRegex(revision, r"^[0-9a-f]{12,40}$")
        for key in ("lfm25-2.6b", "lfm25-8b-a1b"):
            self.assertEqual(models[key]["capabilities"]["thinking"], "always")
        for key in ("granite42-8b", "llama31-8b", "ministral3-8b"):
            server = models[key]["server"]
            self.assertEqual((server["cache_type_k"], server["cache_type_v"]), ("q8_0", "q8_0"))
            self.assertEqual(server["ctx"], 24576)

    def test_vram_budget_recorded_for_every_model(self):
        """Review M6: a per-model VRAM budget (estimate) with its source."""
        for key, m in registry()["models"].items():
            with self.subTest(key=key):
                self.assertGreater(m["vram_budget_bytes"], 0)
                self.assertTrue(m["vram_budget_source"])

    def test_plain_extra_args_still_allowed(self):
        doc = copy.deepcopy(registry())
        doc["models"]["qwen35-9b"]["server"]["extra_args"] = ["--mlock", "--batch-size", "512"]
        mr.validate(doc)

    def test_legacy_server_args_unchanged(self):
        m = mr.validate(registry())["models"]["qwen35-9b"]
        self.assertEqual(mr.server_args(m, ctx=16384), LEGACY_9B_ARGS)


class ValidationTests(unittest.TestCase):
    def mutated(self, change):
        doc = copy.deepcopy(registry())
        change(doc["models"]["qwen35-9b"])
        return doc

    def assert_refused(self, change, needle):
        with self.assertRaises(mr.RegistryError) as raised:
            mr.validate(self.mutated(change))
        self.assertIn(needle, str(raised.exception))

    def test_refusals(self):
        cases = {
            "sha": (lambda m: m.update(sha256="ABC"), "sha256"),
            "size": (lambda m: m.update(size=0), "size"),
            "absolute path": (lambda m: m.update(path="/etc/x.gguf"), "path"),
            "dotdot": (lambda m: m.update(path="../x.gguf"), "path"),
            "image tag": (lambda m: m.update(server_image="llama.cpp:latest"), "server_image"),
            "unknown key": (lambda m: m.update(colour="red"), "colour"),
            "missing key": (lambda m: m.pop("licence"), "licence"),
            "thinking": (lambda m: m["capabilities"].update(thinking="maybe"), "thinking"),
            "tools": (lambda m: m["capabilities"].update(tools=True), "tools"),
            "ctx": (lambda m: m["server"].update(ctx=100), "ctx"),
            "cache type": (lambda m: m["server"].update(cache_type_k="q9"), "cache_type_k"),
            "quantised v without fa": (
                lambda m: m["server"].update(cache_type_v="q8_0"),
                "flash_attn",
            ),
            "managed extra arg": (
                lambda m: m["server"].update(extra_args=["--ctx-size", "4"]),
                "extra_args",
            ),
            "host override": (lambda m: m["server"].update(extra_args=["--host=0"]), "extra_args"),
            # Review M3: sampling, prompt-cache and generation overrides are runner/harness owned.
            "temp": (lambda m: m["server"].update(extra_args=["--temp", "1"]), "extra_args"),
            "cache prompt": (
                lambda m: m["server"].update(extra_args=["--no-cache-prompt"]),
                "extra_args",
            ),
            "override kv": (
                lambda m: m["server"].update(extra_args=["--override-kv", "a=int:1"]),
                "extra_args",
            ),
            "samplers": (
                lambda m: m["server"].update(extra_args=["--samplers", "top_k"]),
                "extra_args",
            ),
            "seed": (lambda m: m["server"].update(extra_args=["-s", "1"]), "extra_args"),
            "single dash": (lambda m: m["server"].update(extra_args=["-x"]), "extra_args"),
            "no vram budget": (lambda m: m.pop("vram_budget_bytes"), "vram_budget_bytes"),
            "vram budget zero": (lambda m: m.update(vram_budget_bytes=0), "vram_budget_bytes"),
            "memory": (lambda m: m.update(memory_limit="lots"), "memory_limit"),
            "short revision downloaded": (
                lambda m: m["source"].update(revision="3885219b6810"),
                "revision",
            ),
            "bool as int": (lambda m: m["server"].update(threads=True), "threads"),
            "sampling": (
                lambda m: m.update(recommended_sampling={"temperature": 1.0}),
                "recommended_sampling",
            ),
            "spe": (lambda m: m.update(seconds_per_episode=0), "seconds_per_episode"),
        }
        for name, (change, needle) in cases.items():
            with self.subTest(name=name):
                self.assert_refused(change, needle)

    def test_bad_key_and_root(self):
        doc = copy.deepcopy(registry())
        doc["models"]["Bad Key"] = doc["models"]["qwen35-9b"]
        with self.assertRaises(mr.RegistryError):
            mr.validate(doc)
        doc = copy.deepcopy(registry())
        doc["model_root"] = "relative/dir"
        with self.assertRaises(mr.RegistryError):
            mr.validate(doc)


class ResolveTests(unittest.TestCase):
    def test_root_override_and_default(self):
        doc = mr.validate(registry())
        default = mr.model_file(doc, "qwen35-9b", env={})
        self.assertEqual(
            default,
            Path("/media/data/mimik/models/target/model-fetch-20261015/Qwen3.5-9B-Q4_K_M.gguf"),
        )
        other = mr.model_file(doc, "qwen35-9b", env={"MIMIK_MODEL_ROOT": "/home/x/mimik"})
        self.assertEqual(
            other, Path("/home/x/mimik/target/model-fetch-20261015/Qwen3.5-9B-Q4_K_M.gguf")
        )
        with self.assertRaises(mr.RegistryError):
            mr.model_file(doc, "qwen35-9b", env={"MIMIK_MODEL_ROOT": "rel"})
        with self.assertRaises(mr.RegistryError):
            mr.model_file(doc, "nope", env={})

    def test_server_args_ctx_kv_and_moe(self):
        """ctx must reach the server; KV cache type, flash attention and n_cpu_moe too."""
        doc = mr.validate(registry())
        args = mr.server_args(doc["models"]["granite42-8b"], ctx=24576)
        joined = " ".join(args)
        self.assertIn("--ctx-size 24576", joined)
        self.assertIn("--cache-type-k q8_0 --cache-type-v q8_0", joined)
        self.assertIn("--flash-attn on", joined)
        moe = doc["models"]["qwen36-35b-a3b"]
        with self.assertRaises(mr.RegistryError):
            mr.server_args(moe, ctx=16384)  # n_cpu_moe required
        self.assertIn("--n-cpu-moe 32", " ".join(mr.server_args(moe, ctx=16384, n_cpu_moe=32)))
        with self.assertRaises(mr.RegistryError):
            mr.server_args(moe, ctx=0, n_cpu_moe=32)


class ShellExportTests(unittest.TestCase):
    def run_cli(self, *argv, env=None):
        return subprocess.run(
            [sys.executable, str(TOOLS / "model_registry.py"), *argv],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )

    def test_shell_export_round_trips_through_bash(self):
        env = {"MIMIK_MODEL_ROOT": "/srv/m o'dels", "PATH": "/usr/bin:/bin"}
        proc = self.run_cli(
            "shell", "qwen36-35b-a3b", "--ctx", "24576", "--n-cpu-moe", "30", env=env
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        script = proc.stdout + '\nprintf "%s\\n" "$reg_path" "$reg_ctx" "${reg_server_args[@]}"'
        out = subprocess.run(["bash", "-c", script], capture_output=True, text=True, check=True)
        lines = out.stdout.splitlines()
        self.assertEqual(
            lines[0], "/srv/m o'dels/target/model-fetch-20261015/Qwen3.6-35B-A3B-Q4_K_M.gguf"
        )
        self.assertEqual(lines[1], "24576")
        self.assertIn("--n-cpu-moe", lines)
        self.assertEqual(lines[lines.index("--ctx-size") + 1], "24576")
        config = json.loads(
            shlex.split(proc.stdout.split("reg_server_config=", 1)[1].splitlines()[0])[0]
        )
        self.assertEqual(config["image"], registry()["models"]["qwen36-35b-a3b"]["server_image"])
        self.assertEqual(config["args"][config["args"].index("--ctx-size") + 1], "24576")

    def test_shell_refuses_not_downloaded_and_unknown(self):
        proc = self.run_cli("shell", "llama31-8b")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("not downloaded", proc.stderr)
        self.assertEqual(proc.stdout, "")
        self.assertNotEqual(self.run_cli("shell", "nope").returncode, 0)

    def test_check_command_and_bad_file(self):
        self.assertEqual(self.run_cli("check").returncode, 0)
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "m.json"
            bad.write_text("{}")
            proc = self.run_cli("--registry", str(bad), "check")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("schema", proc.stderr)


if __name__ == "__main__":
    unittest.main()
