"""Chromium check of the interpretation assistant (step 2c-1, prototype tooling).

Runs a development build (the `dev-endpoint` feature, never a release) against a fake
llama-server on loopback that answers like llama.cpp's OpenAI route (SSE) and `/tokenize`,
with synthetic data only. At 1280 x 720 and 390 x 844: a saved scribble shows "Thinking…",
then the option with the current value, the expression and the effect; its explanation is
shown as text; Use writes the value with a confirmed note and Undo restores it; an
ambiguous reply shows readings without Use; a newer save supersedes a job still polling;
no CSP violation, no console error, no horizontal overflow. A second run with the stored
reminder setting on (step 3c-1) saves a decrease: the reminder shows its hint as text with Use
and Dismiss, Use writes a field the asking field is not among (which is not redrawn), Dismiss
and a newer save hide it.

Run inside the browser container (containers/browser.Dockerfile):
  python3 tools/assist_model_browser.py <dev-endpoint mimik> --fixture <source_fixture out>
"""

import argparse
import http.client
import json
import secrets
import sqlite3
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from assist_demo import (
    PLAYER,
    create_accounts,
    create_sheet,
    demo_passwords,
    fresh_directory,
    write_private,
)
from browser_smoke import CSP_PROBE, no_overflow
from smoke_release import (
    auth_cookie,
    auth_fields,
    auth_login,
    auth_request,
    prepare_unknown_registry_source,
    publish_assist_definition,
    require,
    start,
    stop,
)

MARKUP = '<img src="x"> & </bdi>'


def reply_for(note):
    """The fake model's reply to a phrase (synthetic phrases only)."""
    if note.startswith("ambiguous"):
        return {"kind": "ambiguous", "readings": ["lost 4 again?", "<b>gained</b> 4 again?"]}
    marker = "second" if "second" in note else "first"
    return {
        "kind": "options",
        "options": [
            {
                "expr": {"op": "sub", "l": {"ref": "t"}, "r": {"lit": 15}},
                "explanation": f"{marker}: took 15 {MARKUP}",
            }
        ],
    }


def reminder_reply(envelope):
    """The fake model's reply to a reminder (synthetic data only): lower Max HP by the
    decrease's difference, a literal the decrease's own numbers give."""
    decrease = envelope["decrease"]
    lost = decrease["before"] - decrease["after"]
    other = next(
        (
            field
            for field in envelope["fields"]
            if field["type"] == "integer" and field["id"] not in {"max_hp", decrease["id"]}
        ),
        None,
    )
    if other is None:
        # The reminder prime before Ready (3c-1c) sends a decrease without fields.
        return {"relevant": False, "row": None, "hint": "-", "option": None}
    return {
        "relevant": True,
        "row": None,
        "hint": f"Rest to recover {MARKUP}",
        "option": {
            "changes": [
                {
                    "ref": "max_hp",
                    "expr": {"op": "sub", "l": {"ref": "max_hp"}, "r": {"lit": lost}},
                },
                {
                    "ref": other["id"],
                    "expr": {"op": "add", "l": {"ref": other["id"]}, "r": {"lit": lost}},
                },
            ],
            "explanation": f"matching the {lost} lost {MARKUP}",
        },
    }


def stream_usage(system):
    """The usage of a streamed reply: the system message counts as cached (the fake's
    `/tokenize` counts a token per 3 characters plus one, so the figure covers it)."""
    cached = len(system) // 3 + 1
    return {
        "prompt_tokens": cached + 10,
        "completion_tokens": 5,
        "total_tokens": cached + 15,
        "prompt_tokens_details": {"cached_tokens": cached},
    }


def text_of(content):
    if isinstance(content, str):
        return content
    return "".join(part.get("text", "") for part in content)


