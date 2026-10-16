#!/usr/bin/env python3
"""Unattended queue for tools/run_tool_loop_benchmark.sh (plan §11 items 26-27). Prototype tier.

Stdlib only. One queue file (JSON) lists arms in order; each arm runs the runner once (one
retry for pre-declared infrastructure failures only). The queue is an explicit state machine:

  arm:   pending -> waiting_window -> prechecks -> running -> complete
                                         |            |-> retry (infra) -> prechecks
                                         |            |-> failed | incomplete |
                                         |            |   incomplete_with_errors |
                                         |            |   incomplete_by_window | arm_timeout
                                         |-> refused (config, missing path) | skipped
  queue: running -> done | stopped(<reason>) | crashed(<error>)

An arm is complete only if the runner exited 0, its cleanup was verified (by the runner and
by the queue), its provenance is consistent and the ledger passes the harness's own strict
validation (tool_loop_benchmark.load_ledgers) plus the queue's requested plan: the exact
(case_id, seed) job set, a live run, and the arm's server config, flags, sampling, max_turns,
top_k, benchmark version and harness sha256. Heavy work runs only inside the machine schedule
(AGENTS.md): Saturday and Sunday at any time, Monday-Friday outside 09:00-18:00 local time
(/etc/localtime). An arm starts only if its estimate fits before the window closes; the
runner gets the absolute window end, and a running arm is stopped at the edge
(incomplete_by_window, rerun whole in a later window, never resumed mid-arm). The queue also
starts tools/run_watchdog.py, an independent process that removes owned containers outside
the window or when no queue or runner holds the lock.

Usage:
  run_queue.py run QUEUE.json [--session-dir DIR]
  run_queue.py window-check [--cutoff EPOCH] [--margin S]  (exit 0 open, 1 closed)
  run_queue.py check QUEUE.json         (validate the queue file and print the plan)
Start it detached:  setsid nohup python3 tools/run_queue.py run Q.json > q.console 2>&1 < /dev/null &
The console gets one line per arm transition and never a per-case outcome (plan §11 item 3).
"""

import argparse
import datetime as dt
import fcntl
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

import model_registry
import run_watchdog
import tool_loop_benchmark as tl
import toolloop_ops as ops
from toolloop_ops import (  # noqa: F401 (re-exported for callers and tests)
    epoch,
    heavy_allowed,
    local,
    longest_window_seconds,
    next_window_start,
    window_end,
)

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "tools" / "run_tool_loop_benchmark.sh"
WATCHDOG = ROOT / "tools" / "run_watchdog.py"
PACK = ROOT / "examples" / "tool-loop-demo"
SCHEMA = 1
# Test-only clock for window-check: honoured only with both variables set; the queue never
# passes them to a child and the watchdog never reads them.
TEST_MODE_ENV, TEST_CLOCK_ENV = "MIMIK_TEST_MODE", "MIMIK_TEST_CLOCK_FILE"
SMOKE_CASES = ("dev-001", "dev-005", "dev-008")
MIN_STOP_MARGIN = 300
FIT_MARGIN = 900  # the runner refuses a step with fewer than 900 s left before the cutoff
INFRA_CLASSES = ("server_start", "server_health", "oom", "infra_vram")  # retried once
ENV_ALLOW = ("PATH", "HOME", "MIMIK_MODEL_ROOT")  # the only inherited variables
# Runner pass-through variables a queue file may set (with the C0 / registry defaults).
PASS_THROUGH = (
    "CTX",
    "MAX_TURNS",
    "MAX_TOKENS",
    "THINKING",
    "TEMPERATURE",
    "TOP_P",
    "REQUEST_TIMEOUT",
    "HEALTH_WAIT",
    "SRV_MEMORY",
    "NUDGE",
    "TURNS_REMAINING",
    "SEARCH_NOTES",
    "SANITIZE_ECHO",
    "PENDING_CURRENT",
    "UNTRUSTED",
    "N_CPU_MOE",
    "BUDGET_SECONDS",
)
FLAG_ENV = {
    "NUDGE": ("nudge", True),
    "TURNS_REMAINING": ("turns_remaining", False),
    "SEARCH_NOTES": ("search_notes", False),
    "SANITIZE_ECHO": ("sanitize_echo", True),
    "PENDING_CURRENT": ("pending_current", True),
    "UNTRUSTED": ("untrusted", False),
}
VALUE_RE = re.compile(r"^[A-Za-z0-9._-]{0,64}$")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
SEEDS_RE = re.compile(r"^[0-9]+(,[0-9]+)*$")
CASES_RE = re.compile(r"^[A-Za-z0-9_-]+(,[A-Za-z0-9_-]+)*$")
EVAL_RE = re.compile(r"(?<!prompt )eval time =\s*([0-9.]+) ms /\s*([0-9]+) (?:tokens|runs)")
VRAM_FILE = Path("/sys/class/drm/card0/device/mem_info_vram_used")


class QueueError(ValueError):
    pass


SystemClock = ops.SystemClock


# ---------------------------------------------------------------- queue file


def _env_values(env, where):
    if not isinstance(env, dict):
        raise QueueError(f"{where}: env must be an object")
    for key, value in env.items():
        if key not in PASS_THROUGH:
            raise QueueError(f"{where}: env key {key!r} is not a runner pass-through variable")
        if isinstance(value, bool) or not isinstance(value, (str, int, float)):
            raise QueueError(f"{where}: env {key} must be a string or number")
        if not VALUE_RE.match(str(value)):
            raise QueueError(f"{where}: env {key} has an unsafe value")
    return {k: str(v) for k, v in env.items()}


ARM_KEYS = {
    "id",
    "model",
    "split",
    "seeds",
    "arm",
    "cases",
    "cases_file",
    "env",
    "required",
    "retry",
    "seconds_per_episode",
    "max_arm_seconds",
    "g0",
    "n_cpu_moe_from_g0",
    "preflight_case",
}
QUEUE_KEYS = {
    "schema",
    "name",
    "description",
    "defaults",
    "arms",
    "session_deadline",
    "max_consecutive_failures",
    "disk_checks",
    "preflight_case",
    "quiet",
    "stop_margin_seconds",
    "io_bytes_per_second",
    "startup_seconds",
    "estimate_factor",
    "g0",
}
G0_KEYS = {"vram_max_bytes", "swap_growth_max_mib", "deadline_seconds"}


