"""Live measurements of the model runtime (step 2c-2a; prototype tooling, reported as live
inference, never part of the CI gate). Synthetic data only. Prints one JSON object.

  calibrate: Mimik's own runtime path. Fresh data, an Owner, then `mimik --loopback
      --assistant` with the given models folder, runtime and thread count; Mimik's log lines
      (`assistant: ...`, each stamped with the seconds since start) until `ready` or
      `unavailable`. Gives start to ready, priming, the two samples and the derived budgets.
  cache: prompt-cache reuse across prompts (step 3c-0): a stand-in reminder prompt between
      interpretation requests, with `--cache-ram` 0 (as shipped) against other sizes, and the
      cost of re-priming the interpretation prefix. CPU only; threads capped at 2 in the
      weekday window.
  server: the pinned llama-server run natively (ASSIST-40) with the fixed prefix: per-token
      generation time with the grammar (schema-carrying job-like requests) against without
      (ignore_eos samples), a rendered 1 KiB request's prompt tokens, and cancellation: a
      superseding request's first token after a request is dropped mid-generation and
      mid-prompt.

calibrate and server refuse to start on weekdays 09:00-18:00 and stop their processes if
that window opens while they run (the machine schedule); cache may run in the window with at
most 2 threads.
"""

import argparse
import collections
import http.client
import json
import os
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime
from pathlib import Path

import assist_eval
import assist_gpu_live
import toolloop_ops as ops
from assist_demo import create_accounts, demo_passwords, fresh_directory
from smoke_release import smoke_environment


def heavy_allowed(now=None):
    now = now or datetime.now()
    return now.weekday() >= 5 or not 9 <= now.hour < 18


def window_allows(threads, now=None):
    """The machine schedule for small CPU runs: any thread count outside the weekday window,
    at most 2 threads inside it."""
    return heavy_allowed(now) or threads <= 2


class RefusedError(RuntimeError):
    """The run must not start (schedule, fan or temperature)."""


def gpu_refusal(root, now=None):
    """Why a native GPU run must not start, or None: the schedule, the manual fan and a
    sensor reading under the junction limit."""
    now = time.time() if now is None else now
    why = assist_gpu_live.refusal(now, ops.read_fan(root))
    if why:
        return why
    kind, reason = ops.ThermalGuard(assist_gpu_live.LIMITS).check(
        ops.read_sensors(root), now, ops.read_fan(root)
    )
    return f"{kind}: {reason}" if kind else None


def guard(processes, allowed=heavy_allowed):
    """Kills `processes` (a list it reads live) once `allowed()` turns false (by default
    when the weekday window opens)."""

    def watch():
        while True:
            if not allowed():
                for process in list(processes):
                    if process.poll() is None:
                        process.kill()
                os._exit(3)
            time.sleep(20)

    threading.Thread(target=watch, daemon=True).start()


