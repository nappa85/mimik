#!/usr/bin/env python3
"""Unattended queue for tools/run_tool_loop_benchmark.sh (plan §11 items 26-27). Prototype tier.

Stdlib only. One queue file (JSON) lists arms in order; each arm runs the runner once (one
retry for pre-declared infrastructure failures only). The queue is an explicit state machine:

  arm:   pending -> waiting_window -> prechecks -> running -> complete
                                         |            |-> retry (infra) -> prechecks
                                         |            |-> failed | incomplete | incomplete_by_window
                                         |-> refused (config, missing path) | skipped
  queue: running -> done | stopped(<reason>)

Status comes from the runner's provenance.txt (failure=, harness_exit=, OOM flags) and the
ledger's completeness (run_summary, planned == run, not_run == 0, header identity), never from
the exit code alone. Heavy work runs only inside the machine schedule (AGENTS.md): Saturday
and Sunday at any time, Monday-Friday outside 09:00-18:00 local time. An arm starts only if
its estimated duration fits before the window closes; a running arm is stopped at the window
edge (incomplete_by_window, rerun whole in a later window, never resumed mid-arm).

Usage:
  run_queue.py run QUEUE.json [--session-dir DIR] [--dry-plan]
  run_queue.py window-check             (exit 0 open, 1 closed; the runner calls it)
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

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "tools" / "run_tool_loop_benchmark.sh"
PACK = ROOT / "examples" / "tool-loop-demo"
SCHEMA = 1
BLOCK_START, BLOCK_END = 9, 18  # Monday-Friday [09:00, 18:00) local: no heavy work
NOW_ENV = "MIMIK_QUEUE_NOW"  # test hook for window-check: epoch seconds
SMOKE_EPISODES = 3
SERVER_CONTAINERS = ("mimik-toolloop-v3-harness", "mimik-toolloop-v3-srv")
INFRA_CLASSES = ("server_start", "server_health", "oom")  # pre-declared, retried once
# Environment the queue controls; inherited values are dropped so a stale export never leaks.
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
CONTROLLED = PASS_THROUGH + (
    "ARM",
    "SEEDS",
    "CASES",
    "CASES_FILE",
    "QUIET",
    "PREFLIGHT_CASE",
    "OUT_DIR",
    "HASH_CACHE",
    "CHECK_ONLY",
    "SCHEDULE_OVERRIDE",
    "MODEL_PATH",
    "MODEL_SHA256",
)
VALUE_RE = re.compile(r"^[A-Za-z0-9._-]{0,64}$")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
SEEDS_RE = re.compile(r"^[0-9]+(,[0-9]+)*$")
CASES_RE = re.compile(r"^[A-Za-z0-9_-]+(,[A-Za-z0-9_-]+)*$")
EVAL_RE = re.compile(r"(?<!prompt )eval time =\s*([0-9.]+) ms /\s*([0-9]+) (?:tokens|runs)")


class QueueError(ValueError):
    pass


# ---------------------------------------------------------------- schedule window


def local(t: float) -> dt.datetime:
    return dt.datetime.fromtimestamp(t)  # naive local wall time (system time zone)


def epoch(d: dt.datetime) -> float:
    return d.timestamp()  # naive datetimes are local


def heavy_allowed(t: float) -> bool:
    d = local(t)
    return d.weekday() >= 5 or not BLOCK_START <= d.hour < BLOCK_END


def window_end(t: float) -> float:
    """First weekday 09:00 local after t (when heavy work must have stopped)."""
    d = local(t)
    for days in range(8):
        day = (d + dt.timedelta(days=days)).replace(hour=BLOCK_START, minute=0, second=0)
        day = day.replace(microsecond=0)
        if day.weekday() < 5 and epoch(day) > t:
            return epoch(day)
    raise AssertionError("unreachable: a weekday 09:00 exists within 8 days")


def next_window_start(t: float) -> float:
    """t itself if heavy work is allowed now, else today's 18:00."""
    if heavy_allowed(t):
        return t
    return epoch(local(t).replace(hour=BLOCK_END, minute=0, second=0, microsecond=0))


