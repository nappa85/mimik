"""Live pinned-model download check (local prototype; real Hub, no Switch).

The models directory must already exist, be empty and have mode 0700. Downloads remain there; the
temporary accounts, database and private server log are removed on exit. --timeout
is a total polling budget in seconds (default three hours). No third-party packages.
"""

import argparse
import hashlib
import json
import math
import re
import stat
import sys
import tempfile
import time
import urllib.request
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from assist_demo import OWNER, create_accounts, demo_passwords, fresh_directory
from smoke_release import FormFields, auth_cookie, auth_fields, auth_request, require, start, stop

ROOT = Path(__file__).resolve().parents[1]
PANEL = "/admin/assistant"
MIB = 1 << 20
HASH_CHUNK = 8 * MIB


def redact_urls(text):
    """Drop URL userinfo, queries and fragments, including in diagnostic prose."""

    def clean(match):
        url = match.group().split("?", 1)[0].split("#", 1)[0]
        return re.sub(r"(https?://)[^/\s]*@", r"\1", url, flags=re.IGNORECASE)

    return re.sub(r"https?://[^\s<>\"']+", clean, str(text), flags=re.IGNORECASE)


def say(text):
    print(redact_urls(text), flush=True)


@dataclass(frozen=True)
class Pin:
    hf: str
    local: str
    sha256: str
    size: int

    @property
    def filename(self):
        return self.local.removeprefix("local:")

    @property
    def repo_revision(self):
        location, revision = self.hf.removeprefix("hf:").rsplit("@", 1)
        return "/".join(location.split("/")[:2]), revision

    @property
    def api_url(self):
        repo, revision = self.repo_revision
        return f"https://huggingface.co/api/models/{repo}/revision/{revision}?blobs=true"


def parse_pin(source, chosen):
    """Parse the literal Rust constants, failing closed if their structure changes."""

    def constant(name, numeric=False):
        kind = "u64" if numeric else r"&str"
        value = r"([0-9][0-9_]*)" if numeric else r'("[^"\n]*")'
        matches = re.findall(rf"\bconst\s+{name}\s*:\s*{kind}\s*=\s*{value}\s*;", source)
        require(len(matches) == 1, f"Missing or ambiguous Rust constant {name}")
        return int(matches[0].replace("_", "")) if numeric else json.loads(matches[0])

    prefix, model = {"granite": ("GRANITE", "DEFAULT"), "qwen": ("QWEN", "GPU_DEFAULT")}[chosen]
    pin = Pin(
        constant(prefix + "_HF"),
        constant(model + "_MODEL"),
        constant(model + "_MODEL_SHA256"),
        constant(prefix + "_SIZE", True),
    )
    require(
        bool(re.fullmatch(r"hf:[A-Za-z0-9._/-]+\.gguf@[0-9a-f]{40}", pin.hf)),
        "Invalid pinned Hub spec",
    )
    require(
        bool(re.fullmatch(r"local:[A-Za-z0-9_-][A-Za-z0-9._-]*\.gguf", pin.local)),
        "Invalid pinned local name",
    )
    require(
        bool(re.fullmatch(r"[0-9a-f]{64}", pin.sha256)) and pin.size > 0,
        "Invalid pinned hash or size",
    )
    return pin


class Panel(FormFields):
    """Reuse form parsing, adding visible review rows and the Model trial text."""

    def __init__(self, html):
        super().__init__()
        self.rows = {}
        self.cells = None
        self.cell = None
        self.heading = None
        self.in_trial = False
        self.trial = []
        self.feed(html)
        self.close()

    def handle_starttag(self, tag, attrs):
        super().handle_starttag(tag, attrs)
        if tag == "tr":
            self.cells = []
        if tag in ("th", "td"):
            self.cell = []
        if tag == "h3":
            self.heading = []

    def handle_data(self, data):
        super().handle_data(data)
        if self.cell is not None:
            self.cell.append(data)
        if self.heading is not None:
            self.heading.append(data)
        if self.in_trial and not self.active_form:
            self.trial.append(data)

    def handle_endtag(self, tag):
        if tag in ("td", "th") and self.cell is not None:
            if self.cells is not None:
                self.cells.append("".join(self.cell).strip())
            self.cell = None
        if tag == "tr" and self.cells is not None:
            if len(self.cells) == 2:
                self.rows[self.cells[0]] = self.cells[1]
            self.cells = None
        if tag == "h3" and self.heading is not None:
            title = "".join(self.heading)
            if title.startswith("Trial "):
                self.in_trial = True
                self.trial = [title]
            self.heading = None
        if self.in_trial and tag in ("p", "tr"):
            self.trial.append("\n")
        if tag == "section":
            self.in_trial = False
        super().handle_endtag(tag)

    def form(self, action, identical_ok=False, **match):
        forms = [
            fields
            for route, fields in self.form_list
            if route == action and all(fields.get(k) == str(v) for k, v in match.items())
        ]
        if identical_ok:
            # The main Try form and a file row's Try both name the active model when it is the
            # downloaded default: identical forms are then one action.
            forms = [dict(f) for f in {tuple(sorted(f.items())) for f in forms}]
        require(len(forms) == 1, "Expected exactly one matching panel form")
        require(len(forms[0].get("csrf", "")) == 64, "Panel CSRF missing")
        return dict(forms[0])