def calibrate(args):
    binary = args.binary.resolve(strict=True)
    directory = fresh_directory(args.data)
    create_accounts(binary, directory, demo_passwords())
    command = [
        str(binary),
        "--loopback",
        "--assistant",
        "--models-dir",
        str(args.models_dir.resolve(strict=True)),
        "--assistant-runtime",
        str(args.runtime.resolve(strict=True)),
        "--assistant-threads",
        str(args.threads),
        *(("--assistant-model", f"local:{args.model}") if args.model else ()),
        *(("--assistant-model-sha256", args.sha256) if args.sha256 else ()),
    ]
    began = time.monotonic()
    process = subprocess.Popen(
        command,
        cwd=directory,
        env=smoke_environment(directory),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    processes = [process]
    guard(processes)
    lines = []
    try:
        for line in process.stdout:
            if line.startswith("assistant: "):
                stamp = round(time.monotonic() - began, 1)
                lines.append({"at_s": stamp, "line": line.rstrip("\n")[len("assistant: ") :]})
                if line.startswith(("assistant: ready", "assistant: unavailable")):
                    break
            if time.monotonic() - began > args.limit:
                lines.append({"at_s": round(time.monotonic() - began, 1), "line": "limit"})
                break
    finally:
        process.send_signal(signal.SIGINT)
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
    return {"mode": "calibrate", "threads": args.threads, "lines": lines}


class Server:
    """The pinned llama-server, native, on a free loopback port with a fresh key."""

    def __init__(self, args):
        self.key = secrets.token_urlsafe(32)
        self.dir = tempfile.TemporaryDirectory(prefix="mimik-measure-")
        key_file = Path(self.dir.name) / "key"
        key_file.write_text(self.key + "\n")
        os.chmod(key_file, 0o600)
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            self.port = probe.getsockname()[1]
        self.process = None
        self.watchdog = None
        self.stop_reason = None
        gpu = getattr(args, "gpu_device", None)
        if gpu:
            # A native GPU run needs the manual fan and the thermal and schedule guard, as
            # `assist_gpu_live.py` does: refused before the spawn, watched from the spawn on.
            root = ops.hwmon_root()
            why = gpu_refusal(root)
            if why:
                self.dir.cleanup()
                raise RefusedError(why)
        log = getattr(args, "log", None)
        self.log = open(log, "wb") if log else None  # noqa: SIM115 (closed in stop)
        try:
            self.process = subprocess.Popen(
                self.command(args, key_file, gpu),
                stdout=subprocess.DEVNULL,
                stderr=self.log or subprocess.DEVNULL,
                env={"PATH": "/usr/bin:/bin", "HOME": self.dir.name},
                start_new_session=bool(gpu),
            )
            if gpu:
                self.watchdog = threading.Thread(target=self.watch_gpu, args=(root,), daemon=True)
                self.watchdog.start()
            self.wait_healthy()
        except BaseException:
            # An interrupt or a failure at any point kills and reaps the server.
            self.stop()
            raise

    def command(self, args, key_file, gpu):
        return [
            str(
                args.runtime
                / ("vulkan" if getattr(args, "gpu_device", None) else "cpu")
                / "llama-server"
            ),
            "--model",
            str(args.models_dir / args.model),
            "--alias",
            "measure",
            "--host",
            "127.0.0.1",
            "--port",
            str(self.port),
            "--api-key-file",
            str(key_file),
            "--offline",
            "--no-webui",
            "--no-slots",
            "--no-mmproj",
            "--jinja",
            *(
                ["--n-gpu-layers", "99", "--device", getattr(args, "gpu_device", None)]
                if getattr(args, "gpu_device", None)
                else ["--n-gpu-layers", "0"]
            ),
            "--fit",
            "off",
            "--threads",
            str(args.threads),
            "--threads-batch",
            str(args.threads),
            "--ctx-size",
            "8192",
            "--parallel",
            "1",
            "--no-warmup",
            "--cache-ram",
            str(getattr(args, "cache_ram", 0)),
            "--reasoning",
            "off",
            *(["--verbosity", str(args.verbosity)] if getattr(args, "verbosity", None) else []),
        ]

    def watch_gpu(self, root):
        self.stop_reason = assist_gpu_live.watch(
            self.process,
            ops.ThermalGuard(assist_gpu_live.LIMITS),
            lambda: ops.read_sensors(root),
            lambda: ops.read_fan(root),
            ops.SystemClock(),
            ops.heavy_allowed,
            lambda line: print(line, file=sys.stderr),
        )

    def wait_healthy(self):
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"the server stopped: {self.stop_reason or 'it exited'}")
            try:
                if self.get("/health")[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.5)
        raise RuntimeError("the server did not become healthy")

    def get(self, path):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        connection.request("GET", path)
        response = connection.getresponse()
        return response.status, response.read()

    def post(self, path, body, timeout=600):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
        connection.request(
            "POST",
            path,
            body=json.dumps(body),
            headers={"Content-Type": "application/json", "Authorization": f"Bearer {self.key}"},
        )
        response = connection.getresponse()
        return json.loads(response.read())

    def chat(self, user, max_tokens, ignore_eos=False, schema=False):
        body = {
            "messages": [
                {"role": "system", "content": [{"type": "text", "text": PREFIX}]},
                {"role": "user", "content": [{"type": "text", "text": user}]},
            ],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "top_k": 1,
            "seed": 0,
            "cache_prompt": True,
            "stream": False,
        }
        if ignore_eos:
            body["ignore_eos"] = True
        if schema:
            body["response_format"] = {
                "type": "json_schema",
                "json_schema": {
                    "name": "mimik_interpretation_v2",
                    "strict": True,
                    "schema": assist_eval.load_v2()[1],
                },
            }
        began = time.monotonic()
        answer = self.post("/v1/chat/completions", body)
        answer["wall_ms"] = (time.monotonic() - began) * 1000
        return answer

    def stream_first_token(self, user, max_tokens, drop_after=None, drop_at_token=None):
        """A streamed request. `drop_after` is an absolute deadline in seconds from the send
        (frequent chunks cannot postpone it); `drop_at_token` drops after that many content
        tokens. Returns a [`Stream`]: the seconds to the first content token, and whether the
        connection was dropped before the stream ended (and with how many tokens seen)."""
        body = {
            "messages": [
                {"role": "system", "content": [{"type": "text", "text": PREFIX}]},
                {"role": "user", "content": [{"type": "text", "text": user}]},
            ],
            "max_tokens": max_tokens,
            "ignore_eos": True,
            "temperature": 0.0,
            "top_k": 1,
            "seed": 0,
            "cache_prompt": True,
            "stream": True,
        }
        raw = socket.create_connection(("127.0.0.1", self.port))
        payload = json.dumps(body).encode()
        raw.sendall(
            (
                f"POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\n"
                f"Authorization: Bearer {self.key}\r\nContent-Type: application/json\r\n"
                f"Content-Length: {len(payload)}\r\n\r\n"
            ).encode()
            + payload
        )
        return read_stream(raw, drop_after, drop_at_token)

    def stop(self):
        if self.process is not None:
            if self.watchdog is not None:
                # Kills the whole group and reaps the leader.
                assist_gpu_live.kill(self.process)
            else:
                self.process.terminate()
                try:
                    self.process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
        if self.log is not None:
            self.log.close()
        self.dir.cleanup()


