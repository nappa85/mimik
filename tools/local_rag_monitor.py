"""Optional host-side metadata sampler for the isolated benchmark container only."""

import argparse
import json
import subprocess
import time
from pathlib import Path


def counters(pid):
    result = {}
    if pid:
        try:
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                key, _, value = line.partition(":")
                if key in {"VmRSS", "VmHWM"}:
                    result[key + "_kib"] = int(value.strip().split()[0])
            for line in Path(f"/proc/{pid}/cgroup").read_text().splitlines():
                if line.startswith("0::"):
                    root = Path("/sys/fs/cgroup") / line[3:].lstrip("/")
                    for name in ("memory.current", "memory.peak"):
                        path = root / name
                        if path.is_file():
                            result[name + "_bytes"] = int(path.read_text())
        except (OSError, ValueError):
            result["process_counters_unavailable"] = True
    for name in ("mem_info_vram_used", "mem_info_vram_total", "gpu_busy_percent"):
        path = Path("/sys/class/drm/card0/device") / name
        try:
            result[name] = int(path.read_text())
        except (OSError, ValueError):
            result[name] = None
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--container", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seconds", type=int, default=2700)
    args = parser.parse_args()
    started = time.monotonic()
    with args.output.open("x") as out:
        while time.monotonic() - started < args.seconds:
            inspected = subprocess.run(
                ["docker", "inspect", "--format", "{{.State.Pid}}", args.container],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            pid = int(inspected.stdout.strip()) if inspected.returncode == 0 else 0
            row = {"elapsed_seconds": time.monotonic() - started, "pid": pid, **counters(pid)}
            out.write(json.dumps(row) + "\n")
            out.flush()
            time.sleep(1)


if __name__ == "__main__":
    main()
