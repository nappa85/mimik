"""Run a read-only review through opencode on a sanitized snapshot, with fallback signalling.

Exit codes: 0 review written; 3 external reviewer unavailable (quota, auth, timeout,
empty output) so the caller must fall back to another reviewer; 2 usage errors.
The snapshot excludes secrets, user data, build output and caches. The opencode agent
has edit/write/bash/web tools denied, so it can only read the snapshot.
"""

import argparse
import json
import os
import re
import selectors
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

DEFAULT_MODEL = "opencode/muse-spark-1.3-contributor-free"
EXCLUDES = (
    ".env",
    ".env.*",
    "data",
    "target",
    ".git",
    ".venv*",
    "__pycache__",
    ".ruff_cache",
    "*.sqlite3*",
    "*.pdf",
    "*.gguf",
)
UNAVAILABLE = re.compile(
    r"quota|credit|rate.?limit|insufficient|unauthori[sz]ed|forbidden|429|402|"
    r"exceeded|billing|not available|model not found",
    re.IGNORECASE,
)
AGENT_CONFIG = {
    "$schema": "https://opencode.ai/config.json",
    "agent": {
        "reviewer": {
            "mode": "primary",
            "description": "Read-only adversarial reviewer",
            # Only read/glob/grep/todowrite remain. websearch could carry repo text off-box;
            # task/skill could start agents with other permissions (seen 2026-10-16).
            "tools": {
                "write": False,
                "edit": False,
                "bash": False,
                "webfetch": False,
                "websearch": False,
                "task": False,
                "skill": False,
            },
            "permission": {
                "edit": "deny",
                "bash": "deny",
                "webfetch": "deny",
                "websearch": "deny",
                "task": "deny",
                "skill": "deny",
                # Reads outside the snapshot (seen: ~/.claude/CLAUDE.md) must never be asked.
                "external_directory": "deny",
            },
        }
    },
}


def snapshot(source, dest):
    ignore = shutil.ignore_patterns(*EXCLUDES)
    shutil.copytree(source, dest, ignore=ignore, symlinks=False)
    for leftover in dest.rglob(".env*"):
        if leftover.name != ".env.example":
            raise ValueError(f"Secret-like file survived snapshot: {leftover}")
    (dest / "opencode.json").write_text(json.dumps(AGENT_CONFIG, indent=2))


def opencode_command(model, snap, prompt):
    """opencode takes its project directory from `$PWD`, not the process cwd, so a run
    launched from the real repo would ignore the snapshot. Pin it with `--dir` too."""
    return ["opencode", "run", "--dir", str(snap), "-m", model, "--agent", "reviewer", prompt]


def run_with_idle_timeout(command, cwd, total, idle):
    """Kill the run if it exceeds `total` seconds or emits nothing for `idle` seconds."""
    env = {**os.environ, "PWD": str(cwd)}
    proc = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    chunks, start, last = [], time.monotonic(), time.monotonic()
    sel = selectors.DefaultSelector()
    sel.register(proc.stdout, selectors.EVENT_READ)
    while True:
        now = time.monotonic()
        if now - start > total or now - last > idle:
            proc.kill()
            proc.wait()
            chunks.append(f"\n[killed: {'total' if now - start > total else 'idle'} timeout]")
            return 124, "".join(chunks)
        if sel.select(timeout=1.0):
            line = proc.stdout.readline()
            if not line:
                break
            chunks.append(line)
            last = time.monotonic()
        elif proc.poll() is not None:
            break
    rest = proc.stdout.read()
    chunks.append(rest)
    return proc.wait(), "".join(chunks)


def preflight(model, cwd, timeout=45, attempts=3, base_delay=10):
    """Retry transient failures briefly and log each one. An exhausted quota hangs instead.
    `UnknownError` at startup usually means opencode did not load the snapshot's
    `reviewer` agent (see `opencode_command`)."""
    command = opencode_command(model, cwd, "Reply with exactly: PONG")
    for attempt in range(attempts):
        code, output = run_with_idle_timeout(command, cwd, timeout, timeout)
        if code == 0 and "PONG" in output:
            return True
        tail = " ".join(output.split())[-300:]
        print(
            f"preflight attempt {attempt + 1}/{attempts} failed (exit {code}): {tail}",
            file=sys.stderr,
        )
        if attempt + 1 < attempts:
            time.sleep(base_delay * 2**attempt)
    return False


def classify(returncode, output):
    text = output.strip()
    if returncode != 0 or not text or UNAVAILABLE.search(text[-2000:]) and len(text) < 4000:
        return "unavailable"
    return "ok"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="Repo or worktree to snapshot")
    parser.add_argument("--prompt-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--diff", action="store_true", help="Include `git diff` vs HEAD")
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--idle-timeout", type=int, default=120)
    args = parser.parse_args()
    if not args.prompt_file.is_file() or not (args.source / "AGENTS.md").is_file():
        print("Prompt file or Mimik source missing", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="mimik-review-") as tmp:
        snap = Path(tmp) / "snapshot"
        snapshot(args.source, snap)
        if args.diff:
            diff = subprocess.run(
                ["git", "-C", str(args.source), "diff", "HEAD"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            status = subprocess.run(
                ["git", "-C", str(args.source), "status", "--short"],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            (snap / "REVIEW_DIFF.patch").write_text(diff)
            (snap / "REVIEW_STATUS.txt").write_text(status)
        prompt = args.prompt_file.read_text()
        if not preflight(args.model, snap):
            args.output.write_text("preflight failed: external reviewer unavailable")
            print("External reviewer preflight failed; fall back.", file=sys.stderr)
            return 3
        command = opencode_command(args.model, snap, prompt)
        code, output = run_with_idle_timeout(command, snap, args.timeout, args.idle_timeout)
    args.output.write_text(output)
    if classify(code, output) == "unavailable":
        print(f"External reviewer unavailable (exit {code}); fall back.", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main())