class Stream(collections.namedtuple("Stream", "first cancelled tokens")):
    """`first`: seconds to the first content token (None if none came); `cancelled`: the
    connection was dropped before the stream ended; `tokens`: content tokens seen."""

    __slots__ = ()


def read_stream(raw, drop_after=None, drop_at_token=None, clock=time.monotonic):
    """Reads a streamed completion from `raw`, dropping it at the absolute `drop_after`
    deadline or after `drop_at_token` content tokens, whichever comes first."""
    began = clock()
    deadline = None if drop_after is None else began + drop_after
    seen = 0
    first = None
    buffer = b""
    try:
        while True:
            if deadline is not None:
                left = deadline - clock()
                if left <= 0:
                    return Stream(first, True, seen)
                raw.settimeout(left)
            chunk = raw.recv(65536)
            if not chunk:
                return Stream(first, False, seen)
            buffer += chunk
            while b"\n\n" in buffer:
                event, buffer = buffer.split(b"\n\n", 1)
                if b'"delta":{"content"' in event.replace(b" ", b""):
                    seen += 1
                    if first is None:
                        first = clock() - began
                    if drop_at_token is not None and seen >= drop_at_token:
                        return Stream(first, True, seen)
                if b"[DONE]" in event:
                    return Stream(first, False, seen)
    except TimeoutError:
        return Stream(first, True, seen)
    finally:
        raw.close()


PREFIX = assist_eval.contract_prefix("v2")


def filler(variant, size=1024):
    """Context-item filler like the runtime's calibration (not byte-identical to it)."""
    fields = [("hp", "Hit points", 20), ("strength", "Strength", 3), ("silver", "Silver", 120)]
    items = []
    n = 0
    while True:
        field, label, value = fields[(n + variant) % len(fields)]
        item = json.dumps(
            {
                "alias": f"f{n + 1}",
                "id": f"{field}_{n}",
                "label": label,
                "type": "integer",
                "value": value + n,
            },
            separators=(",", ":"),
        )
        text = ("[" if variant == 0 else '{"items":[') + ",".join([*items, item]) + "]"
        if len(text) > size:
            break
        items.append(item)
        n += 1
    text = ("[" if variant == 0 else '{"items":[') + ",".join(items) + "]"
    return text + " " * (size - len(text))


