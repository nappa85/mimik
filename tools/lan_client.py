"""Separate-client HTTP LAN and configured HTTPS proxy browser acceptance."""

import argparse
import http.client
import json
import os
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from lan_stream import Peer, suite, wait_for_port
from material_browser import run_material_browser
from smoke_release import require, wait_until

OWNER_PASSWORD = "Synthetic initial password 2026!"
STAGE = "startup"


def control(address, operation, value):
    connection = http.client.HTTPConnection(address, 3001, timeout=15)
    try:
        body = json.dumps(value)
        connection.request("POST", operation, body, {"Content-Type": "application/json"})
        response = connection.getresponse()
        require(response.status == 200, "Synthetic LAN control failed")
        return json.loads(response.read())
    finally:
        connection.close()


MASTER_PATHS = ("/campaigns/1/master/events", "/campaigns/1/master/snapshot")


def fixture_version(address):
    """The generic sheet definition's version ID, as the fixture reports it."""
    connection = http.client.HTTPConnection(address, 3001, timeout=15)
    try:
        connection.request("GET", "/")
        response = connection.getresponse()
        require(response.status == 200, "Synthetic LAN control failed")
        return json.loads(response.read())["version"]
    finally:
        connection.close()


def negative_requests(address, advertised, denial=False):
    authority = urlsplit(advertised).netloc
    cases = (
        [("GET", "/assets/app.css", authority, None, 403)]
        + [("GET", path, authority, None, 403) for path in MASTER_PATHS]
        if denial
        else [
            ("GET", "/", "attacker.example:3000", None, 400),
            ("GET", "http://attacker.example:3000/", authority, None, 400),
            ("POST", "/auth/login", authority, "http://localhost:3000", 403),
            ("POST", "/auth/login", "localhost:3000", advertised, 403),
        ]
        + [("GET", path, "attacker.example:3000", None, 400) for path in MASTER_PATHS]
        + [("GET", path, authority, "http://localhost:3000", 403) for path in MASTER_PATHS]
    )
    for method, path, host, origin, status in cases:
        connection = http.client.HTTPConnection(address, 3000, timeout=10)
        try:
            headers = {
                "Host": host,
                "X-Forwarded-For": "127.0.0.1",
                "Forwarded": "for=127.0.0.1",
                "Content-Type": "application/x-www-form-urlencoded",
            }
            if method == "GET" and path in MASTER_PATHS:
                # Refused before any session lookup, even with a well-formed cookie.
                headers["Cookie"] = "mimik_session=" + "A" * 22
            if origin:
                headers["Origin"] = origin
            connection.request(method, path, body="" if method == "POST" else None, headers=headers)
            response = connection.getresponse()
            require(
                response.status == status and response.getheader("Set-Cookie") is None,
                "LAN peer/authority/origin denial differs",
            )
            response.read()
        finally:
            connection.close()
    if not denial:
        connection = http.client.HTTPConnection(address, 3000, timeout=10)
        try:
            connection.putrequest("GET", "/", skip_host=True)
            connection.putheader("Host", authority)
            connection.putheader("Host", authority)
            connection.endheaders()
            response = connection.getresponse()
            require(
                response.status == 400 and response.getheader("Set-Cookie") is None,
                "Duplicate Host was accepted",
            )
            response.read()
        finally:
            connection.close()


