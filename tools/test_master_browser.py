"""The Master browser scenario imports, parses its arguments and keeps the policy and the
module's names in step with the sources (prototype tooling self-test; the scenario itself needs
the browser container)."""

import unittest
from pathlib import Path

import htmx_sse_probe as probe
import master_browser as scenario

ROOT = Path(scenario.__file__).parent.parent


class HelperTests(unittest.TestCase):
    def test_the_viewports_are_the_plans(self):
        sizes = [(v["width"], v["height"], v["touch"]) for v in scenario.VIEWPORTS]
        self.assertEqual(sizes, [(1280, 720, False), (390, 844, True)])

    def test_the_arguments_need_a_fixture(self):
        with self.assertRaises(SystemExit):
            scenario.main(["target/release/mimik"])

    def test_the_strict_policy_is_the_sources(self):
        source = (ROOT / "src/campaigns/web.rs").read_text()
        self.assertEqual(probe.rust_policy(source), scenario.STRICT_CSP)

    def test_the_states_are_the_modules(self):
        module = (ROOT / "assets/master-live.js").read_text()
        for state in scenario.STATUS:
            self.assertIn(f"'{state}'", module, state)

    def test_the_scenario_names_the_rendered_ids(self):
        template = (ROOT / "templates/master.html").read_text()
        for needle in ("players-rows", "npcs-rows", "live-status", "reload", "data-id"):
            self.assertIn(needle, template)


if __name__ == "__main__":
    unittest.main()
