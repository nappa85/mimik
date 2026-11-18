"""Live Chromium check of the assistant page's live progress (2d-4; local prototype tooling,
reported as live inference: a real model server starts).

Host mode starts Mimik (a release build, `--assistant`, the CPU path) on 127.0.0.1:3000 with
synthetic accounts, signs the Owner in and runs browser mode in the browser container
(containers/browser.Dockerfile) on the host network, with the session cookie in a 0600 file.
Browser mode, never reloading the page itself:
  - a start advances through at least two steps in one document, and the page arrives at
    `Ready` by its own navigation;
  - Restart shows the stop step; a page without JavaScript shows the progress block and asks
    for a reload; a typed Model field survives the state change (no reload, "The state
    changed: reload"); Stop updating stops polling;
  - at 1280 x 720 and 390 x 844: no horizontal overflow, no console error, no CSP violation.

  python3 tools/assist_progress_browser.py <mimik> --models <dir> --runtime <llama dir> \
      --image <mimik-browser tag> [--threads 2]
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
BASE = "http://127.0.0.1:3000"
PAGE = BASE + "/admin/assistant"
START_LIMIT_S = 20 * 60


def require(condition, message):
    if not condition:
        raise AssertionError(message)


# ---------------------------------------------------------------- browser mode (container)


def state_of(page):
    return page.locator("section.panel").first.locator("strong").first.inner_text().strip()


def mark(page):
    """A marker that lives as long as this document: gone after any navigation."""
    page.evaluate("window.__progressDoc = 'kept'")


def same_document(page):
    try:
        return page.evaluate("window.__progressDoc === 'kept'")
    except Exception:  # noqa: BLE001 (a navigation in progress destroys the context)
        return False


def clean(page, label):
    from browser_smoke import no_overflow

    no_overflow(page, label)
    violations = page.evaluate("window.__csp || []")
    require(not violations, f"CSP violation on {label}: {violations}")


def watch_start(page, label):
    """The step advances in one document, then the page reaches Ready by itself."""
    require(state_of(page) == "Starting", f"{label}: the page did not open while starting")
    text = page.locator("#progress-text")
    require(text.get_attribute("aria-live") == "polite", "Step text is not aria-live polite")
    bar = page.locator("#progress-bar")
    require(
        bar.get_attribute("aria-labelledby") == "progress-text",
        "Bar is not named by step",
    )
    require(page.locator("#live-stop").is_visible(), "Stop updating button is not shown")
    mark(page)
    steps, times = [text.inner_text()], {page.locator("#progress-time").inner_text()}
    deadline = time.monotonic() + START_LIMIT_S
    while same_document(page) and time.monotonic() < deadline:
        try:
            current = text.inner_text(timeout=1000)
            times.add(page.locator("#progress-time").inner_text(timeout=1000))
        except Exception:  # noqa: BLE001 (the page navigated between the two reads)
            break
        if current != steps[-1]:
            steps.append(current)
        time.sleep(0.5)
    require(len(steps) >= 2, f"{label}: the step never advanced in place: {steps}")
    require(len(times) >= 2, f"{label}: the time line never changed in place")
    page.wait_for_load_state()
    while state_of(page) != "Ready":
        require(time.monotonic() < deadline, f"{label}: never reached Ready by itself")
        page.wait_for_timeout(1000)
    require(page.locator("#live-progress").count() == 0, "Progress block shown while ready")
    print(f"{label}: {len(steps)} steps seen in place: {' / '.join(steps)}", flush=True)
    return steps


def browser(cookie_file):
    from browser_smoke import CSP_PROBE
    from playwright.sync_api import sync_playwright

    value = Path(cookie_file).read_text().strip()
    cookie = {
        "name": "mimik_session",
        "value": value,
        "url": BASE,
        "sameSite": "Strict",
    }
    with sync_playwright() as playwright:
        chromium = playwright.chromium.launch()
        errors = []

        def context(viewport, javascript=True):
            made = chromium.new_context(
                viewport=viewport, java_script_enabled=javascript, base_url=BASE
            )
            made.add_cookies([cookie])
            made.add_init_script(CSP_PROBE)
            page = made.new_page()
            page.on("console", lambda m: m.type == "error" and errors.append(m.text))
            page.on("pageerror", lambda e: errors.append(str(e)))
            return made, page

        wide, page = context({"width": 1280, "height": 720})
        page.goto(PAGE)
        clean(page, "1280 starting")
        watch_start(page, "first start")
        clean(page, "1280 ready")

        # Restart: the stop shows first; the page without JavaScript shows the step.
        page.get_by_role("button", name="Restart", exact=True).click()
        page.wait_for_load_state()
        first = page.locator("#progress-text").inner_text()
        require("starting" in state_of(page).lower(), "Restart did not show Starting")
        # The stop may already be over when the redirect lands; the 9-step list is the one
        # with the stop step.
        require(" of 9, " in first, f"Restart did not walk the list with the stop: {first}")
        print(f"after Restart: {first}", flush=True)
        nojs, plain = context({"width": 390, "height": 844}, javascript=False)
        plain.goto(PAGE)
        require(plain.locator("#progress-text").count() == 1, "No-JS page lacks the step")
        require(plain.locator("#live-stop").is_hidden(), "No-JS page shows Stop updating")
        require(
            "reload it" in plain.locator("#live-progress").inner_text(),
            "No-JS reload text",
        )
        nojs.close()

        # A typed field survives a state change (the restart's budgets going, or Ready): no
        # reload, a note instead.
        mark(page)
        field = page.locator("#model-spec")
        field.fill("local:typed-by-the-check.gguf")
        page.locator("h2", has_text="State").click()
        deadline = time.monotonic() + START_LIMIT_S
        note = page.locator("#live-note")
        while note.inner_text() != "The state changed: reload":
            require(same_document(page), "The page reloaded over a typed field")
            require(time.monotonic() < deadline, "No state change reached the page")
            page.wait_for_timeout(1000)
        require(field.input_value() == "local:typed-by-the-check.gguf", "Typed field lost")
        page.locator("#live-stop").click()
        require(
            note.inner_text() == "Updating stopped: reload to resume",
            "Stop updating text",
        )
        require(page.locator("#live-stop").is_hidden(), "Stop button still shown")

        # The phone width follows the rest of the restart by its own reloads to Ready.
        narrow, small = context({"width": 390, "height": 844})
        small.goto(PAGE)
        clean(small, "390 starting")
        while state_of(small) != "Ready":
            require(time.monotonic() < deadline, "390 page never reached Ready by itself")
            small.wait_for_timeout(1000)
        clean(small, "390 ready")
        narrow.close()
        wide.close()
        require(not errors, f"Console errors: {errors}")
        chromium.close()
    print(
        "PASS: live progress advanced in place, reached Ready by itself, kept a typed "
        "field, worked without JavaScript, at 1280 x 720 and 390 x 844",
        flush=True,
    )


# ---------------------------------------------------------------- host mode


def host(args):
    from assist_demo import OWNER, create_accounts, demo_passwords, fresh_directory
    from smoke_release import auth_cookie, auth_fields, auth_request, start, stop

    try:
        auth_request("/auth/login")
    except ConnectionRefusedError:
        pass
    else:
        raise AssertionError("Port 3000 is already in use")
    with tempfile.TemporaryDirectory(prefix="mimik-progress-browser-") as temp:
        directory = fresh_directory(Path(temp) / "data")
        passwords = demo_passwords()
        create_accounts(args.binary, directory, passwords)
        process, log = start(
            args.binary,
            directory,
            auth=True,
            extra=(
                "--assistant",
                "--models-dir",
                str(args.models),
                "--assistant-runtime",
                str(args.runtime),
                "--assistant-device",
                "cpu",
                "--assistant-threads",
                str(args.threads),
            ),
        )
        try:
            response = auth_request("/auth/login")
            fields = auth_fields(response, "/auth/login")
            response = auth_request(
                "/auth/login",
                cookie=auth_cookie(response),
                form=dict(fields, login=OWNER, password=passwords[OWNER].decode()),
            )
            require(response[0] == 303, "Owner login failed")
            cookie = auth_cookie(response).split("=", 1)[1]
            private = Path(temp) / "cookie"
            private.write_text(cookie)
            # Read by the container's user; the enclosing folder stays 0700 on the host.
            os.chmod(private, 0o644)
            result = subprocess.run(
                [
                    "docker",
                    "run",
                    "--rm",
                    "--network",
                    "host",
                    "--cpus",
                    "2",
                    "-e",
                    "PYTHONDONTWRITEBYTECODE=1",
                    "--mount",
                    f"type=bind,src={TOOLS},dst=/tools,readonly",
                    "--mount",
                    f"type=bind,src={private},dst=/cookie,readonly",
                    "-w",
                    "/tools",
                    "--entrypoint",
                    "python3",
                    args.image,
                    "/tools/assist_progress_browser.py",
                    "browser",
                    "--cookie-file",
                    "/cookie",
                ],
                check=False,
            )
            require(result.returncode == 0, "The browser check failed")
        finally:
            stop(process, log)


def main(argv=None):
    if argv is None:
        argv = sys.argv[1:]
    if argv[:1] == ["browser"]:
        parser = argparse.ArgumentParser()
        parser.add_argument("mode")
        parser.add_argument("--cookie-file", required=True)
        args = parser.parse_args(argv)
        browser(args.cookie_file)
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--threads", type=int, default=2)
    args = parser.parse_args(argv)
    args.binary = args.binary.resolve(strict=True)
    args.models = args.models.resolve(strict=True)
    args.runtime = args.runtime.resolve(strict=True)
    try:
        host(args)
    except AssertionError as error:
        print(f"FAIL: {error}", flush=True)
        return 1
    print("PASS", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
