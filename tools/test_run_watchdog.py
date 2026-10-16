"""Watchdog and shared ops (review fixes 2, 4, 10): fake docker, fake clock.

Failing-first: run before tools/run_watchdog.py and tools/toolloop_ops.py existed.
"""

import datetime as dt
import fcntl
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import run_watchdog as wd
import toolloop_ops as ops


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
    def __init__(self, names=(), network=False, fail_rm=False):
        self.names, self.network, self.fail_rm = set(names), network, fail_rm
        self.cleanups = 0

    def owned(self):
        return sorted(self.names)

    def network_exists(self):
        return self.network

    def cleanup(self):
        self.cleanups += 1
        if not self.fail_rm:
            self.names.clear()
            self.network = False
        return {"ok": not self.names and not self.network, "remaining": sorted(self.names)}


class WatchdogTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.log = Path(self.tmp.name) / "watchdog.log"

    def dog(self, docker, held, start, **kw):
        self.clock = FakeClock(start)
        return wd.Watchdog(docker, lambda: held["value"], self.clock, self.log, **kw)

    def actions(self):
        return [json.loads(x)["action"] for x in self.log.read_text().splitlines()]

    def test_removes_owned_containers_outside_the_window_even_with_a_live_lock(self):
        docker = FakeDocker([ops.SERVER, ops.HARNESS], network=True)
        dog = self.dog(docker, {"value": True}, at(2026, 10, 14, 9, 0, 30))  # Wednesday
        self.assertEqual(dog.tick(), "removed_outside_window")
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


if __name__ == "__main__":
    unittest.main()
