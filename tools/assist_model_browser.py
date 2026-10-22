"""Chromium check of the interpretation assistant (step 2c-1, prototype tooling).

Runs a development build (the `dev-endpoint` feature, never a release) against a fake
llama-server on loopback that answers like llama.cpp's OpenAI route (SSE) and `/tokenize`,
with synthetic data only. At 1280 x 720 and 390 x 844: a saved scribble shows "Thinking…",
then the option with the current value, the expression and the effect; its explanation is
shown as text; Use writes the value with a confirmed note and Undo restores it; an
ambiguous reply shows readings without Use; a newer save supersedes a job still polling;
no CSP violation, no console error, no horizontal overflow.

Run inside the browser container (containers/browser.Dockerfile):
  python3 tools/assist_model_browser.py <dev-endpoint mimik> --fixture <source_fixture out>
"""

import argparse
import json
import secrets
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


def text_of(content):
    if isinstance(content, str):
        return content
    return "".join(part.get("text", "") for part in content)


class FakeModel(BaseHTTPRequestHandler):
    token = ""
    requests = []

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
        envelope = json.loads(user[len("<data>\n") : -len("\n</data>")])
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
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        }
        self.wfile.write(f"data: {json.dumps(stop_chunk)}\n\ndata: [DONE]\n\n".encode())


def until(page, expression, what):
    for _ in range(400):
        if page.evaluate(expression):
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

    # A saved scribble: Thinking…, then the option with the current value, the expression,
    # the effect and the explanation as text.
    hp.fill("took 15 damage first")
    hp.press("Enter")
    until(page, "!!document.querySelector('#assist-0')", "the assistant box")
    until(page, "!!document.querySelector('#assist-0 .assist-use')", "the option")
    box = page.locator("#assist-0")
    shown = box.inner_text()
    value = int(start_value)
    require(f"Now {value}" in shown and f"{value} − 15" in shown, f"The option differs: {shown}")
    require(f"= {value - 15}" in shown, f"The option's value differs: {shown}")
    require(MARKUP in shown, "The explanation is not shown as text")
    require(box.locator("img").count() == 0, "Model text became markup")
    require(box.locator("bdi.assist-explanation").count() == 1, "The explanation is not in a bdi")
    require(box.get_attribute("role") == "status", "The assistant box is not a status region")
    no_overflow(page, "assistant option")

    # Use: the value and its Undo, then Undo restores it.
    box.locator(".assist-use").click()
    until(page, f"document.querySelector('#v0').value === '{value - 15}'", "the applied value")
    until(page, "!!document.querySelector('#undo-0')", "the Undo of the applied value")
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


def main(argv=None):
    from playwright.sync_api import sync_playwright

    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path, help="A development (dev-endpoint) mimik build")
    parser.add_argument("--fixture", type=Path, required=True, help="source_fixture output")
    args = parser.parse_args(argv)
    binary = args.binary.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    FakeModel.token = secrets.token_urlsafe(32)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeModel)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory(prefix="mimik-assistant-browser-") as temp:
        directory = fresh_directory(Path(temp) / "data")
        token_file = Path(temp) / "model.token"
        write_private(token_file, FakeModel.token.encode())
        passwords = demo_passwords()
        create_accounts(binary, directory, passwords)
        prepare_unknown_registry_source(binary, directory, fixture)
        version = publish_assist_definition(binary, directory, fixture)
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
                        scenario(context.new_page(), f"http://127.0.0.1:3000{sheet}")
                        context.close()
                finally:
                    browser.close()
            log_text = (directory / "server.log").read_text()
            for phrase in ("took 15 damage", "ambiguous 4", "slow 15", FakeModel.token):
                require(phrase not in log_text, "Player text or the token reached the server log")
        finally:
            stop(process, log)
            server.shutdown()
    print("Assistant Chromium scenario passed at 1280 and 390 (development build, fake model).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
