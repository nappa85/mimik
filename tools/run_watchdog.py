#!/usr/bin/env python3
"""Independent watchdog for tool-loop heavy work (review fix 2). Prototype tier, stdlib only.

A separate process (the queue starts one; it also runs standalone). Every interval (at most
60 s) it checks the machine schedule with the real clock only, and:

- when heavy work is not allowed, force-removes every owned container (name prefix
  mimik-toolloop-v3-: server, harness, hash/check) and the owned network;
- when owned containers exist but neither the runner lock nor the queue lock is held for
  longer than the grace period, removes them as orphans.

It logs every action (JSON lines), never reads input, and exits after --idle-exit seconds with
no lock holder and no owned container. A second watchdog exits at once (watchdog lock).

Usage: run_watchdog.py [--interval 30] [--grace 120] [--idle-exit 3600] [--log FILE] [--once]
"""

import argparse
import datetime as dt
import fcntl
import json
import os
import sys
from pathlib import Path

import toolloop_ops as ops

ROOT = Path(__file__).resolve().parents[1]
MAX_INTERVAL = 60


class Watchdog:
    def __init__(self, docker, lock_held, clock, log_path, interval=30, grace=120, idle_exit=3600):
        if not 0 < interval <= MAX_INTERVAL:
            raise ValueError(f"interval must be in (0, {MAX_INTERVAL}] seconds")
        self.docker, self.lock_held, self.clock = docker, lock_held, clock
        self.log_path = Path(log_path)
        self.interval, self.grace, self.idle_exit = interval, grace, idle_exit
        self.free_since = None
        self.idle_since = None

    def log(self, action, **fields):
        row = {"time": dt.datetime.fromtimestamp(self.clock.time()).isoformat(), "action": action}
        row.update(fields)
        with open(self.log_path, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(row, sort_keys=True) + "\n")

    def tick(self) -> str:
        now = self.clock.time()
        owned = self.docker.owned()
        held = self.lock_held()
        if owned is None:
            self.log("docker_error")
            return "docker_error"
        if not ops.heavy_allowed(now):
            network = self.docker.network_exists()
            if owned or network:
                result = self.docker.cleanup()
                self.log("removed_outside_window", containers=owned, **result)
                return "removed_outside_window"
        if owned and not held:
            self.free_since = self.free_since if self.free_since is not None else now
            if now - self.free_since >= self.grace:
                result = self.docker.cleanup()
                self.log("removed_orphans", containers=owned, **result)
                self.free_since = None
                return "removed_orphans"
            return "orphan_grace"
        self.free_since = None
        if not owned and not held:
            self.idle_since = self.idle_since if self.idle_since is not None else now
            if now - self.idle_since >= self.idle_exit:
                self.log("idle_exit")
                return "idle_exit"
        else:
            self.idle_since = None
        return "ok"

    def run(self, once=False) -> int:
        self.log("started", interval=self.interval, grace=self.grace)
        while True:
            action = self.tick()
            if once or action == "idle_exit":
                return 0
            self.clock.sleep(self.interval)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--interval", type=float, default=30)
    parser.add_argument("--grace", type=float, default=120)
    parser.add_argument("--idle-exit", type=float, default=3600)
    parser.add_argument("--log", type=Path)
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args(argv)
    ops.pin_local_time()
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    locks = ops.lock_paths(ops.main_checkout(ROOT))
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
    )
    return dog.run(once=args.once)


if __name__ == "__main__":
    sys.exit(main())
