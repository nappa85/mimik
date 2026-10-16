"""Outer elapsed-deadline controller for pinned GGUF transfers (prototype tier).

Every phase of a candidate (HF metadata, provenance, each transport attempt and
resume, SHA256 verification) takes its timeout from ``setup_remaining`` with the
SAME original candidate start, so a transport switch never restarts the clock.
A phase whose remaining time is exhausted is not launched. Each phase runs in a
contained, non-root, capability-free container with only the output directory
mounted; the container is stopped at the deadline and additionally carries an
in-container ``timeout -s KILL`` backstop so the deadline holds even if this
supervisor dies. A file is marked verified only after exact size and SHA256
match the pin.
"""

import argparse
import json
import math
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from screen_limits import setup_remaining

IMAGE = "mimik-model-screen:20261015"
DEFAULT_OUT = Path(__file__).resolve().parents[1] / "target/model-fetch-20261015"
MAX_GLOBAL_HORIZON = 5 * 3600
SAFETY = 0.6
TRANSPORTS = ("curl", "aria2", "curl", "aria2")


@dataclass(frozen=True)
class Candidate:
    ident: str
    repo: str
    revision: str
    filename: str
    sha256: str
    approx_bytes: int
    license: str
    cap_seconds: float = 900

    def url(self, name=None):
        return f"https://huggingface.co/{self.repo}/resolve/{self.revision}/{name or self.filename}"


# Smaller candidate first. Byte counts are the x-linked-size observed for the pin.
CANDIDATES = (
    Candidate(
        "qwen35-9b",
        "unsloth/Qwen3.5-9B-GGUF",
        "3885219b6810b007914f3a7950a8d1b469d598a5",
        "Qwen3.5-9B-Q4_K_M.gguf",
        "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8",
        5_680_522_464,
        "apache-2.0",
    ),
    Candidate(
        "qwen36-35b-a3b",
        "ggml-org/Qwen3.6-35B-A3B-GGUF",
        "baec3ebee244827cda0f4557eafa8b28f7545fa6",
        "Qwen3.6-35B-A3B-Q4_K_M.gguf",
        "671e47e0ec53c665d048b98c3ecbfd5236b5ca9c3e02ed19fc8f81f7b85140c7",
        20_419_565_568,
        "apache-2.0",
    ),
)


@dataclass(frozen=True)
class RunResult:
    code: int
    stdout: str
    timed_out: bool


class DeadlineExhaustedError(Exception):
    pass


def caps_for(throughput_bytes_per_s):
    if (
        isinstance(throughput_bytes_per_s, bool)
        or not math.isfinite(throughput_bytes_per_s)
        or throughput_bytes_per_s <= 0
    ):
        raise ValueError("Measured throughput must be positive and finite")
    return {c.ident: c.approx_bytes / (SAFETY * throughput_bytes_per_s) for c in CANDIDATES}


def container_command(name, out, argv, timeout, network=True):
    command = [
        "docker",
        "run",
        "--rm",
        "--name",
        name,
        "--user",
        "1000:1000",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--tmpfs",
        "/tmp:size=64m",
        "--memory",
        "2g",
        "--pids-limit",
        "256",
        "-e",
        "HOME=/tmp",
        "--mount",
        f"type=bind,src={out},dst=/out",
    ]
    if not network:
        command += ["--network", "none"]
    return [*command, IMAGE, "timeout", "-s", "KILL", str(math.ceil(timeout)), *argv]


def stop_container(name):
    subprocess.run(["docker", "stop", "-t", "1", name], capture_output=True, check=False)
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)


