"""Thermal guard (tools/toolloop_ops.py): fake sysfs trees and a fake clock.

Failing-first: run before the sampler and the guard existed (AttributeError).
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import toolloop_ops as ops

TOOLS = Path(__file__).resolve().parent


def make_hwmon(root: Path, chips):
    """chips: [(hwmon index, name, {n: (label or None, °C or "bad", crit or None)})]."""
    for index, name, temps in chips:
        chip = root / f"hwmon{index}"
        chip.mkdir(parents=True)
        (chip / "name").write_text(name + "\n")
        for n, (label, value, crit) in temps.items():
            if value == "bad":
                (chip / f"temp{n}_input").mkdir()  # reading it fails
            else:
                (chip / f"temp{n}_input").write_text(f"{int(value * 1000)}\n")
            if label:
                (chip / f"temp{n}_label").write_text(label + "\n")
            if crit is not None:
                (chip / f"temp{n}_crit").write_text(f"{int(crit * 1000)}\n")
    return root


def gpu(edge, junction=None, crit_edge=100, crit_junction=110):
    sensors = {ops.GPU_EDGE: {"value": edge, "crit": crit_edge}}
    if junction is not None or junction == "absent":
        sensors[ops.GPU_JUNCTION] = {"value": junction, "crit": crit_junction}
    return sensors


class FakeClock:
    def __init__(self, now=0.0):
        self.now = now

    def time(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class SensorTests(unittest.TestCase):
    def test_discovered_by_name_not_index(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_hwmon(
                Path(tmp),
                [
                    (5, "nvme", {1: ("Composite", 38.85, 84.85)}),
                    (0, "amdgpu", {1: ("edge", 44, 100), 2: ("junction", 50, 110)}),
                    (3, "acpitz", {1: (None, 16.8, None)}),
                ],
            )
            sensors = ops.read_sensors(root)
        self.assertEqual(
            sorted(sensors), ["acpitz:temp1", "amdgpu:edge", "amdgpu:junction", "nvme:Composite"]
        )
        self.assertEqual(sensors["amdgpu:junction"], {"value": 50.0, "crit": 110.0})
        self.assertEqual(sensors["nvme:Composite"]["crit"], 84.85)

    def test_unreadable_input_is_none_not_cool(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_hwmon(Path(tmp), [(1, "amdgpu", {1: ("edge", "bad", 100)})])
            self.assertIsNone(ops.read_sensors(root)[ops.GPU_EDGE]["value"])
            self.assertEqual(ops.read_sensors(Path(tmp) / "missing"), {})

    def test_temps_cli_for_the_runner(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_hwmon(Path(tmp) / "hw", [(1, "amdgpu", {1: ("edge", 44, 100)})])
            peak = Path(tmp) / "peak.json"
            env = {
                "PATH": os.environ.get("PATH", ""),
                "MIMIK_TEST_MODE": "1",
                "MIMIK_TEST_HWMON": str(root),
            }
            cmd = [sys.executable, str(TOOLS / "toolloop_ops.py"), "temps"]
            out = subprocess.run(
                cmd + ["--prefix", "before", "--peak-file", str(peak)],
                env=env,
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            self.assertEqual(out.strip(), "temp_amdgpu_edge_before=44.0")
            (root / "hwmon1" / "temp1_input").write_text("71000\n")
            subprocess.run(
                cmd + ["--peak-file", str(peak)], env=env, check=True, capture_output=True
            )
            (root / "hwmon1" / "temp1_input").write_text("50000\n")
            out = subprocess.run(
                cmd + ["--peak-file", str(peak), "--print-peaks"],
                env=env,
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            self.assertEqual(out.strip(), "temp_amdgpu_edge_peak=71.0")
            self.assertEqual(json.loads(peak.read_text()), {"amdgpu:edge": 71.0})
            # Without MIMIK_TEST_MODE the test tree is ignored (the real /sys is read).
            self.assertEqual(ops.hwmon_root({"MIMIK_TEST_HWMON": str(root)}), ops.HWMON)


class GuardTests(unittest.TestCase):
    def run_samples(self, samples, guard=None):
        guard = guard or ops.ThermalGuard()
        for t, sensors in samples:
            verdict, reason = guard.check(sensors, t)
            if verdict:
                return verdict, reason, t
        return None, None, None

    def test_sustained_over_the_limit_stops_at_60_s(self):
        verdict, reason, t = self.run_samples(
            [(0, gpu(70, 96)), (30, gpu(70, 96)), (60, gpu(70, 97)), (90, gpu(70, 97))]
        )
        self.assertEqual((verdict, t), ("thermal", 60))
        self.assertIn("junction 97.0", reason)
        verdict, _, t = self.run_samples([(0, gpu(89, 90)), (30, gpu(89, 90)), (60, gpu(89, 90))])
        self.assertEqual((verdict, t), ("thermal", 60))

    def test_short_spike_does_not_stop(self):
        samples = [(0, gpu(89, 90)), (30, gpu(89, 96)), (60, gpu(70, 80)), (90, gpu(89, 90))]
        samples += [(120, gpu(70, 80))]
        self.assertEqual(self.run_samples(samples)[0], None)

    def test_crit_proximity_stops_at_once(self):
        verdict, reason, t = self.run_samples([(0, gpu(70, 105))])  # junction crit 110 - 5
        self.assertEqual((verdict, t), ("thermal", 0))
        self.assertIn("crit", reason)
        nvme = dict(gpu(50, 50), **{"nvme:Composite": {"value": 80.0, "crit": 84.85}})
        self.assertEqual(self.run_samples([(0, nvme)])[0], "thermal")

    def test_missing_or_unreadable_gpu_sensor(self):
        self.assertEqual(self.run_samples([(0, {})])[0], "thermal_unknown")
        self.assertEqual(self.run_samples([(0, gpu(None, 50))])[0], "thermal_unknown")
        guard = ops.ThermalGuard()
        self.assertEqual(guard.check(gpu(50, 50), 0)[0], None)
        self.assertEqual(guard.check(gpu(50), 30)[0], "thermal_unknown")  # junction vanished

    def test_edge_stands_in_for_an_absent_junction(self):
        guard = ops.ThermalGuard({"edge_c": 200})  # only the junction rule can fire
        samples = [(t, gpu(96)) for t in (0, 30, 60)]
        self.assertEqual(self.run_samples(samples, guard)[0], "thermal")

    def test_stats_min_max_last(self):
        guard = ops.ThermalGuard()
        for t, edge in ((0, 50), (30, 70), (60, 60)):
            guard.check(gpu(edge, edge), t)
        self.assertEqual(guard.stats[ops.GPU_EDGE], {"min": 50, "max": 70, "last": 60})


class CooldownTests(unittest.TestCase):
    def test_holds_for_five_minutes_and_resets_on_unreadable(self):
        clock = FakeClock()

        def read():
            t = clock.now
            if t < 300:
                return gpu(70, 70)
            if 450 <= t < 465:
                return gpu(None, 50)  # unreadable: resets the hold
            return gpu(55, 55)

        self.assertTrue(ops.cooled(read, clock))
        self.assertGreaterEqual(clock.now, 465 + 300)

    def test_cap(self):
        clock = FakeClock()
        logged = []
        self.assertFalse(ops.cooled(lambda: gpu(70, 70), clock, log=logged.append))
        self.assertGreaterEqual(clock.now, 2700)
        self.assertLess(clock.now, 2700 + 30)
        self.assertEqual(logged, [70])


if __name__ == "__main__":
    unittest.main()
