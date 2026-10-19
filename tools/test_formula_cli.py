"""Runs the shared CEL conformance fixture through the Rust `formula_eval` JSON-lines CLI.

The CLI is the single source of truth for formula semantics in Python prototypes. Build it with
`cargo build --locked --example formula_eval` first (CI does);
set MIMIK_FORMULA_EVAL to override the binary path. The example is excluded from the
corresponding-source package, so this test skips only when its source file is absent.
"""

import hashlib
import json
import os
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "schemas" / "formula-conformance-v1.json"
# Must equal FIXTURE_SHA256 in src/definitions/formula/tests.rs (hash-locked golden fixture).
FIXTURE_SHA256 = "5af7ce8f1610ca4a41a2a8ecd6b0f387b60c0989a7ef8287f116d5c925cb6b40"


def binary() -> Path:
    override = os.environ.get("MIMIK_FORMULA_EVAL")
    if override:
        return Path(override)
    # `cargo build --locked --example formula_eval` (default product features). The
    # hash-suffixed files from `cargo test --all-targets` are test harnesses, not the CLI.
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    return target / "debug" / "examples" / "formula_eval"


class FormulaCli:
    def __init__(self, path: Path):
        self.process = subprocess.Popen(
            [str(path)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )

    def call(self, request: dict) -> dict:
        self.process.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError("formula_eval exited")
        return json.loads(line)

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=30)
        self.process.stdout.close()


def merged(base: dict, overrides) -> dict:
    values = dict(base)
    values.update(overrides or {})
    return values


def outcome(cli: FormulaCli, sheet: dict, values: dict) -> dict:
    compiled = cli.call({"op": "compile", "sheet": sheet})
    if "reject" in compiled:
        return {"reject": compiled["reject"]["code"]}
    evaluated = cli.call({"op": "evaluate", "values": values})
    if "input_error" in evaluated:
        return {"input_error": evaluated["input_error"]["code"]}
    result = evaluated["ok"]
    result["order"] = compiled["ok"]["derived_order"]
    return result


class FormulaCliConformance(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not (ROOT / "examples" / "formula_eval.rs").exists():
            raise unittest.SkipTest("formula_eval is excluded from the source package")
        path = binary()
        if not path.exists():
            raise AssertionError(f"build the formula_eval example first: {path} is missing")
        cls.raw = FIXTURE.read_bytes()
        cls.fixture = json.loads(cls.raw)
        cls.cli = FormulaCli(path)

    @classmethod
    def tearDownClass(cls):
        cls.cli.close()

    def test_fixture_is_hash_locked_and_matches_cli(self):
        digest = hashlib.sha256(self.raw).hexdigest()
        self.assertEqual(digest, FIXTURE_SHA256)
        version = self.cli.call({"op": "version"})["ok"]
        self.assertEqual(version["fixture_sha256"], digest)
        self.assertEqual(version["profile"], self.fixture["profile"])

    def test_expressions(self):
        fx = self.fixture
        for case in fx["expressions"]:
            name = case["declarations"]
            sheet = {
                "declarations": fx["declarations"][name],
                "derived": [{"id": "result", "type": case["type"], "expr": case["expr"]}],
                "constraints": [],
            }
            values = merged(fx["base_values"][name], case.get("values"))
            actual = outcome(self.cli, sheet, values)
            if "derived" in actual:
                actual = actual["derived"]["result"]
            with self.subTest(case=case["id"]):
                self.assertEqual(actual, case["expect"])

    def test_sheets(self):
        fx = self.fixture
        for case in fx["sheets"]:
            declarations = case["declarations"]
            if isinstance(declarations, str):
                declarations = fx["declarations"][declarations]
            sheet = {
                "declarations": declarations,
                "derived": case["derived"],
                "constraints": case["constraints"],
            }
            with self.subTest(case=case["id"]):
                self.assertEqual(outcome(self.cli, sheet, case["values"]), case["expect"])

    def test_protocol_errors(self):
        self.assertEqual(self.cli.call({"op": "nope"}), {"error": "bad_request"})
        fresh = FormulaCli(binary())
        try:
            self.assertEqual(
                fresh.call({"op": "evaluate", "values": {}}), {"error": "not_compiled"}
            )
        finally:
            fresh.close()


class FormulaCliPackaging(unittest.TestCase):
    def test_cli_is_excluded_from_source_package(self):
        import tomllib

        from pathspec import PathSpec

        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        policy = PathSpec.from_lines("gitwildmatch", manifest["package"]["include"])
        self.assertFalse(policy.match_file("examples/formula_eval.rs"))
        self.assertTrue(policy.match_file("examples/source_fixture.rs"))
        self.assertTrue(policy.match_file("schemas/formula-conformance-v1.json"))


if __name__ == "__main__":
    unittest.main()