def per_token(timings, key_ms, key_n):
    return timings[key_ms] / timings[key_n]


def server(args):
    args.runtime = args.runtime.resolve(strict=True)
    args.models_dir = args.models_dir.resolve(strict=True)
    run = Server(args)
    processes = [run.process]
    guard(processes)
    out = {"mode": "server", "threads": args.threads}
    try:
        prime = run.chat(assist_eval._envelope({"note": "prime"}), 1)
        out["priming"] = {"wall_s": round(prime["wall_ms"] / 1000, 1), "timings": prime["timings"]}
        # Without the grammar: ignore_eos samples of max_tokens.
        free = [run.chat(filler(v), args.max_tokens, ignore_eos=True) for v in (0, 1)]
        # With the grammar: job-like requests from the 2b synthetic cases.
        cases = json.loads((Path(__file__).parent / "assist-eval" / "cases.json").read_text())
        jobs = []
        for case in cases["cases"][: args.jobs]:
            request = assist_eval._build_request_v2(case, cases["sheet"], limit=1024)
            jobs.append(run.chat(request["user"], args.max_tokens, schema=True))
        g_free = [per_token(a["timings"], "predicted_ms", "predicted_n") for a in free]
        g_grammar = [
            per_token(a["timings"], "predicted_ms", "predicted_n")
            for a in jobs
            if a["timings"]["predicted_n"] >= 8
        ]
        out["generation_ms_per_token"] = {
            "without_grammar": [round(g, 1) for g in g_free],
            "with_grammar": [round(g, 1) for g in g_grammar],
            "with_grammar_predicted_n": [a["timings"]["predicted_n"] for a in jobs],
        }
        tokens = [
            len(
                run.post(
                    "/tokenize",
                    {
                        "content": assist_eval._build_request_v2(c, cases["sheet"], limit=1024)[
                            "user"
                        ],
                        "add_special": False,
                    },
                )["tokens"]
            )
            for c in cases["cases"][: args.jobs]
        ]
        out["rendered_1k_prompt_tokens"] = tokens
        out["jobs_prompt"] = [{k: a["timings"][k] for k in ("prompt_n", "cache_n")} for a in jobs]
        # Cancellation mid-generation: drop at the 20th token, then a superseding request.
        cancel_generation = run.stream_first_token(filler(0), args.max_tokens, drop_at_token=20)
        mid_generation = run.stream_first_token(filler(1), 8)
        # Mid-prompt: a fresh 1 KiB part (uncached) dropped at an absolute 3 s deadline. It is
        # a mid-prompt cancel only if no token had arrived by then (`tokens` 0); on a fast
        # GPU prompt evaluation ends first and the record says so.
        cancel_prompt = run.stream_first_token(
            filler(2, 1024).replace(" ", "x"), args.max_tokens, drop_after=3
        )
        mid_prompt = run.stream_first_token(filler(1), 8)
        out["superseding_first_token_s"] = {
            "after_mid_generation_cancel": round(mid_generation.first or -1, 2),
            "after_mid_prompt_cancel": round(mid_prompt.first or -1, 2),
        }
        out["superseding_cancels"] = {
            "mid_generation": {
                "cancelled": cancel_generation.cancelled,
                "tokens_before_cancel": cancel_generation.tokens,
            },
            "mid_prompt": {
                "cancelled": cancel_prompt.cancelled,
                "tokens_before_cancel": cancel_prompt.tokens,
                "valid_mid_prompt": cancel_prompt.cancelled and cancel_prompt.tokens == 0,
            },
        }
    finally:
        run.stop()
    return out


def reuse_fraction(cache_n, prefix_n):
    """The share of the interpretation prefix a request reused from the cache (capped at 1)."""
    return min(1.0, cache_n / prefix_n) if prefix_n else 0.0


def passes(cache_n, prefix_n, threshold=0.75):
    """The pass rule of 3c-0 (the calibration rule): at least 75 % of the prefix reused."""
    return reuse_fraction(cache_n, prefix_n) >= threshold


