"""The assistant-on release smoke: state handling, outcome checks and the sign-in client."""

import http.client
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from unittest.mock import patch

import assist_release_smoke as smoke

READY_LOG = (
    "assistant: health after 0.3 s\n"
    "assistant: primed in 6.6 s (2196 prompt tokens)\n"
    "assistant: reminders not prepared: off at the start\n"
    "assistant: ready: generation 1, level 1, landlock full; priming 2196 prefix tokens; "
    "samples [W 2151 ms, prompt 2196 tokens in 100 ms]; B 1024 bytes, N(B) 306 tokens\n"
)
PRIMING_REFUSED = "priming failed (the priming request answered HTTP 400)"


def page(state, reason=""):
    suffix = f": {reason}" if reason else ""
    return (
        '<section class="panel"><h2>State</h2>\n'
        f"<p><strong>{state}</strong>{suffix}</p>\n<p>Model server: x</p></section>"
    )


class Clock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class ReadStateTests(unittest.TestCase):
    def test_reads_state_and_unescaped_reason(self):
        self.assertEqual(smoke.read_state(page("Ready")), ("Ready", ""))
        self.assertEqual(
            smoke.read_state(page("Unavailable", "too slow &amp; late")),
            ("Unavailable", "too slow & late"),
        )

    def test_a_page_without_the_section_fails(self):
        with self.assertRaises(AssertionError):
            smoke.read_state("<h1>Interpretation assistant</h1>")


class WaitTests(unittest.TestCase):
    def wait(self, pages, timeout=10):
        clock = Clock()
        queue = iter(pages)
        return smoke.wait_for_state(
            lambda: next(queue), timeout=timeout, interval=1, clock=clock, sleep=clock.sleep
        )

    def test_returns_ready_after_starting(self):
        state = self.wait([page("Starting", "Priming")] * 3 + [page("Ready")])
        self.assertEqual(state, ("Ready", ""))

    def test_returns_a_terminal_failure_at_once(self):
        state = self.wait([page("Starting", "Loading"), page("Unavailable", "sandbox not set up")])
        self.assertEqual(state, ("Unavailable", "sandbox not set up"))

    def test_times_out_naming_the_last_state(self):
        with self.assertRaises(TimeoutError) as caught:
            self.wait([page("Starting", "Priming")] * 100, timeout=5)
        self.assertIn("Priming", str(caught.exception))


class OutcomeTests(unittest.TestCase):
    def test_ready_with_its_log_passes(self):
        smoke.check_outcome(("Ready", ""), READY_LOG)

    def test_an_unexpected_failure_is_named(self):
        with self.assertRaises(AssertionError) as caught:
            smoke.check_outcome(("Unavailable", "the model server did not start"), "")
        self.assertIn("did not start", str(caught.exception))

    def test_a_priming_failure_is_named_and_fails(self):
        with self.assertRaises(AssertionError) as caught:
            smoke.check_outcome(("Unavailable", PRIMING_REFUSED), "")
        self.assertIn("HTTP 400", str(caught.exception))

    def test_a_refusal_is_not_accepted(self):
        with self.assertRaises(AssertionError):
            smoke.check_outcome(
                ("Unavailable", "reply length too large for the context"), READY_LOG
            )

    def test_ready_without_priming_in_the_log_fails(self):
        with self.assertRaises(AssertionError):
            smoke.check_outcome(("Ready", ""), READY_LOG.replace("primed in", "skipped"))

    def test_ready_below_level_1_fails(self):
        with self.assertRaises(AssertionError):
            smoke.check_outcome(("Ready", ""), READY_LOG.replace("level 1", "level 3"))


COOKIE = "mimik_session=abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=3600"
CSRF = "c" * 64


class Handler(BaseHTTPRequestHandler):
    pages = []
    posts = []

    def log_message(self, *args):
        pass

    def reply(self, status, body="", headers=()):
        self.send_response(status)
        # Lower-case names, as the real server sends them (the shared helpers look them up so).
        for name, value in [("Cache-Control", "no-store"), *headers]:
            self.send_header(name.lower(), value)
        self.send_header("Content-Length", str(len(body.encode())))
        self.end_headers()
        self.wfile.write(body.encode())

    def do_GET(self):
        if self.path == "/auth/login":
            field = f'<input type="hidden" name="csrf" value="{CSRF}">'
            form = f'<form action="/auth/login">{field}</form>'
            self.reply(200, form, [("Set-Cookie", COOKIE)])
        elif self.path == "/admin/assistant":
            if "mimik_session=abc" not in self.headers.get("Cookie", ""):
                self.reply(403)
            else:
                self.reply(200, self.pages.pop(0))
        else:
            self.reply(404)

    def do_POST(self):
        length = int(self.headers["Content-Length"])
        body = self.rfile.read(length).decode()
        self.posts.append((self.headers.get("Origin"), self.headers.get("Host"), body))
        self.reply(303, "", [("Location", "/"), ("Set-Cookie", COOKIE)])


class SignInTests(unittest.TestCase):
    def test_signs_in_through_the_shared_helpers_and_reads_the_page(self):
        Handler.pages = [page("Starting", "Loading"), page("Ready")]
        Handler.posts = []
        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        real = http.client.HTTPConnection

        # The shared helpers address 127.0.0.1:3000; send them to the fake server.
        def connect(host, port, timeout=None):
            self.assertEqual((host, port), ("127.0.0.1", 3000))
            return real(host, server.server_port, timeout=timeout)

        try:
            with patch.object(http.client, "HTTPConnection", connect):
                cookie = smoke.sign_in("owner", b"secret password value")
                state = smoke.wait_for_state(
                    lambda: smoke.assistant_page(cookie), timeout=5, interval=0
                )
        finally:
            server.shutdown()
            server.server_close()
        self.assertEqual(state, ("Ready", ""))
        origin, host, body = Handler.posts[0]
        self.assertEqual(origin, "http://127.0.0.1:3000")
        self.assertEqual(host, "127.0.0.1:3000")
        self.assertIn(f"csrf={CSRF}", body)
        self.assertIn("login=owner", body)


class LeftoverTests(unittest.TestCase):
    def leftovers(self, processes):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for pid, argv in processes.items():
                (root / pid).mkdir()
                (root / pid / "cmdline").write_bytes(b"\0".join(argv) + b"\0")
            (root / "self").mkdir()
            real = smoke.Path
            with patch.object(smoke, "Path", lambda name: root if name == "/proc" else real(name)):
                return smoke.leftovers()

    def test_a_llama_server_and_a_stuck_runner_are_found_and_others_are_not(self):
        found = self.leftovers(
            {
                "101": [b"/x/llama/cpu/llama-server", b"--port", b"1"],
                "102": [b"/x/mimik", b"assistant-runner", b"--probe"],
                "103": [b"/bin/sleep", b"1"],
            }
        )
        self.assertEqual(sorted(found), [101, 102])

    def test_nothing_is_found_when_nothing_runs(self):
        self.assertEqual(self.leftovers({"7": [b"/bin/sleep", b"1"]}), [])


if __name__ == "__main__":
    unittest.main()