def docker_runner(out):
    logs = out / "logs"
    logs.mkdir(parents=True, exist_ok=True)

    def run(name, argv, timeout):
        log = logs / (name + ".log")
        network = not name.split("-")[-2] == "verify"
        with log.open("w") as handle:
            try:
                result = subprocess.run(
                    container_command(name, out, argv, timeout, network=network),
                    stdout=handle,
                    stderr=subprocess.STDOUT,
                    timeout=timeout,
                    check=False,
                )
                code, timed_out = result.returncode, result.returncode in (124, 137)
            except subprocess.TimeoutExpired:
                stop_container(name)
                code, timed_out = -9, True
        return RunResult(code, log.read_text(errors="replace")[-65536:], timed_out)

    return run


class Controller:
    def __init__(self, out, global_cutoff, runner, clock=time.time):
        self.out = Path(out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.global_cutoff = global_cutoff
        self.runner = runner
        self.clock = clock
        self.ledger = self.out / "ledger.jsonl"
        self.sequence = 0

    def log(self, **row):
        row.setdefault("at_unix", self.clock())
        with self.ledger.open("a") as handle:
            handle.write(json.dumps(row, sort_keys=True) + "\n")

    def phase(self, cand, started, phase, argv):
        now = self.clock()
        base = {
            "candidate": cand.ident,
            "phase": phase,
            "candidate_started_unix": started,
        }
        try:
            timeout = setup_remaining(
                self.global_cutoff, started, now, cap_seconds=cand.cap_seconds
            )
        except ValueError as error:
            self.log(**base, status="deadline_exhausted", error=str(error), launched=False)
            raise DeadlineExhaustedError(phase) from error
        self.sequence += 1
        name = f"mimik-fetch-{cand.ident}-{phase}-{self.sequence}"
        result = self.runner(name, argv, timeout)
        status = "timed_out" if result.timed_out else ("ok" if result.code == 0 else "failed")
        self.log(
            **base,
            status=status,
            container=name,
            phase_started_unix=now,
            phase_ended_unix=self.clock(),
            timeout_s=timeout,
            exit_code=result.code,
            bytes=self.size(cand),
        )
        if result.timed_out:
            raise DeadlineExhaustedError(phase)
        return result

    def size(self, cand):
        path = self.out / cand.filename
        return path.stat().st_size if path.exists() else 0

    def fetch(self, cand):
        started = self.clock()
        self.log(
            candidate=cand.ident,
            event="candidate_start",
            candidate_started_unix=started,
            cap_seconds=cand.cap_seconds,
            global_cutoff_unix=self.global_cutoff,
            repo=cand.repo,
            revision=cand.revision,
            filename=cand.filename,
            pinned_sha256=cand.sha256,
        )
        try:
            status = self._fetch(cand, started)
        except DeadlineExhaustedError:
            status = "deadline_exhausted"
        self.log(
            candidate=cand.ident,
            event="candidate_end",
            status=status,
            candidate_started_unix=started,
            elapsed_s=self.clock() - started,
            bytes=self.size(cand),
        )
        return status

    def _fetch(self, cand, started):
        marker = self.out / (cand.filename + ".verified.json")
        marker.unlink(missing_ok=True)
        head = self.phase(cand, started, "metadata", ["curl", "-sS", "-I", cand.url()])
        fields = dict(re.findall(r"(?im)^([a-z-]+):\s*\"?([^\"\r\n]*)\"?\s*$", head.stdout))
        if (
            head.code != 0
            or fields.get("x-linked-etag") != cand.sha256
            or fields.get("x-repo-commit") != cand.revision
            or not fields.get("x-linked-size", "").isdigit()
        ):
            return "metadata_mismatch"
        size = int(fields["x-linked-size"])
        api = f"https://huggingface.co/api/models/{cand.repo}/revision/{cand.revision}"
        self.phase(
            cand,
            started,
            "provenance",
            [
                "sh",
                "-c",
                'curl -fsSL -o "/out/$1.README.md" "$2" && curl -fsSL -o "/out/$1.api.json" "$3"',
                "provenance",
                cand.ident,
                cand.url("README.md"),
                api,
            ],
        )
        for transport in TRANSPORTS:
            if self.size(cand) == size:
                break
            if self.size(cand) > size:
                return "transfer_oversize"
            if transport == "curl":
                argv = [
                    "curl", "-fsSL", "-C", "-", "--speed-limit", "65536", "--speed-time", "120",
                    "-o", "/out/" + cand.filename, cand.url(),
                ]  # fmt: skip
            else:
                argv = [
                    "aria2c", "-c", "-x", "4", "-s", "4", "--file-allocation=none",
                    "--auto-file-renaming=false", "--summary-interval=60",
                    "--console-log-level=warn", "-d", "/out", "-o", cand.filename, cand.url(),
                ]  # fmt: skip
            self.phase(cand, started, transport, argv)
        if self.size(cand) != size:
            return "transfer_incomplete"
        digest = self.phase(cand, started, "verify", ["sha256sum", "/out/" + cand.filename])
        observed = digest.stdout.split()[0] if digest.code == 0 and digest.stdout.split() else ""
        if observed != cand.sha256:
            self.log(
                candidate=cand.ident,
                event="hash",
                status="hash_mismatch",
                sha256=observed,
            )
            return "hash_mismatch"
        marker.write_text(
            json.dumps(
                {
                    "repo": cand.repo,
                    "revision": cand.revision,
                    "filename": cand.filename,
                }
                | {"bytes": size, "sha256": observed, "license": cand.license}
                | {"verified_at_unix": self.clock()},
                sort_keys=True,
            )
        )
        self.log(candidate=cand.ident, event="hash", status="verified", sha256=observed)
        return "verified"


def probe(out, seconds):
    """Bounded byte-range transfer of the larger pin to /dev/null."""
    cand = CANDIDATES[1]
    name = "mimik-fetch-probe-" + str(int(time.time()))
    argv = [
        "curl", "-sS", "-L", "-r", "0-21474836479", "--max-time", str(seconds),
        "-o", "/dev/null", "-w", "%{size_download} %{speed_download}", cand.url(),
    ]  # fmt: skip
    result = docker_runner(out)(name, argv, seconds + 15)
    downloaded, speed = (float(v) for v in result.stdout.split()[-2:])
    row = {
        "event": "probe",
        "seconds": seconds,
        "bytes": downloaded,
        "bytes_per_s": speed,
    }
    with (out / "ledger.jsonl").open("a") as handle:
        handle.write(json.dumps(row | {"at_unix": time.time(), "exit_code": result.code}) + "\n")
    return row


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    sub = parser.add_subparsers(dest="command", required=True)
    probe_cmd = sub.add_parser("probe")
    probe_cmd.add_argument("--seconds", type=int, default=60)
    fetch_cmd = sub.add_parser("fetch")
    fetch_cmd.add_argument("--throughput-bytes-per-s", type=float, required=True)
    fetch_cmd.add_argument("--global-cutoff-unix", type=float, required=True)
    args = parser.parse_args(argv)
    args.out.mkdir(parents=True, exist_ok=True)
    if args.command == "probe":
        print(json.dumps(probe(args.out, args.seconds)))
        return 0
    if not time.time() < args.global_cutoff_unix <= time.time() + MAX_GLOBAL_HORIZON:
        parser.error("global cutoff must be in the future and at most 5 hours away")
    caps = caps_for(args.throughput_bytes_per_s)
    controller = Controller(args.out, args.global_cutoff_unix, docker_runner(args.out))
    projected = sum(caps.values())
    controller.log(
        event="plan",
        throughput_bytes_per_s=args.throughput_bytes_per_s,
        caps_s=caps,
        projected_total_s=projected,
        fits_global=time.time() + projected <= args.global_cutoff_unix,
    )
    statuses = {}
    for cand in CANDIDATES:
        cand = Candidate(**{**cand.__dict__, "cap_seconds": caps[cand.ident]})
        statuses[cand.ident] = controller.fetch(cand)
    print(json.dumps(statuses))
    return 0 if all(s == "verified" for s in statuses.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