def stream_result(lines, began, clock=time.monotonic):
    """Reads server-sent-event `lines` (bytes, from `began`): returns (seconds to the first
    non-empty content delta or None, the last `timings` object seen or None)."""
    first = None
    timings = None
    for raw in lines:
        line = raw.strip()
        if not line.startswith(b"data:") or line.endswith(b"[DONE]"):
            continue
        event = json.loads(line[len(b"data:") :])
        if event.get("timings"):
            timings = event["timings"]
        for choice in event.get("choices") or []:
            if first is None and (choice.get("delta") or {}).get("content"):
                first = clock() - began
    return first, timings


def reminder_system(size=7000):
    """A stand-in reminder system message: different text from the interpretation prefix,
    of about `size` characters (the interpretation prefix is about 7.5 KiB)."""
    rules = [
        "You write a short reminder note for a tabletop role-playing character sheet.",
        "Read the data envelope: it lists items the player marked, each with an id and a name.",
        "Answer with one JSON object holding a list of reminders; each has an item id, a",
        "one-sentence text and a due moment chosen from the allowed list. Never invent ids.",
        "Keep every text under 120 characters and do not repeat an item twice.",
    ]
    text = " ".join(rules)
    n = 0
    while len(text) < size:
        n += 1
        text += (
            f" Rule {n}: when the item group g{n} appears, mention its name once, keep the order"
            f" of the envelope, and choose due moment d{n % 7} only if the name says so."
        )
    return text[:size]


def reminder_user(variant=0):
    items = [
        {"id": f"item_{variant}_{i}", "name": f"Rope {i}", "note": f"synthetic {variant}"}
        for i in range(6)
    ]
    return assist_eval._envelope({"items": items})


def timed(run, system, user, max_tokens=16, schema=False):
    """One streamed chat request: seconds to the first content token, wall seconds and the
    final `timings` (`prompt_n`, `cache_n`, `prompt_ms`, ...)."""
    body = {
        "messages": [
            {"role": "system", "content": [{"type": "text", "text": system}]},
            {"role": "user", "content": [{"type": "text", "text": user}]},
        ],
        "max_tokens": max_tokens,
        "temperature": 0.0,
        "top_k": 1,
        "seed": 0,
        "cache_prompt": True,
        "stream": True,
    }
    if schema:
        body["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "mimik_interpretation_v2",
                "strict": True,
                "schema": assist_eval.load_v2()[1],
            },
        }
    began = time.monotonic()
    connection = http.client.HTTPConnection("127.0.0.1", run.port, timeout=900)
    connection.request(
        "POST",
        "/v1/chat/completions",
        body=json.dumps(body),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {run.key}"},
    )
    first, timings = stream_result(connection.getresponse(), began)
    connection.close()
    timings = timings or {}
    return {
        "ttft_s": None if first is None else round(first, 2),
        "wall_s": round(time.monotonic() - began, 2),
        **{
            k: timings.get(k)
            for k in ("prompt_n", "cache_n", "prompt_ms", "predicted_n", "predicted_ms")
        },
    }


def cache_log_lines(path):
    """The log lines that mention the prompt cache, checkpoints or slot reuse."""
    keys = ("cache", "checkpoint", "similarity", "sim_best", "restor", "mib")
    return [
        line.rstrip()[:300]
        for line in Path(path).read_text(errors="replace").splitlines()
        if any(k in line.lower() for k in keys)
    ]


