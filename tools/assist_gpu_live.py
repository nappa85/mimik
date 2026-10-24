"""The GPU path's live checks (step 2c-2b-1) on the dev host: the confinement test binary's
`live_gpu_*` tests against the pinned runtime and the default model, run natively (ASSIST-40).

Prototype tier, stdlib only. It refuses to start unless heavy work is allowed now (the machine
schedule, `toolloop_ops.heavy_allowed`) and the amdgpu fan is manual at pwm >= 230 (it resets
on every reboot; on the automatic curve the junction reached 85 °C in 22 s at a sustained
143 W, 2026-10-20). While the tests run it samples the GPU every second and kills the whole
test process group when the junction reaches 85 °C, a sensor is unreadable, the fan leaves
manual, or the heavy window ends. It never reads input. The tests themselves refuse to run
without `MIMIK_LIVE_GPU=1` and a manual fan, so the binary run by hand does nothing to the GPU.

`--short` (explicit, never the default) is for the fixed list `SHORT_TESTS` only (the live GPU
checks that last a minute or two; at most 44 °C was seen during them): the automatic fan is
allowed, the junction limit is 75 °C, and the schedule check and the unreadable-sensor stop
stay. Any selected test outside the list is refused in that mode. Long or sustained runs
(benchmarks, model screens, calibration loops, `assist_runtime_measure.py server`, the queue
tooling) keep the manual fan and the 85 °C limit.

  python3 tools/assist_gpu_live.py --binary <confinement-tests> --mimik <release mimik> \
      --runtime <llama dir> \
      --models <0700 folder holding the default model> --scratch <folder> \
      --fake-server <fake_llama_server> --log <file> [--test <name filter>] [--short]

Exit 0 only when every selected test passed; the `GPU-LIVE {...}` lines (the measurement for
docs/results.md) are copied to the log.
"""

import argparse
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

import toolloop_ops as ops

# The run's own junction limit, below the queue's 90 °C (the 2026-10-20 diagnostic used 85).
LIMITS = {"junction_stop_c": 85.0}
# Short mode: lower, because the automatic fan is allowed.
SHORT_LIMITS = {"junction_stop_c": 75.0}
SAMPLE_SECONDS = 1.0
FILTER = "assistant::runtime::launch::live_tests::gpu::live_gpu_"
# The tests short mode accepts (by full name): each runs a minute or two.
SHORT_TESTS = frozenset(
    FILTER[: -len("live_gpu_")] + name
    for name in (
        "live_gpu_the_rx6650_reaches_ready_with_gpu_budgets",
        "live_gpu_killing_llama_server_mid_job_ends_the_job_neutral_and_runs_on_the_cpu",
        "live_gpu_killing_the_whole_sandbox_ends_on_the_cpu_saying_why",
        "live_gpu_landlock_the_rx6650_reaches_ready_under_hakoniwa",
    )
)


def refusal(now, fan, short=False):
    """Why the run must not start, or None. Only `short` (the fixed short tests) goes without
    the manual fan."""
    if not ops.heavy_allowed(now):
        return "heavy work is not allowed now (weekdays 09:00-18:00); run it in the evening or at the weekend"
    if short:
        return None
    return ops.fan_precondition(fan)


def short_refusal(names):
    """Why short mode must refuse the selected tests, or None."""
    other = [name for name in names if name not in SHORT_TESTS]
    if other:
        return f"not a short test (the manual fan is required): {other[0]}"
    return None


def command(args):
    return [
        str(args.binary),
        args.test or FILTER,
        "--include-ignored",
        "--test-threads=1",
        "--nocapture",
    ]


def environment(args, base=None):
    env = dict(os.environ if base is None else base)
    env.pop("MIMIK_LIVE_GPU_SHORT", None)
    if getattr(args, "short", False):
        env["MIMIK_LIVE_GPU_SHORT"] = "1"
    env.update(
        {
            "MIMIK_LIVE_GPU": "1",
            "MIMIK_LIVE_RUNTIME": str(args.runtime),
            "MIMIK_LIVE_MODELS": str(args.models),
            "MIMIK_LIVE_GPU_MODELS": str(args.models),
            "MIMIK_LIVE_SCRATCH": str(args.scratch),
            "MIMIK_LIVE_FAKE_SERVER": str(args.fake_server),
            "MIMIK_LIVE_MIMIK": str(args.mimik),
        }
    )
    return env


def watch(process, guard, read_sensors, read_fan, clock, allowed, log):
    """Samples until the process ends; kills its group on a stop, and on any exception or
    interruption of this loop (the group is never left running). Returns the stop reason or
    None."""
    try:
        while running(process):
            now = clock.time()
            kind, reason = guard.check(read_sensors(), now, read_fan())
            if kind is None and not allowed(now):
                kind, reason = "schedule", "the heavy window ended"
            if kind is not None:
                log(f"STOP {kind}: {reason}")
                kill(process)
                return f"{kind}: {reason}"
            clock.sleep(SAMPLE_SECONDS)
        return None
    finally:
        # Always the whole group, even when the leader has already exited (a child of its
        # group may live on); a group that is gone is no error.
        kill(process)