def longest_window_seconds(t: float) -> float:
    """Longest heavy window in the coming week (Friday 18:00 to Monday 09:00 = 63 h)."""
    best, cursor = 0.0, t
    for _ in range(8):
        start = next_window_start(cursor)
        end = window_end(start)
        best = max(best, end - max(start, t))
        cursor = epoch(local(end).replace(hour=BLOCK_END, minute=0, second=0, microsecond=0))
    return best


class SystemClock:
    def time(self) -> float:
        return time.time()

    def sleep(self, seconds: float) -> None:
        time.sleep(max(0.0, seconds))


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
        "stop_margin_seconds": doc.get("stop_margin_seconds", 120),
        "io_bytes_per_second": doc.get("io_bytes_per_second", 12_600_000),
        "startup_seconds": doc.get("startup_seconds", 180),
        "estimate_factor": doc.get("estimate_factor", 1.25),
        "g0": {
            "vram_max_bytes": 7_500_000_000,
            "swap_growth_max_mib": 64,
            "deadline_seconds": None,
        },
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
    """key=value tokens of provenance.txt; failure_detail keeps its whole line."""
    values = {}
    if not path.exists():
        return values
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("failure_detail="):
            values["failure_detail"] = line.split("=", 1)[1]
            continue
        for key, value in re.findall(r"(?:^|\s)([a-z][a-z0-9_]*)=('[^']*'|\S*)", line):
            values.setdefault(key, value.strip("'"))
    return values


def check_ledger(path: Path, expect: dict) -> dict:
    """Completeness of a ledger: header identity, run_summary, planned == run, not_run == 0."""
    result = {"complete": False, "reasons": [], "planned": None, "run": 0, "not_run": 0}
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
    result["run"] = len(episodes) - result["not_run"]
    result["aborted"] = any(r.get("event") == "run_aborted" for r in rows)
    if result["aborted"]:
        aborted = next(r for r in rows if r.get("event") == "run_aborted")
        result["abort_reason"] = aborted.get("reason")
        result["reasons"].append(f"run_aborted: {aborted.get('reason')}")
    if rows[-1].get("event") != "run_summary":
        result["reasons"].append("no final run_summary")
    if result["planned"] != len(episodes):
        result["reasons"].append(f"{len(episodes)} episode rows, {result['planned']} planned")
    if result["not_run"]:
        result["reasons"].append(f"not_run {result['not_run']}")
    for key, value in expect.items():
        got = header.get(key)
        if key == "case_subset_sha256":
            got = (header.get("case_subset") or {}).get("sha256")
        if got != value:
            result["reasons"].append(f"header {key} {got!r} != expected {value!r}")
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


def docker_cleanup():
    for name in SERVER_CONTAINERS:
        subprocess.run(
            ["docker", "rm", "-f", name],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )


