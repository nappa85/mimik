"""Decision-router worker changes (docs/decision-router-plan.md §2, §5, §6). Failing-first.

No neural dependency: fake llama_cpp / anyjev / torch / kev modules stand in for the real
libraries, so the worker code paths (question and options from the request, CPU mode, hash
checks, timeouts, logs) run here; the real libraries run only in the decision images.
"""

import contextlib
import hashlib
import io
import json
import os
import sys
import tempfile
import types
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import local_decision_benchmark as bench

QUESTION7 = "Which way should this end?"
CHOICES7 = {
    k: f"text of {k}"
    for k in ("proposal", "answer", "conflict", "stale", "refuse", "ask_rule", "ask_fact")
}


# ---------------------------------------------------------------- fake AnyJev / llama.cpp


def fake_anyjev_modules(
    offload_line="load_tensors: offloaded 0/37 layers to GPU", gpu_build=True, calls=None
):
    calls = calls if calls is not None else {}
    llama_cpp = types.ModuleType("llama_cpp")
    llama_cpp.__version__ = "0.3.36-fake"
    llama_cpp.llama_supports_gpu_offload = lambda: gpu_build

    class Llm:
        metadata = {"general.name": "fake"}

        @staticmethod
        def tokenize(data, add_bos=False, special=True):
            return list(range(len(data.split())))

    class Backend:
        def __init__(self, path, **kwargs):
            calls["backend"] = {"path": path, **kwargs}
            if offload_line:
                os.write(2, (offload_line + "\n").encode())
            self.tokenizer = object()
            self.llm = Llm()

        def close(self):
            calls["closed"] = True

    class Question:
        def __init__(self, text, options):
            self.text, self.options = text, options

        @classmethod
        def choice(cls, text, options, name=None):
            calls.setdefault("questions", []).append((text, list(options)))
            return cls(text, options)

    class Decider:
        system = "SYSTEM"

        def __init__(self, backend, **kwargs):
            self.stats = {"calls": 0}

        def decide_batch(self, states, q):
            self.stats["calls"] += 1
            k = len(q.options)
            probs = [0.0] * k
            probs[0] = 1.0
            return [
                SimpleNamespace(probs=probs, diagnostics={"perms": [list(range(k))]}, level="raw")
            ]

    anyjev = types.ModuleType("anyjev")
    anyjev.Decider, anyjev.Question = Decider, Question
    backends = types.ModuleType("anyjev.backends")
    llamacpp = types.ModuleType("anyjev.backends.llamacpp")
    llamacpp.LlamaCppBackend = Backend
    readout = types.ModuleType("anyjev.readout")
    readout.build_prompt = lambda state, q, perm, system: (
        state,
        q.text,
        [q.options[i] for i in perm],
        system,
    )
    readout.render_chat = (
        lambda tok, spec: f"{spec[3]}\nState: {spec[0]}\nQuestion: {spec[1]}\n" + "\n".join(spec[2])
    )
    return {
        "llama_cpp": llama_cpp,
        "anyjev": anyjev,
        "anyjev.backends": backends,
        "anyjev.backends.llamacpp": llamacpp,
        "anyjev.readout": readout,
    }


def gguf(tmp):
    path = Path(tmp) / "model.gguf"
    path.write_bytes(b"gguf")
    return str(path), hashlib.sha256(b"gguf").hexdigest()


def anyjev_args(tmp, **extra):
    model, sha = gguf(tmp)
    dev = Path(tmp) / "dev"
    dev.mkdir(exist_ok=True)
    base = {"model": model, "model_sha256": sha, "raw_only": True, "dev_root": str(dev)}
    base.update(extra)
    return SimpleNamespace(**base)


