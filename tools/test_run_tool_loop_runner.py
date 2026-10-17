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
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import run_queue as rq

# Never see a real user-granted schedule override (toolloop_ops.OVERRIDE_ENV).
os.environ["MIMIK_SCHEDULE_OVERRIDE_FILE"] = "/nonexistent/mimik-schedule-override.json"

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
    "run_watchdog.py",
    "heldout-v3-pin.json",
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
         "--mount", "--tmpfs", "-e", "--pull", "--label"}
def labels_of(opts):
    return dict(l.split("=", 1) for l in opts.get("--label", []))
def label_filter(args):
    for n, a in enumerate(args):
        if a == "--filter" and args[n + 1].startswith("label="):
            return args[n + 1][len("label="):]
    return None
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
    labels = labels_of(opts)
    if labels.get("mimik.toolloop") != "v3" or "mimik.toolloop.run" not in labels:
        sys.exit("fake docker: run without the ownership labels")
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
            reg = env.get("CHECK_REGISTRY")
            if reg and Path(reg).exists():
                (log / "registry-during-hash.txt").write_text(Path(reg).read_text())
            if env.get("FAKE_HASH_SLEEP"):
                (log / "hash.pid").write_text(str(os.getpid()))
                time.sleep(float(env["FAKE_HASH_SLEEP"]))
        print(f"{hashlib.sha256(Path(path).read_bytes()).hexdigest()}  {cmd[1]}")
        sys.exit(0)
    if name.endswith("-srv"):
        (log / "server.json").write_text(json.dumps({"image": image, "args": cmd, "mounts": mounts, "opts": opts}))
        dies = bool(env.get("FAKE_SERVER_DIES_VRAM"))
        state["c"][name] = {"status": "exited" if dies else "running", "rc": 1 if dies else 0, "oom": False, "label": "v3", "run": labels["mimik.toolloop.run"],
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
        for dst in ("/tools", "/pack", "/out", "/heldout/cases-heldout.json"):
            if dst in mounts and (a == dst or a.startswith(dst + "/")):
                a = mounts[dst] + a[len(dst):]
        mapped.append(a)
    with open(log / "harness-calls.jsonl", "a") as h:
        h.write(json.dumps(cmd) + "\n")
    phase = "main" if "--arm" not in cmd or not cmd[cmd.index("--arm") + 1].endswith("-preflight") else "preflight"
    if env.get("FAKE_HARNESS_RUNNING") and phase == "main":
        state["c"][name] = {"status": "running", "rc": 0, "oom": False, "out": "", "err": "", "label": "v3", "run": labels["mimik.toolloop.run"]}
        save()
        print("beef")
        sys.exit(0)
    extra = ["--dry-run"] + (["--allow-heldout-gold-replay"] if "heldout" in mapped else [])
    proc = subprocess.run([sys.executable] + mapped[1:] + extra, capture_output=True, text=True)
    oom = bool(env.get("FAKE_MAIN_OOM")) and phase == "main"
    state["c"][name] = {"status": "exited", "rc": 137 if oom else proc.returncode, "oom": oom,
                        "out": proc.stdout, "err": proc.stderr, "label": "v3", "run": labels["mimik.toolloop.run"]}
    save()
    print("beef")
    sys.exit(0)
if args[0] == "inspect":
    fmt, name = args[2], args[3]
    c = state["c"].get(name)
    if c is None:
        sys.exit(1)
    if "mimik.toolloop.run" in fmt:
        print(c.get("run") or "")
    elif "Config.Labels" in fmt:
        print(c.get("label") or "")
    elif "{{.Id}}" in fmt:
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
if args[0] == "context":
    print(env.get("FAKE_CONTEXT", "default"))
    sys.exit(0)
if args[0] in ("pause", "unpause"):
    with open(log / "pauses.jsonl", "a") as h:
        h.write(json.dumps(args) + "\n")
    if env.get("FAKE_PAUSE_FAIL"):
        sys.exit(1)
    sys.exit(0 if args[1] in state["c"] else 1)
if args[0] == "ps":
    hang("ps")
    wanted = label_filter(args)
    names = sorted(n for n, c in state["c"].items()
                   if wanted is None or ("mimik.toolloop=" + str(c.get("label"))) == wanted)
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
        net = state["n"].get("mimik-toolloop-v3")
        if net is None:
            sys.exit(1)
        print(f"{str(net['internal']).lower()} {net['label'] or '<no value>'}")
        sys.exit(0)
    if sub == "create":
        labels = dict(a.split("=", 1) for n, a in enumerate(args) if n and args[n - 1] == "--label")
        state["n"]["mimik-toolloop-v3"] = {"internal": "--internal" in args,
                                           "label": labels.get("mimik.toolloop")}
        save()
        sys.exit(0)
    if sub == "rm":
        state["n"].pop(args[2], None)
        save()
        sys.exit(0)
    if sub == "ls":
        wanted = label_filter(args)
        names = [n for n, v in state["n"].items()
                 if wanted is None or "mimik.toolloop=" + str(v["label"]) == wanted]
        print("\n".join(([] if wanted else ["bridge"]) + sorted(names)))
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
        # The refusal test needs a not-downloaded entry; flip one in the copy only.
        copied = self.repo / "tools" / "models.json"
        doc = json.loads(copied.read_text())
        doc["models"]["llama31-8b"]["downloaded"] = False
        copied.write_text(json.dumps(doc, indent=2) + "\n")
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
        # A fake hwmon tree: GPU cool, fan manual at 255 (the runner's fan precondition).
        self.dri = self.tmp / "dri"  # stand-ins for /dev/dri/renderD128 and card0
        self.dri.mkdir()
        for node in ("renderD128", "card0"):
            (self.dri / node).write_text("")
        self.hwmon = self.tmp / "hwmon"
        chip = self.hwmon / "hwmon3"
        chip.mkdir(parents=True)
        for name, value in (
            ("name", "amdgpu"),
            ("temp1_input", "44000"),
            ("temp1_label", "edge"),
            ("pwm1_enable", "1"),
            ("pwm1", "255"),
            ("fan1_input", "3200"),
        ):
            (chip / name).write_text(value + "\n")

    def runner_env(self, **env):
        return self._env(env)

    def run_runner(self, *argv, **env):
        base = self._env(env)
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

    def _env(self, env):
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
            "MIMIK_TEST_MODE": "1",
            "MIMIK_TEST_HWMON": str(self.hwmon),
            "MIMIK_TEST_DRI": str(self.dri),
            "OUT_DIR": str(self.tmp / f"out-{len(list(self.tmp.iterdir()))}"),
        }
        base.update({k: v for k, v in env.items() if v is not None})
        for k, v in env.items():
            if v is None:
                base.pop(k, None)
        return base

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
        owned = [n for n, c in state["c"].items() if n.startswith(OWNED) and c.get("label") == "v3"]
        self.assertEqual(owned, [])
        self.assertEqual({n: v for n, v in state["n"].items() if v.get("label") == "v3"}, {})

    def synthetic_heldout(self, tamper=False):
        """A synthetic held-out file (one demo dev case under a new id) and a matching pin in
        the copied repo; never the real frozen file (§11 item 8)."""
        dev = json.loads((DEMO / "cases-dev.json").read_text())["cases"][0]
        case = dict(dev, id="h3-test-001")
        path = self.tmp / "frozen" / "cases-heldout.json"
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps({"encoding": 1, "split": "heldout", "cases": [case]}))
        pin = {
            "encoding": 1,
            "split": "heldout",
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "cases": 1,
            "outcomes": {case["expected"]["outcome"]: 1},
            "default_path": str(path),
            "freeze_record": "docs/tool-loop-heldout-v3-freeze.md",
            "verdict_case_sets": [
                {
                    "name": "full",
                    "rule": "all",
                    "size": 1,
                    "sha256": hashlib.sha256(json.dumps(["h3-test-001"]).encode()).hexdigest(),
                }
            ],
        }
        (self.repo / "tools" / "heldout-v3-pin.json").write_text(json.dumps(pin))
        # Test-only: move the code anchor in the copied repo (production cannot: review of
        # 0b9742d removed every override).
        harness = self.repo / "tools" / "tool_loop_benchmark.py"
        text = harness.read_text()
        anchor = 'HELDOUT_FROZEN_SHA256 = "6f11af05f27347ea31a50996b0e6b6eea4ccbfa3254a085cd7699d4d8ec68974"'
        self.assertIn(anchor, text)
        text = text.replace(anchor, f'HELDOUT_FROZEN_SHA256 = "{pin["sha256"]}"')
        pin_sha = hashlib.sha256(
            (self.repo / "tools" / "heldout-v3-pin.json").read_bytes()
        ).hexdigest()
        text = re.sub(
            r'HELDOUT_PIN_SHA256 = "[0-9a-f]{64}"', f'HELDOUT_PIN_SHA256 = "{pin_sha}"', text
        )
        text = re.sub(
            r"HELDOUT_VERDICT_CASE_SETS = \[.*\]  # noqa: E501",
            "HELDOUT_VERDICT_CASE_SETS = "
            + json.dumps(pin["verdict_case_sets"])
            + "  # noqa: E501",
            text,
        )
        harness.write_text(text)
        git = [
            "git",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-C",
            str(self.repo),
        ]
        subprocess.run(git + ["commit", "-qam", "pin"], check=True)
        if tamper:
            path.write_text(path.read_text() + " ")
        return path, pin

    def test_heldout_runs_only_the_pinned_file_mounted_read_only(self):
        """§11 item 8: before, the held-out split ran the pack's own file with no hash gate."""
        path, pin = self.synthetic_heldout()
        proc, out = self.run_runner(
            "qwen35-9b", "heldout", SEEDS="17", BUDGET_SECONDS="0", HELDOUT_CASES=str(path)
        )
        self.assertEqual(proc.returncode, 0, proc.stderr + (out / "main.stderr").read_text())
        calls = [json.loads(x) for x in (self.log / "harness-calls.jsonl").read_text().splitlines()]
        main = calls[-1]
        self.assertEqual(main[main.index("--heldout-cases") + 1], "/heldout/cases-heldout.json")
        header = self.ledger(out)[0]
        self.assertEqual(header["heldout_cases_sha256"], pin["sha256"])
        docker = (self.log / "calls.jsonl").read_text()
        self.assertIn(f"src={path},dst=/heldout/cases-heldout.json,readonly", docker)
        self.assertIn(f"heldout_cases_sha256={pin['sha256']}", (out / "provenance.txt").read_text())

    def test_heldout_hash_mismatch_or_missing_file_is_refused(self):
        path, _ = self.synthetic_heldout(tamper=True)
        proc, out = self.run_runner("qwen35-9b", "heldout", SEEDS="17", HELDOUT_CASES=str(path))
        self.assertEqual(proc.returncode, 5)
        self.assertIn("failure=heldout_hash", (out / "provenance.txt").read_text())
        proc, out = self.run_runner(
            "qwen35-9b", "heldout", SEEDS="17", HELDOUT_CASES=str(self.tmp / "absent.json")
        )
        self.assertEqual(proc.returncode, 5)
        self.assertIn("failure=heldout_missing", (out / "provenance.txt").read_text())
        self.assertFalse((self.log / "server.json").exists())

    def test_heldout_path_with_a_newline_is_refused(self):
        """Review of 0b9742d (provenance injection): no tab or newline in HELDOUT_CASES."""
        path, _ = self.synthetic_heldout()
        link = self.tmp / "frozen" / "a\ncheck_only=1"
        link.symlink_to(path)
        proc, out = self.run_runner("qwen35-9b", "heldout", SEEDS="17", HELDOUT_CASES=str(link))
        self.assertEqual(proc.returncode, 2)
        self.assertNotIn("check_only", rq.parse_provenance(out / "provenance.txt"))

    def test_heldout_progress_is_forced_quiet(self):
        """Round 2 (Sonnet M1): QUIET=0 printed per-case outcomes for held-out runs."""
        path, _ = self.synthetic_heldout()
        proc, out = self.run_runner(
            "qwen35-9b", "heldout", SEEDS="17", QUIET="0", HELDOUT_CASES=str(path)
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        main = [json.loads(x) for x in (self.log / "harness-calls.jsonl").read_text().splitlines()][
            -1
        ]
        self.assertIn("--quiet-progress", main)
        self.assertIn("quiet_forced=heldout", (out / "provenance.txt").read_text())

    def test_heldout_path_with_a_space_is_refused_and_injects_nothing(self):
        """Round 2 (Sol 3): a space-delimited `failure=window` in the path became a field."""
        path, _ = self.synthetic_heldout()
        spaced = self.tmp / "frozen failure=window"
        spaced.write_bytes(path.read_bytes())
        proc, out = self.run_runner("qwen35-9b", "heldout", SEEDS="17", HELDOUT_CASES=str(spaced))
        self.assertEqual(proc.returncode, 2)
        prov = rq.parse_provenance(out / "provenance.txt")
        self.assertEqual(prov.get("failure"), "config")

    def test_failure_details_never_add_provenance_lines(self):
        """Space Bunny B4: an unvalidated value in a failure message could add a line."""
        proc, out = self.run_runner(
            "qwen35-9b", "dev", CASES_FILE=str(self.tmp / "x\nfailure=heldout_hash")
        )
        self.assertEqual(proc.returncode, 2)
        prov = rq.parse_provenance(out / "provenance.txt")
        self.assertEqual(prov.get("failure"), "config")
        lines = (out / "provenance.txt").read_text().splitlines()
        self.assertFalse(any(line.startswith("failure=heldout_hash") for line in lines))

    def test_an_edited_pin_json_alone_is_refused(self):
        """Review of 0b9742d: the pin JSON must equal the harness's code anchor."""
        path, pin = self.synthetic_heldout()
        harness = self.repo / "tools" / "tool_loop_benchmark.py"
        text = harness.read_text()
        moved = 'HELDOUT_FROZEN_SHA256 = "' + "1" * 64 + '"'
        harness.write_text(text.replace(f'HELDOUT_FROZEN_SHA256 = "{pin["sha256"]}"', moved))
        proc, out = self.run_runner("qwen35-9b", "heldout", SEEDS="17", HELDOUT_CASES=str(path))
        self.assertEqual(proc.returncode, 2)
        self.assertIn("code anchor", (out / "provenance.txt").read_text())

    def test_optional_arrays_expand_safely_under_set_u(self):
        """Review of 0b9742d (Sonnet F6): empty arrays under set -u fail on bash < 4.4."""
        text = (TOOLS / "run_tool_loop_benchmark.sh").read_text()
        names = ("heldout_mount", "heldout_args", "cutoff_args", "gpu_devices", "reg_server_args")
        names += ("quiet", "phase_deadline", "case_args", "arm_args")
        for name in names:
            uses = [line for line in text.splitlines() if f"{name}[@]" in line]
            self.assertTrue(uses)
            for line in uses:
                self.assertIn(f"${{{name}[@]+", line, line)

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
            "not downloaded": (("llama31-8b", "dev"), {}, 2),
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
            "check container": ({"FAKE_CHECK_FAIL": "1"}, 12),
            # A hung docker also cannot verify the cleanup: exit 11, first failure kept.
            "hanging network": ({"FAKE_HANG": "network", "DOCKER_TIMEOUT": "2"}, 11),
        }
        for name, (env, code) in cases.items():
            with self.subTest(name=name):
                proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17", **env)
                self.assertEqual(proc.returncode, code, proc.stderr)
                self.assertEqual(self.prov(out)["failure"], "infra_docker")

    def test_existing_external_network_refused(self):
        for net in ({"internal": False, "label": "v3"}, {"internal": True, "label": None}):
            with self.subTest(net=net):
                state = {"c": {}, "n": {"mimik-toolloop-v3": net}}
                (self.log / "state.json").write_text(json.dumps(state))
                proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
                self.assertEqual(proc.returncode, 12, proc.stderr)
                self.assertIn("not internal and owned", (out / "provenance.txt").read_text())
                if net["label"] is None:  # a foreign network is never removed
                    self.assertIn("mimik-toolloop-v3", self.state()["n"])

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

    # ---- second review round
    def test_foreign_same_prefix_container_survives_cleanup(self):
        foreign = {"status": "running", "rc": 0, "oom": False, "label": None}
        state = {"c": {OWNED + "foreign": foreign}, "n": {}}
        (self.log / "state.json").write_text(json.dumps(state))
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn(OWNED + "foreign", self.state()["c"])
        self.assertEqual(self.prov(out)["cleanup_verified"], "true")
        self.assertIn("run_id", self.prov(out))

    def test_endpoint_override_or_context_refused(self):
        for env in (
            {"DOCKER_HOST": "tcp://elsewhere"},
            {"DOCKER_CONFIG": "/tmp/c"},
            {"DOCKER_CONTEXT": "x"},
            {"FAKE_CONTEXT": "remote"},
        ):
            with self.subTest(env=env):
                proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17", **env)
                self.assertEqual(proc.returncode, 2, proc.stderr)
        self.assertFalse((self.log / "server.json").exists())

    def test_server_frozen_before_diagnostics_on_normal_completion(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        calls = self.calls()
        index = {tuple(c[:2]): i for i, c in reversed(list(enumerate(calls)))}
        pause = index[("pause", OWNED + "srv")]
        stop_srv = next(
            i for i, c in enumerate(calls) if c[:4] == ["stop", "-t", "10", OWNED + "srv"]
        )
        logs_srv = next(i for i, c in enumerate(calls) if c[:2] == ["logs", OWNED + "srv"])
        self.assertLess(pause, stop_srv)
        self.assertLess(stop_srv, logs_srv)
        prov = self.prov(out)
        self.assertEqual(prov["server_paused_for_diagnostics"], "true")
        self.assertIn("server_swap_peak", prov)
        text = (out / "provenance.txt").read_text()
        self.assertLess(text.index("main_exit="), text.index("server_swap_peak="))

    def test_cleanup_not_verified_when_docker_cannot_list(self):
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", CHECK_ONLY="1", FAKE_HANG="ps", DOCKER_TIMEOUT="2"
        )
        self.assertEqual(proc.returncode, 11, proc.stderr)
        self.assertEqual(self.prov(out)["cleanup_verified"], "false")

    def test_stop_kills_a_running_hash(self):
        """Sonnet N4: timeout --foreground keeps the hash in the runner's process group."""
        env = dict(os.environ)
        proc = subprocess.Popen(
            [
                "bash",
                str(self.repo / "tools" / "run_tool_loop_benchmark.sh"),
                "qwen3-4b-2507",
                "smoke",
                "17",
            ],
            env=self.runner_env(FAKE_HASH_SLEEP="120"),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        del env
        pid_file = self.log / "hash.pid"
        deadline = time.time() + 60
        while not pid_file.exists() and time.time() < deadline:
            time.sleep(0.2)
        hash_pid = int(pid_file.read_text())
        os.killpg(proc.pid, signal.SIGTERM)
        self.assertEqual(proc.wait(60), 143)
        time.sleep(0.5)
        with self.assertRaises(ProcessLookupError):
            os.kill(hash_pid, 0)

    def test_hash_bounded_by_the_time_left(self):
        when = self.future(2, 20)
        env = self.at_clock(when)
        cutoff = int(rq.epoch(rq.dt.datetime(*when))) + 905  # 5 s left after the margin
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", WINDOW_END=str(cutoff), FAKE_HASH_SLEEP="60", **env
        )
        self.assertEqual(proc.returncode, 10, proc.stderr)
        text = (out / "provenance.txt").read_text()
        self.assertIn("did not finish before the cutoff", text)
        self.assertIn("hash_limit_seconds=5", text)

    # ---- round 3
    def test_pause_failure_still_reads_diagnostics_before_the_stop(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17", FAKE_PAUSE_FAIL="1")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        prov = self.prov(out)
        self.assertEqual(prov["server_paused_for_diagnostics"], "false")
        self.assertIn("server_swap_peak", prov)
        self.assertIn("vram_used_end", prov)
        calls = self.calls()
        stop_srv = next(
            i for i, c in enumerate(calls) if c[:4] == ["stop", "-t", "10", OWNED + "srv"]
        )
        cgroup_reads = [
            i for i, c in enumerate(calls) if c[:2] == ["inspect", "-f"] and "{{.Id}}" in c[2]
        ]
        self.assertTrue(cgroup_reads)
        self.assertLess(cgroup_reads[0], stop_srv)

    def test_check_containers_registered_while_they_run(self):
        registry = self.tmp / "checks.txt"
        proc, _ = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", CHECK_ONLY="1", CHECK_REGISTRY=str(registry)
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        during = (self.log / "registry-during-hash.txt").read_text().split()
        self.assertTrue(any(n.startswith(OWNED + "check-") for n in during), during)
        self.assertEqual(registry.read_text().split(), [])  # unregistered afterwards

    def test_fixed_names_of_another_run_are_not_stopped(self):
        other = {"status": "running", "rc": 0, "oom": False, "label": "v3", "run": "other-run"}
        state = {"c": {OWNED + "harness": other}, "n": {}}
        (self.log / "state.json").write_text(json.dumps(state))
        proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17", CHECK_ONLY="1")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse(any(c[0] in ("stop", "pause") for c in self.calls()))

    # ---- thermal guard
    def test_temperatures_before_and_peak_in_provenance(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        prov = self.prov(out)
        self.assertEqual(prov["temp_amdgpu_edge_before"], "44.0")
        self.assertEqual(prov["temp_amdgpu_edge_peak"], "44.0")

    def test_fan_not_manual_refuses_before_the_server_load(self):
        for key, value in (("pwm1_enable", "2"), ("pwm1", "200")):
            with self.subTest(key=key):
                (self.hwmon / "hwmon3" / "pwm1_enable").write_text("1\n")
                (self.hwmon / "hwmon3" / "pwm1").write_text("255\n")
                (self.hwmon / "hwmon3" / key).write_text(value + "\n")
                proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
                self.assertEqual(proc.returncode, 14, proc.stderr)
                text = (out / "provenance.txt").read_text()
                self.assertIn("failure=fan_not_manual", text)
                self.assertIn("sudo tee", text)
                self.assertFalse((self.log / "server.json").exists())

    # ---- CPU-only mode (GPU cooling fault, 2026-10-16)
    def test_cpu_arm_has_no_gpu_devices_and_ngl_0(self):
        (self.hwmon / "hwmon3" / "pwm1_enable").write_text("2\n")  # auto fan: irrelevant on CPU
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17", DEVICE="cpu", THREADS="6")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        server = self.server()
        self.assertNotIn("--device", server["opts"])
        self.assertNotIn("--group-add", server["opts"])
        args = server["args"]
        self.assertEqual(args[args.index("--n-gpu-layers") + 1], "0")
        self.assertEqual(args[args.index("--threads") + 1], "6")
        prov = self.prov(out)
        self.assertEqual(prov["device"], "cpu")
        self.assertEqual(prov["cpu_temperature"], "unavailable")
        self.assertIn("temp_amdgpu_edge_before", prov)
        self.assertNotIn("fan_pwm1_enable", prov)  # the fan precondition is skipped
        header = self.ledger(out)[0]
        self.assertEqual(header["device"], "cpu")
        calls = [json.loads(x) for x in (self.log / "harness-calls.jsonl").read_text().splitlines()]
        self.assertEqual(calls[-1][calls[-1].index("--request-timeout") + 1], "900")

    def test_gpu_arm_still_needs_the_fan_and_gpu_devices(self):
        proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("--device", self.server()["opts"])
        (self.hwmon / "hwmon3" / "pwm1_enable").write_text("2\n")
        proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17", DEVICE="gpu")
        self.assertEqual(proc.returncode, 14, proc.stderr)

    def test_bad_device_or_threads_refused(self):
        for env in ({"DEVICE": "tpu"}, {"DEVICE": "cpu", "THREADS": "0"}):
            with self.subTest(env=env):
                proc, _ = self.run_runner("qwen3-4b-2507", "smoke", "17", **env)
                self.assertEqual(proc.returncode, 2, proc.stderr)

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