def streaming_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(HASH_CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # The app refuses API redirects too. Never send a redirected request.
        raise AssertionError("Hub metadata redirect refused")


def write_fixture(pin, chosen, folder, opener=None):
    opener = opener or urllib.request.build_opener(NoRedirect())
    request = urllib.request.Request(pin.api_url, headers={"Accept-Encoding": "identity"})
    with opener.open(request, timeout=20) as response:
        require(response.status == 200, "Hub metadata HTTP status differs")
        require(
            response.headers.get("Content-Encoding", "identity") == "identity",
            "Hub metadata content encoding differs",
        )
        body = response.read(MIB + 1)
    require(len(body) <= MIB, "Hub metadata exceeds 1 MiB")
    value = json.loads(body)
    require(isinstance(value, dict), "Hub metadata must be an object")
    require(value.get("sha") == pin.repo_revision[1], "Hub metadata revision differs")
    require(
        value.get("private") is False and value.get("gated") is False,
        "Hub metadata is not public and ungated",
    )
    require(chosen in ("granite", "qwen"), "Unknown fixture name")
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"hf-model-info-{chosen}.json"
    # Only the JSON body: no request/response headers, auth or cookie jar.
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return path


def should_kill(download, threshold, killed):
    return (
        threshold is not None
        and not killed
        and download["phase"] == "running"
        and threshold <= download["received"] < download["total"]
    )


def resume_floor(partial_size):
    # task.rs: floor to a chunk boundary, then roll back one whole 1 MiB chunk.
    return max(0, partial_size // MIB * MIB - MIB)


def check_resume_sample(received, partial_size):
    floor = resume_floor(partial_size)
    require(floor > 0, "Partial too small to prove a nonzero resume")
    require(received >= floor, "Resume restarted below the retained partial offset")


def trial_allowed(chosen, now=None):
    now = now or datetime.now().astimezone()
    return chosen == "granite" or now.weekday() >= 5 or not 9 <= now.hour < 18


def threads_allowed(threads, now=None):
    return threads > 0 and (threads <= 2 or trial_allowed("qwen", now))


def check_device(device):
    require(
        device == "cpu" or trial_allowed("qwen"), "The GPU is forbidden during weekday 09:00–18:00"
    )


def check_threads(threads):
    require(threads > 0, "Assistant threads must be positive")
    require(threads_allowed(threads), "More than 2 threads forbidden during weekday 09:00–18:00")


class Live:
    def __init__(self, args, pin, directory, password):
        self.args, self.pin, self.directory, self.password = args, pin, directory, password
        self.process = self.log = None
        self.cookie = None
        self.deadline = time.monotonic() + args.timeout

    def remaining(self):
        require(time.monotonic() < self.deadline, "Live check timed out")

    def launch(self):
        self.remaining()
        check_threads(self.args.threads)
        check_device(self.args.device)
        self.process, self.log = start(
            self.args.binary,
            self.directory,
            auth=True,
            extra=(
                "--assistant",
                "--models-dir",
                str(self.args.models),
                *(("--assistant-runtime", str(self.args.runtime)) if self.args.runtime else ()),
                "--assistant-device",
                self.args.device,
                "--assistant-threads",
                str(self.args.threads),
            ),
        )
        response = auth_request("/auth/login")
        cookie = auth_cookie(response)
        fields = auth_fields(response, "/auth/login")
        # Deliberately no auth_login's throttle retry: every failed request is fatal.
        response = auth_request(
            "/auth/login",
            cookie=cookie,
            form=dict(fields, login=OWNER, password=self.password.decode()),
        )
        require(
            response[0] == 303 and response[1].get("location") == "/",
            "Owner login failed (diagnostics redacted)",
        )
        self.cookie = auth_cookie(response)

    def close(self):
        if self.process is not None:
            process, log = self.process, self.log
            self.process = self.log = None
            stop(process, log)

    def get(self, path):
        self.remaining()
        require(self.process.poll() is None, "Mimik exited during the check")
        response = auth_request(path, cookie=self.cookie)
        require(response[0] == 200, "Panel HTTP request failed")
        return response[2]

    def panel(self):
        return Panel(self.get(PANEL))

    def state(self):
        state = json.loads(self.get(PANEL + "/status.json"))
        require(
            state.get("available") is True and "error" not in state,
            "Downloads unavailable or records unreadable",
        )
        return state

    def post(self, panel, action, notice, identical_ok=False, **match):
        self.remaining()
        form = panel.form(action, identical_ok=identical_ok, **match)
        response = auth_request(action, cookie=self.cookie, form=form)
        require(
            response[0] == 303 and response[1].get("location") == PANEL + "?notice=" + notice,
            "Panel action refused (diagnostics redacted)",
        )

    def review(self):
        self.post(self.panel(), PANEL + "/model/review", "reviewing", model=self.pin.hf)
        while True:
            state = self.state()["review"]
            require(state in ("running", "ready"), "Hub review failed or unexpectedly idle")
            if state == "ready":
                break
            time.sleep(0.5)
        panel = self.panel()
        repo, revision = self.pin.repo_revision
        require(
            panel.rows.get("Repository") == repo and panel.rows.get("Revision") == revision,
            "Review repository or revision differs",
        )
        require(
            panel.rows.get("SHA-256") == self.pin.sha256
            and panel.rows.get("Size", "").startswith(f"{self.pin.size} bytes ("),
            "Review hash or size differs from Rust pin",
        )
        for label in ("Licence", "Licence file", "Size", "Repository", "Revision"):
            if label in panel.rows:
                say(f"{label}: {panel.rows[label]}")
        self.post(panel, PANEL + "/download/confirm", "download-started")

    def crash_resume(self, row):
        self.process.kill()  # SIGKILL on the supported Linux live host.
        self.process.wait(timeout=10)
        self.log.close()
        self.process = self.log = None
        partial = self.args.models / ".partial" / self.pin.sha256
        size = partial.stat().st_size
        require(
            resume_floor(size) > 0 and size < self.pin.size,
            "Kill did not leave a provably resumable partial",
        )
        say(f"SIGKILL left {size} bytes; expected retained offset {resume_floor(size)}")
        self.launch()
        panel = self.panel()
        # Presence of this exact row's Resume form is the panel's resumable state.
        panel.form(PANEL + "/download/resume", id=row["id"])
        require(partial.stat().st_size == size, "Restart changed the waiting partial")
        self.post(panel, PANEL + "/download/resume", "download-resuming", id=row["id"])
        return size

    def transfer(self):
        killed = False
        baseline = None
        retained_offset_seen = False
        resumed_progress = False
        last_print = -math.inf
        while True:
            state = self.state()
            rows = [r for r in state["rows"] if r["spec"] == self.pin.hf]
            require(len(rows) == 1, "Confirmed download row missing or ambiguous")
            row, download = rows[0], state["download"]
            require(
                row["file"] == self.pin.filename and row["size"] == self.pin.size,
                "Download row differs from pin",
            )
            require(
                download["phase"] not in ("failed", "stalled", "cancelled"),
                "Download failed, stalled or cancelled; no automatic retry",
            )
            if row["state"] == "complete":
                require(
                    self.args.kill_at_bytes is None or killed,
                    "Transfer finished before requested SIGKILL",
                )
                require(
                    baseline is None or resumed_progress,
                    "Transfer finished without observable resumed progress",
                )
                return
            require(download["id"] == row["id"], "Unexpected active download")
            if should_kill(download, self.args.kill_at_bytes, killed):
                baseline = self.crash_resume(row)
                killed = True
                continue
            if baseline is not None and download["phase"] == "running":
                # JSON briefly starts at 0 before opening the partial. Probe the disk
                # too: it must never shrink below the documented retained offset.
                partial = self.args.models / ".partial" / self.pin.sha256
                try:
                    disk_size = partial.stat().st_size
                except FileNotFoundError:
                    disk_size = None  # Publication can race this read.
                if disk_size is not None:
                    check_resume_sample(disk_size, baseline)
                    retained_offset_seen |= disk_size == resume_floor(baseline)
                if download["received"]:
                    check_resume_sample(download["received"], baseline)
                    retained_offset_seen |= download["received"] == resume_floor(baseline)
                    if download["received"] > baseline:
                        require(
                            retained_offset_seen,
                            "Resume offset was not observed; cannot prove a nonzero resume",
                        )
                        if not resumed_progress:
                            say(
                                f"Resume observed: {download['received']} bytes "
                                f"beyond partial {baseline}"
                            )
                        resumed_progress = True
            if time.monotonic() - last_print >= 10:
                say(
                    f"Download: {download['phase']}, "
                    f"{download['received']}/{download['total']} bytes"
                )
                last_print = time.monotonic()
            # Observe the offset while the resumed request is connecting, before
            # incoming bytes could make a fresh transfer look like a resume.
            interval = 0.01 if baseline is not None and not retained_offset_seen else 0.1
            time.sleep(interval if self.args.kill_at_bytes is not None else 1)

    def trial(self):
        check_threads(self.args.threads)
        require(trial_allowed(self.args.pinned), "Qwen trial forbidden during weekday 09:00–18:00")
        self.post(
            self.panel(),
            PANEL + "/model/try",
            "trying-model",
            identical_ok=True,
            model=self.pin.local,
        )
        while True:
            check_threads(self.args.threads)
            require(trial_allowed(self.args.pinned), "Trial reached the weekday heavy-work window")
            panel = self.panel()
            require(panel.trial, "Trial missing from panel")
            title = panel.trial[0]
            require(f" of {self.pin.local}: " in title, "Trial model differs")
            verdict = title.rsplit(": ", 1)[-1]
            if verdict != "running":
                say("Trial: " + " ".join(panel.trial))
                require(verdict == "passed", "Trial did not pass")
                require(any("Priming " in text for text in panel.trial), "Trial timings missing")
                return
            time.sleep(1)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--pinned", choices=("granite", "qwen"), required=True)
    parser.add_argument("--kill-at-bytes", type=int)
    parser.add_argument(
        "--runtime",
        type=Path,
        help="Pinned llama.cpp runtime dir (default: llama/ beside the binary)",
    )
    parser.add_argument("--threads", type=int, default=2, help="Assistant CPU threads (default: 2)")
    parser.add_argument(
        "--device", default="cpu", help="--assistant-device for Mimik (default: cpu)"
    )
    parser.add_argument("--try", dest="try_model", action="store_true")
    parser.add_argument(
        "--timeout", type=float, default=3 * 60 * 60, help="Total poll budget, seconds"
    )
    args = parser.parse_args(argv)
    try:
        require(
            math.isfinite(args.timeout) and args.timeout > 0, "Timeout must be positive and finite"
        )
        check_threads(args.threads)
        check_device(args.device)
        args.binary = args.binary.resolve(strict=True)
        args.models = args.models.resolve(strict=True)
        require(
            args.models.is_dir() and not any(args.models.iterdir()),
            "Models scratch directory must exist and be empty",
        )
        require(
            stat.S_IMODE(args.models.stat().st_mode) == 0o700,
            "Models scratch directory must have mode 0700",
        )
        pin = parse_pin((ROOT / "src/assistant/model.rs").read_text(), args.pinned)
        if args.kill_at_bytes is not None:
            require(
                2 * MIB <= args.kill_at_bytes < pin.size - 2 * MIB,
                "Kill threshold must be at least 2 MiB and below pinned size minus 2 MiB",
            )
        # The shared server helper binds port 3000; never talk to another instance.
        try:
            auth_request("/auth/login")
        except ConnectionRefusedError:
            pass
        else:
            raise AssertionError("Port 3000 is already in use")
        with tempfile.TemporaryDirectory(prefix="mimik-download-live-") as temp:
            directory = fresh_directory(Path(temp) / "data")
            passwords = demo_passwords()
            create_accounts(args.binary, directory, passwords)
            live = Live(args, pin, directory, passwords[OWNER])
            try:
                live.launch()
                live.review()
                live.transfer()
                final = args.models / pin.filename
                require(
                    final.is_file() and not final.is_symlink(), "Final model file missing or linked"
                )
                require(final.stat().st_size == pin.size, "Final model size differs")
                say(f"Independent SHA-256 check: {pin.filename} ({pin.size} bytes)")
                require(streaming_sha256(final) == pin.sha256, "Final model SHA-256 differs")
                fixture = write_fixture(pin, args.pinned, ROOT / "tools/fixtures")
                say(f"Hub metadata fixture: {fixture.name}")
                if args.try_model:
                    live.trial()
            finally:
                live.close()
        say(
            f"PASS: {args.pinned}; download, size, SHA-256 and Hub fixture checked"
            + ("; SIGKILL/resume checked" if args.kill_at_bytes is not None else "")
            + ("; trial passed" if args.try_model else "")
        )
        return 0
    except KeyboardInterrupt:
        say("FAIL: interrupted; Mimik stopped")
    except Exception as error:
        # Never print arbitrary exceptions: urllib and helpers may embed URLs,
        # response bodies or account material. Assertion messages here are curated.
        detail = str(error) if isinstance(error, AssertionError) else type(error).__name__
        say(f"FAIL: {detail}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