class AnyJevWorkerTests(unittest.TestCase):
    def run_worker(self, args, **fake):
        calls = {}
        with patch.dict(sys.modules, fake_anyjev_modules(calls=calls, **fake)):
            predict, close, meta = bench.anyjev_worker(args)
        return predict, close, meta, calls

    def test_requested_question_and_options_reach_the_anyjev_prompt(self):
        """Before: the worker always used the 4-label QUESTION/CHOICES."""
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _, calls = self.run_worker(anyjev_args(tmp, device="cpu"))
            order = list(reversed(CHOICES7))
            out = predict(
                {
                    "state": "s",
                    "method": "raw",
                    "order": order,
                    "question": QUESTION7,
                    "choices": CHOICES7,
                }
            )
            text, options = calls["questions"][-1]
            self.assertEqual(text, QUESTION7)
            self.assertEqual(options, [f"{k}: {CHOICES7[k]}" for k in order])
            self.assertEqual(out["question_sha256"], bench.question_hash(QUESTION7, CHOICES7))
            self.assertEqual(set(out["probabilities"]), set(CHOICES7))

    def test_default_question_is_the_legacy_four_label_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _, calls = self.run_worker(anyjev_args(tmp, device="cpu"))
            predict({"state": "s", "method": "raw", "order": list(bench.CHOICES)})
            self.assertEqual(calls["questions"][-1][0], bench.QUESTION)
            self.assertEqual(
                calls["questions"][-1][1], [f"{k}: {bench.CHOICES[k]}" for k in bench.CHOICES]
            )

    def test_cpu_mode_requests_zero_layers_and_records_provenance(self):
        """Before: the worker refused without Vulkan and always asked for n_gpu_layers=-1."""
        with tempfile.TemporaryDirectory() as tmp:
            _, _, meta, calls = self.run_worker(
                anyjev_args(tmp, device="cpu", threads=6, n_ctx=8192), gpu_build=False
            )
            self.assertEqual(calls["backend"]["n_gpu_layers"], 0)
            self.assertEqual(calls["backend"]["n_threads"], 6)
            self.assertEqual(calls["backend"]["n_ctx"], 8192)
            cpu = meta["device_provenance"]
            self.assertEqual(cpu["device"], "cpu")
            self.assertEqual(cpu["gpu_device_nodes"], [])
            self.assertEqual(cpu["offloaded_layers"], 0)
            self.assertEqual(cpu["n_gpu_layers"], 0)
            self.assertIn("build property", cpu["llama_supports_gpu_offload"])

    def test_gpu_mode_still_requires_vulkan(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, "Vulkan"):
                self.run_worker(anyjev_args(tmp, device="gpu"), gpu_build=False)
            _, _, _, calls = self.run_worker(anyjev_args(tmp, device="gpu"))
            self.assertEqual(calls["backend"]["n_gpu_layers"], -1)

    def test_default_device_is_gpu_as_before(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = anyjev_args(tmp)
            _, _, _, calls = self.run_worker(args)
            self.assertEqual(calls["backend"]["n_gpu_layers"], -1)
            self.assertEqual(calls["backend"]["n_ctx"], 4096)

    def test_cpu_mode_refuses_a_visible_gpu_node(self):
        with tempfile.TemporaryDirectory() as tmp:
            for node in ("dri", "kfd", "nvidia0"):
                args = anyjev_args(tmp, device="cpu")
                (Path(args.dev_root) / node).mkdir()
                with self.assertRaisesRegex(ValueError, "GPU device node"):
                    self.run_worker(args)
                (Path(args.dev_root) / node).rmdir()

    def test_cpu_mode_refuses_an_offloaded_layer(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, "offloaded"):
                self.run_worker(
                    anyjev_args(tmp, device="cpu"),
                    offload_line="load_tensors: offloaded 3/37 layers to GPU",
                )

    def test_cpu_mode_records_an_absent_offload_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, _, meta, _ = self.run_worker(anyjev_args(tmp, device="cpu"), offload_line=None)
            self.assertEqual(meta["device_provenance"]["offloaded_layers"], "absent")

    def test_unknown_device_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, "device"):
                self.run_worker(anyjev_args(tmp, device="tpu"))

    def test_gguf_hash_mismatch_still_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = anyjev_args(tmp, device="cpu", model_sha256="0" * 64)
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                self.run_worker(args)

    def test_over_context_prompt_fails_as_over_context(self):
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _, _ = self.run_worker(anyjev_args(tmp, device="cpu", n_ctx=5))
            with self.assertRaises(bench.OverContext):
                predict(
                    {
                        "state": "a b c d e f g h",
                        "method": "raw",
                        "order": list(CHOICES7),
                        "question": QUESTION7,
                        "choices": CHOICES7,
                    }
                )

    def test_count_only_returns_prompt_tokens_without_a_decision(self):
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _, calls = self.run_worker(anyjev_args(tmp, device="cpu"))
            out = predict.count(
                {
                    "state": "a b c",
                    "order": list(CHOICES7),
                    "question": QUESTION7,
                    "choices": CHOICES7,
                }
            )
            self.assertGreater(out["prompt_tokens"], 3)
            self.assertNotIn("probabilities", out)


