"""Shared operations for the tool-loop runner, queue and watchdog. Prototype tier, stdlib only.

- Machine schedule (AGENTS.md): heavy work Saturday and Sunday at any time, Monday-Friday
  outside 09:00-18:00, in the machine's local time zone (/etc/localtime, see pin_local_time).
- Owned Docker objects: every container the runner starts is named with OWNED_PREFIX
  (server, harness, hash/check containers) on the internal network NETWORK, so the queue and
  the watchdog can find and remove them. Every docker or git call has a timeout.
- Locks: the runner lock, the queue lock and the watchdog lock live in the main checkout's
  target/ (shared by every worktree). lock_held reads /proc/locks, so probing never takes the
  lock (a probe that took it could make a starting runner exit 9).
"""

import datetime as dt
import os
import re
import subprocess
import time
from pathlib import Path

BLOCK_START, BLOCK_END = 9, 18  # Monday-Friday [09:00, 18:00) local: no heavy work
OWNED_PREFIX = "mimik-toolloop-v3-"
NETWORK = "mimik-toolloop-v3"
SERVER = OWNED_PREFIX + "srv"
HARNESS = OWNED_PREFIX + "harness"
DOCKER_TIMEOUT = 60
GIT_TIMEOUT = 30


def pin_local_time() -> None:
    """Use the machine's /etc/localtime whatever TZ the operator shell exported."""
    os.environ["TZ"] = ":/etc/localtime"
    time.tzset()


# ---------------------------------------------------------------- schedule


def local(t: float) -> dt.datetime:
    return dt.datetime.fromtimestamp(t)  # naive local wall time


def epoch(d: dt.datetime) -> float:
    return d.timestamp()  # naive datetimes are local


def heavy_allowed(t: float) -> bool:
    d = local(t)
    return d.weekday() >= 5 or not BLOCK_START <= d.hour < BLOCK_END


def window_end(t: float) -> float:
    """First weekday 09:00 local after t (when heavy work must have stopped)."""
    d = local(t)
    for days in range(8):
        day = (d + dt.timedelta(days=days)).replace(
            hour=BLOCK_START, minute=0, second=0, microsecond=0
        )
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


# ---------------------------------------------------------------- paths and locks


def main_checkout(root: Path) -> Path:
    try:
        common = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            check=True,
            stdin=subprocess.DEVNULL,
            timeout=GIT_TIMEOUT,
        ).stdout.strip()
        return Path(common).parent
    except (OSError, subprocess.SubprocessError):
        return Path(root)


def lock_paths(main: Path) -> dict:
    target = Path(main) / "target"
    return {
        "runner": target / ".tool-loop-benchmark.lock",
        "queue": target / ".tool-loop-queue.lock",
        "watchdog": target / ".tool-loop-watchdog.lock",
        # Names of the pid-suffixed hash/check containers the runner starts (one per line),
        # for the watchdog's fallback when docker cannot list containers.
        "checks": target / "tool-loop-check-containers.txt",
    }


def lock_held(path: Path, proc_locks: Path = Path("/proc/locks")) -> bool:
    """True if any process holds a lock on path (read from /proc/locks; never locks).

    A lock file that does not exist is not held. Any other error ("cannot determine") counts
    as held, so a probe error never lets the watchdog or the queue kill a live run.
    """
    try:
        st = os.stat(path)
    except FileNotFoundError:
        return False
    except OSError:
        return True
    try:
        text = proc_locks.read_text(encoding="ascii", errors="replace")
    except OSError:
        return True
    want = (os.major(st.st_dev), os.minor(st.st_dev), st.st_ino)
    for line in text.splitlines():
        for field in line.split():
            parts = field.split(":")
            if len(parts) != 3:
                continue
            try:
                got = (int(parts[0], 16), int(parts[1], 16), int(parts[2]))
            except ValueError:
                continue
            if got == want:
                return True
    return False


ENDPOINT_VARIABLES = ("DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG")


def docker_context_problem(docker=None):
    """The docker context must be `default` (checked before any other Docker call)."""
    docker = docker or Docker()
    rc, out = docker.call("context", "show")
    if rc != 0:
        return "docker context show failed or timed out"
    if out.strip() != "default":
        return f"refusing docker context {out.strip()!r} (default only)"
    return None


def docker_problem(env=None, docker=None):
    """Environment refusal, then the context check: None if this Docker endpoint is usable."""
    return docker_endpoint_problem(env) or docker_context_problem(docker)


def docker_endpoint_problem(env=None):
    """One Docker endpoint for runner, queue and watchdog: refuse any endpoint override."""
    env = os.environ if env is None else env
    set_vars = [name for name in ENDPOINT_VARIABLES if env.get(name)]
    if set_vars:
        return f"refusing to run with {', '.join(set_vars)} set (one Docker endpoint only)"
    return None


# ---------------------------------------------------------------- docker


LABEL_KEY, LABEL_VALUE = "mimik.toolloop", "v3"
LABEL = f"{LABEL_KEY}={LABEL_VALUE}"
KNOWN_NAMES = (HARNESS, SERVER)  # the fixed names; check containers carry a pid suffix