def lock_free(path: Path) -> bool:
    """True if the runner lock is not held (probe only; released at once)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return False
        fcntl.flock(handle, fcntl.LOCK_UN)
    return True


def default_lock_path() -> Path:
    try:
        common = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            check=True,
            stdin=subprocess.DEVNULL,
        ).stdout.strip()
        main = Path(common).parent
    except (OSError, subprocess.CalledProcessError):
        main = ROOT
    return main / "target" / ".tool-loop-benchmark.lock"


def pack_case_count(split: str) -> int:
    doc = json.loads((PACK / f"cases-{split}.json").read_text(encoding="utf-8"))
    return len(doc["cases"])


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
        case_count=pack_case_count,
        runner=RUNNER,
        poll_seconds=30.0,
        sleep_chunk=600.0,
        grace_seconds=60.0,
        console=None,
        env=None,
    ):
        self.q = queue
        self.registry = registry
        self.dir = Path(session_dir)
        self.clock = clock or SystemClock()
        self.launcher = launcher or ProcessLauncher()
        self.disk_free = disk_free or (lambda p: shutil.disk_usage(p).free)
        self.lock_probe = lock_probe or (lambda: lock_free(default_lock_path()))
        self.cleanup = cleanup or docker_cleanup
        self.case_count = case_count
        self.runner = Path(runner)
        self.poll_seconds = poll_seconds
        self.sleep_chunk = sleep_chunk
        self.grace_seconds = grace_seconds
        self.console = console or (lambda line: print(line, flush=True))
        self.base_env = dict(os.environ if env is None else env)
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
            handle.write(json.dumps(row, sort_keys=True) + "\n")

    def transition(self, arm, state, **fields):
        arm["state"] = state
        arm.update({k: v for k, v in fields.items() if k in ("reason", "selected_n_cpu_moe")})
        self.log("arm_state", arm=arm["id"], state=state, **fields)
        reason = fields.get("reason")
        self.console(
            f"{local(self.clock.time()).isoformat(timespec='seconds')} {arm['id']}: "
            f"{state}{'' if reason is None else ' (' + str(reason) + ')'}"
        )
        self.write_manifest()

    def write_manifest(self):
        planned = list(self.arms)
        missing = [a["id"] for a in planned if a["config"]["required"] and a["state"] != "complete"]
        manifest = {
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
                    "state": a["state"],
                    "reason": a.get("reason"),
                    "selected_n_cpu_moe": a.get("selected_n_cpu_moe"),
                    "ledger": a.get("ledger"),
                    "attempts": a["attempts"],
                }
                for a in planned
            ],
        }
        tmp = self.dir / "manifest.json.tmp"
        tmp.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        tmp.replace(self.dir / "manifest.json")

    def stop(self, reason):
        self.state, self.stop_reason = "stopped", reason
        self.log("queue_stopped", reason=reason)
        self.console(f"queue stopped: {reason}")
        self.write_manifest()

    # -- estimates
    def episodes(self, arm) -> int:
        cfg = arm["config"]
        seeds = len(cfg["seeds"].split(","))
        if cfg["split"] == "smoke":
            cases = SMOKE_EPISODES
        elif cfg.get("cases"):
            cases = len(cfg["cases"].split(","))
        elif cfg.get("cases_file"):
            cases = len(json.loads(Path(cfg["cases_file"]).read_text(encoding="utf-8"))["case_ids"])
        else:
            cases = self.case_count(cfg["split"])
        return cases * seeds

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
        margin = self.q["stop_margin_seconds"]
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
    def env_for(self, arm, attempt_dir: Path):
        cfg = arm["config"]
        env = {k: v for k, v in self.base_env.items() if k not in CONTROLLED}
        env.update(cfg["env"])
        env.update(
            ARM=cfg["arm"],
            SEEDS=cfg["seeds"],
            OUT_DIR=str(attempt_dir),
            HASH_CACHE=str(self.hash_cache),
            QUIET="1" if self.q["quiet"] else "0",
        )
        if cfg.get("preflight_case"):
            env["PREFLIGHT_CASE"] = str(cfg["preflight_case"])
        if cfg.get("cases"):
            env["CASES"] = cfg["cases"]
        if cfg.get("cases_file"):
            env["CASES_FILE"] = cfg["cases_file"]
        if arm.get("selected_n_cpu_moe") is not None:
            env["N_CPU_MOE"] = str(arm["selected_n_cpu_moe"])
        return env

    def expected_header(self, arm, env) -> dict:
        cfg = arm["config"]
        entry = self.registry["models"][cfg["model"]]
        expect = {
            "arm": cfg["arm"],
            "model_label": entry["label"],
            "model_sha256": entry["sha256"],
            "ctx": int(env.get("CTX", entry["server"]["ctx"])),
            "split": "dev" if cfg["split"] == "smoke" else cfg["split"],
            "seeds": [int(s) for s in cfg["seeds"].split(",")],
        }
        if cfg.get("cases_file"):
            doc = json.loads(Path(cfg["cases_file"]).read_text(encoding="utf-8"))
            expect["case_subset_sha256"] = doc.get("sha256")
        return expect

    def run_attempt(self, arm, number: int) -> dict:
        cfg = arm["config"]
        attempt_dir = self.dir / "arms" / arm["id"] / f"attempt-{number}"
        attempt_dir.mkdir(parents=True, exist_ok=False)
        env = self.env_for(arm, attempt_dir)
        argv = [str(self.runner), cfg["model"], cfg["split"]]
        start = self.clock.time()
        estimate = self.estimate(arm)
        limits = {"window": window_end(start) - self.q["stop_margin_seconds"]}
        limits["arm_deadline"] = start + (cfg.get("max_arm_seconds") or 2 * estimate + 600)
        if self.q["session_deadline"] is not None:
            limits["session_deadline"] = self.q["session_deadline"]
        record = {
            "attempt": number,
            "dir": str(attempt_dir),
            "started": local(start).isoformat(timespec="seconds"),
            "estimate_seconds": round(estimate),
            "excluded": True,
        }
        arm["attempts"].append(record)
        self.transition(arm, "running", attempt=number, estimate=round(estimate))
        self.handle = self.launcher.start(
            argv, env, attempt_dir / "runner.stdout", attempt_dir / "runner.stderr"
        )
        stopped_by = None
        while True:
            rc = self.handle.poll()
            if rc is not None:
                break
            now = self.clock.time()
            hit = [name for name, at in limits.items() if now >= at]
            if hit:
                stopped_by = hit[0]
                rc = self.terminate()
                break
            nearest = min(limits.values())
            self.clock.sleep(min(self.poll_seconds, max(0.5, nearest - now)))
        self.handle = None
        record["runner_exit"] = rc
        record["ended"] = local(self.clock.time()).isoformat(timespec="seconds")
        record.update(self.classify(arm, attempt_dir, env, stopped_by))
        return record

    def terminate(self):
        self.handle.signal(signal.SIGTERM)
        rc = self.handle.wait(self.grace_seconds)
        if rc is None:
            self.handle.signal(signal.SIGKILL)
            rc = self.handle.wait(self.grace_seconds)
        self.cleanup()
        return rc

    def classify(self, arm, attempt_dir: Path, env, stopped_by) -> dict:
        prov = parse_provenance(attempt_dir / "provenance.txt")
        out = {"provenance": bool(prov), "failure": prov.get("failure")}
        oom = "true" in (prov.get("server_oom_killed"), prov.get("harness_oom_killed"))
        ledger = attempt_dir / prov["ledger_file"] if prov.get("ledger_file") else None
        check = check_ledger(ledger, self.expected_header(arm, env)) if ledger else None
        if check is not None:
            out["ledger"] = str(ledger)
            out["ledger_check"] = check
        if stopped_by is not None:
            status = {
                "window": "incomplete_by_window",
                "arm_deadline": "arm_timeout",
                "session_deadline": "session_deadline",
            }[stopped_by]
            return dict(out, status=status, infra=None)
        if not prov:
            return dict(out, status="failed", infra=None, reason="no provenance.txt")
        if oom:
            return dict(out, status="failed", infra="oom", reason="container OOM-killed")
        failure = prov.get("failure")
        if failure in ("server_start", "server_health"):
            return dict(out, status="failed", infra=failure, reason=prov.get("failure_detail"))
        if failure == "window":
            return dict(out, status="incomplete_by_window", infra=None, reason="runner refused")
        if failure is not None:
            return dict(out, status="failed", infra=None, reason=prov.get("failure_detail"))
        if prov.get("harness_exit") is None:
            return dict(out, status="failed", infra=None, reason="no harness_exit in provenance")
        if check and check.get("aborted") and "health" in str(check.get("abort_reason")):
            return dict(out, status="failed", infra="server_health", reason=check["abort_reason"])
        if prov.get("harness_exit") != "0" or check is None:
            reason = f"harness_exit {prov.get('harness_exit')}"
            return dict(out, status="failed", infra=None, reason=reason)
        if check["complete"]:
            return dict(out, status="complete", infra=None)
        if check["not_run"] and prov.get("budget_from_window"):
            return dict(out, status="incomplete_by_window", infra=None, reason="window budget")
        return dict(out, status="incomplete", infra=None, reason="; ".join(check["reasons"]))

    # -- measurements
    def record_measurements(self, arm, record):
        cfg = arm["config"]
        prov = parse_provenance(Path(record["dir"]) / "provenance.txt")
        wall = _int_or_none(prov.get("harness_wall_seconds"))
        check = record.get("ledger_check") or {}
        if record["status"] == "complete" and wall and check.get("run"):
            self.measured_spe[cfg["model"]] = wall / check["run"]
        if cfg["g0"]:
            vram = [_int_or_none(prov.get(k)) for k in ("vram_used_loaded", "vram_used_end")]
            vram = [v for v in vram if v is not None]
            before = _int_or_none(prov.get("swap_used_mib_before"))
            after = _int_or_none(prov.get("swap_used_mib_after"))
            spt = server_seconds_per_token(Path(record["dir"]) / "server.log")
            result = {
                "arm": arm["id"],
                "n_cpu_moe": int(cfg["env"]["N_CPU_MOE"]),
                "status": record["status"],
                "vram_max": max(vram) if vram else None,
                "swap_growth_mib": None if None in (before, after) else after - before,
                "seconds_per_token": spt,
                "seconds_per_token_source": "server.log eval time" if spt else None,
                "seconds_per_episode": wall / check["run"] if wall and check.get("run") else None,
            }
            limits = self.q["g0"]
            result["qualified"] = (
                result["status"] == "complete"
                and result["vram_max"] is not None
                and result["vram_max"] <= limits["vram_max_bytes"]
                and result["swap_growth_mib"] is not None
                and result["swap_growth_mib"] <= limits["swap_growth_max_mib"]
            )
            if result["seconds_per_token"] is None:
                result["seconds_per_token"] = result["seconds_per_episode"]
                result["seconds_per_token_source"] = "fallback: seconds per episode"
            self.g0_results.append(result)
            if not result["qualified"]:
                self.g0_closed = True  # lower n_cpu_moe values need more VRAM
            self.log("g0_result", **result)

    def g0_choice(self):
        """Frozen tie-break (plan §11 item 27): lowest s/token among qualified candidates;
        ties go to the larger n_cpu_moe (less VRAM)."""
        qualified = [r for r in self.g0_results if r["qualified"] and r["seconds_per_token"]]
        if not qualified:
            return None
        best = min(qualified, key=lambda r: (r["seconds_per_token"], -r["n_cpu_moe"]))
        return best["n_cpu_moe"]

    # -- main loop
    def run(self) -> dict:
        self.dir.mkdir(parents=True, exist_ok=True)
        self.log("queue_started", name=self.q["name"], arms=[a["id"] for a in self.arms])
        self.write_manifest()
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
            if final in ("failed", "arm_timeout", "incomplete") and not cfg["g0"]:
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
        return json.loads((self.dir / "manifest.json").read_text(encoding="utf-8"))

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
            if cfg["g0"] and self.g0_started is None:
                self.g0_started = self.clock.time()
            record = self.run_attempt(arm, len(arm["attempts"]) + 1)
            self.record_measurements(arm, record)
            status = record["status"]
            if status == "complete":
                record["excluded"] = False
                arm["ledger"] = record.get("ledger")
                self.transition(arm, "complete")
                return "complete"
            if record.get("infra") in INFRA_CLASSES and cfg["retry"] and not retried:
                retried = 1
                self.transition(arm, "retry", reason=f"infrastructure: {record['infra']}")
                continue
            if status == "incomplete_by_window" and not window_reruns and not cfg["g0"]:
                window_reruns = 1  # whole-arm rerun in a later window, never resumed
                not_before = window_end(self.clock.time())
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
        self.stop(f"signal {signum}")


# ---------------------------------------------------------------- CLI


def window_check(now: float) -> int:
    if heavy_allowed(now):
        end = window_end(now)
        left = int(end - now)
        print(f"schedule_window=open seconds_left={left} closes={local(end).isoformat()}")
        return 0
    start = next_window_start(now)
    print(f"schedule_window=closed opens={local(start).isoformat()}")
    return 1


def hold_queue_lock(session_root: Path):
    session_root.mkdir(parents=True, exist_ok=True)
    handle = open(session_root / ".queue.lock", "a", encoding="utf-8")  # noqa: SIM115
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit("another queue is running") from None
    return handle


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("window-check")
    check = sub.add_parser("check")
    check.add_argument("queue", type=Path)
    run = sub.add_parser("run")
    run.add_argument("queue", type=Path)
    run.add_argument("--session-dir", type=Path)
    args = parser.parse_args(argv)
    if args.command == "window-check":
        now = float(os.environ[NOW_ENV]) if os.environ.get(NOW_ENV) else time.time()
        return window_check(now)
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
    stamp = dt.datetime.now().strftime("%Y%m%dT%H%M%S")
    session = args.session_dir or ROOT / "target" / f"tool-loop-queue-{queue['name']}-{stamp}"
    lock = hold_queue_lock(ROOT / "target")
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)  # never wait for input
    q = Queue(queue, registry, session)
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, lambda signum, _frame: (q.interrupt(signum), sys.exit(128 + signum)))
    print(f"session {session}", flush=True)
    manifest = q.run()
    lock.close()
    return 0 if manifest["study_complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
