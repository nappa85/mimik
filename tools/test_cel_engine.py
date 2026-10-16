"""The benchmark's CelCliEngine over the formula_eval CLI (fails closed like test_formula_cli)."""

import json
import unittest
from pathlib import Path

import tool_loop_benchmark as tl
from test_formula_cli import ROOT, binary

PACK = Path(__file__).resolve().parent / "fixtures" / "tool_loop_pack"


class CelCliEngineTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not (ROOT / "examples" / "formula_eval.rs").exists():
            raise unittest.SkipTest("formula_eval is excluded from the source package")
        cls.cli = binary()
        if not cls.cli.exists():
            raise AssertionError(f"build the formula_eval example first: {cls.cli} is missing")

    def definition(self):
        return json.loads((PACK / "definition.json").read_text())

    def test_derive_violations_and_provenance(self):
        engine = tl.CelCliEngine(self.definition(), self.cli)
        try:
            values = json.loads((PACK / "characters" / "hero.json").read_text())["values"]
            base = {k: v for k, v in values.items() if k not in ("armor", "pace")}
            derived = engine.derive(base)
            self.assertEqual((derived["armor"], derived["pace"]), (1, 5))
            self.assertEqual(engine.violations(derived), [])
            both = dict(
                base, focus=9, equipment=[dict(e, equipped=True) for e in base["equipment"]]
            )
            found = engine.violations(both)
            self.assertEqual(
                [(v["constraint_id"], v["status"]) for v in found],
                [("focus-range", "violated"), ("one-outer-armor", "violated")],
            )
            self.assertEqual(found[0]["rule_ids"], ["FX-1"])
            self.assertEqual(engine.provenance["version"]["profile"], "cel-mimik-1")
            self.assertEqual(engine.provenance["cli_sha256"], tl.file_digest(self.cli))
            self.assertEqual(engine.derived_order, ["armor", "pace"])
        finally:
            engine.close()

    def test_rejection_and_input_errors_are_explicit(self):
        bad = self.definition()
        bad["fields"][3]["derived"] = "armor_nope + 1"
        with self.assertRaises(tl.FormulaEvalError) as ctx:
            tl.CelCliEngine(bad, self.cli)
        self.assertEqual(ctx.exception.code, "unknown_field")
        engine = tl.CelCliEngine(self.definition(), self.cli)
        try:
            with self.assertRaises(tl.FormulaEvalError) as ctx:
                engine.derive({"focus": 1})  # missing declared inputs
            self.assertEqual(ctx.exception.code, "invalid_input")
        finally:
            engine.close()

    def test_derived_error_is_explicit(self):
        definition = self.definition()
        definition["fields"][4]["derived"] = "6 / (armor - 1)"  # armor 1 -> division by zero
        engine = tl.CelCliEngine(definition, self.cli)
        try:
            values = json.loads((PACK / "characters" / "hero.json").read_text())["values"]
            base = {k: v for k, v in values.items() if k not in ("armor", "pace")}
            with self.assertRaises(tl.FormulaEvalError) as ctx:
                engine.derive(base)
            self.assertEqual(ctx.exception.code, "division_by_zero")
        finally:
            engine.close()

    def test_killed_cli_mid_run_is_infra_error(self):
        # Review 2026-10-15: CLI faults must fail closed, never reach the model as a rejection.
        engine = tl.CelCliEngine(self.definition(), self.cli)
        try:
            values = json.loads((PACK / "characters" / "hero.json").read_text())["values"]
            base = {k: v for k, v in values.items() if k not in ("armor", "pace")}
            engine.derive(base)
            engine.process.kill()
            engine.process.wait()
            changed = dict(base, focus=2)  # uncached state forces a CLI request
            with self.assertRaises(tl.FormulaInfraError):
                engine.derive(changed)
            with self.assertRaises(tl.FormulaInfraError):
                engine.violations(dict(base, focus=1))
        finally:
            engine.close()

    def test_close_releases_process_and_pipes(self):
        # Regression: close() used to leave the stdout pipe open (ResourceWarning).
        engine = tl.CelCliEngine(self.definition(), self.cli)
        engine.close()
        self.assertIsNotNone(engine.process.poll())
        self.assertTrue(engine.process.stdin.closed)
        self.assertTrue(engine.process.stdout.closed)
        engine.close()  # idempotent


if __name__ == "__main__":
    unittest.main()
