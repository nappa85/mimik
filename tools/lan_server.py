"""Synthetic LAN/proxy fixture around the unpacked shipping binary; not runtime code."""

import argparse
import http.client
import json
import os
import re
import signal
import sqlite3
import ssl
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from smoke_release import (
    prepare_workspace_materials,
    require,
    require_secret_free,
    smoke_accounts,
    smoke_environment,
    stop,
)


class Fixture:
    def __init__(self, binary, directory, source):
        self.binary, self.directory = binary, directory
        _, self.passwords = smoke_accounts(binary, directory)
        prepare_workspace_materials(binary, directory, source)
        self.process, self.log = None, None
        self.primary, self.external = None, None
        self.start()

    def start(self):
        args = [str(self.binary)]
        if self.primary:
            args += ["--lan-address", self.primary]
        if self.external:
            args += ["--external-url", self.external]
        self.log = open(self.directory / "server.log", "a", encoding="utf-8")
        self.process = subprocess.Popen(
            args,
            cwd=self.directory,
            env=smoke_environment(self.directory),
            stdout=self.log,
            stderr=self.log,
        )
        for _ in range(100):
            require(self.process.poll() is None, "LAN fixture exited before readiness")
            connection = http.client.HTTPConnection("127.0.0.1", 3000, timeout=1)
            try:
                connection.request("GET", "/")
                response = connection.getresponse()
                response.read()
                if response.status == 303:
                    break
            except OSError:
                time.sleep(0.1)
            finally:
                connection.close()
        else:
            raise AssertionError("LAN fixture readiness timed out")
        text = (self.directory / "server.log").read_text()
        if not self.primary:
            match = re.search(r"Mimik auth mode: http://([0-9.]+):3000", text)
            require(
                match is not None and not match[1].startswith("127."),
                "Default startup did not advertise a LAN address",
            )
            self.primary = match[1]

    def close(self):
        if self.process is not None:
            stop(self.process, self.log)
            self.process, self.log = None, None
        require_secret_free((self.directory / "server.log").read_bytes(), self.passwords)


class Control(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass  # Fixed synthetic control calls contain no diagnostic payload logging.

    def do_GET(self):
        self.send_json({"origin": f"http://{self.server.fixture.primary}:3000"})

    def send_json(self, value):
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        try:
            length = int(self.headers.get("Content-Length", "0"))
            require(0 < length <= 4096, "Invalid fixture control input")
            value = json.loads(self.rfile.read(length))
            fixture = self.server.fixture
            if self.path == "/revoke":
                require(
                    re.fullmatch(r"[A-Za-z0-9_-]{22}", value["id"]) is not None,
                    "Invalid synthetic session ID",
                )
                with sqlite3.connect(fixture.directory / "smoke.sqlite3") as db:
                    db.create_function("mimik_auth_now", 0, lambda: int(time.time()))
                    changed = db.execute(
                        "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE id=? AND state='active'",
                        (value["id"],),
                    ).rowcount
                self.send_json({"changed": changed})
            elif self.path == "/account":
                with sqlite3.connect(fixture.directory / "smoke.sqlite3") as db:
                    state = db.execute(
                        "SELECT state FROM accounts WHERE login=?", (value["login"],)
                    ).fetchone()
                self.send_json({"state": state[0] if state else None})
            elif self.path == "/proxy":
                fixture.close()
                fixture.external = "https://mimik.example:3443"
                fixture.start()
                self.send_json({"origin": fixture.external})
            elif self.path == "/restart":
                fixture.close()
                fixture.start()
                self.send_json({"origin": f"http://{fixture.primary}:3000"})
            else:
                self.send_error(404, "Unknown fixture operation")
        except Exception:
            self.send_error(500, "Synthetic control failed; diagnostics redacted")


class Proxy(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        connection = http.client.HTTPConnection("127.0.0.1", 3000, timeout=10)
        try:
            length = int(self.headers.get("Content-Length", "0"))
            require(0 <= length <= 1024 * 1024, "Proxy fixture body bound exceeded")
            body = self.rfile.read(length) if length else None
            headers = {
                name: value
                for name, value in self.headers.items()
                if name.lower() not in {"connection", "proxy-connection"}
            }
            # Deliberately wrong forwarding data must not choose the trusted scheme.
            headers["X-Forwarded-Proto"] = "http"
            connection.request(self.command, self.path, body=body, headers=headers)
            response = connection.getresponse()
            data = response.read()
            self.send_response(response.status)
            for name, value in response.getheaders():
                if name.lower() not in {"connection", "transfer-encoding", "content-length"}:
                    self.send_header(name, value)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except Exception:
            self.send_error(502, "Synthetic proxy failed; diagnostics redacted")
        finally:
            connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--source-fixture", required=True, type=Path)
    args = parser.parse_args()
    signal.alarm(900)
    with tempfile.TemporaryDirectory(prefix="mimik-lan-server-") as temp:
        directory = Path(temp)
        os.chmod(directory, 0o700)
        fixture = Fixture(args.binary.resolve(), directory, args.source_fixture.resolve())
        control = ThreadingHTTPServer(("0.0.0.0", 3001), Control)
        control.fixture = fixture
        certificate, key = directory / "fixture.crt", directory / "fixture.key"
        result = subprocess.run(
            [
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                str(key),
                "-out",
                str(certificate),
                "-days",
                "1",
                "-subj",
                "/CN=mimik.example",
                "-addext",
                "subjectAltName=DNS:mimik.example",
            ],
            capture_output=True,
            timeout=30,
            check=False,
        )
        require(result.returncode == 0, "Synthetic TLS certificate generation failed")
        os.chmod(key, 0o600)
        proxy = ThreadingHTTPServer(("0.0.0.0", 3443), Proxy)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        proxy.socket = context.wrap_socket(proxy.socket, server_side=True)
        thread = threading.Thread(target=proxy.serve_forever, daemon=True)
        thread.start()
        try:
            control.serve_forever()
        finally:
            control.server_close()
            proxy.shutdown()
            proxy.server_close()
            thread.join(timeout=5)
            require(not thread.is_alive(), "TLS fixture did not stop")
            fixture.close()


if __name__ == "__main__":
    try:
        main()
    except Exception:
        raise SystemExit("LAN server fixture failed; diagnostics redacted") from None