def load_queue(path: Path, registry: dict) -> dict:
    doc = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA:
        raise QueueError(f"queue: schema must be {SCHEMA}")
    unknown = set(doc) - QUEUE_KEYS
    if unknown:
        raise QueueError(f"queue: unknown keys {sorted(unknown)}")
    if not isinstance(doc.get("name"), str) or not ID_RE.match(doc["name"]):
        raise QueueError("queue: name must match " + ID_RE.pattern)
    defaults = doc.get("defaults", {})
    if not isinstance(defaults, dict) or set(defaults) - (ARM_KEYS - {"id", "model"}):
        raise QueueError("queue: defaults may hold arm fields except id and model")
    q = {
        "name": doc["name"],
        "session_deadline": None,
        "max_consecutive_failures": doc.get("max_consecutive_failures", 2),
        "disk_checks": doc.get(
            "disk_checks",
            [
                {"path": "/var/lib/docker", "min_free_gb": 10},
                {"path": "/media/data", "min_free_gb": 10},
            ],
        ),
        "quiet": doc.get("quiet", True),
        "stop_margin_seconds": doc.get("stop_margin_seconds", MIN_STOP_MARGIN),
        "io_bytes_per_second": doc.get("io_bytes_per_second", 12_600_000),
        "startup_seconds": doc.get("startup_seconds", 180),
        "estimate_factor": doc.get("estimate_factor", 1.25),
        # §11 item 27: "no swap growth" (0 tolerance).
        "g0": {"vram_max_bytes": 7_500_000_000, "swap_growth_max_mib": 0, "deadline_seconds": None},
        "arms": [],
    }
    if doc.get("session_deadline") is not None:
        try:
            q["session_deadline"] = epoch(dt.datetime.fromisoformat(doc["session_deadline"]))
        except (TypeError, ValueError):
            raise QueueError("queue: session_deadline must be a local ISO time") from None
    g0 = doc.get("g0", {})
    if not isinstance(g0, dict) or set(g0) - G0_KEYS:
        raise QueueError(f"queue: g0 may hold {sorted(G0_KEYS)}")
    q["g0"].update(g0)
    for name in ("max_consecutive_failures", "stop_margin_seconds", "startup_seconds"):
        if not isinstance(q[name], int) or isinstance(q[name], bool) or q[name] < 0:
            raise QueueError(f"queue: {name} must be a non-negative integer")
    if q["stop_margin_seconds"] < MIN_STOP_MARGIN:
        raise QueueError(f"queue: stop_margin_seconds must be >= {MIN_STOP_MARGIN}")
    if q["max_consecutive_failures"] < 1:
        raise QueueError("queue: max_consecutive_failures must be >= 1")
    if not isinstance(q["quiet"], bool):
        raise QueueError("queue: quiet must be true or false")
    for check in q["disk_checks"]:
        if (
            not isinstance(check, dict)
            or set(check) != {"path", "min_free_gb"}
            or not os.path.isabs(str(check["path"]))
            or not isinstance(check["min_free_gb"], (int, float))
        ):
            raise QueueError("queue: disk_checks entries are {path (absolute), min_free_gb}")
    preflight = doc.get("preflight_case", "dev-001")
    arms = doc.get("arms")
    if not isinstance(arms, list) or not arms:
        raise QueueError("queue: arms must be a non-empty list")
    seen = set()
    for n, raw in enumerate(arms):
        where = f"arms[{n}]"
        if not isinstance(raw, dict) or set(raw) - ARM_KEYS:
            raise QueueError(f"{where}: unknown keys {sorted(set(raw) - ARM_KEYS)}")
        arm = {"preflight_case": preflight, "required": True, "retry": True, "g0": False}
        arm.update(defaults)
        arm.update(raw)
        arm["env"] = _env_values(dict(defaults.get("env", {}), **raw.get("env", {})), where)
        if not isinstance(arm.get("id"), str) or not ID_RE.match(arm["id"]) or arm["id"] in seen:
            raise QueueError(f"{where}: id must be unique and match {ID_RE.pattern}")
        seen.add(arm["id"])
        if arm.get("model") not in registry["models"]:
            raise QueueError(f"{where}: model {arm.get('model')!r} is not in the registry")
        if arm.get("split") not in ("smoke", "dev", "heldout"):
            raise QueueError(f"{where}: split must be smoke, dev or heldout")
        arm["seeds"] = str(arm.get("seeds", "17"))
        if not SEEDS_RE.match(arm["seeds"]):
            raise QueueError(f"{where}: seeds must be comma-separated integers")
        if not isinstance(arm.get("arm"), str) or not ID_RE.match(arm["arm"].lower()):
            raise QueueError(f"{where}: arm label is required")
        if arm.get("cases") is not None and arm.get("cases_file") is not None:
            raise QueueError(f"{where}: cases and cases_file are exclusive")
        if arm.get("cases") is not None and not CASES_RE.match(str(arm["cases"])):
            raise QueueError(f"{where}: cases must be comma-separated ids")
        if arm["split"] == "smoke" and (arm.get("cases") or arm.get("cases_file")):
            raise QueueError(f"{where}: smoke has a fixed case list")
        if arm.get("cases_file") is not None:
            arm["cases_file"] = str((ROOT / arm["cases_file"]).resolve())
        for flag in ("required", "retry", "g0"):
            if not isinstance(arm[flag], bool):
                raise QueueError(f"{where}: {flag} must be true or false")
        if arm["g0"]:
            arm["required"], arm["retry"] = False, False  # a failed load is a G0 result
            if "N_CPU_MOE" not in arm["env"]:
                raise QueueError(f"{where}: a g0 arm needs env N_CPU_MOE")
        if arm.get("n_cpu_moe_from_g0") and "N_CPU_MOE" in arm["env"]:
            raise QueueError(f"{where}: n_cpu_moe_from_g0 and env N_CPU_MOE are exclusive")
        pf = arm.get("preflight_case")
        if pf is not None and not CASES_RE.match(str(pf)):
            raise QueueError(f"{where}: preflight_case must be a dev case id or null")
        for name in ("seconds_per_episode", "max_arm_seconds"):
            value = arm.get(name)
            if value is not None and (isinstance(value, bool) or not value > 0):
                raise QueueError(f"{where}: {name} must be positive")
        q["arms"].append(arm)
    return q


# ---------------------------------------------------------------- evidence


def parse_provenance(path: Path) -> dict:
    """key=value tokens of provenance.txt (first value wins); failure_detail keeps its line."""
    values = {}
    if not path.exists():
        return values
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("failure_detail="):
            values.setdefault("failure_detail", line.split("=", 1)[1])
            continue
        for key, value in re.findall(r"(?:^|\s)([a-z][a-z0-9_]*)=('[^']*'|\S*)", line):
            values.setdefault(key, value.strip("'"))
    return values