class FakeModel(BaseHTTPRequestHandler):
    token = ""
    requests = []
    reminders = 0
    primed = None

    def log_message(self, *args):
        pass

    def authorized(self):
        return self.headers.get("Authorization") == f"Bearer {self.token}"

    def body(self):
        return json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))

    def send_json(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):  # noqa: N802 (http.server API)
        self.send_json(200 if self.path == "/health" else 404, {"status": "ok"})

    def handle_one_request(self):
        # A superseded job's request is dropped mid-stream by design.
        try:
            super().handle_one_request()
        except (BrokenPipeError, ConnectionResetError):
            self.close_connection = True

    def do_POST(self):  # noqa: N802 (http.server API)
        if not self.authorized():
            self.send_json(401, {"error": "token"})
            return
        body = self.body()
        if self.path == "/tokenize":
            self.send_json(200, {"tokens": [1] * (len(body.get("content", "")) // 3 + 1)})
            return
        if self.path != "/v1/chat/completions":
            self.send_json(404, {})
            return
        user = text_of(body["messages"][1]["content"])
        if body.get("stream") is not True:
            # Priming and calibration (step 2c-2a): non-streamed, with llama.cpp's timings;
            # the fixed prefix counts as cached after the first request.
            system = text_of(body["messages"][0]["content"])
            prefix = len(system) // 3
            cached = FakeModel.primed == system
            FakeModel.primed = system
            generated = body["max_tokens"] if body.get("ignore_eos") else 1
            prompt = len(user) // 3 + 20 + (0 if cached else prefix)
            self.send_json(
                200,
                {
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "x"}}],
                    "timings": {
                        "cache_n": prefix if cached else 0,
                        "prompt_n": prompt,
                        "prompt_ms": prompt * 0.2,
                        "predicted_n": generated,
                        "predicted_ms": generated * 0.5,
                    },
                },
            )
            return
        envelope = json.loads(user[len("<data>\n") : -len("\n</data>")])
        schema = (body.get("response_format") or {}).get("json_schema", {}).get("name")
        if schema == "mimik_reminder_v1":
            FakeModel.reminders += 1
            content = json.dumps(reminder_reply(envelope))
        else:
            note = envelope["note"]
            FakeModel.requests.append(note)
            if note.startswith("slow"):
                time.sleep(4)
            content = json.dumps(reply_for(note))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for offset in range(0, len(content), 16):
            chunk = {
                "object": "chat.completion.chunk",
                "choices": [
                    {
                        "index": 0,
                        "delta": {"content": content[offset : offset + 16]},
                        "finish_reason": None,
                    }
                ],
            }
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
        stop_chunk = {
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            # As llama.cpp b12110 (server-task.cpp): Mimik's cache check after a reminder reads
            # `prompt_tokens_details.cached_tokens` (3c-1c) and switches reminders off without it.
            "usage": stream_usage(text_of(body["messages"][0]["content"])),
        }
        self.wfile.write(f"data: {json.dumps(stop_chunk)}\n\ndata: [DONE]\n\n".encode())


class StallingProxy(BaseHTTPRequestHandler):
    """A same-origin front for Mimik (the page's CSP refuses any other origin): forwards
    every request to 127.0.0.1:3000 with Host and Origin rewritten, except an apply POST,
    which it answers with JSON headers and a body that never finishes."""

    stall_hits = 0
    upstream = 3000

    def log_message(self, *args):
        pass

    def forward(self, method):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else None
        if method == "POST" and "/proposals/" in self.path:
            StallingProxy.stall_hits += 1
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", "100000")
            self.end_headers()
            self.wfile.write(b'{"status": "saved", ')
            self.wfile.flush()
            time.sleep(90)
            return
        headers = {k: v for k, v in self.headers.items() if k.lower() not in ("host", "origin")}
        headers["Host"] = f"127.0.0.1:{self.upstream}"
        if self.headers.get("Origin"):
            headers["Origin"] = f"http://127.0.0.1:{self.upstream}"
        connection = http.client.HTTPConnection("127.0.0.1", self.upstream, timeout=60)
        try:
            connection.request(method, self.path, body, headers)
            response = connection.getresponse()
            data = response.read()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in ("content-length", "transfer-encoding", "connection"):
                    self.send_header(key, value)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        finally:
            connection.close()

    def do_GET(self):  # noqa: N802 (http.server API)
        self.forward("GET")

    def do_POST(self):  # noqa: N802 (http.server API)
        try:
            self.forward("POST")
        except (BrokenPipeError, ConnectionResetError):
            self.close_connection = True


def until(page, expression, what):
    for _ in range(400):
        if page.evaluate(expression):
            return
        page.wait_for_timeout(50)
    raise AssertionError(f"Timed out waiting for {what}")


def until_python(page, check, what):
    for _ in range(400):
        if check():
            return
        page.wait_for_timeout(50)
    raise AssertionError(f"Timed out waiting for {what}")


