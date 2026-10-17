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
ROUTER = OWNED_PREFIX + "router"  # tools/run_decision_router.sh (decision-router plan §5)
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
KNOWN_NAMES = (HARNESS, SERVER, ROUTER)  # the fixed names; check containers carry a pid suffix


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
    # Tightened after the 2026-10-16 SW CTF shutdown (junction 86 °C in 60 s on auto fan).
    "junction_stop_c": 90.0,  # GPU junction (edge if no junction sensor): immediate stop
    "edge_c": 80.0,  # GPU edge, sustained for edge_sustain_seconds
    "edge_sustain_seconds": 20.0,
    "crit_margin_c": 5.0,  # any sensor at or above its own crit minus this: immediate
    "fan_pwm_min": 230.0,  # pwm1 (0-255) with pwm1_enable == 1 (manual)
    "fan_rpm_min": 2000.0,  # fan1_input while the junction is above fan_rpm_check_above_c
    "fan_rpm_check_above_c": 60.0,
    "fan_fault_seconds": 10.0,
    "cooldown_edge_c": 60.0,
    "cooldown_hold_seconds": 300.0,
    "cooldown_cap_seconds": 2700.0,
    "max_thermal_stops": 2,
    "sample_seconds": 5.0,
}
EMERGENCY_JUNCTION_C = 95.0  # the watchdog's own limit, beside crit - 5


def read_fan(root: Path = HWMON) -> dict:
    """pwm1_enable, pwm1 and fan1_input of the amdgpu chip, found by name (None if unreadable)."""
    fan = {"pwm1_enable": None, "pwm1": None, "fan1_input": None, "path": None}
    try:
        chips = sorted(Path(root).iterdir())
    except OSError:
        return fan
    for chip in chips:
        try:
            if (chip / "name").read_text().strip() != "amdgpu":
                continue
        except OSError:
            continue
        fan["path"] = str(chip)
        for key in ("pwm1_enable", "pwm1", "fan1_input"):
            try:
                fan[key] = int((chip / key).read_text().strip())
            except (OSError, ValueError):
                fan[key] = None
        break
    return fan


def fan_command(fan: dict) -> str:
    path = fan.get("path") or "/sys/class/hwmon/hwmon<N>  (the chip whose name is amdgpu)"
    return f"echo 1 | sudo tee {path}/pwm1_enable && echo 255 | sudo tee {path}/pwm1"


def fan_precondition(fan: dict, limits=None):
    """None if the GPU fan is in manual mode at >= fan_pwm_min, else the refusal message."""
    lim = dict(THERMAL_DEFAULTS, **(limits or {}))
    if fan.get("pwm1_enable") == 1 and (fan.get("pwm1") or 0) >= lim["fan_pwm_min"]:
        return None
    return (
        f"GPU fan not in manual mode at pwm >= {lim['fan_pwm_min']:.0f} "
        f"(pwm1_enable={fan.get('pwm1_enable')}, pwm1={fan.get('pwm1')}); the manual setting "
        f"resets on reboot. Set it with: {fan_command(fan)}"
    )


def crit_breaches(sensors: dict, margin: float) -> list:
    """Sensors at or above their own crit minus margin (the watchdog's hard limit)."""
    return [
        f"{key} {s['value']:.1f} °C >= crit {s['crit']:.1f} - {margin:.0f}"
        for key, s in sorted(sensors.items())
        if s["value"] is not None and s["crit"] is not None and s["value"] >= s["crit"] - margin
    ]


def gpu_junction(sensors: dict):
    """The junction reading, or the edge where there is no junction sensor."""
    if GPU_JUNCTION in sensors:
        return sensors[GPU_JUNCTION]["value"]
    return sensors.get(GPU_EDGE, {}).get("value")


class ThermalGuard:
    """Decides, per sample, whether a running attempt must stop for temperature or fan.

    check() returns (None, None), ("thermal", reason), ("thermal_unknown", reason) or
    ("fan_fault", reason). The GPU edge sensor is required: missing or unreadable is
    thermal_unknown, never "cool"; an unreadable fan counts as a fault. Per-sensor
    min/max/last are kept in stats (the fan under "amdgpu:fan1_rpm" and "amdgpu:pwm1").
    """

    def __init__(self, limits=None):
        self.limits = dict(THERMAL_DEFAULTS, **(limits or {}))
        self.edge_hot_since = None
        self.fan_bad_since = None
        self.stats = {}
        self.has_junction = None

    def _note(self, key, value):
        entry = self.stats.setdefault(key, {"min": None, "max": None, "last": None})
        entry["last"] = value
        if value is not None:
            entry["min"] = value if entry["min"] is None else min(entry["min"], value)
            entry["max"] = value if entry["max"] is None else max(entry["max"], value)

    def record(self, sensors: dict, fan=None):
        for key, s in sensors.items():
            self._note(key, s["value"])
        if fan is not None:
            self._note("amdgpu:fan1_rpm", fan.get("fan1_input"))
            self._note("amdgpu:pwm1", fan.get("pwm1"))

    def check(self, sensors: dict, now: float, fan=None):
        self.record(sensors, fan)
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
        if junction >= lim["junction_stop_c"]:
            return "thermal", f"GPU junction {junction:.1f} °C >= {lim['junction_stop_c']:.0f}"
        if edge >= lim["edge_c"]:
            self.edge_hot_since = now if self.edge_hot_since is None else self.edge_hot_since
            if now - self.edge_hot_since >= lim["edge_sustain_seconds"]:
                return "thermal", (
                    f"GPU edge {edge:.1f} °C >= {lim['edge_c']:.0f} for "
                    f"{now - self.edge_hot_since:.0f} s"
                )
        else:
            self.edge_hot_since = None
        if fan is not None:
            problem = self.fan_problem(fan, junction)
            if problem is None:
                self.fan_bad_since = None
            else:
                self.fan_bad_since = now if self.fan_bad_since is None else self.fan_bad_since
                if now - self.fan_bad_since >= lim["fan_fault_seconds"]:
                    seconds = now - self.fan_bad_since
                    return "fan_fault", f"{problem} for {seconds:.0f} s; {fan_command(fan)}"
        return None, None

    def fan_problem(self, fan: dict, junction: float):
        lim = self.limits
        if fan.get("pwm1_enable") != 1:
            return f"pwm1_enable {fan.get('pwm1_enable')} (not manual)"
        if fan.get("pwm1") is None or fan["pwm1"] < lim["fan_pwm_min"]:
            return f"pwm1 {fan.get('pwm1')} < {lim['fan_pwm_min']:.0f}"
        rpm = fan.get("fan1_input")
        if junction > lim["fan_rpm_check_above_c"] and (rpm is None or rpm < lim["fan_rpm_min"]):
            return (
                f"fan {rpm} RPM < {lim['fan_rpm_min']:.0f} with the junction at {junction:.1f} °C"
            )
        return None


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
    sub.add_parser("fancheck")
    args = parser.parse_args(argv)
    if args.command == "fancheck":
        fan = read_fan(hwmon_root())
        problem = fan_precondition(fan)
        print(
            f"fan_pwm1_enable={fan['pwm1_enable']} fan_pwm1={fan['pwm1']} "
            f"fan_rpm={fan['fan1_input']}"
        )
        if problem:
            print(problem, file=__import__("sys").stderr)
            return 1
        return 0
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