def check_ledger(path: Path, plan: dict) -> dict:
    """Strict completeness against the requested plan.

    The harness's own merge gate (load_ledgers: header, run_summary, declared job set,
    planned == rows, duplicates, aborted runs) runs first; then the header must equal the
    plan field by field, the (case_id, seed) set must be exactly the requested one, and the
    run must be live. Error and not_run episodes are counted and make the arm incomplete.
    """
    result = {"complete": False, "reasons": [], "planned": None, "run": 0, "not_run": 0}
    result["errors"] = 0
    try:
        rows = [json.loads(x) for x in path.read_text(encoding="utf-8").splitlines() if x.strip()]
    except (OSError, json.JSONDecodeError) as error:
        result["reasons"].append(f"unreadable ledger: {type(error).__name__}")
        return result
    if not rows or rows[0].get("event") != "run_started":
        result["reasons"].append("no run_started header")
        return result
    header = rows[0]
    episodes = [r for r in rows if r.get("event") == "episode"]
    result["planned"] = header.get("planned_episodes")
    result["not_run"] = sum(1 for r in episodes if r.get("outcome") == "not_run")
    result["errors"] = sum(1 for r in episodes if r.get("outcome") == "error")
    result["run"] = len(episodes) - result["not_run"]
    aborted = next((r for r in rows if r.get("event") == "run_aborted"), None)
    result["aborted"] = aborted is not None
    if aborted is not None:
        result["abort_reason"] = aborted.get("reason")
    try:
        tl.load_ledgers([path])
    except SystemExit as refusal:
        result["reasons"].append(f"harness gate: {refusal}")
    for key, value in plan["header"].items():
        got = header.get(key)
        if key == "case_subset_sha256":
            got = (header.get("case_subset") or {}).get("sha256")
        if json.dumps(got, sort_keys=True) != json.dumps(value, sort_keys=True):
            result["reasons"].append(f"header {key} {got!r} != expected {value!r}")
    jobs = [(r.get("case_id"), r.get("seed")) for r in episodes]
    if len(jobs) != len(set(jobs)):
        result["reasons"].append("duplicate (case_id, seed) rows")
    if set(jobs) != plan["jobs"]:
        missing, extra = len(plan["jobs"] - set(jobs)), len(set(jobs) - plan["jobs"])
        result["reasons"].append(
            f"job set differs from the plan ({missing} missing, {extra} extra)"
        )
    if result["planned"] != len(plan["jobs"]):
        result["reasons"].append(f"planned_episodes {result['planned']} != {len(plan['jobs'])}")
    if result["not_run"]:
        result["reasons"].append(f"not_run {result['not_run']}")
    if result["errors"]:
        result["reasons"].append(f"error episodes {result['errors']}")
    result["complete"] = not result["reasons"]
    return result


def server_seconds_per_token(path: Path):
    """Generation seconds per token from llama.cpp's 'eval time' log lines (None if absent)."""
    if not path.exists():
        return None
    ms = tokens = 0.0
    for match in EVAL_RE.finditer(path.read_text(encoding="utf-8", errors="replace")):
        ms += float(match.group(1))
        tokens += int(match.group(2))
    return None if not tokens else ms / 1000.0 / tokens


def _int_or_none(value):
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def read_vram():
    try:
        return int(VRAM_FILE.read_text().strip())
    except (OSError, ValueError):
        return None


# ---------------------------------------------------------------- process launcher


class ProcessLauncher:
    """Starts the runner in its own process group, stdin /dev/null, output to files."""

    def start(self, argv, env, stdout_path: Path, stderr_path: Path):
        with open(stdout_path, "wb") as out, open(stderr_path, "wb") as err:
            proc = subprocess.Popen(
                argv,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=err,
                start_new_session=True,
            )
        return ProcessHandle(proc)


class ProcessHandle:
    def __init__(self, proc):
        self.proc = proc

    def poll(self):
        return self.proc.poll()

    def signal(self, sig):
        try:
            os.killpg(self.proc.pid, sig)
        except ProcessLookupError:
            pass

    def wait(self, timeout):
        try:
            return self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return None


class WatchdogSupervisor:
    """Starts tools/run_watchdog.py and checks its heartbeat (the readiness handshake).

    A watchdog started elsewhere (standalone) counts if its heartbeat is fresh. No heavy work
    may start or continue while this reports the watchdog unavailable.
    """

    def __init__(self, heartbeat: Path, log_path: Path, fresh_seconds=90, ready_timeout=60):
        self.heartbeat, self.log_path = Path(heartbeat), Path(log_path)
        self.fresh_seconds, self.ready_timeout = fresh_seconds, ready_timeout
        self.proc = None

    def alive(self) -> bool:
        try:
            beat = json.loads(self.heartbeat.read_text(encoding="utf-8"))
            os.kill(int(beat["pid"]), 0)
        except (OSError, ValueError, KeyError, TypeError):
            return False
        return time.time() - float(beat["time"]) <= self.fresh_seconds

    def ensure(self) -> bool:
        if self.alive():
            return True
        with open(self.log_path.with_suffix(".console"), "ab") as out:
            self.proc = subprocess.Popen(
                [sys.executable, str(WATCHDOG), "--log", str(self.log_path)],
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=out,
                start_new_session=True,
                env={k: os.environ[k] for k in ENV_ALLOW if k in os.environ},
            )
        deadline = time.time() + self.ready_timeout
        while time.time() < deadline:
            if self.alive():
                return True
            time.sleep(0.5)
        return False


class NoWatchdog:
    """The safe default: without a watchdog, no heavy work starts."""

    def alive(self):
        return False

    def ensure(self):
        return False


CASE_ID_RE = re.compile(r"\b[a-z]{2,8}-\d{3}\b")


def console_safe(text) -> str:
    """No case id ever reaches the console (plan §11 item 3)."""
    return CASE_ID_RE.sub("<case>", str(text))


def pack_case_ids(split: str) -> list:
    doc = json.loads((PACK / f"cases-{split}.json").read_text(encoding="utf-8"))
    return [c["id"] for c in doc["cases"]]


# ---------------------------------------------------------------- the queue