# ---------------------------------------------------------------- fake Kev


def fake_kev_modules(calls):
    torch = types.ModuleType("torch")
    torch.set_num_threads = lambda n: calls.setdefault("threads", n)
    torch.set_num_interop_threads = lambda n: None
    torch.float32 = "float32"
    torch.__version__ = "2.8.0-fake"
    api = types.ModuleType("kev.api")

    class Req:
        def __init__(self, state, questions):
            self.state, self.questions = state, questions

    def to_record(req):
        q = req.questions["review"]
        options = [f"{k}: {v}" for k, v in q["criteria"].items()]
        return {
            "state": req.state,
            "questions": [{"instr": q["instructions"], "options": options}],
        }, None

    api.SystemOneRequest, api.to_record = Req, to_record
    checkpoint = types.ModuleType("kev.checkpoint")

    class Model:
        head = SimpleNamespace(temperature=1.0)
        dtype = "float32"
        lm = object()

        def named_modules(self):
            return []

    class Ckpt:
        def __init__(self, path):
            self.meta = SimpleNamespace(base=bench.KEV_BASE, base_revision=bench.KEV_BASE_REVISION)

        def load(self, device, options):
            return object(), Model()

    checkpoint.Checkpoint, checkpoint.LoadOptions = Ckpt, lambda **kw: kw
    model = types.ModuleType("kev.model")
    model.admit = lambda m, tok, rec: {"ids": list(range(len(rec["state"].split()) + 10))}
    serve = types.ModuleType("kev.serve")

    class Server:
        def __init__(self, *a):
            self.prefix_cache = SimpleNamespace(
                hits=0, misses=0, size=1, min_tokens=1, clear=lambda: None
            )

        def probs(self, record):
            k = len(record["questions"][0]["options"])
            return [[1.0 / k] * k], {}

        def close(self):
            pass

    serve.Server = Server
    hub = types.ModuleType("huggingface_hub")
    hub.hf_hub_download = lambda *a, **kw: calls["base_weights"]
    kev = types.ModuleType("kev")
    return {
        "torch": torch,
        "kev": kev,
        "kev.api": api,
        "kev.checkpoint": checkpoint,
        "kev.model": model,
        "kev.serve": serve,
        "huggingface_hub": hub,
    }


