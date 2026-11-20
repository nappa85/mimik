"""Unpacked-release smoke with the assistant on, on the production runtime path.

Starts the unpacked release binary with `--assistant` (the bundled `llama/` folder, the CI
test model, `--assistant-device cpu`, a fresh data directory, `--loopback`), signs in as a
freshly bootstrapped Owner, and polls the assistant page (`/admin/assistant`, its State
section) until the runtime leaves "Starting". It requires `Ready`, then checks the server log
for the generation's own lines (the start ran at confinement level 1, was primed, and was
measured), stops the server with SIGINT and checks that it exits cleanly and leaves no
llama-server behind. Any other final state is a failure and is printed with its reason.

Why `Ready` is the required state, and why the model is SmolLM2-135M-Instruct Q4_K_M
(`schemas/llama-runtime-v1.json`, `smoke_model`): on 2026-11-20 the trusted release build,
started the same way (`--assistant --assistant-device cpu`), was measured with both CI models.
stories260K never started: three counted attempts, each "priming failed (the priming request
answered HTTP 400)". SmolLM2-135M-Instruct Q4_K_M reached Ready: health after 0.3 s, primed in
6.6 s (2196 prompt tokens), then "ready: generation 1, level 1 ... samples [...]; B 1024 bytes,
N(B) 306 tokens, R 25 s, D 1, Q 97 s", SIGINT exit 0 and no llama-server left. The manager
reaches Ready only after /health, priming and two calibration samples (`manager.rs`, `warm` and
`calibrate_budgets`). If a future model or runtime reaches a refusal instead, that is a failure
here: investigate it, do not accept several states.

Standard library plus `smoke_release.py` (the release smoke's HTTP, sign-in, start and stop
helpers); run it as `python3 tools/assist_release_smoke.py ...`.
"""

import argparse
import html
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from smoke_release import (
    auth_cookie,
    auth_fields,
    auth_login,
    auth_request,
    require,
    smoke_environment,
    start,
    stop,
)

REQUIRED = "Ready"
# The log lines of a generation that really started, primed and measured (`manager.rs`: the
# priming line, then "ready: generation N, level L; priming ... samples ...").
LOG_PRIMED = "assistant: primed in "
LOG_READY = re.compile(r"^assistant: ready: generation \d+, level 1\b.*samples \[", re.M)
STATE = re.compile(r"<h2>State</h2>\s*<p><strong>([^<]*)</strong>(?::\s*([^<]*))?</p>")
OWNER_PASSWORD = b"a synthetic owner passphrase for the smoke"


def read_state(page):
    """The runtime state the page shows: `(state, reason)`, e.g. `("Unavailable", "...")`."""
    match = STATE.search(page)
    require(match is not None, "the assistant page has no State section (assistant off?)")
    return html.unescape(match[1]).strip(), html.unescape(match[2] or "").strip()


def wait_for_state(fetch, *, timeout, interval=1.0, clock=time.monotonic, sleep=time.sleep):
    """Polls `fetch()` (the page's HTML) until the state is not "Starting"; returns it.

    Raises `TimeoutError` naming the last state when `timeout` seconds pass first."""
    began = clock()
    last = ("none", "")
    while True:
        last = read_state(fetch())
        if last[0] != "Starting":
            return last
        if clock() - began >= timeout:
            raise TimeoutError(f"still {last[0]}: {last[1]} after {timeout} s")
        sleep(interval)


def check_outcome(state, log_text):
    """`state` is the final `(state, reason)`; `log_text` the server's stderr."""
    name, reason = state
    require(
        name == REQUIRED,
        f"the assistant ended {name}" + (f": {reason}" if reason else "") + f", not {REQUIRED}",
    )
    require(LOG_PRIMED in log_text, "the log has no priming line: the start did not prime")
    require(
        LOG_READY.search(log_text) is not None,
        "the log has no 'ready ... level 1 ... samples' line: not measured at level 1",
    )


