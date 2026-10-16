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
    }


def lock_held(path: Path, proc_locks: Path = Path("/proc/locks")) -> bool:
    """True if any process holds a lock on path (read from /proc/locks; never locks)."""
    try:
        st = os.stat(path)
        text = proc_locks.read_text(encoding="ascii", errors="replace")
    except OSError:
        return False
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


# ---------------------------------------------------------------- docker


class Docker:
    """docker CLI with a timeout on every call; returns (rc, stdout), rc None on timeout."""

    def __init__(self, binary="docker", timeout=DOCKER_TIMEOUT, run=subprocess.run):
        self.binary, self.timeout, self._run = binary, timeout, run

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
        """Names of every container (any state) with the owned prefix; None if docker fails."""
        rc, out = self.call("ps", "-a", "--format", "{{.Names}}")
        if rc != 0:
            return None
        return sorted(n for n in out.split() if n.startswith(OWNED_PREFIX))

    def network_exists(self):
        rc, out = self.call("network", "ls", "--format", "{{.Name}}")
        if rc != 0:
            return None
        return NETWORK in out.split()

    def cleanup(self) -> dict:
        """Remove every owned container and the network, then verify that they are gone."""
        errors, removed = [], []
        names = self.owned()
        if names is None:
            errors.append("docker ps failed or timed out")
            names = [HARNESS, SERVER]
        for name in names:  # harness before server: stop the load first
            rc, _ = self.call("rm", "-f", name)
            if rc == 0:
                removed.append(name)
            else:
                errors.append(f"rm -f {name}: {'timeout' if rc is None else rc}")
        if self.network_exists() is not False:
            rc, _ = self.call("network", "rm", NETWORK)
            if rc not in (0,):
                errors.append(f"network rm: {'timeout' if rc is None else rc}")
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
