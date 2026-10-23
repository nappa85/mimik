"""The GPU path's live checks (step 2c-2b-1) on the dev host: the confinement test binary's
`live_gpu_*` tests against the pinned runtime and the default model, run natively (ASSIST-40).

Prototype tier, stdlib only. It refuses to start unless heavy work is allowed now (the machine
schedule, `toolloop_ops.heavy_allowed`) and the amdgpu fan is manual at pwm >= 230 (it resets
on every reboot; on the automatic curve the junction reached 85 °C in 22 s, 2026-10-20). While
the tests run it samples the GPU every second and kills the whole test process group when the
junction reaches 85 °C, a sensor is unreadable, the fan leaves manual, or the heavy window
ends. It never reads input. The tests themselves refuse to run without `MIMIK_LIVE_GPU=1` and a
manual fan, so the binary run by hand does nothing to the GPU.

  python3 tools/assist_gpu_live.py --binary <confinement-tests> --runtime <llama dir> \
      --models <0700 folder holding the default model> --scratch <folder> \
      --fake-server <fake_llama_server> --log <file> [--test <name filter>]

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
SAMPLE_SECONDS = 1.0
FILTER = "assistant::runtime::launch::live_tests::gpu::live_gpu_"


def refusal(now, fan):
    """Why the run must not start, or None."""
    if not ops.heavy_allowed(now):
        return "heavy work is not allowed now (weekdays 09:00-18:00); run it in the evening or at the weekend"
    return ops.fan_precondition(fan)


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
    env.update(
        {
            "MIMIK_LIVE_GPU": "1",
            "MIMIK_LIVE_RUNTIME": str(args.runtime),
            "MIMIK_LIVE_MODELS": str(args.models),
            "MIMIK_LIVE_GPU_MODELS": str(args.models),
            "MIMIK_LIVE_SCRATCH": str(args.scratch),
            "MIMIK_LIVE_FAKE_SERVER": str(args.fake_server),
        }
    )
    return env


def watch(process, guard, read_sensors, read_fan, clock, allowed, log):
    """Samples until the process ends; kills its group on a stop, and on any exception or
    interruption of this loop (the group is never left running). Returns the stop reason or
    None."""
    try:
        while process.poll() is None:
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
        if process.poll() is None:
            kill(process)


def kill(process):
    """SIGKILL of the whole group, then reap."""
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def selected(args):
    """The tests the filter selects, from the binary's own listing (nothing runs). Refuses an
    empty selection and any test that is not a `live_gpu_` one."""
    listing = subprocess.run(  # noqa: S603 (fixed argv from the operator's paths)
        [str(args.binary), args.test or FILTER, "--include-ignored", "--list"],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
        stdin=subprocess.DEVNULL,
    )
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
    for name in ("--binary", "--runtime", "--models", "--scratch", "--fake-server", "--log"):
        parser.add_argument(name, type=Path, required=True)
    parser.add_argument("--test")
    args = parser.parse_args(argv)
    ops.pin_local_time()
    root = ops.hwmon_root()
    fan = ops.read_fan(root)
    why = refusal(time.time(), fan)
    if why:
        print(f"assist-gpu-live: refusing: {why}", file=sys.stderr)
        return 2
    names, why = selected(args)
    if why:
        print(f"assist-gpu-live: refusing: {why}", file=sys.stderr)
        return 2
    args.log.parent.mkdir(parents=True, exist_ok=True)
    with open(args.log, "a", encoding="utf-8") as out:
        start = out.tell()

        def log(line):
            out.write(line + "\n")
            out.flush()

        log(f"START {time.strftime('%Y-%m-%dT%H:%M:%S')} fan {fan}")
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
            ops.ThermalGuard(LIMITS),
            lambda: ops.read_sensors(root),
            lambda: ops.read_fan(root),
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