def running(process):
    """Whether the leader is still running, without reaping it: an exited leader stays a
    zombie, which keeps its PID, and so the group ID, from being reused until `kill` has
    signalled the group."""
    if process.returncode is not None:
        return False
    try:
        return os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None
    except ChildProcessError:
        return False


def kill(process):
    """SIGKILL of the whole group while the leader is still unreaped (so the group ID is
    still ours), then reap the leader. A leader that was already reaped, by `poll` or by
    another waiter, is never signalled through its PID: the ID may belong to another process
    by now."""
    if process.returncode is None:
        try:
            # Peeks without reaping; ECHILD means another waiter has reaped the leader.
            os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        except ChildProcessError:
            pass
        else:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    process.wait()


def selected(args):
    """The tests the filter selects, from the binary's own listing (nothing runs). Refuses an
    empty selection and any test that is not a `live_gpu_` one."""
    try:
        listing = subprocess.run(  # noqa: S603 (fixed argv from the operator's paths)
            [str(args.binary), args.test or FILTER, "--include-ignored", "--list"],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
            stdin=subprocess.DEVNULL,
        )
    except (subprocess.TimeoutExpired, OSError, UnicodeDecodeError) as error:
        return None, f"the test listing could not be run ({type(error).__name__})"
    names = [
        line[: -len(": test")] for line in listing.stdout.splitlines() if line.endswith(": test")
    ]
    if listing.returncode != 0 or not names:
        return None, "the filter selects no test"
    other = [name for name in names if "live_gpu_" not in name]
    if other:
        return None, f"not a live GPU test: {other[0]}"
    return names, None


SUMMARY = re.compile(r"^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored", re.M)


def verified(text, names):
    """Why the run's output does not show every selected test passing, or None: the summary
    must say ok with exactly the selected count passed and none failed or ignored, and every
    selected name must have been started."""
    found = SUMMARY.findall(text)
    if not found:
        return "no test summary in the output"
    status, passed, failed, ignored = found[-1]
    if status != "ok" or int(failed) or int(ignored) or int(passed) != len(names):
        return f"summary {status}: {passed} passed, {failed} failed, {ignored} ignored, expected {len(names)}"
    missing = [name for name in names if f"test {name} ..." not in text]
    if missing:
        return f"test did not run: {missing[0]}"
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for name in (
        "--binary",
        "--mimik",
        "--runtime",
        "--models",
        "--scratch",
        "--fake-server",
        "--log",
    ):
        parser.add_argument(name, type=Path, required=True)
    parser.add_argument("--test")
    parser.add_argument(
        "--short",
        action="store_true",
        help="the fixed short tests only: automatic fan allowed, junction limit 75 °C",
    )
    args = parser.parse_args(argv)
    ops.pin_local_time()
    root = ops.hwmon_root()
    fan = ops.read_fan(root)
    why = refusal(time.time(), fan, args.short)
    if why:
        print(f"assist-gpu-live: refusing: {why}", file=sys.stderr)
        return 2
    names, why = selected(args)
    if not why and args.short:
        why = short_refusal(names)
    if why:
        print(f"assist-gpu-live: refusing: {why}", file=sys.stderr)
        return 2
    args.log.parent.mkdir(parents=True, exist_ok=True)
    with open(args.log, "a", encoding="utf-8") as out:
        start = out.tell()

        def log(line):
            out.write(line + "\n")
            out.flush()

        log(f"START {time.strftime('%Y-%m-%dT%H:%M:%S')} fan {fan} short {args.short}")
        process = subprocess.Popen(  # noqa: S603 (fixed argv from the operator's paths)
            command(args),
            env=environment(args),
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        stopped = watch(
            process,
            ops.ThermalGuard(SHORT_LIMITS if args.short else LIMITS),
            lambda: ops.read_sensors(root),
            # Short mode does not watch the fan (the automatic curve is allowed there).
            (lambda: None) if args.short else (lambda: ops.read_fan(root)),
            ops.SystemClock(),
            ops.heavy_allowed,
            log,
        )
        log(f"END status {process.returncode} stop {stopped}")
    if stopped:
        print(f"assist-gpu-live: stopped: {stopped}", file=sys.stderr)
        return 3
    if process.returncode != 0:
        return 1
    with open(args.log, encoding="utf-8", errors="replace") as written:
        written.seek(start)
        problem = verified(written.read(), names)
    if problem:
        print(f"assist-gpu-live: not a pass: {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
