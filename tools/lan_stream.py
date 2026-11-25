"""Master event stream and probe checks over direct LAN HTTP, loopback and the HTTPS proxy.

Stdlib HTTP only, one raw connection per request, so incremental delivery is observed on the
wire: an event must arrive on an open stream after a change, not just its headers. Synthetic
accounts and data only; a verification fixture, not runtime code.
"""

import http.client
import json
import re
import socket
import ssl
import time
from http.cookies import SimpleCookie
from urllib.parse import urlencode

from smoke_release import FormFields, require

PASSWORD = "Synthetic initial password 2026!"
# Forwarding data a client (or a hostile hop) can add: the real peer, Host and the configured
# origin must win, so none of it may change an answer or a cookie's Secure attribute.
SPOOF = {
    "X-Forwarded-For": "127.0.0.1",
    "Forwarded": "for=127.0.0.1;proto=https;host=attacker.example",
    "X-Forwarded-Proto": "https",
    "X-Forwarded-Host": "attacker.example",
    "X-Real-IP": "127.0.0.1",
}
PROBE_TIMEOUT = 8.0
HOLD_SECONDS = 45


class Reply:
    def __init__(self, response, body):
        self.status = response.status
        self.headers = {name.lower(): value for name, value in response.getheaders()}
        self.cookies = response.msg.get_all("Set-Cookie") or []
        self.body = body

    @property
    def text(self):
        return self.body.decode("utf-8", "replace")


class Peer:
    """One way of reaching the server: a socket address plus the Host and Origin a browser on
    that transport would use."""

    def __init__(self, address, port, authority, origin, tls=False, spoof=True):
        self.address, self.port, self.authority = address, port, authority
        self.origin, self.tls, self.spoof = origin, tls, spoof

    def connect(self, timeout=10):
        if self.tls:
            context = ssl.create_default_context()
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE  # the fixture's self-signed certificate
            return http.client.HTTPSConnection(
                self.address, self.port, timeout=timeout, context=context
            )
        return http.client.HTTPConnection(self.address, self.port, timeout=timeout)

    def headers(self, cookie=None, extra=None):
        values = {"Host": self.authority}
        if self.spoof:
            values.update(SPOOF)
        if cookie:
            values["Cookie"] = cookie
        values.update(extra or {})
        return values

    def request(self, method, path, *, cookie=None, form=None, headers=None):
        values = self.headers(cookie, headers)
        body = None
        if form is not None:
            body = urlencode(form)
            values.update(
                {"Origin": self.origin, "Content-Type": "application/x-www-form-urlencoded"}
            )
        connection = self.connect()
        try:
            connection.request(method, path, body, values)
            response = connection.getresponse()
            return Reply(response, response.read())
        finally:
            connection.close()


def wait_for_port(address, port, seconds=20):
    deadline = time.monotonic() + seconds
    while True:
        try:
            socket.create_connection((address, port), timeout=1).close()
            return
        except OSError:
            require(time.monotonic() < deadline, f"{address}:{port} did not accept connections")
            time.sleep(0.2)


def session_cookie(replies, tls):
    """The one session cookie of a login step, its attributes checked against the transport:
    Secure exactly on the configured HTTPS origin, whatever the forwarded headers claim."""
    jar = SimpleCookie()
    for reply in replies:
        for cookie in reply.cookies:
            jar.load(cookie)
    require(set(jar) == {"mimik_session"}, "Login did not set exactly the session cookie")
    cookie = jar["mimik_session"]
    require(
        bool(cookie["httponly"])
        and cookie["samesite"] == "Strict"
        and cookie["path"] == "/"
        and bool(cookie["secure"]) is tls
        and 0 < int(cookie["max-age"] or "0") <= 28800,
        "Session cookie attributes differ from the transport",
    )
    return "mimik_session=" + cookie.value