class Queue:
    def __init__(
        self,
        queue: dict,
        registry: dict,
        session_dir: Path,
        clock=None,
        launcher=None,
        disk_free=None,
        lock_probe=None,
        cleanup=None,
        case_ids=pack_case_ids,
        runner=RUNNER,
        poll_seconds=30.0,
        sleep_chunk=600.0,
        grace_seconds=60.0,
        console=None,
        env=None,
        vram_reader=None,
        cooldown_seconds=30.0,
        vram_wait_seconds=180.0,
        extra_env=None,
        watchdog=None,
    ):
        self.q = queue
        self.registry = registry
        self.dir = Path(session_dir)
        self.clock = clock or SystemClock()
        self.launcher = launcher or ProcessLauncher()
        self.disk_free = disk_free or (lambda p: shutil.disk_usage(p).free)
        locks = ops.lock_paths(ops.main_checkout(ROOT))
        self.lock_probe = lock_probe or (lambda: not ops.lock_held(locks["runner"]))
        self.cleanup = cleanup or ops.Docker().cleanup
        self.case_ids = case_ids
        self.runner = Path(runner)
        self.poll_seconds = poll_seconds
        self.sleep_chunk = sleep_chunk
        self.grace_seconds = grace_seconds
        self.console = console or (lambda line: print(line, flush=True))
        self.base_env = dict(os.environ if env is None else env)
        self.extra_env = dict(extra_env or {})
        self.watchdog = watchdog or NoWatchdog()  # Python-only seam (real-docker smoke test)
        self.vram_reader = vram_reader or read_vram
        self.cooldown_seconds = cooldown_seconds
        self.vram_wait_seconds = vram_wait_seconds
        self.harness_sha256 = tl.file_digest(self.runner.parent / "tool_loop_benchmark.py")
        self.started = self.clock.time()
        self.state = "running"
        self.stop_reason = None
        self.measured_spe = {}
        self.g0_results = []
        self.g0_started = None
        self.g0_closed = False
        self.handle = None
        self.arms = [
            {"id": a["id"], "state": "pending", "attempts": [], "config": a} for a in queue["arms"]
        ]
        self.hash_cache = self.dir / "hash-cache.txt"

    # -- logging and manifest
    def log(self, event, **fields):
        row = {"time": dt.datetime.fromtimestamp(self.clock.time()).isoformat(), "event": event}
        row.update(fields)
        with open(self.dir / "queue.log", "a", encoding="utf-8") as handle:
            handle.write(json.dumps(row, sort_keys=True, default=str) + "\n")

    def transition(self, arm, state, **fields):
        arm["state"] = state
        arm.update({k: v for k, v in fields.items() if k in ("reason", "selected_n_cpu_moe")})
        self.log("arm_state", arm=arm["id"], state=state, **fields)
        reason = fields.get("reason")
        stamp = local(self.clock.time()).isoformat(timespec="seconds")
        suffix = "" if reason is None else f" ({console_safe(reason)})"
        self.console(f"{stamp} {arm['id']}: {state}{suffix}")
        self.write_manifest()

    def manifest(self) -> dict:
        planned = list(self.arms)
        missing = [a["id"] for a in planned if a["config"]["required"] and a["state"] != "complete"]
        return {
            "queue": self.q["name"],
            "state": self.state,
            "stop_reason": self.stop_reason,
            "started": local(self.started).isoformat(timespec="seconds"),
            "updated": local(self.clock.time()).isoformat(timespec="seconds"),
            "planned_arms": len(planned),
            "complete_arms": sum(1 for a in planned if a["state"] == "complete"),
            "missing_required": missing,
            "study_complete": self.state == "done" and not missing,
            "g0": self.g0_results,
            "arms": [
                {
                    "id": a["id"],
                    "model": a["config"]["model"],
                    "arm": a["config"]["arm"],
                    "split": a["config"]["split"],
                    "required": a["config"]["required"],
                    "vram_budget_bytes": self.registry["models"][a["config"]["model"]][
                        "vram_budget_bytes"
                    ],
                    "state": a["state"],
                    "reason": a.get("reason"),
                    "selected_n_cpu_moe": a.get("selected_n_cpu_moe"),
                    "ledger": a.get("ledger"),
                    "attempts": a["attempts"],
                }
                for a in planned
            ],
        }

    def write_manifest(self):
        tmp = self.dir / "manifest.json.tmp"
        tmp.write_text(json.dumps(self.manifest(), indent=2, default=str) + "\n", encoding="utf-8")
        tmp.replace(self.dir / "manifest.json")

    def stop(self, reason):
        self.state, self.stop_reason = "stopped", reason
        self.log("queue_stopped", reason=reason)
        self.console(f"queue stopped: {console_safe(reason)}")
        self.write_manifest()

    # -- plan and estimates
    def requested_cases(self, arm) -> list:
        cfg = arm["config"]
        if cfg["split"] == "smoke":
            return list(SMOKE_CASES)
        if cfg.get("cases"):
            return cfg["cases"].split(",")
        if cfg.get("cases_file"):
            return json.loads(Path(cfg["cases_file"]).read_text(encoding="utf-8"))["case_ids"]
        return list(self.case_ids(cfg["split"]))

    def episodes(self, arm) -> int:
        return len(self.requested_cases(arm)) * len(arm["config"]["seeds"].split(","))

    def seconds_per_episode(self, arm):
        cfg = arm["config"]
        if cfg.get("seconds_per_episode"):
            return float(cfg["seconds_per_episode"]), "queue file"
        if cfg["model"] in self.measured_spe:
            return self.measured_spe[cfg["model"]], "measured this session"
        entry = self.registry["models"][cfg["model"]]
        return float(entry["seconds_per_episode"]), "registry"

    def estimate(self, arm) -> float:
        cfg = arm["config"]
        entry = self.registry["models"][cfg["model"]]
        spe, _ = self.seconds_per_episode(arm)
        episodes = self.episodes(arm) + (1 if cfg.get("preflight_case") else 0)
        io = entry["size"] / self.q["io_bytes_per_second"]
        reads = (0 if self.hashed(cfg["model"]) else 1) + (1 if entry["prewarm"] else 0)
        startup = self.q["startup_seconds"] + reads * io
        return startup + episodes * spe * self.q["estimate_factor"]

    def hashed(self, key) -> bool:
        sha = self.registry["models"][key]["sha256"]
        return self.hash_cache.exists() and sha in self.hash_cache.read_text(encoding="utf-8")

    def plan(self, arm, env) -> dict:
        """What the ledger must show for this arm (strict completeness, review fix 6)."""
        cfg = arm["config"]
        entry = self.registry["models"][cfg["model"]]
        ctx = int(env.get("CTX", entry["server"]["ctx"]))
        moe = env.get("N_CPU_MOE")
        server = {
            "image": entry["server_image"],
            "args": model_registry.server_args(entry, ctx, None if moe is None else int(moe)),
        }
        flags = {
            name: env.get(var, "1" if on else "0") == "1" for var, (name, on) in FLAG_ENV.items()
        }
        seeds = [int(s) for s in cfg["seeds"].split(",")]
        header = {
            "event": "run_started",
            "benchmark_version": tl.BENCHMARK_VERSION,
            "harness_sha256": self.harness_sha256,
            "dry_run": False,
            "arm": cfg["arm"],
            "model_label": entry["label"],
            "model_sha256": entry["sha256"],
            "split": "dev" if cfg["split"] == "smoke" else cfg["split"],
            "seeds": seeds,
            "ctx": ctx,
            "max_turns": int(env.get("MAX_TURNS", 8)),
            "top_k": tl.DEFAULT_TOP_K,
            "flags": tl.resolve_flags(flags),
            "sampling": {
                "temperature": float(env.get("TEMPERATURE", 0.1)),
                "top_p": float(env.get("TOP_P", 0.95)),
                "max_tokens": int(env.get("MAX_TOKENS", 4096)),
                "tool_choice": "auto",
                "thinking": env.get("THINKING", "off"),
                "cache_prompt": True,
            },
            "thinking_capability": entry["capabilities"]["thinking"],
            "server": server,
        }
        if cfg.get("cases_file"):
            doc = json.loads(Path(cfg["cases_file"]).read_text(encoding="utf-8"))
            header["case_subset_sha256"] = doc.get("sha256")
        cases = self.requested_cases(arm)
        return {"header": header, "jobs": {(c, s) for s in seeds for c in cases}}

    # -- prechecks
    def model_path(self, key) -> Path:
        return model_registry.model_file(self.registry, key, env=self.base_env)

    def prechecks(self, arm):
        """Returns ('ok', None) | ('refuse', why) | ('stop', why)."""
        cfg = arm["config"]
        entry = self.registry["models"][cfg["model"]]
        if not entry["downloaded"]:
            return "refuse", f"{cfg['model']} is registered but not downloaded"
        root = model_registry.model_root(self.registry, env=self.base_env)
        if not root.exists():
            return "refuse", f"model root {root} does not exist"
        path = self.model_path(cfg["model"])
        if not path.exists():
            return "refuse", f"model file {path} does not exist"
        for check in self.q["disk_checks"]:
            where = check["path"]
            if not os.path.exists(where):
                return "refuse", f"disk check path {where} does not exist"
            free = self.disk_free(where)
            if free < check["min_free_gb"] * 1e9:
                return "stop", f"disk low: {where} has {free / 1e9:.1f} GB free"
        if not self.lock_probe():
            return "stop", "lock held: another tool-loop run is active"
        return "ok", None

    # -- schedule
    def wait_for_window(self, arm, not_before=None) -> bool:
        """Sleep until the arm fits in a heavy window (starting at or after not_before).
        False if it never can."""
        estimate = self.estimate(arm)
        margin = max(self.q["stop_margin_seconds"], FIT_MARGIN)
        if estimate + margin > longest_window_seconds(self.clock.time()):
            self.transition(arm, "skipped", reason=f"estimate {estimate:.0f}s never fits a window")
            return False
        announced = False
        while True:
            now = self.clock.time()
            deadline = self.q["session_deadline"]
            if deadline is not None and now + estimate > deadline:
                self.transition(arm, "skipped", reason="does not fit before the session deadline")
                return False
            later = not_before is None or now >= not_before
            if later and heavy_allowed(now) and now + estimate <= window_end(now) - margin:
                return True
            if not announced:
                self.transition(arm, "waiting_window", reason=f"estimate {estimate:.0f}s")
                announced = True
            if not later:
                target = not_before
            elif heavy_allowed(now):
                target = window_end(now) + 1
            else:
                target = next_window_start(now)
            self.clock.sleep(min(self.sleep_chunk, max(1.0, target - now)))

    # -- one attempt
    def env_for(self, arm, attempt_dir: Path, window_close: float):
        """Child environment from an allowlist; inherited pass-through values never leak."""
        cfg = arm["config"]
        env = {k: self.base_env[k] for k in ENV_ALLOW if k in self.base_env}
        env["TZ"] = ":/etc/localtime"
        env.update(cfg["env"])
        env.update(
            ARM=cfg["arm"],
            SEEDS=cfg["seeds"],
            OUT_DIR=str(attempt_dir),
            HASH_CACHE=str(self.hash_cache),
            QUIET="1" if self.q["quiet"] else "0",
            WINDOW_END=str(int(window_close)),
            QUEUE_PID=str(os.getpid()),
        )
        if cfg.get("preflight_case"):
            env["PREFLIGHT_CASE"] = str(cfg["preflight_case"])
        if cfg.get("cases"):
            env["CASES"] = cfg["cases"]
        if cfg.get("cases_file"):
            env["CASES_FILE"] = cfg["cases_file"]
        if arm.get("selected_n_cpu_moe") is not None:
            env["N_CPU_MOE"] = str(arm["selected_n_cpu_moe"])
        env.update(self.extra_env)
        return env

    def cleanup_checked(self) -> dict:
        try:
            result = self.cleanup()
        except Exception as error:  # a cleanup crash is an unresolved cleanup
            result = {"ok": False, "errors": [f"{type(error).__name__}: {error}"]}
        if not isinstance(result, dict) or "ok" not in result:
            result = {"ok": False, "errors": ["cleanup returned no verification"]}
        self.log("cleanup", **result)
        return result

    def run_attempt(self, arm, number: int) -> dict:
        cfg = arm["config"]
        attempt_dir = self.dir / "arms" / arm["id"] / f"attempt-{number}"
        attempt_dir.mkdir(parents=True, exist_ok=False)
        start = self.clock.time()
        close = window_end(start)
        env = self.env_for(arm, attempt_dir, close)
        argv = [str(self.runner), cfg["model"], cfg["split"]]
        estimate = self.estimate(arm)
        limits = {"window": close - self.q["stop_margin_seconds"]}
        limits["arm_deadline"] = start + (cfg.get("max_arm_seconds") or 2 * estimate + 600)
        if self.q["session_deadline"] is not None:
            limits["session_deadline"] = self.q["session_deadline"]
        g0_deadline = self.q["g0"]["deadline_seconds"]
        if cfg["g0"] and g0_deadline is not None and self.g0_started is not None:
            limits["g0_deadline"] = self.g0_started + g0_deadline
        record = {
            "attempt": number,
            "dir": str(attempt_dir),
            "started": local(start).isoformat(timespec="seconds"),
            "estimate_seconds": round(estimate),
            "window_end": local(close).isoformat(timespec="seconds"),
            "env": {k: env[k] for k in sorted(env) if k in PASS_THROUGH},
            "excluded": True,
        }
        arm["attempts"].append(record)
        self.transition(arm, "running", attempt=number, estimate=round(estimate))
        rc = stopped_by = None
        try:
            self.handle = self.launcher.start(
                argv, env, attempt_dir / "runner.stdout", attempt_dir / "runner.stderr"
            )
            while True:
                rc = self.handle.poll()
                if rc is not None:
                    break
                now = self.clock.time()
                hit = [name for name, at in limits.items() if now >= at]
                if not hit and not self.watchdog.alive() and not self.watchdog.ensure():
                    hit = ["watchdog"]
                if hit:
                    stopped_by = hit[0]
                    rc = self.terminate()
                    break
                nearest = min(limits.values())
                self.clock.sleep(min(self.poll_seconds, max(0.5, nearest - now)))
        finally:
            if self.handle is not None and self.handle.poll() is None:
                rc = self.terminate()  # an exception mid-attempt: never leave the runner
            self.handle = None
            record["queue_cleanup"] = self.cleanup_checked()  # after every attempt
        record["runner_exit"] = rc
        record["ended"] = local(self.clock.time()).isoformat(timespec="seconds")
        record.update(self.classify(arm, attempt_dir, env, rc, stopped_by, record["queue_cleanup"]))
        return record

    def terminate(self):
        """SIGTERM to the runner's process group, then SIGKILL; None if it will not die."""
        self.handle.signal(signal.SIGTERM)
        rc = self.handle.wait(self.grace_seconds)
        if rc is None:
            self.log("runner_ignored_sigterm")
            self.handle.signal(signal.SIGKILL)
            rc = self.handle.wait(self.grace_seconds)
        return rc

    def classify(self, arm, attempt_dir: Path, env, rc, stopped_by, cleanup) -> dict:
        prov = parse_provenance(attempt_dir / "provenance.txt")
        out = {"provenance": bool(prov), "failure": prov.get("failure")}
        oom_keys = ("server_oom_killed", "preflight_oom_killed", "main_oom_killed")
        oom = [k for k in oom_keys if prov.get(k) == "true"]
        ledger = attempt_dir / prov["ledger_file"] if prov.get("ledger_file") else None
        try:
            check = check_ledger(ledger, self.plan(arm, env)) if ledger else None
        except Exception as error:  # noqa: BLE001 (fails this arm, not the queue)
            out["ledger_error"] = f"{type(error).__name__}: {error}"
            return dict(out, status="failed", infra=None, reason=f"ledger check error: {error}")
        if check is not None:
            out["ledger"] = str(ledger)
            out["ledger_check"] = check
        if not cleanup.get("ok"):
            return dict(
                out,
                status="failed",
                infra=None,
                reason="queue cleanup unresolved",
                cleanup_unresolved=True,
            )
        if rc is None:
            return dict(
                out,
                status="failed",
                infra=None,
                reason="runner did not exit",
                cleanup_unresolved=True,
            )
        if stopped_by is not None:
            status = {
                "window": "incomplete_by_window",
                "arm_deadline": "arm_timeout",
                "session_deadline": "session_deadline",
                "g0_deadline": "arm_timeout",
                "watchdog": "failed",
            }[stopped_by]
            return dict(out, status=status, infra=None, reason=f"stopped: {stopped_by}")
        if not prov:
            return dict(out, status="failed", infra=None, reason=f"no provenance.txt (exit {rc})")
        if prov.get("cleanup_verified") != "true":
            return dict(
                out,
                status="failed",
                infra=None,
                reason="runner cleanup not verified",
                cleanup_unresolved=prov.get("cleanup_verified") == "false",
            )
        if oom:
            return dict(out, status="failed", infra="oom", reason=f"OOM-killed: {', '.join(oom)}")
        failure = prov.get("failure")
        if failure in ("server_start", "server_health", "infra_vram"):
            return dict(out, status="failed", infra=failure, reason=prov.get("failure_detail"))
        if failure == "window":
            return dict(
                out, status="incomplete_by_window", infra=None, reason=prov.get("failure_detail")
            )
        if failure is not None:
            return dict(out, status="failed", infra=None, reason=prov.get("failure_detail"))
        if prov.get("main_exit") is None:
            return dict(out, status="failed", infra=None, reason=f"no main_exit (exit {rc})")
        if check and check.get("aborted") and "health" in str(check.get("abort_reason")):
            return dict(out, status="failed", infra="server_health", reason=check["abort_reason"])
        if prov.get("main_exit") != "0" or rc != 0 or check is None:
            reason = f"main_exit {prov.get('main_exit')}, runner exit {rc}"
            return dict(out, status="failed", infra=None, reason=reason)
        if check["complete"]:
            return dict(out, status="complete", infra=None)
        if check["errors"]:
            return dict(
                out, status="incomplete_with_errors", infra=None, reason="; ".join(check["reasons"])
            )
        deadline_rows = check["not_run"] and prov.get("main_cutoff")
        if deadline_rows:
            return dict(out, status="incomplete_by_window", infra=None, reason="window budget")
        return dict(out, status="incomplete", infra=None, reason="; ".join(check["reasons"]))

    # -- measurements
    def record_measurements(self, arm, record):
        cfg = arm["config"]
        prov = parse_provenance(Path(record["dir"]) / "provenance.txt")
        wall = _int_or_none(prov.get("main_wall_seconds"))
        check = record.get("ledger_check") or {}
        if record["status"] == "complete" and wall and check.get("run"):
            self.measured_spe[cfg["model"]] = wall / check["run"]
        if not cfg["g0"]:
            return
        vram = [_int_or_none(prov.get(k)) for k in ("vram_used_loaded", "vram_used_end")]
        vram = [v for v in vram if v is not None]
        swap_peak = _int_or_none(prov.get("server_swap_peak"))
        before = _int_or_none(prov.get("swap_used_mib_before"))
        after = _int_or_none(prov.get("swap_used_mib_after"))
        if swap_peak is not None:
            swap, swap_source = swap_peak / 2**20, "server cgroup memory.swap.peak"
        elif None not in (before, after):
            # Host-wide swap moves with other load: recorded, but indeterminate (never closes G0).
            swap, swap_source = after - before, "host-wide (cgroup swap not readable)"
            self.log("g0_swap_fallback", arm=arm["id"], host_swap_growth_mib=swap)
        else:
            swap, swap_source = None, None
        spt = server_seconds_per_token(Path(record["dir"]) / "server.log")
        spt_source = "server.log eval time" if spt else None
        if spt is None and wall and check.get("run"):  # fallback first, then qualification
            spt, spt_source = wall / check["run"], "fallback: seconds per episode"
        result = {
            "arm": arm["id"],
            "n_cpu_moe": int(cfg["env"]["N_CPU_MOE"]),
            "status": record["status"],
            "vram_max": max(vram) if vram else None,
            "swap_growth_mib": swap,
            "swap_source": swap_source,
            "seconds_per_token": spt,
            "seconds_per_token_source": spt_source,
        }
        limits = self.q["g0"]
        indeterminate = record["status"] in (
            "incomplete_by_window",
            "arm_timeout",
            "session_deadline",
        ) or (
            record["status"] == "complete"
            and (spt is None or swap is None or not vram or swap_peak is None)
        )
        result["indeterminate"] = indeterminate
        result["qualified"] = (
            not indeterminate
            and record["status"] == "complete"
            and result["vram_max"] <= limits["vram_max_bytes"]
            and swap <= limits["swap_growth_max_mib"]
        )
        self.g0_results.append(result)
        if not indeterminate and not result["qualified"]:
            self.g0_closed = True  # lower n_cpu_moe values need more VRAM
        self.log("g0_result", **result)

    def g0_choice(self):
        """Frozen tie-break (plan §11 item 27): lowest s/token among qualified candidates;
        ties go to the larger n_cpu_moe (less VRAM)."""
        qualified = [r for r in self.g0_results if r["qualified"]]
        if not qualified:
            return None
        best = min(qualified, key=lambda r: (r["seconds_per_token"], -r["n_cpu_moe"]))
        return best["n_cpu_moe"]

    def wait_vram_release(self, baseline):
        """Before a retry: wait until VRAM is back near its pre-run level, then cool down."""
        start = self.clock.time()
        while baseline is not None:
            used = self.vram_reader()
            if used is None or used <= baseline + 256 * 2**20:
                break
            if self.clock.time() - start >= self.vram_wait_seconds:
                self.log("vram_not_released", used=used, baseline=baseline)
                break
            self.clock.sleep(5)
        self.clock.sleep(self.cooldown_seconds)

    # -- main loop
    def reconcile(self) -> bool:
        """At startup: remove stale owned containers and the network (never a live run's)."""
        if not self.lock_probe():
            self.stop("lock held: another tool-loop run is active")
            return False
        result = self.cleanup_checked()
        if not result["ok"]:
            self.stop("startup cleanup unresolved")
            return False
        return True

    def run(self) -> dict:
        self.dir.mkdir(parents=True, exist_ok=True)
        try:
            self._run()
        except BaseException as error:
            self.crash(error)
            raise
        return self.manifest()

    def crash(self, error):
        try:
            if self.handle is not None:
                self.terminate()
        except Exception:  # noqa: BLE001 (best effort on the way out)
            pass
        self.handle = None
        self.cleanup_checked()
        if not (isinstance(error, SystemExit) and self.state == "stopped"):
            self.state, self.stop_reason = "crashed", f"{type(error).__name__}: {error}"
        try:
            self.log("queue_crashed", error=self.stop_reason)
            self.write_manifest()
        except Exception:  # noqa: BLE001 (the manifest itself may be what failed)
            fallback = {
                "queue": self.q["name"],
                "state": self.state,
                "stop_reason": self.stop_reason,
            }
            (self.dir / "manifest.json").write_text(json.dumps(fallback) + "\n", encoding="utf-8")

    def _run(self):
        self.log("queue_started", name=self.q["name"], arms=[a["id"] for a in self.arms])
        self.write_manifest()
        if not self.reconcile():
            return
        failures = 0
        for arm in self.arms:
            if self.state != "running":
                break
            cfg = arm["config"]
            if cfg["g0"]:
                deadline = self.q["g0"]["deadline_seconds"]
                if self.g0_started is not None and deadline is not None:
                    if self.clock.time() - self.g0_started > deadline:
                        self.g0_closed = True
                if self.g0_closed:
                    self.transition(arm, "skipped", reason="G0 closed (failure or deadline)")
                    continue
            if cfg.get("n_cpu_moe_from_g0"):
                choice = self.g0_choice()
                if choice is None:
                    self.transition(arm, "refused", reason="no G0 candidate qualified")
                    continue
                arm["selected_n_cpu_moe"] = choice
            final = self.run_arm(arm)
            if self.state != "running":
                break
            failed = ("failed", "arm_timeout", "incomplete", "incomplete_with_errors")
            if final in failed and not cfg["g0"]:
                failures += 1
                if failures >= self.q["max_consecutive_failures"]:
                    self.stop(f"{failures} consecutive failed arms")
            elif final == "complete":
                failures = 0
        if self.state == "running":
            self.state = "done"
            self.log("queue_done")
            self.console("queue done")
        self.write_manifest()

    def g0_expired(self, arm) -> bool:
        deadline = self.q["g0"]["deadline_seconds"]
        return bool(
            arm["config"]["g0"]
            and deadline is not None
            and self.g0_started is not None
            and self.clock.time() - self.g0_started > deadline
        )

    def check_pass(self, arm):
        """CHECK_ONLY pre-pass at window start (plan §8): config, hashes (cached afterwards)
        and provenance, before the arm's first heavy step. Returns (ok, reason)."""
        cfg = arm["config"]
        n = sum(1 for _ in (self.dir / "arms" / arm["id"]).glob("check-*")) + 1
        check_dir = self.dir / "arms" / arm["id"] / f"check-{n}"
        check_dir.mkdir(parents=True, exist_ok=False)
        env = self.env_for(arm, check_dir, window_end(self.clock.time()))
        env["CHECK_ONLY"] = "1"
        start = self.clock.time()
        limit = min(window_end(start) - self.q["stop_margin_seconds"], start + 3 * 3600)
        rc = None
        try:
            self.handle = self.launcher.start(
                [str(self.runner), cfg["model"], cfg["split"]],
                env,
                check_dir / "runner.stdout",
                check_dir / "runner.stderr",
            )
            while True:
                rc = self.handle.poll()
                if rc is not None:
                    break
                if self.clock.time() >= limit:
                    rc = self.terminate()
                    return False, "did not finish before the window edge"
                self.clock.sleep(min(self.poll_seconds, max(0.5, limit - self.clock.time())))
        finally:
            if self.handle is not None and self.handle.poll() is None:
                self.terminate()
            self.handle = None
        prov = parse_provenance(check_dir / "provenance.txt")
        self.log("check_pass", arm=arm["id"], rc=rc, failure=prov.get("failure"))
        if rc == 0 and prov.get("check_only") == "1" and prov.get("failure") is None:
            return True, None
        return False, prov.get("failure_detail") or f"runner exit {rc}"

    def run_arm(self, arm) -> str:
        cfg = arm["config"]
        retried = window_reruns = 0
        not_before = None
        while True:
            if not self.wait_for_window(arm, not_before):
                return arm["state"]
            self.transition(arm, "prechecks")
            verdict, why = self.prechecks(arm)
            if verdict == "refuse":
                self.transition(arm, "refused", reason=why)
                return "refused"
            if verdict == "stop":
                self.transition(arm, "not_started", reason=why)
                self.stop(why)
                return "not_started"
            if not self.watchdog.ensure():
                self.transition(arm, "not_started", reason="watchdog unavailable")
                self.stop("watchdog unavailable: no heavy work without it")
                return "not_started"
            self.log("watchdog_ready", arm=arm["id"])
            ok, why = self.check_pass(arm)
            if not ok:
                self.transition(arm, "refused", reason=f"CHECK_ONLY pre-pass: {why}")
                return "refused"
            if self.g0_expired(arm):  # rechecked after every wait and precheck
                self.transition(arm, "skipped", reason="G0 deadline passed")
                return "skipped"
            if cfg["g0"] and self.g0_started is None:
                self.g0_started = self.clock.time()
            record = self.run_attempt(arm, len(arm["attempts"]) + 1)
            self.record_measurements(arm, record)
            status = record["status"]
            if record.get("reason") == "stopped: watchdog":
                self.transition(arm, "failed", reason="watchdog unavailable")
                self.stop("watchdog unavailable: no heavy work without it")
                return "failed"
            if record.get("cleanup_unresolved"):
                self.transition(arm, "failed", reason=record.get("reason"))
                self.stop("cleanup unresolved: heavy work may still be running")
                return "failed"
            if status == "complete":
                record["excluded"] = False
                arm["ledger"] = record.get("ledger")
                self.transition(arm, "complete")
                return "complete"
            if record.get("infra") in INFRA_CLASSES and cfg["retry"] and not retried:
                retried = 1
                self.transition(arm, "retry", reason=f"infrastructure: {record['infra']}")
                prov = parse_provenance(Path(record["dir"]) / "provenance.txt")
                self.wait_vram_release(_int_or_none(prov.get("vram_used_before")))
                continue
            if status == "incomplete_by_window" and not window_reruns and not cfg["g0"]:
                window_reruns = 1  # whole-arm rerun, never resumed: the fit rule picks the
                not_before = None  # first window start that can hold the estimate
                self.transition(arm, "incomplete_by_window", reason="rerun in a later window")
                continue
            if status == "session_deadline":
                self.transition(arm, "incomplete", reason="session deadline")
                self.stop("session deadline")
                return "incomplete"
            self.transition(arm, status, reason=record.get("reason"))
            return status

    def interrupt(self, signum):
        if self.handle is not None:
            self.terminate()
            self.handle = None
        self.cleanup_checked()
        self.stop(f"signal {signum}")


