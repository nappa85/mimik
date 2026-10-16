"""Vocab-only token measurement (decision-router plan §2, §6 "Tokens"). New module."""

import types
import unittest

import decision_router as dr
import router_tokens as rt


class FakeLlama:
    calls = []

    def __init__(self, **kwargs):
        FakeLlama.calls.append(kwargs)


class TokenTests(unittest.TestCase):
    def test_gguf_opened_vocabulary_only_on_no_gpu(self):
        FakeLlama.calls = []
        rt.open_vocab("/m.gguf", types.SimpleNamespace(Llama=FakeLlama))
        self.assertEqual(
            FakeLlama.calls,
            [{"model_path": "/m.gguf", "vocab_only": True, "n_gpu_layers": 0, "verbose": False}],
        )

    def test_weight_loads_refused_and_restored(self):
        torch = types.ModuleType("torch")
        torch.load = lambda *a, **k: "weights"
        st = types.ModuleType("safetensors.torch")
        st.load_file = lambda *a, **k: "weights"

        class PreTrainedModel:
            @classmethod
            def from_pretrained(cls, *a, **k):
                return "model"

        transformers = types.ModuleType("transformers")
        transformers.PreTrainedModel = PreTrainedModel
        modules = {"torch": torch, "safetensors.torch": st, "transformers": transformers}
        with rt.refuse_weight_loads(modules):
            for call in (
                lambda: torch.load("x"),
                lambda: st.load_file("x"),
                lambda: PreTrainedModel.from_pretrained("x"),
            ):
                with self.assertRaises(rt.WeightLoadRefused):
                    call()
        self.assertEqual(torch.load("x"), "weights")
        self.assertEqual(PreTrainedModel.from_pretrained("x"), "model")

    def test_measurement_covers_every_state_variant_and_reports_over_n_ctx(self):
        states = {
            f"k{n:03d}": {"key": f"k{n:03d}", "state": "x" * (n + 1), "request_only": "{}"}
            for n in range(174)
        }
        seen = []

        def render(state, order):
            seen.append((state, tuple(order)))
            return f"{state}|{order}", len(state) + 100

        report = rt.measure(states, render, n_ctx=200)
        self.assertEqual(len(seen), 174 * 4)
        full = report["distributions"]["full/canonical"]
        self.assertEqual(
            (full["n"], full["min"], full["max"], full["over_n_ctx"]), (174, 101, 274, 74)
        )
        self.assertEqual(len(report["per_key_full_canonical"]), 174)
        self.assertIn((states["k000"]["state"], tuple(reversed(dr.CANONICAL))), seen)
        self.assertIn(("", tuple(dr.CANONICAL)), seen)

    def test_sentinel_scan_counts_hits(self):
        states = {"a": {"key": "a", "state": "clean", "request_only": dr.SENTINEL + "x"}}
        result = rt.sentinel_scan(states, lambda s, o: (s, 1))
        self.assertEqual(result["rendered_prompts"], 6)
        self.assertEqual(
            result["sentinel_hits"], ["a:request_only/canonical", "a:request_only/reversed"]
        )


if __name__ == "__main__":
    unittest.main()