class Session:
    """One signed-in account with its own cookie, independent of every other client."""

    def __init__(self, peer, login, password=PASSWORD):
        self.peer, self.login = peer, login
        for attempt in range(8):
            form = peer.request("GET", "/auth/login")
            if form.status == 429 and attempt < 7:
                time.sleep(6)
                continue
            require(form.status == 200, "Sign-in form unavailable")
            parser = FormFields()
            parser.feed(form.text)
            fields = dict(parser.forms["/auth/login"], login=login, password=password)
            first = session_cookie([form], peer.tls)
            done = peer.request("POST", "/auth/login", cookie=first, form=fields)
            if done.status == 429 and attempt < 7:
                time.sleep(6)  # the process-wide sign-in quota
                continue
            require(
                done.status == 303 and done.headers.get("location") == "/",
                "Stream account sign-in failed (diagnostics redacted)",
            )
            self.cookie = session_cookie([done], peer.tls)
            return
        raise AssertionError("Sign-in stayed throttled")

    def get(self, path, **kwargs):
        return self.peer.request("GET", path, cookie=self.cookie, **kwargs)

    def submit(self, path, fields, **updates):
        for attempt in range(8):
            reply = self.peer.request(
                "POST", path, cookie=self.cookie, form=dict(fields, **updates)
            )
            if reply.status == 429 and attempt < 7:
                time.sleep(6)
                continue
            require(reply.status == 303, f"POST {path} answered HTTP{reply.status}")
            return reply.headers["location"]
        raise AssertionError("Form quota stayed throttled")

    def form(self, path, action=None):
        reply = self.get(path)
        require(reply.status == 200, f"{path} answered HTTP{reply.status}")
        parser = FormFields()
        parser.feed(reply.text)
        actions = [a for a in parser.forms if a != "/auth/logout" and "/seen" not in a]
        action = action or actions[0]
        require(action in parser.forms, f"{path} has no form {action}")
        return action, dict(parser.forms[action])


class Table:
    """A campaign made over the transport: `master` created it, `player` joined and was accepted."""

    def __init__(self, master, player, version, title):
        self.master, self.player, self.version = master, player, version
        _, fields = master.form("/campaigns", "/campaigns")
        self.path = master.submit(
            "/campaigns", fields, title=title, game_id="synthetic-game", edition_id="first"
        )
        self.id = int(self.path.rsplit("/", 1)[1])
        action, fields = player.form(self.path, f"{self.path}/join")
        player.submit(action, fields)
        requests = re.findall(
            rf'action="({re.escape(self.path)}/requests/\d+)"', master.get(self.path).text
        )
        require(len(requests) == 1, "The join request is not listed once")
        action, fields = master.form(self.path, requests[0])
        master.submit(action, fields, action="accept")

    @property
    def events(self):
        return f"/campaigns/{self.id}/master/events"

    @property
    def snapshot(self):
        return f"/campaigns/{self.id}/master/snapshot"

    def create_sheet(self, name):
        link = f"{self.path}/sheets/new?version={self.version}"
        action, fields = self.player.form(link)
        return self.player.submit(action, fields, name=name)


