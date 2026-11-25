"""Does the htmx SSE extension work under the sheet pages' strict policy? (DEP-33, prototype.)

A throwaway server serves one page with `SHEET_CSP` (copied from `src/campaigns/web.rs`; a
test in `test_htmx_sse_probe.py` keeps the copy equal), the bundled htmx and the SSE extension
2.2.4, and a same-origin event stream of `snapshot` events whose data is plain HTML. A control
page has no policy. Chromium reports whether the pushed markup reached the DOM, and every CSP
or Trusted Types violation and script error. Run inside the browser container:

  python3 tools/htmx_sse_probe.py --htmx assets/htmx.min.js --sse /path/to/sse.min.js

`--sse` is htmx-ext-sse 2.2.4 from https://unpkg.com/htmx-ext-sse@2.2.4/dist/sse.min.js
(SHA-256 below). Exit status 0 means the probe ran and printed its observations; it does not
assert the outcome, so the gap is recorded rather than turned into a failing gate.
"""

import argparse
import hashlib
import json
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SSE_SHA256 = "98a46496de0c3605fbffdce9167ba427bdd9553184f83f149c261891a92c0136"
SHEET_CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none'; "
    "base-uri 'none'; form-action 'self'; frame-ancestors 'none'; "
    "require-trusted-types-for 'script'; trusted-types 'none'"
)
PAGE = (
    "<!doctype html><title>probe</title><script src='/htmx.js'></script>"
    "<script src='/sse.js'></script><body hx-ext='sse'>"
    "<div id='live' sse-connect='/events' sse-swap='snapshot'>initial</div></body>"
)
CSP_PROBE = """
window.__csp = [];
document.addEventListener('securitypolicyviolation', (event) => {
  window.__csp.push(`${event.violatedDirective} ${event.blockedURI}`);
});
"""


def rust_policy(source):
    """The policy string of `SHEET_CSP` in `src/campaigns/web.rs`."""
    match = re.search(r'SHEET_CSP: &str = "((?:[^"\\]|\\.)*)"', source, re.S)
    return re.sub(r"\\\n\s*", "", match[1])


def serve(htmx, sse):
    files = {"/htmx.js": htmx, "/sse.js": sse}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            path = self.path.split("?")[0]
            self.send_response(200)
            if path in files:
                self.send_header("Content-Type", "text/javascript")
                body = files[path]
            elif path == "/events":
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(b"event: snapshot\ndata: <p id='pushed'>pushed</p>\n\n")
                self.wfile.flush()
                self.server.stop.wait(5)
                return
            else:
                self.send_header("Content-Type", "text/html")
                if path == "/strict":
                    self.send_header("Content-Security-Policy", SHEET_CSP)
                body = PAGE.encode()
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.stop = threading.Event()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def observe(browser, url):
    context = browser.new_context()
    page = context.new_page()
    page.add_init_script(CSP_PROBE)
    consoles, errors = [], []
    page.on("console", lambda m: consoles.append(m.text) if m.type == "error" else None)
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(url)
    page.wait_for_timeout(2000)
    result = {
        "url": url.rsplit("/", 1)[1],
        "pushed_markup_in_dom": page.locator("#pushed").count() == 1,
        "live_text": page.locator("#live").inner_text(),
        "csp_violations": page.evaluate("window.__csp"),
        "console_errors": consoles,
        "page_errors": errors,
    }
    context.close()
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--htmx", required=True)
    parser.add_argument("--sse", required=True)
    args = parser.parse_args()
    sse = Path(args.sse).read_bytes()
    assert hashlib.sha256(sse).hexdigest() == SSE_SHA256, "unexpected SSE extension build"
    server = serve(Path(args.htmx).read_bytes(), sse)
    base = f"http://127.0.0.1:{server.server_address[1]}"
    from playwright.sync_api import sync_playwright

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        print(
            json.dumps(
                [observe(browser, f"{base}/{name}") for name in ("open", "strict")], indent=1
            )
        )
        browser.close()
    server.stop.set()


if __name__ == "__main__":
    main()
