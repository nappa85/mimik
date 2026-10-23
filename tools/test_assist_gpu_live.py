import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

import assist_gpu_live as live
import toolloop_ops as ops

EDGE, JUNCTION = ops.GPU_EDGE, ops.GPU_JUNCTION


def sensors(junction=60.0, edge=50.0):
    return {
        EDGE: {"value": edge, "crit": 100.0},
        JUNCTION: {"value": junction, "crit": 110.0},
    }


MANUAL = {"pwm1_enable": 1, "pwm1": 255, "fan1_input": 3200, "path": "/sys/class/hwmon/hwmon4"}
AUTO = dict(MANUAL, pwm1_enable=2, pwm1=60)


class Clock:
    def __init__(self):
        self.now = 0.0

    def time(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


def sleeper(seconds):
    return subprocess.Popen(
        [sys.executable, "-c", f"import time; time.sleep({seconds})"],
        start_new_session=True,
    )


class RefusalTests(unittest.TestCase):
    def test_weekday_hours_or_an_automatic_fan_refuse_the_run(self):
        with mock.patch.object(ops, "heavy_allowed", return_value=False):
            self.assertIn("not allowed now", live.refusal(0.0, MANUAL))
        with mock.patch.object(ops, "heavy_allowed", return_value=True):
            self.assertIn("not in manual mode", live.refusal(0.0, AUTO))
            self.assertIn("not in manual mode", live.refusal(0.0, dict(MANUAL, pwm1=200)))
            self.assertIsNone(live.refusal(0.0, MANUAL))

    def test_main_refuses_before_starting_anything(self):
        with (
            mock.patch.object(ops, "heavy_allowed", return_value=True),
            mock.patch.object(ops, "read_fan", return_value=AUTO),
            mock.patch.object(subprocess, "Popen") as popen,
        ):
            code = live.main(
                [
                    *("--binary", "b", "--runtime", "r", "--models", "m"),
                    *("--scratch", "s", "--fake-server", "f", "--log", "/nonexistent/x"),
                ]
            )
        self.assertEqual(code, 2)
        popen.assert_not_called()

    def test_the_tests_get_the_gpu_flag_and_only_the_gpu_filter(self):
        args = live.argparse.Namespace(
            binary=Path("/b"),
            runtime=Path("/r/llama"),
            models=Path("/m"),
            scratch=Path("/s"),
            fake_server=Path("/f"),
            test=None,
        )
        self.assertEqual(live.command(args)[1], live.FILTER)
        env = live.environment(args, {})
        self.assertEqual(env["MIMIK_LIVE_GPU"], "1")
        self.assertEqual(env["MIMIK_LIVE_GPU_MODELS"], "/m")


class WatchTests(unittest.TestCase):
    def run_watch(self, readings, fan=MANUAL, allowed=lambda now: True):
        process = sleeper(60)
        lines = []
        stream = iter(readings)
        last = readings[-1]
        stop = live.watch(
            process,
            ops.ThermalGuard(live.LIMITS),
            lambda: next(stream, last),
            lambda: fan,
            Clock(),
            allowed,
            lines.append,
        )
        return stop, process, lines

    def test_a_hot_junction_kills_the_tests(self):
        stop, process, lines = self.run_watch([sensors(70.0), sensors(85.0)])
        self.assertIn("junction 85.0", stop)
        self.assertIsNotNone(process.returncode)
        self.assertTrue(lines[0].startswith("STOP thermal"))

    def test_an_unreadable_sensor_kills_the_tests(self):
        stop, _, _ = self.run_watch([{EDGE: {"value": None, "crit": None}}])
        self.assertTrue(stop.startswith("thermal_unknown"))

    def test_the_end_of_the_heavy_window_kills_the_tests(self):
        stop, _, _ = self.run_watch([sensors()], allowed=lambda now: now < 3)
        self.assertEqual(stop, "schedule: the heavy window ended")

    def test_a_fan_leaving_manual_kills_the_tests(self):
        stop, _, _ = self.run_watch([sensors()], fan=AUTO)
        self.assertTrue(stop.startswith("fan_fault"))

    def test_a_run_that_ends_by_itself_is_no_stop(self):
        process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        process.wait()
        stop = live.watch(
            process,
            ops.ThermalGuard(live.LIMITS),
            sensors,
            lambda: MANUAL,
            Clock(),
            lambda now: True,
            print,
        )
        self.assertIsNone(stop)


class CiTests(unittest.TestCase):
    def test_ci_never_runs_the_live_gpu_checks(self):
        """The CI confinement step selects `launch::live_tests::` with `--include-ignored`; the
        GPU-only `live_gpu_*` tests (they refuse without the fan check) must be skipped there."""
        ci = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text()
        step = ci[ci.index("target/confinement-tests --include-ignored") :]
        step = step[: step.index("tee target/confinement.log")]
        self.assertIn("--skip live_gpu_", step)


if __name__ == "__main__":
    unittest.main()
