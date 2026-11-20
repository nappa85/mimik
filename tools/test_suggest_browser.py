"""The suggestions browser scenario imports, parses its arguments and reads names the page
renders (prototype tooling self-test; the scenario itself needs the browser container)."""

import unittest
from pathlib import Path

import suggest_browser as scenario


class HelperTests(unittest.TestCase):
    def test_the_viewports_are_the_plans(self):
        sizes = [(v["width"], v["height"], v["touch"]) for v in scenario.VIEWPORTS]
        self.assertEqual(sizes, [(1280, 720, False), (390, 844, True)])

    def test_the_arguments_need_a_fixture(self):
        with self.assertRaises(SystemExit):
            scenario.main(["target/release/mimik"])

    def test_the_ids_the_scenario_uses_are_the_pages(self):
        # Row cells are `row-<f>-<r>-<p>`; the listbox and its status follow the module's
        # naming (`suggest-<f>-<r>-<p>`), and the new row's input is `row-new-<f>-<p>`.
        path = Path(scenario.__file__)
        source = path.read_text()
        module = (path.parent.parent / "assets" / "sheet-assist.js").read_text()
        self.assertIn("`suggest-${control.id.replace(/^row-/, '')}`", module)
        self.assertIn("id: `${id}-status`", module)
        self.assertIn("#row-new-4-0", source)
        self.assertEqual(scenario.LIST, 4)


if __name__ == "__main__":
    unittest.main()
