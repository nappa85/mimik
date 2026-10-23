"""Live measurements of the model runtime (step 2c-2a; prototype tooling, reported as live
inference, never part of the CI gate). Synthetic data only. Prints one JSON object.

  calibrate: Mimik's own runtime path. Fresh data, an Owner, then `mimik --loopback
      --assistant` with the given models folder, runtime and thread count; Mimik's log lines
      (`assistant: ...`, each stamped with the seconds since start) until `ready` or
      `unavailable`. Gives start to ready, priming, the two samples and the derived budgets.
  server: the pinned llama-server run natively (ASSIST-40) with the fixed prefix: per-token
      generation time with the grammar (schema-carrying job-like requests) against without
      (ignore_eos samples), a rendered 1 KiB request's prompt tokens, and cancellation: a
      superseding request's first token after a request is dropped mid-generation and
      mid-prompt.

Both refuse to start on weekdays 09:00-18:00 and stop their processes if that window opens
while they run (the machine schedule).
"""

import argparse
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
from assist_demo import create_accounts, demo_passwords, fresh_directory
from smoke_release import smoke_environment


def heavy_allowed(now=None):
    now = now or datetime.now()
    return now.weekday() >= 5 or not 9 <= now.hour < 18


def guard(processes):
    """Kills `processes` (a list it reads live) once the weekday window opens."""

    def watch():
        while True:
            if not heavy_allowed():
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
        self.process = subprocess.Popen(
            [
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
                "0",
                "--reasoning",
                "off",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env={"PATH": "/usr/bin:/bin", "HOME": self.dir.name},
        )
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
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
        """A streamed request; returns seconds to its first content token. With `drop_after`
        (seconds) or `drop_at_token` (tokens), the connection is closed then instead."""
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
        began = time.monotonic()
        if drop_after is not None:
            raw.settimeout(drop_after)
        seen = 0
        first = None
        buffer = b""
        try:
            while True:
                chunk = raw.recv(65536)
                if not chunk:
                    break
                buffer += chunk
                while b"\n\n" in buffer:
                    event, buffer = buffer.split(b"\n\n", 1)
                    if b'"content"' in event and b'"delta":{"content"' in event.replace(b" ", b""):
                        seen += 1
                        if first is None:
                            first = time.monotonic() - began
                        if drop_at_token is not None and seen >= drop_at_token:
                            raw.close()
                            return first
                    if b"[DONE]" in event:
                        raw.close()
                        return first
        except TimeoutError:
            pass
        raw.close()
        return first

    def stop(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
        self.dir.cleanup()


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
        run.stream_first_token(filler(0), args.max_tokens, drop_at_token=20)
        mid_generation = run.stream_first_token(filler(1), 8)
        # Mid-prompt: a fresh 1 KiB part (uncached) dropped after 3 s of prompt evaluation.
        run.stream_first_token(filler(2, 1024).replace(" ", "x"), args.max_tokens, drop_after=3)
        mid_prompt = run.stream_first_token(filler(1), 8)
        out["superseding_first_token_s"] = {
            "after_mid_generation_cancel": round(mid_generation or -1, 2),
            "after_mid_prompt_cancel": round(mid_prompt or -1, 2),
        }
    finally:
        run.stop()
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
    args = parser.parse_args(argv)
    if not heavy_allowed():
        print(json.dumps({"refused": "weekday 09:00-18:00"}))
        return 2
    result = calibrate(args) if args.command == "calibrate" else server(args)
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
