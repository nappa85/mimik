"""Real-docker cleanup smoke (review fix 11). Skipped unless MIMIK_REAL_DOCKER_SMOKE=1.

Not heavy: the runner's server and harness containers are replaced by `sleep` containers of
the local mimik-ci image (a docker wrapper on PATH rewrites only those two `run -d` calls and
answers the health probe; every other call, including the hash/check containers, the network
and all cleanup, goes to the real docker). It proves that SIGTERM and SIGKILL of the runner,
and of the queue, leave no owned container and no network, and that the watchdog removes
orphans and anything outside the window. Run on the host:

  MIMIK_REAL_DOCKER_SMOKE=1 MIMIK_FORMULA_EVAL=<built formula_eval> python3 -m unittest \
      test_real_docker_cleanup        (from tools/)
"""

import datetime as dt
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

import run_watchdog
import toolloop_ops as ops

TOOLS = Path(__file__).resolve().parent
DEMO = TOOLS.parent / "examples" / "tool-loop-demo"
ENABLED = os.environ.get("MIMIK_REAL_DOCKER_SMOKE") == "1" and shutil.which("docker")
SLEEP_IMAGE = "sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f"
FORMULA_SHA = "f0e13322ef921fa89685c48831e9a7658cf789e842514a8a93ff891fa334fc10"
COPIED = (
    "run_tool_loop_benchmark.sh",
    "model_registry.py",
    "models.json",
    "run_queue.py",
    "toolloop_ops.py",
    "tool_loop_benchmark.py",
    "run_watchdog.py",
)

WRAPPER = r"""#!PYTHON
import os, sys
real = os.environ["REAL_DOCKER"]
args = sys.argv[1:]
def opt(name):
    return args[args.index(name) + 1] if name in args else None
if args[:2] == ["run", "-d"]:
    name = opt("--name")
    if name in ("mimik-toolloop-v3-srv", "mimik-toolloop-v3-harness"):
        new = [real, "run", "-d", "--init", "--pull", "never", "--name", name,
               "--network", opt("--network"), "--user", "1000:1000", "--cap-drop", "ALL",
               "--read-only", os.environ["SLEEP_IMAGE"], "sleep", "600"]
        os.execv(real, new)
if args[:1] == ["exec"] and "curl" in args:
    print("{}")
    sys.exit(0)
os.execv(real, [real] + args)
"""


def owned_state():
    docker = ops.Docker()
    return docker.owned(), docker.network_exists()


