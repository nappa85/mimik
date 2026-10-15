"""One bounded, sequential container tournament over already verified local references."""

import argparse
import json
import subprocess
import time
from pathlib import Path

from screen_limits import admit_execution

CUTOFF = 1792072246.0  # 2026-10-15 13:50:46 UTC, approved inference hard stop.
REFERENCES = [
    (
        "instruct4",
        "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        "2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e",
    ),
    (
        "qwen8",
        "Qwen3-8B-Q4_K_M.gguf",
        "d98cdcbd03e17ce47681435b5150e34c1417f50b5c0019dd560e4882c5745785",
    ),
    (
        "qwen4",
        "Qwen3-4B-Q4_K_M.gguf",
        "7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5",
    ),
    ("kev08", None, None),
]


def available_memory():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise ValueError("Host memory availability unknown")


def reservation(used, cutoff, free_bytes):
    admit_execution(cutoff, 384 - used, free_bytes)
    if used + 48 > 384:
        raise ValueError("Whole candidate allowance exceeds aggregate cap")
    return used + 48


def available_vram():
    for total in Path("/sys/class/drm").glob("card[0-9]/device/mem_info_vram_total"):
        return int(total.read_text()) - int(total.with_name("mem_info_vram_used").read_text())
    raise ValueError("GPU memory admission unavailable")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root, out = args.root.resolve(), args.output.resolve()
    if not out.is_dir() or not (out / "cases.json").is_file():
        raise ValueError("Freeze cases before launching")
    ledger = out / "launches.jsonl"
    used = 0
    with ledger.open("x") as log:
        for ident, model, model_hash in REFERENCES:
            name = "mimik-scope-" + ident + "-20261015"
            started = time.time()
            record = {
                "candidate": ident,
                "started_at_unix": started,
                "cutoff_unix": CUTOFF,
            }
            try:
                free = available_memory()
                used = reservation(used, CUTOFF, free)
                image = "mimik-decision-anyjev:20261015" if model else "mimik-decision-kev:20261015"
                command = [
                    "docker",
                    "run",
                    "--name",
                    name,
                    "--network",
                    "none",
                    "--memory",
                    "24g",
                    "--memory-swap",
                    "24g",
                    "--cpus",
                    "6",
                    "--user",
                    "1000:1000",
                    "--security-opt",
                    "no-new-privileges",
                    "--cap-drop",
                    "ALL",
                    "--mount",
                    f"type=bind,src={root / 'tools'},dst=/checks,readonly",
                    "--mount",
                    f"type=bind,src={out},dst=/results",
                    "-e",
                    "HF_HUB_OFFLINE=1",
                    "-e",
                    "TRANSFORMERS_OFFLINE=1",
                    "-e",
                    "HF_HUB_DISABLE_TELEMETRY=1",
                    "-e",
                    "HOME=/tmp",
                ]
                if model:
                    if available_vram() < 6 * 1024**3:
                        raise ValueError("Insufficient free VRAM for reference screen")
                    command += [
                        "--device",
                        "/dev/dri",
                        "--group-add",
                        str(Path("/dev/dri/renderD128").stat().st_gid),
                        "--group-add",
                        str(Path("/dev/dri/card0").stat().st_gid),
                        "--mount",
                        f"type=bind,src={root / 'target/local-rag-benchmark-20261015' / model},dst=/model.gguf,readonly",
                    ]
                else:
                    command += [
                        "--mount",
                        f"type=bind,src={root / 'target/local-decision-benchmark-20261015/hf'},dst=/models,readonly",
                        "-e",
                        "HF_HUB_CACHE=/models",
                        "-e",
                        "KEV_PREFIX_CACHE=4",
                        "-e",
                        "KEV_PREFIX_MAX_TOKENS=8192",
                    ]
                command += [
                    image,
                    "python",
                    "/checks/local_model_screen.py",
                    "--backend",
                    "anyjev" if model else "kev",
                    "--cases",
                    "/results/cases.json",
                    "--output",
                    "/results/" + ident + ".jsonl",
                    "--budget-seconds",
                    "320",
                    "--cutoff-unix",
                    str(CUTOFF),
                    "--remaining-evaluations",
                    "48",
                    "--free-bytes",
                    str(free),
                ]
                if model:
                    command += ["--model", "/model.gguf", "--model-sha256", model_hash]
                else:
                    command += [
                        "--kev-checkpoint",
                        "/models/models--jaredpalmer--kev-0.8b/snapshots/bf75a6a8848ea6960ff2ed108d9ed44c2941174f",
                    ]
                with (out / (ident + ".console.log")).open("x") as console:
                    result = subprocess.run(
                        command,
                        stdout=console,
                        stderr=subprocess.STDOUT,
                        timeout=max(0.001, CUTOFF - time.time()),
                        check=False,
                    )
                record.update(
                    status="exited",
                    exit_code=result.returncode,
                    reserved_aggregate=used,
                )
            except (ValueError, OSError, subprocess.TimeoutExpired) as error:
                record.update(
                    status="blocked_or_stopped",
                    error=str(error),
                    reserved_aggregate=used,
                )
                subprocess.run(
                    ["docker", "stop", "-t", "1", name],
                    capture_output=True,
                    check=False,
                )
            finally:
                record["finished_at_unix"] = time.time()
                log.write(json.dumps(record) + "\n")
                log.flush()


if __name__ == "__main__":
    main()
