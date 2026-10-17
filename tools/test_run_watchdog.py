"""Watchdog and shared ops (review fixes 2, 4, 10): fake docker, fake clock.

Failing-first: run before tools/run_watchdog.py and tools/toolloop_ops.py existed.
"""

import datetime as dt
import fcntl
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import run_watchdog as wd
import toolloop_ops as ops

# Never see a real user-granted schedule override (toolloop_ops.OVERRIDE_ENV).
os.environ["MIMIK_SCHEDULE_OVERRIDE_FILE"] = "/nonexistent/mimik-schedule-override.json"

_EMPTY_HWMON = tempfile.mkdtemp(prefix="no-hwmon-")


def setUpModule():
    # Unit tests never read the host's real sensors: an empty hwmon tree reads as no sensors.
    ops.HWMON = Path(_EMPTY_HWMON)


def at(*args):
    return dt.datetime(*args).timestamp()


class FakeClock:
    def __init__(self, now):
        self.now = now

    def time(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class FakeDocker:
    def __init__(self, names=(), network=False, fail_rm=False, ps_fails=False):
        self.names, self.network, self.fail_rm = set(names), network, fail_rm
        self.ps_fails = ps_fails
        self.cleanups = 0

    def owned(self):
        return None if self.ps_fails else sorted(self.names)

    def network_exists(self):
        return self.network

    def cleanup(self):
        self.cleanups += 1
        if not self.fail_rm:
            self.names.clear()
            self.network = False
        return {"ok": not self.names and not self.network, "remaining": sorted(self.names)}


class WatchdogBase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.log = Path(self.tmp.name) / "watchdog.log"

    def dog(self, docker, held, start, **kw):
        self.clock = FakeClock(start)
        return wd.Watchdog(docker, lambda: held["value"], self.clock, self.log, **kw)

    def actions(self):
        return [json.loads(x)["action"] for x in self.log.read_text().splitlines()]


class WatchdogTests(WatchdogBase):
    def test_removes_owned_containers_outside_the_window_even_with_a_live_lock(self):
        docker = FakeDocker([ops.SERVER, ops.HARNESS], network=True)
        dog = self.dog(docker, {"value": True}, at(2026, 10, 14, 9, 0, 30))  # Wednesday
        self.assertEqual(dog.tick(), "removed_at_window_edge")
        self.assertEqual(docker.owned(), [])
        self.assertFalse(docker.network)

    def test_leaves_live_work_alone_inside_the_window(self):
        docker = FakeDocker([ops.SERVER])
        dog = self.dog(docker, {"value": True}, at(2026, 10, 17, 12, 0))  # Saturday
        for _ in range(10):
            self.assertEqual(dog.tick(), "ok")
            self.clock.sleep(30)
        self.assertEqual(docker.cleanups, 0)

    def test_orphans_removed_after_the_grace_period(self):
        docker = FakeDocker([ops.SERVER, ops.OWNED_PREFIX + "check-1-model"])
        dog = self.dog(docker, {"value": False}, at(2026, 10, 17, 12, 0), grace=120)
        self.assertEqual(dog.tick(), "orphan_grace")
        self.clock.sleep(60)
        self.assertEqual(dog.tick(), "orphan_grace")
        self.clock.sleep(60)
        self.assertEqual(dog.tick(), "removed_orphans")
        self.assertEqual(docker.owned(), [])
        self.assertIn("removed_orphans", self.actions())

    def test_lock_coming_back_resets_the_grace(self):
        held = {"value": False}
        docker = FakeDocker([ops.SERVER])
        dog = self.dog(docker, held, at(2026, 10, 17, 12, 0), grace=120)
        dog.tick()
        self.clock.sleep(100)
        held["value"] = True
        self.assertEqual(dog.tick(), "ok")
        held["value"] = False
        self.clock.sleep(100)
        self.assertEqual(dog.tick(), "orphan_grace")

    def test_failed_removal_is_logged(self):
        docker = FakeDocker([ops.SERVER], fail_rm=True)
        dog = self.dog(docker, {"value": False}, at(2026, 10, 14, 10, 0))
        dog.tick()
        row = json.loads(self.log.read_text().splitlines()[-1])
        self.assertFalse(row["ok"])
        self.assertEqual(row["remaining"], [ops.SERVER])

    def test_idle_exit_and_run_loop(self):
        dog = self.dog(FakeDocker(), {"value": False}, at(2026, 10, 17, 12, 0), idle_exit=90)
        dog.interval = 30
        self.assertEqual(dog.run(), 0)
        self.assertEqual(self.actions()[-1], "idle_exit")
        self.assertGreaterEqual(self.clock.now - at(2026, 10, 17, 12, 0), 90)

    def test_interval_capped_at_60s(self):
        with self.assertRaises(ValueError):
            wd.Watchdog(FakeDocker(), lambda: False, FakeClock(0), self.log, interval=61)

    def test_production_entry_uses_the_real_clock_only(self):
        source = Path(wd.__file__).read_text()
        self.assertNotIn("MIMIK_QUEUE_NOW", source)
        self.assertNotIn("MIMIK_TEST", source)
        self.assertIn("ops.SystemClock()", source)


class DockerTests(unittest.TestCase):
    def fake_run(self, script):
        calls = []

        def run(argv, **kw):
            calls.append(argv[1:])
            self.assertIn("timeout", kw)
            result = script(argv[1:])
            if isinstance(result, BaseException):
                raise result
            rc, out = result
            return subprocess.CompletedProcess(argv, rc, out, "")

        return run, calls

    def test_cleanup_removes_owned_and_network_and_verifies(self):
        state = {"names": {ops.SERVER, "other-container", ops.HARNESS}, "net": True}

        def script(args):
            if args[:2] == ["ps", "-a"]:
                return 0, "\n".join(sorted(state["names"]))
            if args[:2] == ["network", "ls"]:
                return 0, ops.NETWORK + "\nbridge" if state["net"] else "bridge"
            if args[:2] == ["rm", "-f"]:
                state["names"].discard(args[2])
                return 0, ""
            if args[:2] == ["network", "rm"]:
                state["net"] = False
                return 0, ""
            return 1, ""

        run, calls = self.fake_run(script)
        result = ops.Docker(run=run).cleanup()
        self.assertTrue(result["ok"], result)
        self.assertEqual(state["names"], {"other-container"})  # never touches foreign names
        self.assertIn(["network", "rm", ops.NETWORK], calls)

    def test_hanging_docker_times_out_and_is_unresolved(self):
        def script(args):
            return subprocess.TimeoutExpired(args, 1)

        run, _ = self.fake_run(script)
        result = ops.Docker(run=run).cleanup()
        self.assertFalse(result["ok"])
        self.assertIn("docker ps failed or timed out", result["errors"])

    def test_failing_removal_is_unresolved(self):
        def script(args):
            if args[:2] == ["ps", "-a"]:
                return 0, ops.SERVER
            if args[:2] == ["network", "ls"]:
                return 0, "bridge"
            return 1, ""

        run, _ = self.fake_run(script)
        result = ops.Docker(run=run).cleanup()
        self.assertFalse(result["ok"])
        self.assertEqual(result["remaining"], [ops.SERVER])


class LockTests(unittest.TestCase):
    def test_lock_held_reads_proc_locks_without_taking_the_lock(self):
        if not Path("/proc/locks").exists():
            self.skipTest("no /proc/locks")
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lock"
            path.touch()
            self.assertFalse(ops.lock_held(path))
            with open(path, "a") as handle:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.assertTrue(ops.lock_held(path))
                with open(path, "a") as other:  # the probe did not take it either
                    with self.assertRaises(BlockingIOError):
                        fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertFalse(ops.lock_held(path))
            self.assertFalse(ops.lock_held(Path(tmp) / "missing"))

    def test_lock_paths_are_shared_in_the_main_checkout(self):
        paths = ops.lock_paths(Path("/repo"))
        self.assertEqual(paths["runner"], Path("/repo/target/.tool-loop-benchmark.lock"))
        self.assertEqual(paths["queue"].parent, paths["runner"].parent)


class ReviewTwoWatchdogTests(WatchdogBase):
    """Second review round: reserve, docker ps failure, probe errors, logging, heartbeat."""

    def test_removes_before_the_edge_with_a_reserve(self):
        docker = FakeDocker([ops.SERVER])
        dog = self.dog(docker, {"value": True}, at(2026, 10, 15, 8, 55, 0))  # Thursday
        self.assertEqual(dog.tick(), "ok")  # 300 s before 09:00: not yet
        self.clock.now = at(2026, 10, 15, 8, 56, 30)  # 210 s before: inside the 240 s reserve
        self.assertEqual(dog.tick(), "removed_at_window_edge")
        row = json.loads(self.log.read_text().splitlines()[-1])
        self.assertEqual(row["reserve"], 240)

    def test_docker_ps_failure_still_enforces_and_retries(self):
        docker = FakeDocker(ps_fails=True)
        dog = self.dog(docker, {"value": True}, at(2026, 10, 14, 10, 0))  # outside the window
        self.assertEqual(dog.tick(), "removed_at_window_edge")
        self.assertEqual(dog.tick(), "removed_at_window_edge")  # retried every tick
        self.assertEqual(docker.cleanups, 2)
        orphan = FakeDocker(ps_fails=True)
        dog = self.dog(orphan, {"value": False}, at(2026, 10, 17, 12, 0), grace=60)
        self.assertEqual(dog.tick(), "orphan_grace")
        self.clock.sleep(60)
        self.assertEqual(dog.tick(), "removed_orphans")
        live = FakeDocker(ps_fails=True)
        dog = self.dog(live, {"value": True}, at(2026, 10, 17, 12, 0))
        self.assertEqual(dog.tick(), "docker_error")
        self.assertEqual(live.cleanups, 0)

    def test_lock_probe_error_counts_as_held(self):
        def broken():
            raise OSError("cannot read /proc/locks")

        docker = FakeDocker([ops.SERVER])
        self.clock = FakeClock(at(2026, 10, 17, 12, 0))
        dog = wd.Watchdog(docker, broken, self.clock, self.log, grace=0)
        self.assertEqual(dog.tick(), "ok")
        self.assertEqual(docker.cleanups, 0)

    def test_log_failure_does_not_stop_enforcement(self):
        docker = FakeDocker([ops.SERVER])
        self.clock = FakeClock(at(2026, 10, 14, 10, 0))
        unwritable = Path(self.tmp.name)  # a directory: every log write fails
        dog = wd.Watchdog(docker, lambda: True, self.clock, unwritable)
        self.assertEqual(dog.tick(), "removed_at_window_edge")
        self.assertEqual(docker.owned(), [])

    def test_heartbeat_on_every_tick(self):
        beat = Path(self.tmp.name) / "hb"
        self.clock = FakeClock(at(2026, 10, 17, 12, 0))
        dog = wd.Watchdog(FakeDocker(), lambda: True, self.clock, self.log, heartbeat=beat)
        dog.tick()
        first = json.loads(beat.read_text())
        self.assertEqual(first["pid"], os.getpid())

    def test_tick_exception_keeps_the_loop_alive(self):
        class Flaky(FakeDocker):
            calls = 0

            def owned(self):
                Flaky.calls += 1
                if Flaky.calls == 1:
                    raise RuntimeError("boom")
                return []

        self.clock = FakeClock(at(2026, 10, 17, 12, 0))
        dog = wd.Watchdog(Flaky(), lambda: False, self.clock, self.log, idle_exit=30)
        self.assertEqual(dog.run(), 0)
        actions = [json.loads(x)["action"] for x in self.log.read_text().splitlines()]
        self.assertIn("tick_error", actions)
        self.assertEqual(actions[-1], "idle_exit")

    def test_endpoint_override_refused(self):
        self.assertIsNone(ops.docker_endpoint_problem({}))
        for name in ("DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG"):
            self.assertIn(name, ops.docker_endpoint_problem({name: "x"}))
        proc = subprocess.run(
            [sys.executable, str(Path(wd.__file__)), "--once"],
            env={"PATH": os.environ.get("PATH", ""), "DOCKER_HOST": "tcp://x"},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("DOCKER_HOST", proc.stderr)


class ReviewThreeWatchdogTests(WatchdogBase):
    def test_heartbeat_written_before_a_slow_cleanup(self):
        beat = Path(self.tmp.name) / "hb"
        seen = {}

        class Slow(FakeDocker):
            def cleanup(inner):
                seen["beat_before_cleanup"] = beat.exists()
                time.sleep(1.2)  # a slow docker
                return FakeDocker.cleanup(inner)

        self.clock = FakeClock(at(2026, 10, 14, 10, 0))
        dog = wd.Watchdog(Slow([ops.SERVER]), lambda: True, self.clock, self.log, heartbeat=beat)
        start = time.time()
        dog.tick()
        self.assertTrue(seen["beat_before_cleanup"])
        self.assertGreaterEqual(json.loads(beat.read_text())["time"], start + 1.2)  # and after
        self.assertGreaterEqual(wd.WORST_TICK_SECONDS, 10 * ops.DOCKER_TIMEOUT)

    def test_ps_failure_fallback_includes_registered_check_containers(self):
        registry = Path(self.tmp.name) / "checks.txt"
        check = ops.OWNED_PREFIX + "check-42-1"
        registry.write_text(f"{check}\n{ops.OWNED_PREFIX}check-43-1\nnot-ours\n")
        labels = {check: "v3", ops.SERVER: "v3"}
        removed = []

        def run(argv, **kw):
            args = argv[1:]
            if args[0] in ("ps", "network"):
                return subprocess.CompletedProcess(argv, 1, "", "")
            if args[0] == "inspect":
                name = args[-1]
                return subprocess.CompletedProcess(
                    argv, 0 if name in labels else 1, labels.get(name, "") + "\n", ""
                )
            if args[:2] == ["rm", "-f"]:
                removed.append(args[2])
                return subprocess.CompletedProcess(argv, 0, "", "")
            return subprocess.CompletedProcess(argv, 1, "", "")

        ops.Docker(run=run, check_registry=registry).cleanup()
        self.assertEqual(sorted(removed), sorted([check, ops.SERVER]))


class ThermalWatchdogTests(WatchdogBase):
    def hot(self, value=106.0):
        return {
            "amdgpu:edge": {"value": 70.0, "crit": 100.0},
            "amdgpu:junction": {"value": value, "crit": 110.0},
        }

    def test_hard_limit_removes_owned_containers_inside_the_window(self):
        docker = FakeDocker([ops.SERVER])
        self.clock = FakeClock(at(2026, 10, 17, 12, 0))  # Saturday: the window is open
        dog = wd.Watchdog(docker, lambda: True, self.clock, self.log, thermal_reader=self.hot)
        self.assertEqual(dog.tick(), "removed_thermal")
        self.assertEqual(docker.owned(), [])
        row = json.loads(self.log.read_text().splitlines()[-1])
        self.assertIn("amdgpu:junction", row["breaches"][0])

    def test_below_the_hard_limit_leaves_live_work_alone(self):
        docker = FakeDocker([ops.SERVER])
        self.clock = FakeClock(at(2026, 10, 17, 12, 0))
        dog = wd.Watchdog(
            docker, lambda: True, self.clock, self.log, thermal_reader=lambda: self.hot(94.9)
        )
        self.assertEqual(dog.tick(), "ok")
        self.assertEqual(docker.cleanups, 0)

    def test_emergency_junction_95(self):
        docker = FakeDocker([ops.SERVER])
        self.clock = FakeClock(at(2026, 10, 17, 12, 0))
        dog = wd.Watchdog(
            docker, lambda: True, self.clock, self.log, thermal_reader=lambda: self.hot(95.0)
        )
        self.assertEqual(dog.tick(), "removed_thermal")
        row = json.loads(self.log.read_text().splitlines()[-1])
        self.assertIn("junction 95.0", row["breaches"][-1])

    def test_watchdog_checks_every_10_s_by_default(self):
        import inspect

        self.assertEqual(inspect.signature(wd.Watchdog).parameters["interval"].default, 10)
        source = Path(wd.__file__).read_text()
        self.assertIn('parser.add_argument("--interval", type=float, default=10)', source)

    def test_sensor_read_error_is_logged_not_fatal(self):
        def broken():
            raise OSError("hwmon gone")

        docker = FakeDocker([ops.SERVER])
        self.clock = FakeClock(at(2026, 10, 17, 12, 0))
        dog = wd.Watchdog(docker, lambda: True, self.clock, self.log, thermal_reader=broken)
        self.assertEqual(dog.tick(), "ok")
        self.assertIn("thermal_read_error", self.log.read_text())


class LabelTests(unittest.TestCase):
    """Second review round N4: ownership is label AND name prefix."""

    def docker(self, ps_works=True):
        state = {
            "c": {
                ops.SERVER: "v3",
                ops.OWNED_PREFIX + "check-1-1": "v3",
                ops.HARNESS: None,  # same prefix, no label: foreign
                "other": "v3",  # label, other name: not ours either
            },
            "net": True,
        }

        def run(argv, **kw):
            args = argv[1:]
            if args[:2] == ["ps", "-a"]:
                if not ps_works:
                    return subprocess.CompletedProcess(argv, 1, "", "")
                names = [n for n, label in state["c"].items() if label == "v3"]
                assert f"label={ops.LABEL}" in args
                return subprocess.CompletedProcess(argv, 0, "\n".join(names), "")
            if args[:2] == ["network", "ls"]:
                out = ops.NETWORK if state["net"] else ""
                return subprocess.CompletedProcess(argv, 0, out, "")
            if args[0] == "inspect":
                label = state["c"].get(args[-1])
                rc = 0 if args[-1] in state["c"] else 1
                return subprocess.CompletedProcess(argv, rc, f"{label or ''}\n", "")
            if args[:2] == ["rm", "-f"]:
                state["c"].pop(args[2], None)
                return subprocess.CompletedProcess(argv, 0, "", "")
            if args[:2] == ["network", "rm"]:
                state["net"] = False
                return subprocess.CompletedProcess(argv, 0, "", "")
            return subprocess.CompletedProcess(argv, 1, "", "")

        return ops.Docker(run=run), state

    def test_foreign_same_prefix_container_left_alone(self):
        docker, state = self.docker()
        self.assertEqual(docker.owned(), [ops.OWNED_PREFIX + "check-1-1", ops.SERVER])
        result = docker.cleanup()
        self.assertTrue(result["ok"], result)
        self.assertIn(ops.HARNESS, state["c"])
        self.assertIn("other", state["c"])
        self.assertNotIn(ops.SERVER, state["c"])

    def test_ps_failure_falls_back_to_labelled_known_names(self):
        docker, state = self.docker(ps_works=False)
        result = docker.cleanup()
        self.assertFalse(result["ok"])  # cannot verify without ps
        self.assertNotIn(ops.SERVER, state["c"])  # labelled known name removed
        self.assertIn(ops.HARNESS, state["c"])  # unlabelled known name kept


class LockProbeErrorTests(unittest.TestCase):
    def test_unreadable_proc_locks_counts_as_held(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lock"
            path.touch()
            self.assertTrue(ops.lock_held(path, proc_locks=Path(tmp) / "no-such-file"))
            self.assertFalse(ops.lock_held(Path(tmp) / "missing-lock"))


if __name__ == "__main__":
    unittest.main()