class ClipboardReader(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"Local test clipboard reader"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def read_clipboard(browser, page):
    server = HTTPServer(("127.0.0.1", 0), ClipboardReader)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    # Permission is confined to a separate loopback reader, never the LAN app.
    context = browser.new_context()
    context.grant_permissions(["clipboard-read"], origin=origin)
    try:
        reader = context.new_page()
        reader.goto(origin)
        value = reader.evaluate("navigator.clipboard.readText()")
        require(
            value == page.locator("#credential-link").input_value(),
            "Copy button did not copy the actual sharing link",
        )
        return value
    finally:
        context.close()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        require(not thread.is_alive(), "Clipboard reader did not stop")


def visit(page, url):
    response = page.goto(url)
    if response.status == 429:
        require(response.headers.get("retry-after") == "6", "Form quota retry is missing")
        time.sleep(6)
        response = page.goto(url)
    require(response.status == 200, "LAN form unavailable")


def submit(page, button, path, expected=303):
    with page.expect_response(
        lambda response: response.request.method == "POST" and urlsplit(response.url).path == path
    ) as response:
        button.click()
    require(
        response.value.status == expected,
        f"LAN form returned HTTP{response.value.status}; expected HTTP{expected}",
    )
    return response.value


def login(page, origin, name, password):
    visit(page, origin + "/auth/login")
    page.locator('input[name="login"]').fill(name)
    page.locator('input[name="password"]').fill(password)
    submit(page, page.get_by_role("button", name="Sign in", exact=True), "/auth/login")
    page.wait_for_url("**/")


def login_errors(browser, address, origin, proxy=False):
    global STAGE
    STAGE = "proxy" if proxy else "LAN"
    context = browser.new_context(ignore_https_errors=proxy)
    context.set_default_timeout(15_000)
    page = context.new_page()
    try:
        visit(page, origin + "/auth/login")
        page.locator('input[name="login"]').fill("  SMOKE-OWNER  ")
        page.locator('input[name="password"]').fill("Wrong synthetic owner password")
        submit(page, page.get_by_role("button", name="Sign in", exact=True), "/auth/login", 401)
        page.get_by_role("alert").wait_for()
        require(
            page.locator('input[name="login"]').input_value() == "smoke-owner"
            and not page.locator('input[name="password"]').input_value(),
            "Login error lost canonical username or retained password",
        )
        page.locator('input[name="password"]').fill(OWNER_PASSWORD)
        submit(page, page.get_by_role("button", name="Sign in", exact=True), "/auth/login")
        page.wait_for_url("**/")
        visit(page, origin + "/auth/login")
        require(
            control(address, "/revoke", {"id": context.cookies()[0]["value"]})["changed"] == 1,
            "Login form revocation positive control failed",
        )
        page.locator('input[name="login"]').fill("smoke-owner")
        page.locator('input[name="password"]').fill(OWNER_PASSWORD)
        submit(page, page.get_by_role("button", name="Sign in", exact=True), "/auth/login", 403)
        page.get_by_role("alert").wait_for()
        require(
            page.locator("form").count() == 0, "Expired login form offered stale POST admission"
        )
        with page.expect_request(
            lambda request: request.method == "GET"
            and urlsplit(request.url).path == "/auth/login"
            and urlsplit(request.url).query == "fresh=1"
        ):
            page.get_by_role("link", name="Open a fresh sign-in form", exact=True).click()
        page.locator('input[name="csrf"]').wait_for(state="attached")
        require(
            not page.locator('input[name="password"]').input_value(),
            "Fresh login form retained password",
        )
        page.locator('input[name="login"]').fill("smoke-owner")
        page.locator('input[name="password"]').fill(OWNER_PASSWORD)
        submit(page, page.get_by_role("button", name="Sign in", exact=True), "/auth/login")
        page.wait_for_url("**/")
    finally:
        context.close()


def copy_link(browser, page, origin, insecure):
    page.locator(".invitation-qr svg").wait_for(state="visible")
    require(page.locator("#issued-token").count() == 0, "Duplicate masked token field remains")
    require(page.locator(".invitation-qr svg").count() == 1, "Sharing QR is missing")
    if insecure:
        require(
            page.evaluate("!window.isSecureContext && !navigator.clipboard"),
            "HTTP LAN copy test accidentally uses secure-context privileges",
        )
    page.get_by_role("button", name="Copy link", exact=True).click()
    wait_until(page, 'document.getElementById("copy-status").textContent === "Link copied."')
    link = read_clipboard(browser, page)
    require(
        link.startswith(origin + "/auth/")
        and re.fullmatch(r"token=[0-9a-f]{64}", urlsplit(link).fragment) is not None,
        "Sharing link does not use the canonical origin",
    )
    require("Unix" not in page.locator("main").inner_text(), "Expiry is still a raw timestamp")
    require(
        page.locator("time").get_attribute("datetime").endswith("Z"),
        "Expiry lacks RFC3339 UTC fallback",
    )
    require(
        "1970" not in page.locator("time").inner_text(), "Expiry is not the current readable date"
    )
    require(
        re.search(r"GMT\+9|JST", page.locator("time").inner_text()) is not None,
        "Expiry was not localized to the browser timezone",
    )
    return link


def passwords(page, value, confirmation=None):
    page.locator('input[name="password"]').fill(value)
    page.locator('input[name="confirmation"]').fill(confirmation or value)


def scenario(browser, address, origin, viewport, name, verification=None, proxy=False):
    global STAGE
    STAGE = name + ": owner sign-in"
    context = browser.new_context(
        viewport=viewport, locale="en-US", timezone_id="Asia/Tokyo", ignore_https_errors=proxy
    )
    context.set_default_timeout(15_000)
    owner = context.new_page()
    recipient_context = browser.new_context(viewport=viewport, ignore_https_errors=proxy)
    recipient_context.set_default_timeout(15_000)
    recipient = recipient_context.new_page()
    violations = []
    for jar in (context, recipient_context):
        jar.on(
            "request",
            lambda request: violations.append(
                bool(
                    urlsplit(request.url).fragment
                    or re.search(r"[0-9a-f]{64}", request.url)
                    or re.search(r"[0-9a-f]{64}", request.headers.get("referer", ""))
                )
            ),
        )
    try:
        login(owner, origin, "smoke-owner", OWNER_PASSWORD)
        STAGE = name + ": material review"
        run_material_browser(owner, origin)
        require(
            recipient.goto(origin + "/admin/materials").status == 200
            and urlsplit(recipient.url).path == "/auth/login",
            "Anonymous material request did not reach login",
        )
        require(
            context.cookies()[0]["secure"] is proxy,
            "Cookie Secure does not match the configured transport",
        )
        owner.goto(origin + "/admin")
        owner.locator('form[action="/admin/invite"] input[name="login"]').fill(name)
        owner.locator('form[action="/admin/invite"] input[name="display_name"]').fill(
            "Phone player"
        )
        submit(
            owner,
            owner.get_by_role("button", name="Create invitation", exact=True),
            "/admin/invite",
            200,
        )
        STAGE = name + ": copy and QR"
        link = copy_link(browser, owner, origin, not proxy)
        token = urlsplit(link).fragment.removeprefix("token=")
        if verification:
            owner.locator(".invitation-qr").screenshot(path=str(verification / f"{name}.png"))
            (verification / f"{name}.link").write_text(link)
            os.chmod(verification / f"{name}.link", 0o600)
        require(
            owner.evaluate("document.documentElement.scrollWidth <= window.innerWidth + 1"),
            "Sharing page overflows the phone viewport",
        )
        visit(recipient, link)
        STAGE = name + ": token retry"
        require(
            not urlsplit(recipient.url).fragment
            and recipient.locator('input[name="token"]').input_value() == token,
            "Link did not prefill and remove its fragment",
        )
        require(
            control(address, "/account", {"login": name})["state"] == "pending",
            "Opening a link consumed the credential",
        )
        passwords(recipient, "abcdefghijkl", "different1234")
        submit(
            recipient,
            recipient.get_by_role("button", name="Complete enroll", exact=True),
            "/auth/enroll",
            422,
        )
        require(
            recipient.locator('input[name="token"]').input_value() == token
            and not recipient.locator('input[name="password"]').input_value()
            and not recipient.locator('input[name="confirmation"]').input_value(),
            "Retry lost the token or echoed passwords",
        )
        session_id = recipient_context.cookies()[0]["value"]
        require(
            control(address, "/revoke", {"id": session_id})["changed"] == 1,
            "Form revocation positive control failed",
        )
        STAGE = name + ": fresh document recovery"
        passwords(recipient, "abcdefghijkl")
        submit(
            recipient,
            recipient.get_by_role("button", name="Complete enroll", exact=True),
            "/auth/enroll",
            403,
        )
        require(
            recipient.locator("form").count() == 0, "Invalid-CSRF recovery offered a POST admission"
        )
        with recipient.expect_request(
            lambda request: request.method == "GET"
            and urlsplit(request.url).path == "/auth/enroll"
            and urlsplit(request.url).query == "fresh=1"
        ):
            recipient.get_by_role("link", name="Open a fresh form", exact=True).click()
        wait_until(
            recipient,
            'location.hash === "" && document.querySelector(\'input[name="token"]\')?.value.length === 64',
        )
        require(
            not urlsplit(recipient.url).fragment
            and recipient.locator('input[name="token"]').input_value() == token,
            "Fresh document recovery lost the token",
        )
        require(
            not recipient.locator('input[name="password"]').input_value(),
            "Fresh form retained a password",
        )
        require(
            control(address, "/account", {"login": name})["state"] == "pending",
            "Fresh GET consumed the credential",
        )
        passwords(recipient, "abcdefghijkl")
        submit(
            recipient,
            recipient.get_by_role("button", name="Complete enroll", exact=True),
            "/auth/enroll",
        )
        recipient.wait_for_url("**/auth/login")
        login(recipient, origin, name, "abcdefghijkl")
        require(
            recipient.goto(origin + "/admin/materials").status == 403,
            "Player gained material administration",
        )
        STAGE = name + ": password reset"
        owner.goto(origin + "/admin")
        row = owner.locator("article").filter(has_text=name)
        row.get_by_role("button", name="Issue password reset", exact=True).click()
        reset = copy_link(browser, owner, origin, not proxy)
        visit(recipient, reset)
        require(
            urlsplit(recipient.url).path == "/auth/reset"
            and not urlsplit(recipient.url).fragment
            and recipient.locator('input[name="token"]').input_value()
            == urlsplit(reset).fragment.removeprefix("token="),
            "Revoked-session reset link lost prefill",
        )
        passwords(recipient, "mnopqrstuvwx")
        submit(
            recipient,
            recipient.get_by_role("button", name="Complete reset", exact=True),
            "/auth/reset",
        )
        recipient.wait_for_url("**/auth/login")
        require(
            recipient_context.cookies()[0]["secure"] is proxy,
            "Reset form cookie transport policy differs",
        )
        if proxy:
            owner.goto(origin + "/")
            owner.locator("header.site details.account summary").click()
            response = submit(
                owner, owner.get_by_role("button", name="Sign out", exact=True), "/auth/logout"
            )
            owner.wait_for_url("**/auth/login")
            deleted = [
                header["value"]
                for header in response.headers_array()
                if header["name"].lower() == "set-cookie"
            ]
            require(
                bool(deleted) and all("secure" in cookie.lower() for cookie in deleted),
                "HTTPS logout/deletion cookie lost Secure",
            )
        require(not any(violations), "Credential entered a request URL or referrer")
    finally:
        context.close()
        recipient_context.close()


def main():
    global STAGE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--origin", required=True)
    parser.add_argument("--verification", type=Path)
    parser.add_argument("--denial-only", action="store_true")
    parser.add_argument(
        "--streams-only",
        action="store_true",
        help="only the Master stream and probe checks over --origin (the loopback run)",
    )
    parser.add_argument(
        "--proxy-address",
        help="address of the HTTPS reverse proxy (ports 3443 and 3444); default --address",
    )
    args = parser.parse_args()
    negative_requests(args.address, args.origin, args.denial_only)
    if args.denial_only:
        print("Real disallowed-peer/forwarding-spoof denial passed.")
        return
    direct = Peer(args.address, 3000, urlsplit(args.origin).netloc, args.origin, tls=False)
    if args.streams_only:
        suite(direct, fixture_version(args.address), "loopback")
        print("Master stream and probe checks over " + args.origin + " passed.")
        return
    from playwright.sync_api import sync_playwright

    proxy_address = args.proxy_address or args.address
    version = fixture_version(args.address)
    STAGE = "LAN stream"
    suite(
        direct,
        version,
        "LAN",
        restart=lambda: control(args.address, "/restart", {}),
    )
    control(args.address, "/restart", {})  # an independent form quota for the scenarios below

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(
            args=["--host-resolver-rules=MAP mimik.example " + proxy_address]
        )
        try:
            login_errors(browser, args.address, args.origin)
            control(args.address, "/restart", {})
            for name, viewport in [
                ("desktop", {"width": 1280, "height": 720}),
                ("mobile", {"width": 390, "height": 844}),
            ]:
                scenario(browser, args.address, args.origin, viewport, name, args.verification)
                control(
                    args.address, "/restart", {}
                )  # Independent workflow quota, persisted account state.
            proxy = control(args.address, "/proxy", {})["origin"]
            STAGE = "proxy stream"
            wait_for_port(proxy_address, 3443)
            wait_for_port(proxy_address, 3444)
            wait_for_port(proxy_address, 3445)
            suite(
                Peer(proxy_address, 3443, "mimik.example:3443", proxy, tls=True),
                version,
                "proxy",
                restart=lambda: control(args.address, "/restart", {}),
                idle=Peer(proxy_address, 3444, "mimik.example:3443", proxy, tls=True),
                hold=Peer(proxy_address, 3445, "mimik.example:3443", proxy, tls=True),
                direct=False,
            )
            control(args.address, "/restart", {})
            login_errors(browser, args.address, proxy, True)
            control(args.address, "/restart", {})
            scenario(
                browser,
                args.address,
                proxy,
                {"width": 390, "height": 844},
                "proxy-player",
                args.verification,
                proxy=True,
            )
        finally:
            browser.close()
    print(
        "Separate-client LAN HTTP and HTTPS proxy Master stream and probe, copy/link/QR/date/retry and Origin/Secure-cookie workflows passed."
    )


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:
        raise SystemExit(f"LAN client {STAGE}: {error}") from None
    except Exception as error:
        raise SystemExit(
            f"LAN client {STAGE}: {type(error).__name__}; credential diagnostics redacted"
        ) from None
