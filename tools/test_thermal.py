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


def fan(enable=1, pwm=255, rpm=3200):
    return {
        "pwm1_enable": enable,
        "pwm1": pwm,
        "fan1_input": rpm,
        "path": "/sys/class/hwmon/hwmonX",
    }


class GuardTests(unittest.TestCase):
    """Rules tightened after the 2026-10-16 SW CTF shutdown."""

    def run_samples(self, samples, guard=None):
        guard = guard or ops.ThermalGuard()
        for sample in samples:
            t, sensors = sample[0], sample[1]
            verdict, reason = guard.check(sensors, t, sample[2] if len(sample) > 2 else None)
            if verdict:
                return verdict, reason, t
        return None, None, None

    def test_junction_90_stops_immediately(self):
        self.assertEqual(self.run_samples([(0, gpu(60, 89.9))])[0], None)
        verdict, reason, t = self.run_samples([(0, gpu(60, 70)), (5, gpu(60, 90))])
        self.assertEqual((verdict, t), ("thermal", 5))
        self.assertIn("junction 90.0", reason)

    def test_edge_80_sustained_for_20_s(self):
        samples = [(t, gpu(81, 85)) for t in (0, 5, 10, 15, 20, 25)]
        verdict, reason, t = self.run_samples(samples)
        self.assertEqual((verdict, t), ("thermal", 20))
        self.assertIn("edge 81.0", reason)

    def test_short_edge_spike_does_not_stop(self):
        samples = [(t, gpu(81, 85)) for t in (0, 5, 10, 15)]
        samples += [(20, gpu(70, 75))] + [(t, gpu(81, 85)) for t in (25, 30, 35)]
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
        self.assertEqual(guard.check(gpu(50), 5)[0], "thermal_unknown")  # junction vanished

    def test_edge_stands_in_for_an_absent_junction(self):
        self.assertEqual(self.run_samples([(0, gpu(91))])[0], "thermal")

    def test_pwm_reverting_to_auto_mid_run_is_a_fan_fault(self):
        samples = [(0, gpu(50, 60), fan()), (5, gpu(50, 60), fan(enable=2, pwm=56))]
        samples += [(10, gpu(50, 60), fan(enable=2, pwm=56)), (15, gpu(50, 60), fan(enable=2))]
        verdict, reason, t = self.run_samples(samples)
        self.assertEqual((verdict, t), ("fan_fault", 15))
        self.assertIn("not manual", reason)
        self.assertIn("pwm1_enable", reason)

    def test_short_fan_glitch_does_not_stop(self):
        samples = [(0, gpu(50, 60), fan(pwm=200)), (5, gpu(50, 60), fan(pwm=200))]
        samples += [(10, gpu(50, 60), fan()), (15, gpu(50, 60), fan(pwm=200))]
        self.assertEqual(self.run_samples(samples)[0], None)

    def test_low_rpm_only_counts_while_the_junction_is_warm(self):
        cold = [(t, gpu(45, 55), fan(rpm=1200)) for t in (0, 5, 10, 15)]
        self.assertEqual(self.run_samples(cold)[0], None)
        warm = [(t, gpu(55, 70), fan(rpm=1200)) for t in (0, 5, 10, 15)]
        self.assertEqual(self.run_samples(warm)[:1], ("fan_fault",))

    def test_unreadable_fan_is_a_fault(self):
        unread = {"pwm1_enable": None, "pwm1": None, "fan1_input": None, "path": None}
        samples = [(t, gpu(50, 60), unread) for t in (0, 5, 10)]
        self.assertEqual(self.run_samples(samples)[0], "fan_fault")

    def test_stats_min_max_last(self):
        guard = ops.ThermalGuard()
        for t, edge in ((0, 50), (5, 70), (10, 60)):
            guard.check(gpu(edge, edge), t, fan(rpm=3000 + t))
        self.assertEqual(guard.stats[ops.GPU_EDGE], {"min": 50, "max": 70, "last": 60})
        self.assertEqual(guard.stats["amdgpu:fan1_rpm"]["max"], 3010)

    def test_defaults(self):
        d = ops.THERMAL_DEFAULTS
        self.assertEqual(
            (d["junction_stop_c"], d["edge_c"], d["edge_sustain_seconds"]), (90, 80, 20)
        )
        self.assertEqual(
            (d["fan_pwm_min"], d["fan_rpm_min"], d["fan_fault_seconds"]), (230, 2000, 10)
        )
        self.assertEqual(d["sample_seconds"], 5)


class FanTests(unittest.TestCase):
    def tree(self, tmp, enable=1, pwm=255, rpm=3200):
        root = make_hwmon(Path(tmp), [(4, "amdgpu", {1: ("edge", 44, 100)}), (1, "nvme", {})])
        chip = root / "hwmon4"
        for key, value in (("pwm1_enable", enable), ("pwm1", pwm), ("fan1_input", rpm)):
            if value is not None:
                (chip / key).write_text(f"{value}\n")
        return root

    def test_read_fan_by_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self.tree(tmp)
            self.assertEqual(
                ops.read_fan(root),
                {"pwm1_enable": 1, "pwm1": 255, "fan1_input": 3200, "path": str(root / "hwmon4")},
            )

    def test_precondition(self):
        self.assertIsNone(ops.fan_precondition(fan()))
        for bad in (fan(enable=2), fan(pwm=229), fan(enable=None, pwm=None)):
            with self.subTest(bad=bad):
                message = ops.fan_precondition(bad)
                self.assertIn("sudo tee", message)
                self.assertIn("pwm1_enable", message)
        self.assertIsNone(ops.fan_precondition(fan(pwm=200), {"fan_pwm_min": 150}))

    def test_fancheck_cli(self):
        with tempfile.TemporaryDirectory() as tmp:
            for enable, code in ((1, 0), (2, 1)):
                root = self.tree(Path(tmp) / str(enable), enable=enable)
                env = {
                    "PATH": os.environ.get("PATH", ""),
                    "MIMIK_TEST_MODE": "1",
                    "MIMIK_TEST_HWMON": str(root),
                }
                proc = subprocess.run(
                    [sys.executable, str(TOOLS / "toolloop_ops.py"), "fancheck"],
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(proc.returncode, code, proc.stderr)
                self.assertIn(f"fan_pwm1_enable={enable}", proc.stdout)
                if code:
                    self.assertIn(str(root / "hwmon4") + "/pwm1_enable", proc.stderr)


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