class KevWorkerTests(unittest.TestCase):
    def setup_files(self, tmp, good=True):
        ckpt = Path(tmp) / "ckpt"
        ckpt.mkdir()
        hashes = {}
        for name in bench.KEV_ARTEFACT_SHA256:
            data = f"{name}".encode()
            (ckpt / name).write_bytes(data)
            hashes[name] = hashlib.sha256(data).hexdigest()
        base = Path(tmp) / "base.safetensors"
        base.write_bytes(b"base")
        base_sha = hashlib.sha256(b"base").hexdigest()
        if not good:
            return ckpt, base, {}, base_sha
        return ckpt, base, hashes, base_sha

    def worker(self, tmp, good=True, n_ctx=None, artefacts=None):
        calls = {}
        ckpt, base, hashes, base_sha = self.setup_files(tmp, good)
        calls["base_weights"] = str(base)
        args = SimpleNamespace(kev_checkpoint=str(ckpt), n_ctx=n_ctx)
        expected = artefacts if artefacts is not None else (hashes or bench.KEV_ARTEFACT_SHA256)
        with (
            patch.dict(sys.modules, fake_kev_modules(calls)),
            patch.object(bench, "KEV_ARTEFACT_SHA256", expected),
            patch.object(bench, "KEV_BASE_WEIGHTS_SHA256", base_sha if good else "0" * 64),
        ):
            return bench.kev_worker(args)

    def test_hash_literals_unchanged(self):
        self.assertEqual(
            bench.KEV_ARTEFACT_SHA256,
            {
                "adapter_model.safetensors": "9b908623acb162118575f4e7a94524f9c139c335be4bfb74d6cfceca01e1885a",
                "head.pt": "f400bd12802b2b105ae45d6b03774a158a3db4fccff42413734ddca2e5c920b6",
            },
        )
        self.assertEqual(
            bench.KEV_BASE_WEIGHTS_SHA256,
            "c2b1e5a17d9c1e27685d92ed9b382911ebb99955ecd89052d1721241adfbab6c",
        )
        self.assertEqual(
            (bench.KEV_BASE, bench.KEV_BASE_REVISION),
            ("Qwen/Qwen3.5-0.8B-Base", "dc7cdfe2ee4154fa7e30f5b51ca41bfa40174e68"),
        )

    def test_requested_question_reaches_kev_and_hash_returned(self):
        """Before: Kev asserted the record against the module QUESTION/CHOICES."""
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _ = self.worker(tmp)
            order = list(CHOICES7)
            out = predict(
                {"state": "s", "order": order, "question": QUESTION7, "choices": CHOICES7}
            )
            self.assertEqual(out["question_sha256"], bench.question_hash(QUESTION7, CHOICES7))
            self.assertEqual(list(out["probabilities"]), order)

    def test_kev_refuses_a_record_that_differs_from_the_request(self):
        with tempfile.TemporaryDirectory() as tmp:
            # A converter that rewrites the instruction must be refused, not trusted.
            calls = {}
            mods = fake_kev_modules(calls)
            original = mods["kev.api"].to_record

            def tampered(req):
                record, meta = original(req)
                record["questions"][0]["instr"] = "something else"
                return record, meta

            mods["kev.api"].to_record = tampered
            ckpt, base, hashes, base_sha = self.setup_files(tmp)
            calls["base_weights"] = str(base)
            with (
                patch.dict(sys.modules, mods),
                patch.object(bench, "KEV_ARTEFACT_SHA256", hashes),
                patch.object(bench, "KEV_BASE_WEIGHTS_SHA256", base_sha),
            ):
                predict2, _, _ = bench.kev_worker(
                    SimpleNamespace(kev_checkpoint=str(ckpt), n_ctx=None)
                )
                with self.assertRaisesRegex(ValueError, "question"):
                    predict2(
                        {
                            "state": "s",
                            "order": list(CHOICES7),
                            "question": QUESTION7,
                            "choices": CHOICES7,
                        }
                    )

    def test_kev_default_question_is_legacy(self):
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _ = self.worker(tmp)
            out = predict({"state": "s", "order": list(bench.CHOICES)})
            self.assertEqual(
                out["question_sha256"], bench.question_hash(bench.QUESTION, bench.CHOICES)
            )

    def test_kev_artefact_hash_mismatch_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                self.worker(tmp, artefacts=bench.KEV_ARTEFACT_SHA256)

    def test_kev_base_weight_hash_mismatch_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            calls = {}
            ckpt, base, hashes, _ = self.setup_files(tmp)
            calls["base_weights"] = str(base)
            with (
                patch.dict(sys.modules, fake_kev_modules(calls)),
                patch.object(bench, "KEV_ARTEFACT_SHA256", hashes),
            ):
                with self.assertRaisesRegex(ValueError, "Base weight hash mismatch"):
                    bench.kev_worker(SimpleNamespace(kev_checkpoint=str(ckpt), n_ctx=None))

    def test_kev_over_context(self):
        with tempfile.TemporaryDirectory() as tmp:
            predict, _, _ = self.worker(tmp, n_ctx=12)
            with self.assertRaises(bench.OverContext):
                predict(
                    {
                        "state": "a b c d e",
                        "order": list(CHOICES7),
                        "question": QUESTION7,
                        "choices": CHOICES7,
                    }
                )


# ---------------------------------------------------------------- worker loop, timeouts, logs


