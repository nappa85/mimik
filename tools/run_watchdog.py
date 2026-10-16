#!/usr/bin/env python3
"""Independent watchdog for tool-loop heavy work (review fix 2). Prototype tier, stdlib only.

A separate process (the queue starts one and waits for its readiness; it also runs
standalone). Every interval (at most 60 s) it checks the machine schedule with the real
clock only, and:

- from the window edge minus a reserve (RESERVE_SECONDS, 240 s) and whenever heavy work is
  not allowed, force-removes every owned container (label mimik.toolloop=v3 and name prefix
  mimik-toolloop-v3-: server, harness, hash/check) and the owned network;
- when owned containers exist but neither the runner lock nor the queue lock is held for
  longer than the grace period, removes them as orphans. A lock probe that cannot decide
  counts as held.

If docker cannot list containers, it falls back to the known fixed names (removed only after
their label is confirmed) and retries on every tick. Every tick rewrites a heartbeat file
(the readiness handshake the queue checks); log or heartbeat write errors never stop
enforcement. It never reads input, refuses to start with an endpoint override (DOCKER_HOST,
DOCKER_CONTEXT, DOCKER_CONFIG), and exits after --idle-exit seconds with no lock holder and
no owned container. A second watchdog exits at once (watchdog lock).

Usage: run_watchdog.py [--interval 30] [--grace 120] [--idle-exit 3600] [--log FILE] [--once]
"""

import argparse
import datetime as dt
import fcntl
import json
import os
import sys
import time
from pathlib import Path

import toolloop_ops as ops

ROOT = Path(__file__).resolve().parents[1]
MAX_INTERVAL = 60
RESERVE_SECONDS = 240


class Watchdog:
    def __init__(
        self,
        docker,
        lock_held,
        clock,
        log_path,
        interval=30,
        grace=120,
        idle_exit=3600,
        heartbeat=None,
        reserve=RESERVE_SECONDS,
    ):
        if not 0 < interval <= MAX_INTERVAL:
            raise ValueError(f"interval must be in (0, {MAX_INTERVAL}] seconds")
        self.docker, self.lock_held, self.clock = docker, lock_held, clock
        self.log_path = Path(log_path)
        self.interval, self.grace, self.idle_exit = interval, grace, idle_exit
        self.heartbeat = None if heartbeat is None else Path(heartbeat)
        self.reserve = reserve
        self.free_since = None
        self.idle_since = None

    def log(self, action, **fields):
        row = {"time": dt.datetime.fromtimestamp(self.clock.time()).isoformat(), "action": action}
        row.update(fields)
        try:
            with open(self.log_path, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(row, sort_keys=True, default=str) + "\n")
        except OSError:
            pass  # a log failure must never stop enforcement

    def beat(self):
        if self.heartbeat is None:
            return
        try:
            tmp = self.heartbeat.with_suffix(".tmp")
            tmp.write_text(json.dumps({"pid": os.getpid(), "time": time.time()}))
            tmp.replace(self.heartbeat)
        except OSError:
            pass

    def closing(self, now) -> bool:
        return not ops.heavy_allowed(now) or now >= ops.window_end(now) - self.reserve

    def held(self) -> bool:
        try:
            return bool(self.lock_held())
        except Exception:  # noqa: BLE001 (cannot determine: treat as held)
            return True

    def tick(self) -> str:
        try:
            return self._tick()
        finally:
            self.beat()

    def _tick(self) -> str:
        now = self.clock.time()
        owned = self.docker.owned()  # None: docker could not list; assume there may be some
        held = self.held()
        if self.closing(now):
            network = self.docker.network_exists()
            if owned or owned is None or network or network is None:
                result = self.docker.cleanup()
                self.log("removed_at_window_edge", containers=owned, reserve=self.reserve, **result)
                return "removed_at_window_edge"
        if (owned or owned is None) and not held:
            self.free_since = self.free_since if self.free_since is not None else now
            if now - self.free_since >= self.grace:
                result = self.docker.cleanup()
                self.log("removed_orphans", containers=owned, **result)
                if result.get("ok"):
                    self.free_since = None
                return "removed_orphans"
            return "orphan_grace"
        self.free_since = None
        if owned is None:
            self.log("docker_error")
            return "docker_error"
        if not owned and not held:
            self.idle_since = self.idle_since if self.idle_since is not None else now
            if now - self.idle_since >= self.idle_exit:
                self.log("idle_exit")
                return "idle_exit"
        else:
            self.idle_since = None
        return "ok"

    def run(self, once=False) -> int:
        self.log("started", interval=self.interval, grace=self.grace, reserve=self.reserve)
        while True:
            try:
                action = self.tick()
            except Exception as error:  # noqa: BLE001 (keep enforcing; retry next tick)
                self.log("tick_error", error=f"{type(error).__name__}: {error}")
                action = "tick_error"
            if once or action == "idle_exit":
                return 0
            self.clock.sleep(self.interval)


def heartbeat_path(main_dir: Path) -> Path:
    return ops.lock_paths(main_dir)["watchdog"].parent / "tool-loop-watchdog.heartbeat"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--interval", type=float, default=30)
    parser.add_argument("--grace", type=float, default=120)
    parser.add_argument("--idle-exit", type=float, default=3600)
    parser.add_argument("--log", type=Path)
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args(argv)
    problem = ops.docker_endpoint_problem()
    if problem:
        print(problem, file=sys.stderr)
        return 2
    ops.pin_local_time()
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    main_dir = ops.main_checkout(ROOT)
    locks = ops.lock_paths(main_dir)
    locks["watchdog"].parent.mkdir(parents=True, exist_ok=True)
    handle = open(locks["watchdog"], "a", encoding="utf-8")  # noqa: SIM115 (held for life)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("watchdog already running", file=sys.stderr)
        return 0
    log = args.log or locks["watchdog"].parent / "tool-loop-watchdog.log"
    dog = Watchdog(
        ops.Docker(),
        lambda: ops.lock_held(locks["runner"]) or ops.lock_held(locks["queue"]),
        ops.SystemClock(),  # the real clock only: no test hook here
        log,
        args.interval,
        args.grace,
        args.idle_exit,
        heartbeat=heartbeat_path(main_dir),
    )
    return dog.run(once=args.once)


if __name__ == "__main__":
    sys.exit(main())
