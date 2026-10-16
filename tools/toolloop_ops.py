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