class WorkerLoopTests(unittest.TestCase):
    def loop(self, predict, requests):
        stdin = io.StringIO("".join(json.dumps(r) + "\n" for r in requests))
        out = io.StringIO()

        def factory(_args):
            return predict, lambda: None, {}

        with patch.object(sys, "stdin", stdin), contextlib.redirect_stdout(out):
            bench.worker(SimpleNamespace(backend="kev"), factory)
        lines = [json.loads(x[len(bench.MARKER) :]) for x in out.getvalue().splitlines()]
        return lines[1:]

    def test_invalid_distribution_has_an_error_class(self):
        """Before: a failed decision carried only {status: failed, error}."""

        def predict(_request):
            return {"probabilities": {"a": 0.6, "b": 0.6}}

        (result,) = self.loop(predict, [{"order": ["a", "b"], "tolerance": 1e-6}])
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["error_class"], "invalid_distribution")
        self.assertNotIn("probabilities", result)

    def test_shared_tolerance_from_the_request(self):
        def predict(_request):
            return {"probabilities": {"a": 0.5, "b": 0.5 + 5e-6}}

        legacy, strict = self.loop(
            predict, [{"order": ["a", "b"]}, {"order": ["a", "b"], "tolerance": 1e-6}]
        )
        self.assertEqual(legacy["status"], "completed")
        self.assertEqual(strict["error_class"], "invalid_distribution")

    def test_over_context_class(self):
        def predict(_request):
            raise bench.OverContext("too long")

        (result,) = self.loop(predict, [{"order": ["a"]}])
        self.assertEqual(result["error_class"], "over_context")

    def test_worker_log_appends_across_restarts(self):
        """Before: Worker opened its log with mode 'w', so a restart truncated it (A4)."""
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "worker.log"
            log.write_text("first start\n")
            script = Path(tmp) / "w.py"
            script.write_text("import sys\nsys.stderr.write('second start\\n')\n")
            args = SimpleNamespace(
                output=Path(tmp) / "out.jsonl",
                worker_log=log,
                worker_script=str(script),
                backend="kev",
                model=None,
                kev_checkpoint=None,
            )
            w = bench.Worker(args)
            w.process.wait(timeout=30)
            w.close()
            self.assertEqual(log.read_text(), "first start\nsecond start\n")


class SlowWorker:
    """Fake native worker: each decision takes 2 s of 'native' time; a smaller timeout kills it."""

    def __init__(self, _args):
        self.alive = True
        self.process = SimpleNamespace(poll=lambda: None if self.alive else -9)
        self.timeouts = []

    def receive(self, timeout):
        return {"ready": {}}

    def predict(self, payload, timeout):
        self.timeouts.append(timeout)
        if timeout < 2:
            self.alive = False
            raise TimeoutError("Native worker exceeded absolute deadline")
        return {
            "status": "completed",
            "probabilities": {k: (0.7 if k == "supported" else 0.1) for k in bench.CHOICES},
            "seconds": 2.0,
            "native": {"prefix_cache_hit": bool(payload.get("clear_cache") is False)},
            "cache_hits_delta": int(payload.get("clear_cache") is False),
            "cache_misses_delta": int(payload.get("clear_cache") is not False),
        }

    def close(self):
        pass


DEMO = Path(__file__).resolve().parents[1] / "examples/sheet-harness-demo"


class TimeoutTests(unittest.TestCase):
    def run_main(self, output, extra):
        argv = ["probe", "--backend", "kev", "--demo", str(DEMO), "--output", str(output), *extra]
        with (
            patch.object(sys, "argv", argv),
            patch.object(bench, "Worker", SlowWorker),
            patch.object(bench, "DEFAULT_TIMEOUT", 1),
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.suppress(RuntimeError),
        ):
            bench.main()
        return [json.loads(x) for x in output.read_text().splitlines() if '"finished"' in x]

    def test_decision_timeout_lets_a_slow_decision_complete(self):
        """Before: the 180 s legacy cap was hard-coded; with it injected as 1 s, a 2 s decision
        timed out and killed the worker, so every later decision became not_run."""
        with tempfile.TemporaryDirectory() as tmp:
            legacy = self.run_main(Path(tmp) / "a.jsonl", [])
            self.assertEqual(legacy[0]["status"], "failed")
            self.assertEqual(legacy[1]["status"], "not_run")
            slow = self.run_main(
                Path(tmp) / "b.jsonl", ["--decision-timeout", "5", "--ready-timeout", "5"]
            )
            self.assertEqual(slow[0]["status"], "completed")
            self.assertEqual(slow[1]["status"], "completed")

    def test_defaults_stay_180(self):
        self.assertEqual(bench.DEFAULT_TIMEOUT, 180)
        self.assertEqual(bench.LEGACY_SUM_TOLERANCE, 1e-5)


if __name__ == "__main__":
    unittest.main()