class Docker:
    """docker CLI with a timeout on every call; returns (rc, stdout), rc None on timeout.

    Ownership is the name prefix AND the label mimik.toolloop=v3: a foreign container with
    the same prefix but without the label is never touched.
    """

    def __init__(
        self, binary="docker", timeout=DOCKER_TIMEOUT, run=subprocess.run, check_registry=None
    ):
        self.binary, self.timeout, self._run = binary, timeout, run
        self.check_registry = None if check_registry is None else Path(check_registry)

    def registered_checks(self):
        try:
            lines = self.check_registry.read_text(encoding="utf-8").split()
        except (OSError, AttributeError):
            return []
        return [n for n in lines if n.startswith(OWNED_PREFIX + "check-")]

    def call(self, *args, timeout=None):
        try:
            proc = self._run(
                [self.binary, *args],
                capture_output=True,
                text=True,
                stdin=subprocess.DEVNULL,
                timeout=timeout or self.timeout,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return None, ""
        except OSError as error:
            return None, str(error)
        return proc.returncode, proc.stdout

    def owned(self):
        """Owned containers (any state); None if docker fails."""
        rc, out = self.call("ps", "-a", "--filter", f"label={LABEL}", "--format", "{{.Names}}")
        if rc != 0:
            return None
        return sorted(n for n in out.split() if n.startswith(OWNED_PREFIX))

    def network_exists(self):
        """True if the owned (labelled) network exists; None if docker fails."""
        rc, out = self.call("network", "ls", "--filter", f"label={LABEL}", "--format", "{{.Name}}")
        if rc != 0:
            return None
        return NETWORK in out.split()

    def is_owned(self, name):
        rc, out = self.call("inspect", "-f", '{{index .Config.Labels "' + LABEL_KEY + '"}}', name)
        return rc == 0 and out.strip() == LABEL_VALUE

    def cleanup(self) -> dict:
        """Remove every owned container and the owned network, then verify they are gone.

        If listing fails, fall back to the known fixed names, each removed only after its
        label is confirmed; the caller retries on its next tick.
        """
        errors, removed = [], []
        names = self.owned()
        if names is None:
            errors.append("docker ps failed or timed out")
            candidates = list(KNOWN_NAMES) + self.registered_checks()
            names = [n for n in candidates if self.is_owned(n)]
        for name in names:  # harness before server: stop the load first
            rc, _ = self.call("rm", "-f", name)
            if rc == 0:
                removed.append(name)
            else:
                errors.append(f"rm -f {name}: {'timeout' if rc is None else rc}")
        present = self.network_exists()
        if present:
            rc, _ = self.call("network", "rm", NETWORK)
            if rc != 0:
                errors.append(f"network rm: {'timeout' if rc is None else rc}")
        elif present is None:
            errors.append("docker network ls failed or timed out")
        remaining = self.owned()
        network = self.network_exists()
        ok = remaining == [] and network is False
        return {
            "ok": ok,
            "removed": removed,
            "remaining": remaining,
            "network_present": network,
            "errors": errors,
        }


# ---------------------------------------------------------------- temperatures (thermal guard)

HWMON = Path("/sys/class/hwmon")
GPU_EDGE, GPU_JUNCTION = "amdgpu:edge", "amdgpu:junction"
TEST_HWMON_ENV = "MIMIK_TEST_HWMON"  # honoured only with MIMIK_TEST_MODE=1 (tests)


def hwmon_root(env=None) -> Path:
    env = os.environ if env is None else env
    if env.get("MIMIK_TEST_MODE") == "1" and env.get(TEST_HWMON_ENV):
        return Path(env[TEST_HWMON_ENV])
    return HWMON


def _millidegrees(path: Path):
    try:
        return int(path.read_text().strip()) / 1000.0
    except (OSError, ValueError):
        return None


def read_sensors(root: Path = HWMON) -> dict:
    """{"<chip name>:<label or tempN>": {"value": °C or None, "crit": °C or None}}.

    Chips are found by their `name` file, never by hwmon index (it changes across boots).
    An unreadable input is present with value None: callers must not treat it as cool.
    """
    sensors = {}
    try:
        chips = sorted(Path(root).iterdir())
    except OSError:
        return sensors
    for chip in chips:
        try:
            name = (chip / "name").read_text().strip()
        except OSError:
            continue
        for item in sorted(chip.glob("temp*_input")):
            index = item.name[: -len("_input")]
            try:
                label = (chip / f"{index}_label").read_text().strip()
            except OSError:
                label = index
            key = f"{name}:{label}"
            if key in sensors:  # two chips with one name: keep both
                key = f"{name}.{chip.name}:{label}"
            sensors[key] = {
                "value": _millidegrees(item),
                "crit": _millidegrees(chip / f"{index}_crit"),
            }
    return sensors


def sensor_slug(key: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", key.lower()).strip("_")


THERMAL_DEFAULTS = {
    "junction_c": 95.0,  # GPU junction (or edge if no junction sensor)
    "edge_c": 88.0,  # GPU edge
    "sustain_seconds": 60.0,  # continuously over a limit
    "crit_margin_c": 5.0,  # any sensor at or above its own crit minus this: immediate
    "cooldown_edge_c": 60.0,
    "cooldown_hold_seconds": 300.0,
    "cooldown_cap_seconds": 2700.0,
    "max_thermal_stops": 2,
    "sample_seconds": 30.0,
}


def crit_breaches(sensors: dict, margin: float) -> list:
    """Sensors at or above their own crit minus margin (the watchdog's hard limit)."""
    return [
        f"{key} {s['value']:.1f} °C >= crit {s['crit']:.1f} - {margin:.0f}"
        for key, s in sorted(sensors.items())
        if s["value"] is not None and s["crit"] is not None and s["value"] >= s["crit"] - margin
    ]


class ThermalGuard:
    """Decides, per sample, whether a running attempt must stop for temperature.

    check() returns (None, None), ("thermal", reason) or ("thermal_unknown", reason). The
    GPU edge sensor is required: if it is missing or unreadable, the result is
    thermal_unknown, never "cool". Per-sensor min/max/last are kept in stats.
    """

    def __init__(self, limits=None):
        self.limits = dict(THERMAL_DEFAULTS, **(limits or {}))
        self.hot_since = None
        self.stats = {}
        self.has_junction = None

    def record(self, sensors: dict):
        for key, s in sensors.items():
            value = s["value"]
            entry = self.stats.setdefault(key, {"min": None, "max": None, "last": None})
            entry["last"] = value
            if value is not None:
                entry["min"] = value if entry["min"] is None else min(entry["min"], value)
                entry["max"] = value if entry["max"] is None else max(entry["max"], value)

    def check(self, sensors: dict, now: float):
        self.record(sensors)
        lim = self.limits
        breaches = crit_breaches(sensors, lim["crit_margin_c"])
        if breaches:
            return "thermal", "crit proximity: " + "; ".join(breaches)
        edge = sensors.get(GPU_EDGE, {}).get("value")
        if self.has_junction is None:
            self.has_junction = GPU_JUNCTION in sensors
        junction = sensors.get(GPU_JUNCTION, {}).get("value") if self.has_junction else edge
        if edge is None or junction is None:
            return "thermal_unknown", "GPU temperature sensor missing or unreadable"
        hot = junction >= lim["junction_c"] or edge >= lim["edge_c"]
        if not hot:
            self.hot_since = None
            return None, None
        self.hot_since = now if self.hot_since is None else self.hot_since
        if now - self.hot_since >= lim["sustain_seconds"]:
            return "thermal", (
                f"GPU edge {edge:.1f} °C / junction {junction:.1f} °C over the limit "
                f"(edge {lim['edge_c']:.0f}, junction {lim['junction_c']:.0f}) for "
                f"{now - self.hot_since:.0f} s"
            )
        return None, None


def cooled(read, clock, limits=None, log=None) -> bool:
    """Wait until the GPU edge is <= cooldown_edge_c for cooldown_hold_seconds in a row.

    An unreadable sensor resets the hold. False if the cap passes first.
    """
    lim = dict(THERMAL_DEFAULTS, **(limits or {}))
    start = clock.time()
    cool_since = None
    while True:
        now = clock.time()
        edge = read().get(GPU_EDGE, {}).get("value")
        if edge is not None and edge <= lim["cooldown_edge_c"]:
            cool_since = now if cool_since is None else cool_since
            if now - cool_since >= lim["cooldown_hold_seconds"]:
                return True
        else:
            cool_since = None
        if now - start >= lim["cooldown_cap_seconds"]:
            if log:
                log(edge)
            return False
        clock.sleep(15)


def _temps_main(argv=None) -> int:
    """`toolloop_ops.py temps [--prefix P] [--peak-file F]`: key=value lines for provenance."""
    import argparse
    import json

    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    temps = sub.add_parser("temps")
    temps.add_argument("--prefix", default="")
    temps.add_argument("--peak-file", type=Path)
    temps.add_argument("--print-peaks", action="store_true")
    args = parser.parse_args(argv)
    sensors = read_sensors(hwmon_root())
    peaks = {}
    if args.peak_file and args.peak_file.exists():
        try:
            peaks = json.loads(args.peak_file.read_text())
        except (OSError, ValueError):
            peaks = {}
    for key, s in sensors.items():
        if s["value"] is not None:
            peaks[key] = max(s["value"], peaks.get(key, s["value"]))
    if args.peak_file:
        args.peak_file.write_text(json.dumps(peaks))
    if args.print_peaks:
        values = {k: peaks.get(k) for k in sorted(set(peaks) | set(sensors))}
        print(
            " ".join(
                f"temp_{sensor_slug(k)}_peak={'n/a' if v is None else v}" for k, v in values.items()
            )
        )
    else:
        suffix = f"_{args.prefix}" if args.prefix else ""
        print(
            " ".join(
                f"temp_{sensor_slug(k)}{suffix}={'n/a' if s['value'] is None else s['value']}"
                for k, s in sorted(sensors.items())
            )
            or "temp_sensors=none"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(_temps_main())