class Stream:
    """A server-sent event stream read incrementally from an open connection."""

    def __init__(self, peer, cookie, path, headers=None):
        self.connection = peer.connect(timeout=10)
        self.connection.connect()
        self.sock = self.connection.sock  # kept: a closing response drops connection.sock
        self.connection.request(
            "GET",
            path,
            headers=peer.headers(cookie, {"Accept": "text/event-stream", **(headers or {})}),
        )
        self.response = self.connection.getresponse()
        self.status = self.response.status
        self.headers = {name.lower(): value for name, value in self.response.getheaders()}

    def next(self, timeout):
        """The next event `{"event", "data", "retry"}` or comment `{"comment"}`; None at the end
        of the body; `TimeoutError` when nothing arrives within `timeout` seconds."""
        self.sock.settimeout(timeout)
        event, data, retry = None, [], None
        while True:
            line = self.response.readline()
            if not line:
                return None
            text = line.decode("utf-8").rstrip("\r\n")
            if text == "":
                if event is not None or data:
                    return {"event": event or "message", "data": "\n".join(data), "retry": retry}
                continue
            if text.startswith(":"):
                return {"comment": text[1:].strip()}
            field, _, value = text.partition(":")
            value = value.removeprefix(" ")
            if field == "event":
                event = value
            elif field == "data":
                data.append(value)
            elif field == "retry":
                retry = value

    def until(self, wanted, seconds=PROBE_TIMEOUT):
        """Reads until `wanted(item)`; returns every item read, the match last."""
        deadline = time.monotonic() + seconds
        seen = []
        while True:
            left = deadline - time.monotonic()
            require(left > 0, "Timed out waiting for a stream event")
            try:
                item = self.next(left)
            except TimeoutError:
                raise AssertionError("Timed out waiting for a stream event") from None
            require(item is not None, "The stream ended before the expected event")
            seen.append(item)
            if wanted(item):
                return seen

    def ended(self, seconds):
        """True when the body ends within `seconds`, reading through whatever it sends; False
        as soon as a read finds nothing before the deadline."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                item = self.next(max(deadline - time.monotonic(), 0.05))
            except TimeoutError:
                return False
            except (OSError, http.client.HTTPException):
                return True  # reset by the peer
            if item is None:
                return True
        return False

    def hold(self, seconds):
        """Watches an open stream for `seconds`: it must not end; returns the `:` comments."""
        deadline = time.monotonic() + seconds
        comments = 0
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                return comments
            try:
                item = self.next(left)
            except TimeoutError:
                return comments
            require(item is not None, "The held stream ended")
            comments += "comment" in item

    def close(self):
        self.connection.close()  # an abrupt client-side drop, as a sleeping phone does


def is_snapshot(item):
    return item.get("event") == "snapshot"


def names(item):
    snapshot = json.loads(item["data"])
    return sorted(row["name"] for part in ("players", "npcs") for row in snapshot[part]["rows"])


def open_stream(peer, cookie, table):
    """Opens the stream and reads its two first events: `status live` and a full snapshot."""
    stream = Stream(peer, cookie, table.events)
    require(stream.status == 200, f"Stream handshake answered HTTP{stream.status}")
    first = stream.until(lambda item: item.get("event") == "status")[-1]
    status = json.loads(first["data"])
    require(
        status["code"] == "live" and re.fullmatch(r"[0-9a-f]{32}", status["stream"]) is not None,
        "First stream event is not status live with a token",
    )
    require(first["retry"] == "3000", "The stream lost its retry hint")
    snapshot = stream.until(is_snapshot)[-1]
    return stream, status["stream"], snapshot


def check_stream_headers(stream, direct):
    h = stream.headers
    require(
        h.get("content-type", "").startswith("text/event-stream")
        and h.get("cache-control") == "no-store"
        and "content-encoding" not in h
        and "access-control-allow-origin" not in h
        and "set-cookie" not in h,
        "Stream response headers differ",
    )
    if direct:
        # A proxy consumes this hint; only a direct peer still sees it.
        require(h.get("x-accel-buffering") == "no", "The stream lost X-Accel-Buffering: no")
    else:
        require("x-accel-buffering" not in h, "The proxy passed the internal buffering hint on")


def expect(reply, status, what):
    require(
        reply.status == status and not reply.cookies,
        f"{what}: expected HTTP{status} without a cookie, received HTTP{reply.status}",
    )


def denials(peer, table, master, player):
    """Uniform refusals on the stream and the probe. The peer sends forwarded headers claiming
    loopback, HTTPS and another host on every request: none may help."""
    for path in (table.events, table.snapshot):
        expect(peer.request("GET", path), 401, f"{path} without a session")
        expect(peer.request("GET", path, cookie="mimik_session=" + "A" * 22), 401, f"{path} forged")
        expect(
            peer.request("GET", path, cookie=master.cookie, headers={"Host": "attacker.example"}),
            400,
            f"{path} with a wrong Host",
        )
        for origin in ("http://attacker.example", "https://attacker.example"):
            expect(
                peer.request("GET", path, cookie=master.cookie, headers={"Origin": origin}),
                403,
                f"{path} with the foreign Origin {origin}",
            )
        expect(
            peer.request(
                "GET", path, cookie=master.cookie, headers={"Sec-Fetch-Site": "cross-site"}
            ),
            403,
            f"{path} cross-site",
        )
        expect(player.get(path), 403, f"{path} for a player")


def probe(peer, master, table, token=None):
    query = f"?stream={token}" if token else ""
    reply = peer.request("GET", table.snapshot + query, cookie=master.cookie)
    require(
        reply.status == 200
        and reply.headers.get("content-type") == "application/json"
        and reply.headers.get("cache-control") == "no-store",
        "Snapshot probe answer differs",
    )
    return reply


def full_snapshot_after_drop(peer, master, table, expected, what):
    stream, token, snapshot = open_stream(peer, master.cookie, table)
    require(
        names(snapshot) == sorted(expected),
        f"{what}: the reconnect did not start with the full snapshot",
    )
    return stream, token


def suite(peer, version, label, *, restart=None, idle=None, hold=None, direct=True):
    """The stream and probe checks over one transport.

    `restart()` recreates the server process (a dropped connection from the server side);
    `idle` is a peer whose proxy cuts a silent upstream after a few seconds; `direct` is
    whether the proxy hint header is visible; `hold` is a peer whose proxy read timeout (20 s) is
    above the 15 s heartbeat: a stream held on it for 45 s must survive on heartbeats alone.
    """
    master = Session(peer, "stream-master")
    player = Session(peer, "stream-player")
    require(master.cookie != player.cookie, f"{label}: two clients share one session")
    table = Table(master, player, version, f"Stream {label}")
    table.create_sheet("Stream first")
    denials(peer, table, master, player)
    stream, token, snapshot = open_stream(peer, master.cookie, table)
    try:
        check_stream_headers(stream, direct)
        require(names(snapshot) == ["Stream first"], f"{label}: first snapshot differs")
        require(
            probe(peer, master, table, token).headers.get("x-master-stream") == "live"
            and probe(peer, master, table, "no-such-stream").headers.get("x-master-stream")
            == "unknown"
            and "x-master-stream" not in probe(peer, master, table).headers,
            f"{label}: probe stream state differs",
        )
        # Incremental delivery: a different client changes the table while this stream idles.
        started = time.monotonic()
        table.create_sheet("Stream pushed")
        pushed = stream.until(is_snapshot)[-1]
        require(
            names(pushed) == ["Stream first", "Stream pushed"],
            f"{label}: the pushed snapshot differs",
        )
        require(time.monotonic() - started < PROBE_TIMEOUT, f"{label}: the push was late")
    finally:
        stream.close()
    # A dropped connection: a change happens while away, the reconnect is a full snapshot.
    table.create_sheet("Stream while away")
    everything = ["Stream first", "Stream pushed", "Stream while away"]
    again, _ = full_snapshot_after_drop(peer, master, table, everything, f"{label} drop")
    try:
        if restart is not None:
            restart()
            require(again.ended(20), f"{label}: the stream outlived a server restart")
            table.create_sheet("Stream after restart")
            everything.append("Stream after restart")
            again, _ = full_snapshot_after_drop(peer, master, table, everything, f"{label} restart")
    finally:
        again.close()
    if idle is not None:
        cut, _, _ = open_stream(idle, master.cookie, table)
        try:
            require(cut.ended(12), f"{label}: the proxy's idle closure did not end the stream")
        finally:
            cut.close()
        table.create_sheet("Stream after idle")
        everything.append("Stream after idle")
        last, _ = full_snapshot_after_drop(idle, master, table, everything, f"{label} idle")
        last.close()
    if hold is not None:
        held, _, _ = open_stream(hold, master.cookie, table)
        try:
            beats = held.hold(HOLD_SECONDS)
            require(beats >= 2, f"{label}: only {beats} heartbeat(s) in {HOLD_SECONDS} s")
        finally:
            held.close()
