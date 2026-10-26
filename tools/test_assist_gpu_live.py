import os
import re
import signal
import subprocess
import sys
import tempfile
import time
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

    def test_the_automatic_fan_is_accepted_for_short_tests_and_refused_for_long_ones(self):
        with mock.patch.object(ops, "heavy_allowed", return_value=True):
            self.assertIsNone(live.refusal(0.0, AUTO, short=True))
            self.assertIn("not in manual mode", live.refusal(0.0, AUTO))
            self.assertIn("not in manual mode", live.refusal(0.0, AUTO, short=False))
        # The schedule check stays in short mode.
        with mock.patch.object(ops, "heavy_allowed", return_value=False):
            self.assertIn("not allowed now", live.refusal(0.0, AUTO, short=True))

    def test_short_mode_takes_only_the_fixed_short_tests(self):
        short = sorted(live.SHORT_TESTS)
        self.assertIsNone(live.short_refusal(short))
        for name in short:
            self.assertIn("::live_gpu_", name)
        self.assertIn("not a short test", live.short_refusal([*short, "a::live_gpu_sustained"]))
        self.assertIn("not a short test", live.short_refusal(["bench::calibrate_loop"]))

    def test_the_rust_tests_short_list_is_the_tools_list(self):
        source = (
            Path(__file__).parent.parent / "src/assistant/runtime/live_gpu_tests.rs"
        ).read_text()
        block = source.split("const SHORT_TESTS: [&str;")[1].split("];")[0]
        rust = {name.rsplit("::", 1)[-1] for name in re.findall(r'"(live_gpu_\w+)"', block)}
        tool = {name.rsplit("::", 1)[-1] for name in live.SHORT_TESTS}
        self.assertEqual(rust, tool)

    def test_each_gpu_test_passes_its_own_name_to_the_gate(self):
        source = (
            Path(__file__).parent.parent / "src/assistant/runtime/live_gpu_tests.rs"
        ).read_text()
        found = re.findall(
            r'async fn (live_gpu_\w+)\(\) \{\n(?:    use [^\n]*\n)?    gpu_allowed\("(\w+)"\);',
            source,
        )
        self.assertEqual(len(found), 4)
        for function, passed in found:
            self.assertEqual(function, passed)
        self.assertEqual(
            {name for name, _ in found}, {n.rsplit("::", 1)[-1] for n in live.SHORT_TESTS}
        )

    def test_short_is_never_the_default_and_reaches_the_tests_only_when_asked(self):
        def args(**more):
            return live.argparse.Namespace(
                binary=Path("/b"),
                runtime=Path("/r"),
                models=Path("/m"),
                scratch=Path("/s"),
                fake_server=Path("/f"),
                mimik=Path("/x"),
                test=None,
                **more,
            )

        self.assertNotIn("MIMIK_LIVE_GPU_SHORT", live.environment(args(), {}))
        self.assertNotIn("MIMIK_LIVE_GPU_SHORT", live.environment(args(short=False), {}))
        self.assertEqual(live.environment(args(short=True), {})["MIMIK_LIVE_GPU_SHORT"], "1")
        # A variable inherited from the caller never switches it on.
        inherited = {"MIMIK_LIVE_GPU_SHORT": "1"}
        self.assertNotIn("MIMIK_LIVE_GPU_SHORT", live.environment(args(short=False), inherited))
        self.assertEqual(live.SHORT_LIMITS["junction_stop_c"], 75.0)
        self.assertEqual(live.LIMITS["junction_stop_c"], 85.0)

    def test_main_refuses_an_automatic_fan_without_short_and_a_long_test_with_it(self):
        base = [
            *("--binary", "b", "--mimik", "x", "--runtime", "r", "--models", "m"),
            *("--scratch", "s", "--fake-server", "f", "--log", "/nonexistent/x"),
        ]
        with (
            mock.patch.object(ops, "heavy_allowed", return_value=True),
            mock.patch.object(ops, "read_fan", return_value=AUTO),
            mock.patch.object(live, "selected", return_value=(["a::live_gpu_sustained"], None)),
            mock.patch.object(subprocess, "Popen") as popen,
        ):
            self.assertEqual(live.main(base), 2)
            self.assertEqual(live.main([*base, "--short"]), 2)
        popen.assert_not_called()

    def test_an_unreadable_fan_is_refused_for_a_long_run(self):
        with mock.patch.object(ops, "heavy_allowed", return_value=True):
            self.assertIn("not in manual mode", live.refusal(0.0, dict(AUTO, pwm1_enable=None)))

    def test_main_refuses_before_starting_anything(self):
        with (
            mock.patch.object(ops, "heavy_allowed", return_value=True),
            mock.patch.object(ops, "read_fan", return_value=AUTO),
            mock.patch.object(subprocess, "Popen") as popen,
        ):
            code = live.main(
                [
                    *("--binary", "b", "--mimik", "x", "--runtime", "r", "--models", "m"),
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
            mimik=Path("/mimik"),
            test=None,
        )
        self.assertEqual(live.command(args)[1], live.FILTER)
        env = live.environment(args, {})
        self.assertEqual(env["MIMIK_LIVE_GPU"], "1")
        self.assertEqual(env["MIMIK_LIVE_GPU_MODELS"], "/m")
        self.assertEqual(env["MIMIK_LIVE_MIMIK"], "/mimik")


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

    def test_short_mode_stops_at_75_degrees_sampled_every_second_without_a_fan(self):
        process = sleeper(60)
        lines, clock = [], Clock()
        readings = iter([sensors(60.0), sensors(74.9), sensors(75.0)])
        stop = live.watch(
            process,
            ops.ThermalGuard(live.SHORT_LIMITS),
            lambda: next(readings),
            lambda: None,
            clock,
            lambda now: True,
            lines.append,
        )
        self.assertIn("junction 75.0", stop)
        # Three samples, one second apart.
        self.assertEqual(clock.now, 2.0)
        self.assertIsNotNone(process.returncode)

    def test_short_mode_ignores_an_automatic_fan_but_not_an_unreadable_sensor(self):
        process = sleeper(60)
        stop = live.watch(
            process,
            ops.ThermalGuard(live.SHORT_LIMITS),
            lambda: {EDGE: {"value": None, "crit": None}},
            lambda: None,
            Clock(),
            lambda now: True,
            lambda line: None,
        )
        self.assertTrue(stop.startswith("thermal_unknown"))
        process = sleeper(0.2)
        stop = live.watch(
            process,
            ops.ThermalGuard(live.SHORT_LIMITS),
            lambda: sensors(50.0),
            lambda: None,
            Clock(),
            lambda now: True,
            lambda line: None,
        )
        # The process ended by itself; the wall clock is real, the fake one only counts.
        self.assertIsNone(stop)

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


class WatchCleanupTests(unittest.TestCase):
    def test_an_exception_in_the_watch_loop_kills_the_group(self):
        process = sleeper(60)

        def broken():
            raise OSError("sensor vanished")

        with self.assertRaises(OSError):
            live.watch(
                process,
                ops.ThermalGuard(live.LIMITS),
                broken,
                lambda: MANUAL,
                Clock(),
                lambda now: True,
                print,
            )
        self.assertIsNotNone(process.poll())
        with self.assertRaises(ProcessLookupError):
            os.killpg(process.pid, signal.SIG_DFL)


def alive(pid):
    """Running, not a zombie (a zombie answers `kill(pid, 0)`, and in a container whose PID 1
    does not reap it stays one)."""
    try:
        with open(f"/proc/{pid}/stat", encoding="ascii") as stat:
            return stat.read().rsplit(")", 1)[1].split()[0] != "Z"
    except (FileNotFoundError, ProcessLookupError):
        return False


class LeaderExitTests(unittest.TestCase):
    def test_a_same_group_child_of_a_leader_that_exited_is_killed(self):
        code = (
            "import subprocess, sys;c = subprocess.Popen(['sleep', '60']);print(c.pid, flush=True)"
        )
        leader = subprocess.Popen(
            [sys.executable, "-c", code], stdout=subprocess.PIPE, text=True, start_new_session=True
        )
        child = int(leader.stdout.readline())
        # The leader has exited but is not reaped (a zombie keeps its group ID).
        os.waitid(os.P_PID, leader.pid, os.WEXITED | os.WNOWAIT)
        self.assertTrue(alive(child))  # it outlived its leader
        live.watch(
            leader,
            ops.ThermalGuard(live.LIMITS),
            sensors,
            lambda: MANUAL,
            Clock(),
            lambda now: True,
            print,
        )
        for _ in range(100):
            if not alive(child):
                return
            time.sleep(0.02)
        self.fail("the leader's same-group child survived the watch")


class ReapedLeaderTests(unittest.TestCase):
    def test_a_leader_reaped_elsewhere_is_never_signalled_through_its_pid(self):
        process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        process.wait()  # reaped: its PID may be reused by now
        with mock.patch.object(os, "killpg") as killpg:
            live.watch(
                process,
                ops.ThermalGuard(live.LIMITS),
                sensors,
                lambda: MANUAL,
                Clock(),
                lambda now: True,
                print,
            )
        killpg.assert_not_called()

    def test_a_leader_reaped_by_another_waiter_is_never_signalled(self):
        process = sleeper(0)
        time.sleep(0.2)
        os.waitpid(process.pid, 0)  # reaped behind Popen's back: returncode is still None
        with mock.patch.object(os, "killpg") as killpg:
            live.kill(process)
        killpg.assert_not_called()

    def test_the_group_is_signalled_before_the_leader_is_reaped(self):
        process = sleeper(60)
        order = []
        real_killpg, real_wait = os.killpg, process.wait

        def killpg(pid, sig):
            order.append(("killpg", process.returncode))
            real_killpg(pid, sig)

        def wait(*args, **kwargs):
            order.append(("wait", None))
            return real_wait(*args, **kwargs)

        with (
            mock.patch.object(os, "killpg", killpg),
            mock.patch.object(process, "wait", wait),
        ):
            live.watch(
                process,
                ops.ThermalGuard(live.LIMITS),
                lambda: sensors(90.0),
                lambda: MANUAL,
                Clock(),
                lambda now: True,
                print,
            )
        self.assertEqual(order[0], ("killpg", None))
        self.assertEqual(order[1][0], "wait")


FAKE_BINARY = """#!{python}
import sys
args = sys.argv[1:]
if "--list" in args:
    for name in {names!r}:
        print(name + ": test")
    sys.exit(0)
print({output!r})
"""


class RunTests(unittest.TestCase):
    """`main` against a fake test binary (no GPU, no real tests)."""

    def run_main(self, names, output):
        with tempfile.TemporaryDirectory() as folder:
            binary = Path(folder) / "tests"
            binary.write_text(FAKE_BINARY.format(python=sys.executable, names=names, output=output))
            binary.chmod(0o755)
            log = Path(folder) / "run.log"
            with (
                mock.patch.object(ops, "heavy_allowed", return_value=True),
                mock.patch.object(ops, "read_fan", return_value=MANUAL),
                mock.patch.object(ops, "read_sensors", return_value=sensors()),
                mock.patch.object(ops, "hwmon_root", return_value=folder),
            ):
                code = live.main(
                    [
                        *(
                            "--binary",
                            str(binary),
                            "--mimik",
                            "x",
                            "--runtime",
                            folder,
                            "--models",
                            folder,
                        ),
                        *("--scratch", folder, "--fake-server", "f", "--log", str(log)),
                    ]
                )
            return code

    def test_a_successful_summary_of_zero_tests_is_no_pass(self):
        zero = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out"
        self.assertNotEqual(self.run_main(["m::live_gpu_a"], zero), 0)

    def test_a_listing_that_cannot_run_or_hangs_is_a_clean_refusal(self):
        args = live.argparse.Namespace(binary=Path("/nonexistent/tests"), test=None)
        names, why = live.selected(args)
        self.assertIsNone(names)
        self.assertIn("listing", why)
        with mock.patch.object(subprocess, "run", side_effect=subprocess.TimeoutExpired("x", 60)):
            names, why = live.selected(args)
        self.assertIsNone(names)
        self.assertIn("listing", why)
        with mock.patch.object(
            subprocess, "run", side_effect=UnicodeDecodeError("utf-8", b"", 0, 1, "x")
        ):
            self.assertIn("listing", live.selected(args)[1])

    def test_main_exits_2_when_the_listing_cannot_run(self):
        with tempfile.TemporaryDirectory() as folder:
            with (
                mock.patch.object(ops, "heavy_allowed", return_value=True),
                mock.patch.object(ops, "read_fan", return_value=MANUAL),
                mock.patch.object(ops, "hwmon_root", return_value=folder),
            ):
                code = live.main(
                    [
                        *("--binary", "/nonexistent/tests", "--mimik", "x", "--runtime", folder),
                        *("--models", folder, "--scratch", folder, "--fake-server", "f"),
                        *("--log", str(Path(folder) / "x.log")),
                    ]
                )
        self.assertEqual(code, 2)

    def test_an_empty_selection_or_a_non_gpu_test_is_refused_before_launch(self):
        self.assertEqual(self.run_main([], "x"), 2)
        self.assertEqual(self.run_main(["m::live_fake_gpu_a"], "x"), 2)

    def test_every_selected_test_must_run_and_pass(self):
        names = ["m::live_gpu_a", "m::live_gpu_b"]
        both = "test m::live_gpu_a ... ok\ntest m::live_gpu_b ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        self.assertEqual(self.run_main(names, both), 0)
        one = "test m::live_gpu_a ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        self.assertNotEqual(self.run_main(names, one), 0)
        ignored = "test m::live_gpu_a ... ok\ntest m::live_gpu_b ... ignored\ntest result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
        self.assertNotEqual(self.run_main(names, ignored), 0)
        renamed = "test m::live_gpu_a ... ok\ntest m::live_gpu_c ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        self.assertNotEqual(self.run_main(names, renamed), 0)


class CiTests(unittest.TestCase):
    def test_ci_never_runs_the_live_gpu_checks(self):
        """The CI confinement step selects `launch::live_tests::` with `--include-ignored`; the
        GPU-only `live_gpu_*` tests (they refuse without the fan check) must be skipped there."""
        ci = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text()
        step = ci[ci.index("target/confinement-tests --test-threads=2 --include-ignored") :]
        step = step[: step.index("tee target/confinement.log")]
        self.assertIn("--skip live_gpu_", step)


if __name__ == "__main__":
    unittest.main()
