"""The htmx SSE probe tests the policy the pages carry (prototype tooling self-test; the probe
itself needs the browser container)."""

import unittest
from pathlib import Path

import htmx_sse_probe as probe


class ProbeTests(unittest.TestCase):
    def test_the_probe_uses_the_sheet_policy_of_the_source(self):
        source = (Path(probe.__file__).parent.parent / "src/campaigns/web.rs").read_text()
        self.assertEqual(probe.rust_policy(source), probe.SHEET_CSP)

    def test_the_probe_names_the_pinned_extension(self):
        self.assertEqual(len(probe.SSE_SHA256), 64)


if __name__ == "__main__":
    unittest.main()