# ---------------------------------------------------------------- CLI


def window_clock_now():
    """The window-check clock: real time, unless both test variables are set."""
    if os.environ.get(TEST_MODE_ENV) == "1" and os.environ.get(TEST_CLOCK_ENV):
        return float(Path(os.environ[TEST_CLOCK_ENV]).read_text().strip())
    return time.time()


def window_check(now: float, cutoff=None, margin: float = 0) -> int:
    if not heavy_allowed(now):
        print(
            f"schedule_window=closed now={int(now)} opens={local(next_window_start(now)).isoformat()}"
        )
        return 1
    end = window_end(now)
    if cutoff is not None:
        end = min(end, cutoff)
    left = int(end - now)
    state = "open" if left > margin else "closing"
    print(f"schedule_window={state} now={int(now)} seconds_left={left} cutoff={int(end)}")
    return 0 if state == "open" else 1


def hold_lock(path: Path, what: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = open(path, "a", encoding="utf-8")  # noqa: SIM115 (held for the session)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit(f"another {what} is running") from None
    return handle


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    wc = sub.add_parser("window-check")
    wc.add_argument("--cutoff", type=float)
    wc.add_argument("--margin", type=float, default=0)
    check = sub.add_parser("check")
    check.add_argument("queue", type=Path)
    run = sub.add_parser("run")
    run.add_argument("queue", type=Path)
    run.add_argument("--session-dir", type=Path)
    args = parser.parse_args(argv)
    ops.pin_local_time()
    if args.command != "window-check":
        problem = ops.docker_endpoint_problem()
        if problem:
            raise SystemExit(problem)
    if args.command == "window-check":
        return window_check(window_clock_now(), args.cutoff, args.margin)
    registry = model_registry.load()
    queue = load_queue(args.queue, registry)
    if args.command == "check":
        q = Queue(queue, registry, ROOT / "target" / "queue-check-unused")  # nothing written
        for arm in q.arms:
            spe, source = q.seconds_per_episode(arm)
            print(
                f"{arm['id']}: {arm['config']['model']} {arm['config']['split']} "
                f"episodes={q.episodes(arm)} spe={spe:.0f}s ({source}) "
                f"estimate={q.estimate(arm) / 3600:.2f}h"
            )
        return 0
    main_dir = ops.main_checkout(ROOT)
    stamp = dt.datetime.now().strftime("%Y%m%dT%H%M%S")
    session = args.session_dir or ROOT / "target" / f"tool-loop-queue-{queue['name']}-{stamp}"
    lock = hold_lock(ops.lock_paths(main_dir)["queue"], "queue")
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)  # never wait for input
    session.mkdir(parents=True, exist_ok=True)
    supervisor = WatchdogSupervisor(run_watchdog.heartbeat_path(main_dir), session / "watchdog.log")
    q = Queue(queue, registry, session, watchdog=supervisor)

    def handler(signum, _frame):
        q.interrupt(signum)
        sys.exit(128 + signum)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, handler)
    print(f"session {session}", flush=True)
    manifest = q.run()
    lock.close()
    return 0 if manifest["study_complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
