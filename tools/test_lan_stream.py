"""Helper checks of the LAN/proxy stream client against a local fake server."""

import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import lan_stream
from lan_stream import Peer, Stream, names, session_cookie


class Fake(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_GET(self):
        try:
            self.stream()
        except (BrokenPipeError, ConnectionResetError):
            pass  # the client left, as the silent-stream test does

    def stream(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        def chunk(text):
            data = text.encode()
            self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()

        chunk('retry: 3000\nevent: status\ndata: {"code":"live"}\n\n')
        chunk(": hello\n\n")
        time.sleep(0.4)
        # One event split over two writes, the second after a pause.
        chunk("event: snapshot\ndata: ")
        time.sleep(0.2)
        chunk('{"players":{"rows":[{"name":"B"},{"name":"A"}],"next":null},')
        chunk('"npcs":{"rows":[],"next":null}}\n\n')
        self.wfile.write(b"0\r\n\r\n")


class StreamTests(unittest.TestCase):
    def setUp(self):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Fake)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.peer = Peer("127.0.0.1", self.server.server_port, "x:1", "http://x:1")

    def test_events_arrive_incrementally_and_the_body_ends(self):
        stream = Stream(self.peer, None, "/")
        self.addCleanup(stream.close)
        started = time.monotonic()
        first = stream.next(2)
        self.assertEqual((first["event"], first["retry"]), ("status", "3000"))
        self.assertLess(time.monotonic() - started, 1.0, "the first event waited for the body")
        self.assertEqual(stream.next(2), {"comment": "hello"})
        snapshot = stream.until(lan_stream.is_snapshot)[-1]
        self.assertEqual(names(snapshot), ["A", "B"])
        self.assertTrue(stream.ended(2))

    def test_a_silent_stream_times_out_instead_of_hanging(self):
        stream = Stream(self.peer, None, "/")
        self.addCleanup(stream.close)
        stream.next(2)
        stream.next(2)
        with self.assertRaises(TimeoutError):
            stream.next(0.05)

    def test_ended_gives_up_at_the_first_silence_and_hold_refuses_an_ended_body(self):
        stream = Stream(self.peer, None, "/")
        self.addCleanup(stream.close)
        stream.next(2)
        stream.next(2)
        self.assertFalse(stream.ended(0.05))  # silent for now; the stream is unusable after
        other = Stream(self.peer, None, "/")
        self.addCleanup(other.close)
        with self.assertRaises(AssertionError):
            other.hold(5)  # the fake body ends within a second

    def test_spoofed_forwarding_is_sent_on_every_request(self):
        headers = self.peer.headers("mimik_session=x")
        self.assertEqual(headers["X-Forwarded-Proto"], "https")
        self.assertEqual(headers["Host"], "x:1")
        self.assertEqual(headers["Cookie"], "mimik_session=x")
        self.assertNotIn("X-Forwarded-Proto", Peer("a", 1, "h", "o", spoof=False).headers())


def reply(*cookies):
    return SimpleNamespace(cookies=list(cookies))


GOOD = "mimik_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800"


class CookieTests(unittest.TestCase):
    def test_secure_must_match_the_transport(self):
        self.assertEqual(session_cookie([reply(GOOD)], tls=False), "mimik_session=abc")
        self.assertEqual(session_cookie([reply(GOOD + "; Secure")], tls=True), "mimik_session=abc")
        with self.assertRaises(AssertionError):
            session_cookie([reply(GOOD + "; Secure")], tls=False)
        with self.assertRaises(AssertionError):
            session_cookie([reply(GOOD)], tls=True)

    def test_lax_or_script_readable_cookies_are_refused(self):
        for bad in (GOOD.replace("Strict", "Lax"), GOOD.replace("HttpOnly; ", "")):
            with self.assertRaises(AssertionError):
                session_cookie([reply(bad)], tls=False)


if __name__ == "__main__":
    unittest.main()
