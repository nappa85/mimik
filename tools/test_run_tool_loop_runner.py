"""tools/run_tool_loop_benchmark.sh with a fake `docker` on PATH: no GPU, no server.

The fake docker keeps container and network state in files, hashes bind-mounted files for
real, records the server vector, and runs the real harness in --dry-run for the harness
container (the --endpoint is dropped), so these tests check that the runner's pass-through
reaches the llama.cpp args and the ledger header, and that the schedule gates, the cleanup
and the failure classes behave. Failing-first: run against the 2566040 runner (hard-coded
case statement) and the review regressions against 7bb41d6.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import run_queue as rq

TOOLS = Path(__file__).resolve().parent
DEMO = TOOLS.parent / "examples" / "tool-loop-demo"
FORMULA = os.environ.get("MIMIK_FORMULA_EVAL", "")
COPIED = (
    "run_tool_loop_benchmark.sh",
    "model_registry.py",
    "models.json",
    "run_queue.py",
    "toolloop_ops.py",
    "tool_loop_benchmark.py",
)
SUBSET = TOOLS / "case-subsets" / "dev-screen-40.json"
OWNED = "mimik-toolloop-v3-"

FAKE_DOCKER = r'''#!PYTHON
"""Fake docker for the runner tests: state in $FAKE_DOCKER_LOG/state.json."""
import hashlib, json, os, subprocess, sys, time
from pathlib import Path

log = Path(os.environ["FAKE_DOCKER_LOG"])
args = sys.argv[1:]
with open(log / "calls.jsonl", "a") as h:
    h.write(json.dumps(args) + "\n")
state_file = log / "state.json"
state = json.loads(state_file.read_text()) if state_file.exists() else {"c": {}, "n": {}}
env = os.environ
def save():
    state_file.write_text(json.dumps(state))
def advance(seconds):
    clock = env.get("MIMIK_TEST_CLOCK_FILE")
    if clock and seconds:
        Path(clock).write_text(str(float(Path(clock).read_text()) + float(seconds)))
def hang(what):
    if env.get("FAKE_HANG") == what:
        time.sleep(3600)
VALUE = {"--name", "--network", "--network-alias", "--user", "--cap-drop", "--security-opt",
         "--memory", "--memory-swap", "--cpus", "--pids-limit", "--device", "--group-add",
         "--mount", "--tmpfs", "-e", "--pull"}
if args[0] == "run":
    opts, i, mounts = {}, 1, {}
    while args[i].startswith("-"):
        if args[i] in VALUE:
            opts.setdefault(args[i], []).append(args[i + 1])
            if args[i] == "--mount":
                kv = dict(p.split("=", 1) for p in args[i + 1].split(",") if "=" in p)
                mounts[kv["dst"]] = kv.get("src")
            i += 2
        else:
            opts.setdefault(args[i], []).append(True)
            i += 1
    image, cmd = args[i], args[i + 1:]
    name = opts.get("--name", [""])[0]
    for need in ("--pull", "--init"):
        if need not in opts:
            sys.exit(f"fake docker: run without {need}")
    if not name.startswith("mimik-toolloop-v3-"):
        sys.exit("fake docker: unowned container name " + name)
    if cmd[:1] == ["sha256sum"]:
        if env.get("FAKE_CHECK_FAIL"):
            sys.exit(125)
        path = cmd[1]
        for dst, src in mounts.items():
            if path.startswith(dst) and src and src.startswith("/"):
                path = src + path[len(dst):]
        if path.startswith("/formula/"):
            path = env["MIMIK_FORMULA_EVAL"]
        else:
            advance(env.get("FAKE_HASH_SECONDS", 0))
        print(f"{hashlib.sha256(Path(path).read_bytes()).hexdigest()}  {cmd[1]}")
        sys.exit(0)
    if name.endswith("-srv"):
        (log / "server.json").write_text(json.dumps({"image": image, "args": cmd, "mounts": mounts, "opts": opts}))
        dies = bool(env.get("FAKE_SERVER_DIES_VRAM"))
        state["c"][name] = {"status": "exited" if dies else "running", "rc": 1 if dies else 0, "oom": False,
                            "log": "ggml_vulkan: ErrorOutOfDeviceMemory\n" if dies else
                                   "       eval time =   100.00 ms /    10 tokens\n"}
        save()
        print("cafe")
        sys.exit(0)
    # harness container: run the real harness as a dry run on the host paths
    mapped, skip = [], False
    for a in cmd:
        if skip:
            skip = False
            continue
        if a == "--endpoint":
            skip = True
            continue
        if a.startswith("/formula/"):
            a = env["MIMIK_FORMULA_EVAL"]
        for dst in ("/tools", "/pack", "/out"):
            if a == dst or a.startswith(dst + "/"):
                a = mounts[dst] + a[len(dst):]
        mapped.append(a)
    with open(log / "harness-calls.jsonl", "a") as h:
        h.write(json.dumps(cmd) + "\n")
    phase = "main" if "--arm" not in cmd or not cmd[cmd.index("--arm") + 1].endswith("-preflight") else "preflight"
    if env.get("FAKE_HARNESS_RUNNING") and phase == "main":
        state["c"][name] = {"status": "running", "rc": 0, "oom": False, "out": "", "err": ""}
        save()
        print("beef")
        sys.exit(0)
    proc = subprocess.run([sys.executable] + mapped[1:] + ["--dry-run"], capture_output=True, text=True)
    oom = bool(env.get("FAKE_MAIN_OOM")) and phase == "main"
    state["c"][name] = {"status": "exited", "rc": 137 if oom else proc.returncode, "oom": oom,
                        "out": proc.stdout, "err": proc.stderr}
    save()
    print("beef")
    sys.exit(0)
if args[0] == "inspect":
    fmt, name = args[2], args[3]
    c = state["c"].get(name)
    if c is None:
        sys.exit(1)
    if "{{.Id}}" in fmt:
        print("cafe" + name[-3:])
    elif "State.Status" in fmt:
        if c["status"] == "running":
            advance(env.get("FAKE_RUN_ADVANCE", 0))
        print(f"{c['status']} {c['rc']} {str(c['oom']).lower()}")
    elif "State.Running" in fmt:
        print(str(c["status"] == "running").lower())
    elif "OOMKilled" in fmt:
        print(str(c["oom"]).lower())
    sys.exit(0)
if args[0] == "exec":
    c = state["c"].get(args[1])
    print("{}")
    sys.exit(0 if c and c["status"] == "running" else 1)
if args[0] == "ps":
    hang("ps")
    names = sorted(state["c"])
    if "-q" in args:
        print("\n".join("id" + n for n in names if n.startswith("mimik-toolloop-v3-")))
    else:
        print("\n".join(names))
    sys.exit(0)
if args[0] == "logs":
    c = state["c"].get(args[1], {})
    sys.stdout.write(c.get("out", c.get("log", "")))
    sys.stderr.write(c.get("err", ""))
    sys.exit(0)
if args[0] == "stop":
    for n in args[3:]:
        if n in state["c"]:
            state["c"][n]["status"] = "exited"
    save()
    sys.exit(0)
if args[0] == "rm":
    for n in args[2:]:
        if env.get("FAKE_RM_FAIL") and n.endswith("-srv"):
            sys.exit(1)
        state["c"].pop(n, None)
    save()
    sys.exit(0)
if args[0] == "network":
    hang("network")
    sub = args[1]
    if sub == "inspect":
        if "mimik-toolloop-v3" not in state["n"]:
            sys.exit(1)
        print(str(state["n"]["mimik-toolloop-v3"]).lower())
        sys.exit(0)
    if sub == "create":
        state["n"]["mimik-toolloop-v3"] = "--internal" in args
        save()
        sys.exit(0)
    if sub == "rm":
        state["n"].pop(args[2], None)
        save()
        sys.exit(0)
    if sub == "ls":
        print("\n".join(["bridge"] + sorted(state["n"])))
        sys.exit(0)
if args[0] == "image":
    print("image_id=sha256:fake version=b11610 revision=x")
    sys.exit(0)
sys.exit(f"fake docker: unhandled {args}")
'''


@unittest.skipUnless(FORMULA and Path(FORMULA).exists(), "needs MIMIK_FORMULA_EVAL (formula CLI)")
@unittest.skipUnless(shutil.which("git") and shutil.which("flock"), "needs git and flock")
class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "repo"
        (self.repo / "tools").mkdir(parents=True)
        for name in COPIED:
            shutil.copy2(TOOLS / name, self.repo / "tools" / name)
        (self.repo / "examples").mkdir()
        (self.repo / "examples" / "tool-loop-demo").symlink_to(DEMO)
        git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
        git += ["-C", str(self.repo)]
        subprocess.run(git + ["init", "-q"], check=True)
        subprocess.run(git + ["add", "-A"], check=True)
        subprocess.run(git + ["commit", "-q", "-m", "t"], check=True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text(FAKE_DOCKER.replace("#!PYTHON", "#!" + sys.executable))
        docker.chmod(0o755)
        python3 = self.bin / "python3"
        python3.write_text(f'#!/bin/sh\nexec "{sys.executable}" "$@"\n')
        python3.chmod(0o755)
        self.log = self.tmp / "log"
        self.log.mkdir()
        real = self.tmp / "store" / "real.gguf"
        real.parent.mkdir()
        real.write_bytes(b"fake gguf")
        self.link = self.tmp / "link.gguf"
        self.link.symlink_to(real)
        self.real, self.sha = real, hashlib.sha256(b"fake gguf").hexdigest()
        self.formula_sha = hashlib.sha256(Path(FORMULA).read_bytes()).hexdigest()
        self.clock = self.tmp / "clock"

    def run_runner(self, *argv, **env):
        base = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.tmp),
            "FAKE_DOCKER_LOG": str(self.log),
            "MIMIK_FORMULA_EVAL": FORMULA,
            "FORMULA_CLI_SHA256": self.formula_sha,
            "LOCK_FILE": str(self.tmp / "lock"),
            "MODEL_PATH": str(self.link),
            "MODEL_SHA256": self.sha,
            "SCHEDULE_OVERRIDE": "user-approved",
            "RUNNER_POLL_SECONDS": "0",
            "OUT_DIR": str(self.tmp / f"out-{len(list(self.tmp.iterdir()))}"),
        }
        base.update({k: v for k, v in env.items() if v is not None})
        for k, v in env.items():
            if v is None:
                base.pop(k, None)
        proc = subprocess.run(
            ["bash", str(self.repo / "tools" / "run_tool_loop_benchmark.sh"), *argv],
            env=base,
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            check=False,
            timeout=300,
        )
        return proc, Path(base["OUT_DIR"])

    @staticmethod
    def future(weekday, hour):
        """A (y, m, d, h, 0) tuple on the given weekday, at least a day ahead of real time, so
        the harness (real wall clock) is still before the deadline the runner computes."""
        day = rq.dt.datetime.now().replace(hour=hour, minute=0, second=0, microsecond=0)
        day += rq.dt.timedelta(days=1)
        while day.weekday() != weekday:
            day += rq.dt.timedelta(days=1)
        return (day.year, day.month, day.day, hour, 0)

    def at_clock(self, when):
        """The test clock (needs both test variables) at a naive local time."""
        self.clock.write_text(str(rq.epoch(rq.dt.datetime(*when))))
        return {
            "SCHEDULE_OVERRIDE": None,
            "MIMIK_TEST_MODE": "1",
            "MIMIK_TEST_CLOCK_FILE": str(self.clock),
        }

    def ledger(self, out):
        prov = rq.parse_provenance(out / "provenance.txt")
        return [json.loads(x) for x in (out / prov["ledger_file"]).read_text().splitlines()]

    def server(self):
        return json.loads((self.log / "server.json").read_text())

    def calls(self):
        return [json.loads(x) for x in (self.log / "calls.jsonl").read_text().splitlines()]

    def state(self):
        path = self.log / "state.json"
        return json.loads(path.read_text()) if path.exists() else {"c": {}, "n": {}}

    def prov(self, out):
        return rq.parse_provenance(out / "provenance.txt")

    def assert_clean(self):
        state = self.state()
        self.assertEqual([n for n in state["c"] if n.startswith(OWNED)], [])
        self.assertEqual(state["n"], {})

    def test_pass_through_reaches_server_and_header(self):
        proc, out = self.run_runner(
            "qwen35-9b",
            "dev",
            CTX="24576",
            ARM="S40-c0",
            SEEDS="17",
            CASES_FILE=str(SUBSET),
            MAX_TURNS="12",
            MAX_TOKENS="2048",
            THINKING="off",
            UNTRUSTED="1",
            SEARCH_NOTES="1",
            NUDGE="0",
            REQUEST_TIMEOUT="420",
            QUIET="1",
            PREFLIGHT_CASE="dev-001",
            BUDGET_SECONDS="0",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr + (out / "main.stderr").read_text())
        server = self.server()
        args = server["args"]
        self.assertEqual(args[args.index("--ctx-size") + 1], "24576")
        self.assertEqual(server["mounts"]["/models/model.gguf"], str(self.real))  # resolved
        self.assertIn("--init", server["opts"])
        rows = self.ledger(out)
        header = rows[0]
        self.assertEqual((header["arm"], header["ctx"], header["max_turns"]), ("S40-c0", 24576, 12))
        self.assertEqual(header["sampling"]["max_tokens"], 2048)
        flags = header["flags"]
        self.assertEqual(
            (flags["untrusted"], flags["search_notes"], flags["nudge"]), (True, True, False)
        )
        self.assertEqual(header["server"]["args"], args)
        self.assertEqual(header["thinking_effective"], "off")
        doc = json.loads(SUBSET.read_text())
        self.assertEqual(header["case_subset"]["sha256"], doc["sha256"])
        self.assertEqual(header["planned_episodes"], 40)
        self.assertEqual(header["runner_provenance"]["registry_key"], "qwen35-9b")
        self.assertEqual(rows[-1]["event"], "run_summary")
        harness_calls = (self.log / "harness-calls.jsonl").read_text().splitlines()
        calls = [json.loads(x) for x in harness_calls]
        self.assertEqual(len(calls), 2)  # preflight, then the arm
        self.assertEqual(calls[1][calls[1].index("--request-timeout") + 1], "420")
        self.assertIn("--quiet-progress", calls[1])
        self.assertNotIn("outcome=", (out / "main.stderr").read_text())
        pre = [json.loads(x) for x in (out / "preflight-ledger.jsonl").read_text().splitlines()]
        self.assertEqual((pre[0]["case_order"], pre[0]["arm"]), (["dev-001"], "S40-c0-preflight"))
        prov = self.prov(out)
        self.assertEqual((prov["main_exit"], prov["preflight_exit"]), ("0", "0"))
        oom = (prov["main_oom_killed"], prov["preflight_oom_killed"])
        self.assertEqual(oom, ("false", "false"))
        self.assertEqual(prov["model_sha256_source"], "hashed")
        self.assertEqual(prov["cleanup_verified"], "true")
        self.assert_clean()

    def test_cleanup_stops_heavy_work_before_collecting_logs(self):
        proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        calls = self.calls()
        first_stop = next(i for i, c in enumerate(calls) if c[0] == "stop")
        server_logs = [i for i, c in enumerate(calls) if c[:2] == ["logs", OWNED + "srv"]]
        self.assertTrue(server_logs)
        self.assertLess(first_stop, server_logs[0])
        for c in calls:
            if c[0] == "run":
                self.assertIn("--init", c)
                self.assertIn("--pull", c)
        self.assert_clean()

    def test_defaults_are_c0_and_registry(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        header = self.ledger(out)[0]
        self.assertEqual(header["ctx"], 16384)
        self.assertEqual(
            header["flags"],
            {
                "nudge": True,
                "turns_remaining": False,
                "search_notes": False,
                "sanitize_echo": True,
                "pending_current": True,
                "untrusted": False,
            },
        )
        self.assertEqual((header["max_turns"], header["sampling"]["max_tokens"]), (8, 4096))
        self.assertEqual(header["sampling"]["temperature"], 0.1)
        self.assertEqual(header["thinking_capability"], "never")
        self.assertEqual(header["case_order"], ["dev-001", "dev-005", "dev-008"])
        self.assertIsNone(header["deadline_epoch"])  # schedule override: no cutoff
        args = self.server()["args"]
        self.assertEqual(args[args.index("--alias") + 1], "qwen3-4b-instruct-2507-q4_k_m")
        self.assertEqual(self.server()["opts"]["--memory"], ["14g"])

    def test_hash_cache_skips_the_second_hash_and_sees_mtime_changes(self):
        cache = self.tmp / "hash-cache.txt"
        self.run_runner("qwen3-4b-2507", "smoke", "17", HASH_CACHE=str(cache), CHECK_ONLY="1")
        fields = cache.read_text().rstrip("\n").split("\t")
        self.assertEqual((fields[0], fields[-1]), (str(self.real), self.sha))
        self.assertRegex(fields[2], r"^\d+\.\d{9}$")  # nanosecond mtime
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", HASH_CACHE=str(cache), CHECK_ONLY="1"
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.prov(out)["model_sha256_source"], "session_cache")
        st = self.real.stat()
        os.utime(self.real, ns=(st.st_atime_ns, st.st_mtime_ns + 1))
        _, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", HASH_CACHE=str(cache), CHECK_ONLY="1"
        )
        self.assertEqual(self.prov(out)["model_sha256_source"], "hashed")

    def test_check_only_never_starts_a_server(self):
        proc, out = self.run_runner("qwen36-35b-a3b", "smoke", N_CPU_MOE="32", CHECK_ONLY="1")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse((self.log / "server.json").exists())
        text = (out / "provenance.txt").read_text()
        self.assertIn("--n-cpu-moe 32", text)
        self.assertIn("check_only=1", text)

    def test_refusals(self):
        cases = {
            "unknown model": (("nope", "dev"), {}, 2),
            "not downloaded": (("granite42-3b", "dev"), {}, 2),
            "moe required": (("qwen36-35b-a3b", "smoke"), {}, 2),
            "bad flag": (("qwen35-9b", "dev"), {"UNTRUSTED": "yes"}, 2),
            "bad ctx": (("qwen35-9b", "dev"), {"CTX": "12k"}, 2),
            "both case args": (
                ("qwen35-9b", "dev"),
                {"CASES": "dev-001", "CASES_FILE": str(SUBSET)},
                2,
            ),
            "smoke cases": (("qwen35-9b", "smoke"), {"CASES": "dev-001"}, 2),
            "bad thinking": (("qwen35-9b", "dev"), {"THINKING": "maybe"}, 2),
            "bad window end": (("qwen35-9b", "dev"), {"WINDOW_END": "soon"}, 2),
            "model hash": (("qwen35-9b", "smoke"), {"MODEL_SHA256": "0" * 64}, 5),
            "model missing": (("qwen35-9b", "smoke"), {"MODEL_PATH": "/nonexistent.gguf"}, 5),
        }
        for name, (argv, env, code) in cases.items():
            with self.subTest(name=name):
                proc, _ = self.run_runner(*argv, **env)
                self.assertEqual(proc.returncode, code, proc.stderr)
        self.assertFalse((self.log / "server.json").exists())

    def test_registry_size_check_and_missing_root(self):
        root = self.tmp / "models"
        path = root / "target/local-rag-benchmark-20261015/Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
        path.parent.mkdir(parents=True)
        path.write_bytes(b"short")
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", MODEL_PATH=None, MIMIK_MODEL_ROOT=str(root)
        )
        self.assertEqual(proc.returncode, 5)
        self.assertIn("failure=model_size", (out / "provenance.txt").read_text())
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", MODEL_PATH=None, MIMIK_MODEL_ROOT=str(self.tmp / "absent")
        )
        self.assertEqual(proc.returncode, 5)
        self.assertIn("failure=model_missing", (out / "provenance.txt").read_text())

    def test_schedule_window_refused_on_a_weekday_afternoon(self):
        proc, out = self.run_runner("qwen35-9b", "smoke", **self.at_clock((2026, 10, 14, 10, 0)))
        self.assertEqual(proc.returncode, 10, proc.stderr)
        text = (out / "provenance.txt").read_text()
        self.assertIn("failure=window", text)
        self.assertIn("before hash", text)
        self.assertFalse((self.log / "server.json").exists())

    def test_one_test_variable_alone_does_not_move_the_clock(self):
        """Review fix 7: the old MIMIK_QUEUE_NOW hook is gone from the runner path."""
        env = self.at_clock((2026, 10, 14, 10, 0))
        env["MIMIK_TEST_MODE"] = None
        env["MIMIK_QUEUE_NOW"] = str(rq.epoch(rq.dt.datetime(2026, 10, 14, 10, 0)))
        proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17", **env)
        real_open = rq.heavy_allowed(time.time())
        self.assertEqual(proc.returncode == 10, not real_open, proc.stderr)

    def test_harness_start_plus_budget_within_the_cutoff(self):
        """Review fix 1: budget and deadline come from the moment of launch."""
        when = self.future(2, 20)  # a Wednesday 20:00
        env = self.at_clock(when)
        start = rq.epoch(rq.dt.datetime(*when))
        window_end = int(rq.window_end(start))
        proc, out = self.run_runner(
            "qwen3-4b-2507",
            "smoke",
            "17",
            WINDOW_END=str(window_end),
            PREFLIGHT_CASE="dev-001",
            FAKE_HASH_SECONDS="1800",
            **env,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        prov = self.prov(out)
        for phase in ("preflight", "main"):
            begun = int(prov[f"{phase}_start_epoch"])
            budget = int(prov[f"{phase}_budget_seconds"])
            self.assertLessEqual(begun + budget, window_end - 900, phase)
            self.assertGreaterEqual(begun, int(start) + 1800)
        header = self.ledger(out)[0]
        self.assertEqual(header["deadline_epoch"], window_end - 900)
        text = (out / "provenance.txt").read_text()
        for step in ("hash", "prewarm", "server", "preflight", "main"):
            self.assertIn(f"window_gate_{step}=open", text)

    def test_setup_that_crosses_the_cutoff_stops_before_the_server(self):
        when = self.future(2, 20)
        env = self.at_clock(when)
        cutoff = int(rq.epoch(rq.dt.datetime(*when))) + 3600
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", WINDOW_END=str(cutoff), FAKE_HASH_SECONDS="3000", **env
        )
        self.assertEqual(proc.returncode, 10, proc.stderr)
        text = (out / "provenance.txt").read_text()
        self.assertIn("failure=window", text)
        self.assertIn("before prewarm", text)
        self.assertFalse((self.log / "server.json").exists())
        self.assert_clean()

    def test_running_harness_stopped_at_the_window_edge(self):
        env = self.at_clock(self.future(3, 6))  # a Thursday 06:00: 3 h to the edge
        proc, out = self.run_runner(
            "qwen3-4b-2507",
            "smoke",
            "17",
            FAKE_HARNESS_RUNNING="1",
            FAKE_RUN_ADVANCE="3600",
            **env,
        )
        self.assertEqual(proc.returncode, 10, proc.stderr)
        text = (out / "provenance.txt").read_text()
        self.assertIn("stopped at the window edge during main", text)
        self.assertIn("cleanup_verified=true", text)
        self.assert_clean()

    def test_runner_stops_when_the_queue_is_gone(self):
        dead = subprocess.Popen(["true"])
        dead.wait()
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", QUEUE_PID=str(dead.pid), FAKE_HARNESS_RUNNING="1"
        )
        self.assertEqual(proc.returncode, 13, proc.stderr)
        self.assertIn("failure=orphan", (out / "provenance.txt").read_text())
        self.assert_clean()

    def test_unresolved_cleanup_exits_11(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17", FAKE_RM_FAIL="1")
        self.assertEqual(proc.returncode, 11, proc.stderr)
        prov = self.prov(out)
        self.assertEqual(prov["cleanup_verified"], "false")
        self.assertIn(OWNED + "srv", prov["cleanup_remaining"])

    def test_docker_failures_are_infra_docker(self):
        cases = {
            "check container": {"FAKE_CHECK_FAIL": "1"},
            "hanging network": {"FAKE_HANG": "network", "DOCKER_TIMEOUT": "2"},
        }
        for name, env in cases.items():
            with self.subTest(name=name):
                proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17", **env)
                self.assertEqual(proc.returncode, 12, proc.stderr)
                self.assertIn("failure=infra_docker", (out / "provenance.txt").read_text())

    def test_existing_external_network_refused(self):
        state = {"c": {}, "n": {"mimik-toolloop-v3": False}}
        (self.log / "state.json").write_text(json.dumps(state))
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 12, proc.stderr)
        self.assertIn("not internal", (out / "provenance.txt").read_text())

    def test_vram_failure_is_infra_vram(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17", FAKE_SERVER_DIES_VRAM="1")
        self.assertEqual(proc.returncode, 4, proc.stderr)
        self.assertIn("failure=infra_vram", (out / "provenance.txt").read_text())
        self.assert_clean()

    def test_preflight_ok_then_main_oom(self):
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", PREFLIGHT_CASE="dev-001", FAKE_MAIN_OOM="1"
        )
        self.assertEqual(proc.returncode, 137, proc.stderr)
        prov = self.prov(out)
        self.assertEqual((prov["preflight_exit"], prov["preflight_oom_killed"]), ("0", "false"))
        self.assertEqual((prov["main_exit"], prov["main_oom_killed"]), ("137", "true"))

    def test_preflight_failure_stops_before_the_arm(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", PREFLIGHT_CASE="no-such-case")
        self.assertEqual(proc.returncode, 6, proc.stderr)
        self.assertIn("failure=preflight", (out / "provenance.txt").read_text())
        calls = (self.log / "harness-calls.jsonl").read_text().splitlines()
        self.assertEqual(len(calls), 1)
        self.assert_clean()

    def test_shellcheck_clean_if_available(self):
        if not shutil.which("shellcheck"):
            self.skipTest("shellcheck not in this image (run koalaman/shellcheck separately)")
        proc = subprocess.run(
            ["shellcheck", str(TOOLS / "run_tool_loop_benchmark.sh")],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stdout)


if __name__ == "__main__":
    unittest.main()