def scenario(page, sheet):
    errors = []
    page.add_init_script(CSP_PROBE)
    page.on(
        "console", lambda message: errors.append(message.text) if message.type == "error" else None
    )
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(sheet)
    until(page, "document.documentElement.dataset.sheetAssist === 'ready'", "the module")
    hp = page.locator("#v0")
    start_value = hp.input_value()
    if not start_value.isdigit():
        # A scribble left by the previous viewport is echoed (N2): enter a known value first.
        hp.fill("84")
        hp.press("Enter")
        until(page, "!document.querySelector('form[aria-busy]')", "the save")
        page.wait_for_timeout(300)
        start_value = hp.input_value()

    # A saved scribble: Thinking…, then the option with the current value, the expression,
    # the effect and the explanation as text.
    hp.fill("took 15 damage first")
    hp.press("Enter")
    until(page, "!!document.querySelector('#assist-0')", "the assistant box")
    until(page, "!!document.querySelector('#assist-0 .assist-use')", "the option")
    box = page.locator("#assist-0")
    shown = box.inner_text()
    value = int(start_value)
    require(
        f": now {value}," in shown and "HP (" in shown and f"({value}) − 15" in shown,
        f"The option differs: {shown}",
    )
    require(f"= {value - 15}" in shown, f"The option's value differs: {shown}")
    require(MARKUP in shown, "The explanation is not shown as text")
    require(box.locator("img").count() == 0, "Model text became markup")
    require(box.locator("bdi.assist-explanation").count() == 1, "The explanation is not in a bdi")
    require(box.get_attribute("role") == "status", "The assistant box is not a status region")
    no_overflow(page, "assistant option")

    # Use: held at the network while the player types, so the draft typed meanwhile must
    # survive the answer (the value is saved, the control keeps the draft); then Undo.
    held = []
    page.route("**/proposals/**", lambda route: held.append(route))
    box.locator(".assist-use").click()
    until_python(page, lambda: bool(held), "the apply request")
    hp.fill("70 typed meanwhile")
    route = held[0]
    route.fulfill(response=route.fetch())
    page.unroute("**/proposals/**")
    until(page, "!document.querySelector('form[aria-busy]')", "the apply answer")
    until(page, "!!document.querySelector('#undo-0')", "the Undo of the applied value")
    page.wait_for_timeout(300)
    require(
        hp.input_value() == "70 typed meanwhile",
        "A draft typed while Use was in flight was overwritten",
    )
    # Back to the saved value, so leaving the field for Undo saves nothing.
    hp.fill(str(value - 15))
    require(
        page.locator("#was-0").inner_text() == f"Last change: was {value}",
        "The applied value offers no Undo to the value before",
    )
    with page.expect_navigation(wait_until="domcontentloaded"):
        page.locator("#undo-0").click()
    until(page, "document.documentElement.dataset.sheetAssist === 'ready'", "the module")
    require(page.locator("#v0").input_value() == str(value), "Undo did not restore the value")

    # An ambiguous reply: readings, never a Use button.
    hp = page.locator("#v0")
    hp.fill("ambiguous 4")
    hp.press("Enter")
    until(page, "!!document.querySelector('#assist-0 .assist-readings')", "the readings")
    readings = page.locator("#assist-0")
    require(readings.locator(".assist-use").count() == 0, "An ambiguous reply offered Use")
    require("<b>gained</b> 4 again?" in readings.inner_text(), "A reading was not shown as text")
    require(readings.locator("b").count() == 0, "A reading became markup")

    # Supersession while polling: a newer save replaces the slow job's box.
    hp.fill("slow 15 first")
    hp.press("Enter")
    until(
        page,
        "(document.querySelector('#assist-0')?.textContent || '').includes('Thinking')",
        "Thinking",
    )
    hp.fill("took 15 second")
    hp.press("Enter")
    until(
        page,
        "(document.querySelector('#assist-0')?.textContent || '').includes('second:')",
        "the newer option",
    )
    page.wait_for_timeout(5000)
    require(
        "first:" not in page.locator("#assist-0").inner_text(),
        "A superseded job's option was shown",
    )
    require(
        page.evaluate("window.__csp.length") == 0,
        f"CSP violations: {page.evaluate('window.__csp')}",
    )
    require(not errors, f"Console errors: {errors}")
    no_overflow(page, "assistant after supersession")