def cache_steps(run):
    """The request sequence of 3c-0 against one server: the steps' records."""

    def interpretation(n):
        return timed(run, PREFIX, assist_eval._envelope({"note": f"case {n}"}), schema=True)

    def reminder(n):
        return timed(run, reminder_system(), reminder_user(n))

    def prime():
        return timed(run, PREFIX, assist_eval._envelope({"note": "prime"}), 1)

    short = (
        ("I1 cold", lambda: interpretation(1)),
        ("R1", lambda: reminder(1)),
        ("I2 after R1", lambda: interpretation(3)),
        ("R2", lambda: reminder(2)),
        ("I3 after R2", lambda: interpretation(4)),
        ("R3", lambda: reminder(3)),
        ("I4 after R3", lambda: interpretation(5)),
    )
    plan = (
        short
        if getattr(run, "short", False)
        else (
            ("I1 cold", lambda: interpretation(1)),
            ("I1b immediate repeat", lambda: interpretation(2)),
            ("R1", lambda: reminder(1)),
            ("I2 after R1", lambda: interpretation(3)),
            ("R2", lambda: reminder(2)),
            ("I3 after R2", lambda: interpretation(4)),
            ("R3", lambda: reminder(3)),
            ("prime after R3 (a)", prime),
            ("I4 after re-prime", lambda: interpretation(5)),
            ("R4", lambda: reminder(4)),
            ("R4b reminder repeat", lambda: reminder(4)),
            ("I5 after R4b", lambda: interpretation(6)),
        )
    )
    return [{"step": name, **step()} for name, step in plan]


def cache_variant(args, cache_ram, rep, log_path):
    """One fresh server with `--cache-ram cache_ram`; returns the steps' records."""
    args.cache_ram = cache_ram
    args.log = log_path
    run = Server(args)
    run.short = args.short
    processes = [run.process]
    guard(processes, lambda: window_allows(args.threads))
    try:
        steps = cache_steps(run)
    finally:
        run.stop()
    return {"cache_ram": cache_ram, "rep": rep, "steps": steps, "log": cache_log_lines(log_path)}


def cache(args):
    args.runtime = args.runtime.resolve(strict=True)
    args.models_dir = args.models_dir.resolve(strict=True)
    scratch = Path(args.log_dir)
    scratch.mkdir(parents=True, exist_ok=True)
    out = {"mode": "cache", "threads": args.threads, "variants": []}
    for rep in range(args.reps):
        for cache_ram in args.cache_ram_sizes:
            if not window_allows(args.threads):
                out["stopped"] = "the weekday window opened"
                return out
            out["variants"].append(
                cache_variant(args, cache_ram, rep, str(scratch / f"c{cache_ram}-r{rep}.log"))
            )
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    cal = commands.add_parser("calibrate")
    cal.add_argument("binary", type=Path)
    cal.add_argument("--models-dir", type=Path, required=True)
    cal.add_argument("--runtime", type=Path, required=True)
    cal.add_argument("--threads", type=int, required=True)
    cal.add_argument("--data", type=Path, required=True)
    cal.add_argument("--limit", type=int, default=1800)
    cal.add_argument("--model", help="a model file other than the default Granite one")
    cal.add_argument("--sha256", help="its SHA-256 (required with --model)")
    srv = commands.add_parser("server")
    srv.add_argument("--models-dir", type=Path, required=True)
    srv.add_argument("--runtime", type=Path, required=True)
    srv.add_argument("--model", default="granite-4.2-3b-Q4_K_M.gguf")
    srv.add_argument("--threads", type=int, required=True)
    srv.add_argument("--max-tokens", type=int, default=352)
    srv.add_argument(
        "--gpu-device", help="run the Vulkan build on this device (for example Vulkan0)"
    )
    srv.add_argument("--jobs", type=int, default=4)
    che = commands.add_parser("cache")
    che.add_argument("--models-dir", type=Path, required=True)
    che.add_argument("--runtime", type=Path, required=True)
    che.add_argument("--model", default="granite-4.2-3b-Q4_K_M.gguf")
    che.add_argument("--threads", type=int, required=True)
    che.add_argument("--cache-ram-sizes", type=int, nargs="+", default=[0, 512, 2048])
    che.add_argument("--reps", type=int, default=2)
    che.add_argument("--short", action="store_true", help="the shorter request sequence")
    che.add_argument("--verbosity", type=int, help="llama-server log verbosity (for sizes)")
    che.add_argument("--log-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    if args.command == "cache":
        if not window_allows(args.threads):
            print(json.dumps({"refused": "weekday 09:00-18:00 allows at most 2 threads"}))
            return 2
        print(json.dumps(cache(args), indent=2))
        return 0
    if not heavy_allowed():
        print(json.dumps({"refused": "weekday 09:00-18:00"}))
        return 2
    result = calibrate(args) if args.command == "calibrate" else server(args)
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