@unittest.skipUnless(ENABLED, "real-docker smoke: set MIMIK_REAL_DOCKER_SMOKE=1 (host only)")
class RealDockerCleanupSmoke(unittest.TestCase):
    def setUp(self):
        self.assertEqual(owned_state(), ([], False), "owned containers exist before the smoke")
        self.tmp = Path(tempfile.mkdtemp(prefix="mimik-smoke-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.addCleanup(lambda: ops.Docker().cleanup())
        repo = self.tmp / "repo"
        (repo / "tools").mkdir(parents=True)
        for name in COPIED:
            shutil.copy2(TOOLS / name, repo / "tools" / name)
        (repo / "examples").mkdir()
        (repo / "examples" / "tool-loop-demo").symlink_to(DEMO)
        git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", "-C", str(repo)]
        for cmd in (["init", "-q"], ["add", "-A"], ["commit", "-q", "-m", "t"]):
            subprocess.run(git + cmd, check=True, timeout=30)
        self.repo = repo
        bin_dir = self.tmp / "bin"
        bin_dir.mkdir()
        wrapper = bin_dir / "docker"
        wrapper.write_text(WRAPPER.replace("#!PYTHON", "#!" + sys.executable))
        wrapper.chmod(0o755)
        model = self.tmp / "model.gguf"  # the hash container really hashes this
        model.write_bytes(b"smoke model")
        self.env = {
            "PATH": f"{bin_dir}:{os.environ['PATH']}",
            "HOME": os.environ.get("HOME", str(self.tmp)),
            "REAL_DOCKER": shutil.which("docker"),
            "SLEEP_IMAGE": SLEEP_IMAGE,
            "MODEL_PATH": str(model),
            "MODEL_SHA256": hashlib.sha256(b"smoke model").hexdigest(),
            "FORMULA_CLI_SHA256": FORMULA_SHA,
            "SCHEDULE_OVERRIDE": "user-approved",
            "RUNNER_POLL_SECONDS": "1",
        }

    def start_runner(self, **extra):
        out = self.tmp / f"out-{len(list(self.tmp.iterdir()))}"
        env = dict(self.env, OUT_DIR=str(out), **extra)
        with open(self.tmp / f"{out.name}.stderr", "w") as err:
            proc = subprocess.Popen(
                [
                    "bash",
                    str(self.repo / "tools" / "run_tool_loop_benchmark.sh"),
                    "qwen3-4b-2507",
                    "smoke",
                    "17",
                ],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=err,
                start_new_session=True,
            )
        return proc, out

    def wait_for_harness(self, proc, seconds=180):
        deadline = time.time() + seconds
        while time.time() < deadline:
            names, _ = owned_state()
            if names and ops.HARNESS in names:
                return
            self.assertIsNone(proc.poll(), "runner exited before the harness started")
            time.sleep(1)
        self.fail("harness container never appeared")

    def assert_gone(self):
        self.assertEqual(owned_state(), ([], False))

    def test_runner_sigterm_leaves_nothing(self):
        proc, out = self.start_runner()
        self.wait_for_harness(proc)
        os.killpg(proc.pid, signal.SIGTERM)
        self.assertEqual(proc.wait(120), 143)
        self.assert_gone()
        text = (out / "provenance.txt").read_text()
        self.assertIn("stopped_by_signal=TERM", text)
        self.assertIn("cleanup_verified=true", text)

    def test_runner_sigkill_then_queue_cleanup_and_watchdog(self):
        proc, _ = self.start_runner()
        self.wait_for_harness(proc)
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(30)
        names, network = owned_state()
        self.assertIn(ops.SERVER, names)  # SIGKILL skips the trap: orphans remain
        # The watchdog (real docker, no lock holder, grace 0, inside the window) removes them.
        saturday = dt.datetime(2026, 10, 17, 12, 0).timestamp()

        class Clock:
            def time(self):
                return saturday

            def sleep(self, s):
                pass

        log = self.tmp / "watchdog.log"
        dog = run_watchdog.Watchdog(ops.Docker(), lambda: False, Clock(), log, grace=0)
        self.assertEqual(dog.tick(), "removed_orphans")
        self.assert_gone()
        row = json.loads(log.read_text().splitlines()[-1])
        self.assertTrue(row["ok"])

    def test_watchdog_outside_the_window_removes_even_with_a_lock(self):
        subprocess.run(
            ["docker", "network", "create", "--internal", ops.NETWORK],
            check=True,
            capture_output=True,
            timeout=60,
        )
        subprocess.run(
            [
                "docker",
                "run",
                "-d",
                "--init",
                "--pull",
                "never",
                "--name",
                ops.SERVER,
                "--network",
                ops.NETWORK,
                SLEEP_IMAGE,
                "sleep",
                "600",
            ],
            check=True,
            capture_output=True,
            timeout=60,
        )
        wednesday = dt.datetime(2026, 10, 14, 10, 0).timestamp()

        class Clock:
            def time(self):
                return wednesday

            def sleep(self, s):
                pass

        dog = run_watchdog.Watchdog(ops.Docker(), lambda: True, Clock(), self.tmp / "w.log")
        self.assertEqual(dog.tick(), "removed_outside_window")
        self.assert_gone()

    def driver(self, session):
        """A queue process: the real Queue, launcher, cleanup and signal handling."""
        script = self.tmp / "driver.py"
        script.write_text(
            textwrap.dedent(f"""\
            import json, signal, sys, time
            sys.path.insert(0, {str(self.repo / "tools")!r})
            import model_registry, run_queue as rq
            class Clock:
                def __init__(self):
                    self.now = {dt.datetime(2026, 10, 17, 12, 0).timestamp()}
                def time(self):
                    return self.now
                def sleep(self, s):
                    time.sleep(min(s, 0.5)); self.now += min(s, 0.5)
            reg = model_registry.load()
            doc = {{"schema": 1, "name": "smoke", "preflight_case": None,
                   "disk_checks": [{{"path": "/", "min_free_gb": 1}}],
                   "arms": [{{"id": "a", "model": "qwen3-4b-2507", "split": "smoke",
                             "arm": "SMOKE", "seeds": "17", "max_arm_seconds": 3600}}]}}
            open({str(self.tmp / "q.json")!r}, "w").write(json.dumps(doc))
            q = rq.Queue(rq.load_queue({str(self.tmp / "q.json")!r}, reg), reg,
                         {str(session)!r}, clock=Clock(), lock_probe=lambda: True,
                         poll_seconds=0.5, grace_seconds=30,
                         env={{"PATH": {self.env["PATH"]!r}, "HOME": {self.env["HOME"]!r},
                               "MIMIK_MODEL_ROOT": "/nonexistent-unused"}},
                         extra_env={{k: v for k, v in {self.env!r}.items()
                                    if k not in ("PATH", "HOME")}})
            q.prechecks = lambda arm: ("ok", None)
            def handler(signum, frame):
                q.interrupt(signum)
                sys.exit(128 + signum)
            signal.signal(signal.SIGTERM, handler)
            q.run()
            """)
        )
        with open(self.tmp / "driver.out", "w") as out:
            return subprocess.Popen(
                [sys.executable, str(script)],
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )

    def test_queue_sigterm_leaves_nothing(self):
        proc = self.driver(self.tmp / "session-term")
        self.wait_for_harness(proc)
        os.kill(proc.pid, signal.SIGTERM)  # the queue only, not its group
        proc.wait(180)
        self.assert_gone()
        manifest = json.loads((self.tmp / "session-term" / "manifest.json").read_text())
        self.assertEqual(manifest["state"], "stopped")

    def test_queue_sigkill_runner_notices_and_cleans(self):
        proc = self.driver(self.tmp / "session-kill")
        self.wait_for_harness(proc)
        os.kill(proc.pid, signal.SIGKILL)
        proc.wait(30)
        deadline = time.time() + 120
        while time.time() < deadline and owned_state() != ([], False):
            time.sleep(1)
        self.assert_gone()
        prov = next((self.tmp / "session-kill" / "arms" / "a").glob("attempt-*/provenance.txt"))
        self.assertIn("failure=orphan", prov.read_text())


if __name__ == "__main__":
    unittest.main()