def reminder_scenario(page, sheet):
    """Step 3c-1: a saved decrease shows a reminder below the field; its hint and explanation
    are text; Use changes Max HP only (the asking field is not redrawn); Dismiss and a newer
    save hide it."""
    errors = []
    page.add_init_script(CSP_PROBE)
    page.on(
        "console", lambda message: errors.append(message.text) if message.type == "error" else None
    )
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(sheet)
    until(page, "document.documentElement.dataset.sheetAssist === 'ready'", "the module")
    hp = page.locator("#v0")
    max_hp = page.locator("#v1")
    hp.fill("84")
    hp.press("Enter")
    until(page, "!document.querySelector('form[aria-busy]')", "the save")
    require(
        page.locator("#remind-0").count() == 0, "An increase or a first value offered a reminder"
    )

    hp.fill("70")
    hp.press("Enter")
    until(page, "!!document.querySelector('#remind-0 .assist-use')", "the reminder")
    box = page.locator("#remind-0")
    shown = box.inner_text()
    require(f"Reminder: Rest to recover {MARKUP}" in shown, f"The hint differs: {shown}")
    require("matching the 14 lost" in shown, f"The explanation differs: {shown}")
    # Every change is shown with its target: both fields the option writes.
    require(
        box.locator(".assist-change").count() == 2
        and "Max HP" in shown
        and shown.count(": now ") == 2,
        f"The reminder does not show both changes with their targets: {shown}",
    )
    require(box.locator("img").count() == 0, "Model text became markup")
    require(box.get_attribute("role") == "status", "The reminder box is not a status region")
    require(box.locator("button.remind-dismiss").count() == 1, "The reminder has no Dismiss")
    require(page.locator("#assist-0").count() == 0, "A reminder used the interpretation box")
    no_overflow(page, "reminder")

    # Use writes Max HP; HP, the asking field, is not among the changes and keeps its control.
    before = int(max_hp.input_value())
    box.locator(".assist-use").click()
    until(page, f"document.querySelector('#v1').value === '{before - 14}'", "Max HP after Use")
    until(page, "!document.querySelector('#remind-0')", "the reminder to go after Use")
    require(hp.input_value() == "70", "Use redrew the asking field")

    # Dismiss hides it in the page; nothing is stored, so a reload shows none either.
    hp.fill("60")
    hp.press("Enter")
    until(page, "!!document.querySelector('#remind-0 .remind-dismiss')", "the second reminder")
    box.locator(".remind-dismiss").click()
    require(page.locator("#remind-0").count() == 0, "Dismiss left the reminder")
    page.wait_for_timeout(500)
    require(page.locator("#remind-0").count() == 0, "A dismissed reminder came back")

    # A newer save (here an increase) hides a reminder that no longer applies.
    hp.fill("50")
    hp.press("Enter")
    until(page, "!!document.querySelector('#remind-0 .remind-dismiss')", "the third reminder")
    hp.fill("65")
    hp.press("Enter")
    until(page, "!document.querySelector('#remind-0')", "the older reminder to go")
    require(
        page.evaluate("window.__csp.length") == 0,
        f"CSP violations: {page.evaluate('window.__csp')}",
    )
    require(not errors, f"Console errors: {errors}")
    no_overflow(page, "after reminders")


def stalled_use(page, sheet, stall_url):
    """Use whose answer never comes (stall_url None: the request is held unanswered at the
    browser; otherwise the page is served through StallingProxy, whose apply answer is JSON
    headers with a body that never finishes): past the 30 s bound the module gives up, the
    newer draft queued meanwhile is saved and a page post waiting for the saves is
    released."""
    page.clock.install()
    page.goto(sheet)
    until(page, "document.documentElement.dataset.sheetAssist === 'ready'", "the module")
    hp = page.locator("#v0")
    hp.fill("took 15 damage first")
    hp.press("Enter")
    until(page, "!!document.querySelector('#assist-0 .assist-use')", "the option")
    held = []
    hits = StallingProxy.stall_hits
    if stall_url is None:
        page.route("**/proposals/**", lambda route: held.append(route))
    page.locator("#assist-0 .assist-use").click()
    if stall_url is None:
        until_python(page, lambda: bool(held), "the apply request")
    else:
        until_python(page, lambda: StallingProxy.stall_hits > hits, "the stalled apply")
    # A newer draft (queued behind Use) and a page post (waiting for every save).
    hp.fill("70")
    hp.press("Enter")
    quick = page.locator("#quick-4")
    quick.fill("1 lantern")
    page.wait_for_timeout(300)
    with page.expect_navigation(wait_until="domcontentloaded", timeout=20000):
        quick.press("Enter")
        page.wait_for_timeout(300)
        # The stall is real: Use is still in flight, and the stalled body was requested.
        require(
            page.locator("form[aria-busy]").count() >= 1, "Use was not in flight before the timeout"
        )
        if stall_url is not None:
            require(StallingProxy.stall_hits > hits, "The stalled JSON body was never requested")
        page.clock.fast_forward(31000)
    if stall_url is None:
        page.unroute("**/proposals/**")
    until(page, "document.documentElement.dataset.sheetAssist === 'ready'", "the module")
    require(
        page.locator("#v0").input_value() == "70", "The draft queued behind a stalled Use was lost"
    )
    require(
        "lantern" in page.content(), "The page post waiting behind a stalled Use was not released"
    )
    # Back to a known value for the next stage.
    hp = page.locator("#v0")
    hp.fill("84")
    hp.press("Enter")
    until(page, "!document.querySelector('form[aria-busy]')", "the save")