def leftovers():
    """PIDs of running processes that are a llama-server or an `assistant-runner` (a runner or
    reaper that never exec'd the server), by executable name or any argument."""
    found = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            argv = (entry / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if os.path.basename(argv[0]) == b"llama-server" or b"assistant-runner" in argv:
            found.append(int(entry.name))
    return found


def no_leftovers(timeout=10):
    began = time.monotonic()
    while True:
        left = leftovers()
        if not left:
            return
        require(time.monotonic() - began < timeout, f"model server processes left running: {left}")
        time.sleep(0.2)


def sign_in(login, password):
    """The Owner's session cookie, through the release smoke's own sign-in helpers."""
    response = auth_request("/auth/login")
    return auth_login(auth_cookie(response), auth_fields(response, "/auth/login"), login, password)


def assistant_page(cookie):
    status, _, page = auth_request("/admin/assistant", cookie=cookie)
    require(status == 200, f"/admin/assistant answered {status}")
    return page


def bootstrap_owner(binary, directory, password):
    path = directory / "owner.password"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(password)
    result = subprocess.run(
        [
            str(binary),
            "--database",
            str(directory / "smoke.sqlite3"),
            "bootstrap-owner",
            "--login",
            "owner",
            "--display-name",
            "Owner",
            "--password-file",
            str(path),
        ],
        cwd=directory,
        env=smoke_environment(directory),
        capture_output=True,
        timeout=60,
        check=False,
    )
    tail = result.stderr.decode(errors="replace").strip()[-400:]
    require(result.returncode == 0, f"bootstrap-owner exited {result.returncode}: {tail}")


def run(args):
    binary = args.binary.resolve(strict=True)
    runtime = args.runtime.resolve(strict=True)
    models = args.models_dir.resolve(strict=True)
    require(not leftovers(), "a model server process was already running before the smoke")
    directory = args.data
    directory.mkdir(mode=0o700, parents=True)
    directory = directory.resolve(strict=True)
    bootstrap_owner(binary, directory, OWNER_PASSWORD)
    # `start` serves on 127.0.0.1:3000 (`--loopback`) and waits up to 10 s for the sign-in page;
    # the model server starts in the background after the HTTP server's setup, so the page is
    # up long before the runtime is measured.
    process, log = start(
        binary,
        directory,
        auth=True,
        extra=(
            "--assistant",
            "--assistant-runtime",
            str(runtime),
            "--models-dir",
            str(models),
            "--assistant-model",
            args.model,
            "--assistant-model-sha256",
            args.model_sha256,
            "--assistant-device",
            "cpu",
        ),
    )
    log_path = directory / "server.log"
    try:
        cookie = sign_in("owner", OWNER_PASSWORD)
        state = wait_for_state(lambda: assistant_page(cookie), timeout=args.timeout)
        print(f"assistant state: {state[0]}" + (f": {state[1]}" if state[1] else ""))
        # SIGINT, a clean exit within 10 s (and a kill with a failure otherwise).
        stop(process, log)
        no_leftovers()
        check_outcome(state, log_path.read_text(encoding="utf-8", errors="replace"))
    except BaseException:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        log.close()
        print("--- server log (last 40 lines) ---", file=sys.stderr)
        text = log_path.read_text(encoding="utf-8", errors="replace")
        print("\n".join(text.splitlines()[-40:]), file=sys.stderr)
        raise


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--models-dir", type=Path, required=True)
    parser.add_argument("--model", required=True, help="local:<name>.gguf")
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--data", type=Path, help="a new directory (default: a temporary one)")
    parser.add_argument("--timeout", type=int, default=600, help="seconds in Starting")
    args = parser.parse_args(argv)
    if args.data is None:
        args.data = Path(tempfile.mkdtemp(prefix="assist-release-")) / "data"
    try:
        run(args)
    except (AssertionError, TimeoutError, subprocess.SubprocessError) as why:
        print(f"FAILED: {why}", file=sys.stderr)
        return 1
    print("assistant-on release smoke: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