def run_assistant(binary, fixture, server, front, stage, reminders):
    """One server run over the fake model: the interpretation scenarios, or with `reminders` the
    stored reminder setting on and the reminder scenario."""
    from playwright.sync_api import sync_playwright

    with tempfile.TemporaryDirectory(prefix="mimik-assistant-browser-") as temp:
        directory = fresh_directory(Path(temp) / "data")
        token_file = Path(temp) / "model.token"
        write_private(token_file, FakeModel.token.encode())
        passwords = demo_passwords()
        create_accounts(binary, directory, passwords)
        prepare_unknown_registry_source(binary, directory, fixture)
        version = publish_assist_definition(binary, directory, fixture)
        if reminders:
            # The owner's stored choice (on the CPU path, reminders are off until chosen).
            with sqlite3.connect(directory / "smoke.sqlite3") as db:
                db.execute(
                    "INSERT INTO assistant_reminders(id,enabled,changed_by,changed_at)"
                    " VALUES(1,1,1,1)"
                )
        extra = (
            "--assistant",
            "--assistant-dev-endpoint",
            f"http://127.0.0.1:{server.server_address[1]}",
            "--assistant-dev-token-file",
            str(token_file),
        )
        process, log = start(binary, directory, auth=True, extra=extra)
        try:
            sheet = create_sheet(passwords[PLAYER], version)
            response = auth_request("/auth/login")
            cookie = auth_login(
                auth_cookie(response),
                auth_fields(response, "/auth/login"),
                PLAYER,
                passwords[PLAYER],
            )
            name, value = cookie.split("=", 1)
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(args=["--disable-background-networking"])
                try:
                    for viewport in [{"width": 1280, "height": 720}, {"width": 390, "height": 844}]:
                        context = browser.new_context(viewport=viewport)
                        context.add_cookies(
                            [{"name": name, "value": value, "url": "http://127.0.0.1:3000"}]
                        )
                        if reminders:
                            reminder_scenario(context.new_page(), f"http://127.0.0.1:3000{sheet}")
                            context.close()
                            continue
                        stages = (None, "front")
                        if stage == "all":
                            scenario(context.new_page(), f"http://127.0.0.1:3000{sheet}")
                        else:
                            stages = ("front",)
                        for stall in stages:
                            base = front if stall else "http://127.0.0.1:3000"
                            stalled_use(context.new_page(), f"{base}{sheet}", stall)
                        context.close()
                finally:
                    browser.close()
            log_text = (directory / "server.log").read_text()
            phrases = ("took 15 damage", "ambiguous 4", "slow 15", "Rest to recover")
            for phrase in (*phrases, FakeModel.token):
                require(phrase not in log_text, "Player text or the token reached the server log")
        finally:
            stop(process, log)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path, help="A development (dev-endpoint) mimik build")
    parser.add_argument("--fixture", type=Path, required=True, help="source_fixture output")
    parser.add_argument(
        "--stage",
        choices=("all", "stall-json"),
        default="all",
        help="stall-json runs only the stalled-JSON-body stage (red runs)",
    )
    args = parser.parse_args(argv)
    binary = args.binary.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    FakeModel.token = secrets.token_urlsafe(32)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeModel)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    proxy = ThreadingHTTPServer(("127.0.0.1", 0), StallingProxy)
    threading.Thread(target=proxy.serve_forever, daemon=True).start()
    front = f"http://127.0.0.1:{proxy.server_address[1]}"
    run_assistant(binary, fixture, server, front, args.stage, reminders=False)
    if args.stage == "all":
        run_assistant(binary, fixture, server, front, args.stage, reminders=True)
        require(FakeModel.reminders >= 3, "The fake model saw no reminder request")
    server.shutdown()
    print("Assistant Chromium scenario passed at 1280 and 390 (development build, fake model).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
