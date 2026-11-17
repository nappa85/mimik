"""Offline upload checks in mimik-browser (dev-endpoint binary required).

  python /checks/upload_browser.py /app/mimik-dev
No user namespaces or PDF runtime is needed: --upload-dev-worker-ms selects the
compile-time development-only worker, which accepts exactly the shared synthetic fixture.
Required CI browser-smoke step; the image includes this script and that fixture.
"""

import argparse
import tempfile
import time
from pathlib import Path

from assist_demo import OWNER, create_accounts, demo_passwords, fresh_directory
from browser_smoke import CSP_PROBE, csp_clean, no_overflow
from review_browser import ORIGIN, STRICT_CSP, rows
from smoke_release import registry_cli, require, start, stop, wait_until

PDF = (Path(__file__).resolve().parent / "fixtures/upload-three-pages.pdf.txt").read_bytes()
EXTRA = ("--upload-dev-worker-ms", "2000")


def wait_rows(directory, sql, predicate):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        value = rows(directory, sql)
        if predicate(value):
            return value
        time.sleep(0.05)
    raise AssertionError(f"Upload state deadline: {sql}: {value}")


def scenario(browser, binary, directory, viewport, javascript, password):
    context = browser.new_context(viewport=viewport, java_script_enabled=javascript)
    context.add_init_script(CSP_PROBE)
    page = context.new_page()
    consoles, errors, policies = [], [], []
    page.on("console", lambda message: consoles.append(message.text))
    page.on("pageerror", lambda error: errors.append(str(error)))

    def check_policy(response):
        upload = response.url.startswith(ORIGIN + "/admin/materials/uploads")
        script = response.url == ORIGIN + "/assets/source-upload.js"
        if (upload or script) and 200 <= response.status < 400:
            if response.headers.get("content-security-policy") != STRICT_CSP:
                policies.append(response.url)

    page.on("response", check_policy)
    process, log = start(binary, directory, auth=True, extra=EXTRA)

    def clean():
        no_overflow(page, "upload")
        require(not policies, f"Upload responses without strict CSP: {policies}")
        if javascript:
            csp_clean(page, consoles, errors)

    def goto(path):
        response = page.goto(ORIGIN + path, wait_until="load")
        require(response.status == 200, f"Upload GET failed: {path}")
        require(response.headers.get("content-security-policy") == STRICT_CSP, "Upload CSP changed")
        clean()

    def submit(kind, redirect=True):
        from playwright.sync_api import TimeoutError as PlaywrightTimeout

        form = page.locator(f'form[data-form="{kind}"]').first
        target = ORIGIN + form.get_attribute("action")
        posted = []

        def is_post(response):
            matches = response.url == target and response.request.method == "POST"
            if matches:
                posted.append(response)
            return matches

        def is_redirect_get(response):
            previous = response.request.redirected_from
            return (
                response.request.method == "GET"
                and previous is not None
                and previous.method == "POST"
                and previous.url == target
            )

        if not redirect:
            with page.expect_navigation(wait_until="load") as response:
                form.locator('button:not([type="button"])').click()
            require(response.value.status == 200, f"Upload {kind} failed")
            clean()
            return
        with page.expect_response(is_post) as response:
            with page.expect_response(is_redirect_get) as following:
                for attempt in range(10):
                    try:
                        form.locator('button:not([type="button"])').click(timeout=1000)
                        break
                    except PlaywrightTimeout:
                        if posted:
                            break
                        require(attempt < 9, f"Upload {kind} kept refreshing before submit")
        require(response.value.status == 303, f"Upload {kind} failed: {response.value.status}")
        expected = ORIGIN + response.value.headers["location"]
        require(following.value.url == expected, f"Upload {kind} redirected unexpectedly")
        page.wait_for_url(expected, wait_until="load")
        clean()

    try:
        page.goto(ORIGIN + "/auth/login")
        page.locator('[name="login"]').fill(OWNER)
        page.locator('[name="password"]').fill(password.decode())
        with page.expect_navigation(wait_until="load"):
            page.locator('form[action="/auth/login"] button').click()
        goto("/admin/materials/uploads")
        token = page.locator('[name="csrf"]').input_value()
        refused = context.request.post(
            ORIGIN + "/admin/materials/uploads",
            headers={"Origin": ORIGIN, "Content-Type": "multipart/form-data; boundary=bad"},
            data=(
                '--bad\r\nContent-Disposition: form-data; name="system"\r\n\r\nmork-borg\r\n'
                f'--bad\r\nContent-Disposition: form-data; name="csrf"\r\n\r\n{token}\r\n'
                "--bad--\r\n"
            ),
        )
        require(refused.status == 422, "Out-of-order upload accepted")
        page.locator('[name="system"]').fill("mork-borg")
        page.locator('[name="edition"]').fill("fixture-edition")
        page.locator('[name="language"]').fill("en")
        page.locator('[name="file"]').set_input_files(
            {"name": "BUYER-MARKER-NAME.pdf", "mimeType": "application/pdf", "buffer": PDF}
        )
        submit("upload")
        path = page.url.removeprefix(ORIGIN)
        wait_rows(directory, "SELECT state FROM source_jobs", lambda r: r == [("succeeded",)])
        # Exercise automatic JS polling / no-JS meta refresh, not just manual reload.
        page.get_by_text("Inspection: 3 pages", exact=True).wait_for(timeout=10000)
        clean()
        for n in range(1, 4):
            page.locator(f'[name="page_{n}"]').check()
        submit("set")
        submit("render")
        wait_rows(directory, "SELECT count(*) FROM source_upload_pages", lambda r: r[0][0] >= 1)
        submit("cancel")
        require(rows(directory, "SELECT state FROM source_jobs") == [("cancelled",)], "Cancel lost")
        submit("render")
        wait_rows(directory, "SELECT state FROM source_jobs", lambda r: r == [("running",)])
        # Deliberately crash rather than graceful shutdown: queued/running recovery is real.
        process.kill()
        process.wait(timeout=5)
        log.close()
        process, log = start(binary, directory, auth=True, extra=EXTRA)
        goto(path)
        require(
            "interrupted" in page.locator("#upload-progress").inner_text(), "Restart reason missing"
        )
        submit("render")
        wait_rows(directory, "SELECT state FROM source_jobs", lambda r: r == [("succeeded",)])
        page.locator(".upload-image").nth(2).wait_for(timeout=10000)
        for index in range(3):
            page.locator(".upload-image").nth(index).scroll_into_view_if_needed()
        wait_until(
            page,
            "Array.from(document.querySelectorAll('.upload-image')).every("
            "image => image.complete && image.naturalWidth > 0)",
        )
        clean()
        if javascript:
            # A numeric zoom chosen before the PNG arrives must survive its load.
            delayed = []

            def delay_image(route):
                delayed.append(route)

            page.route("**/pages/1/image", delay_image)
            page.goto(ORIGIN + path, wait_until="domcontentloaded")
            first_image = page.locator(".upload-image").first
            first_image.scroll_into_view_if_needed()
            deadline = time.monotonic() + 10
            while not delayed and time.monotonic() < deadline:
                page.wait_for_timeout(50)
            require(delayed, "Delayed image route was not requested")
            page.locator('.upload-zoom [data-upload-zoom="2"]').first.click()
            require(
                first_image.evaluate("image => image.naturalWidth") == 0, "Image was not delayed"
            )
            require(
                first_image.evaluate('image => image.style.width !== "0px"'),
                "Zoom collapsed the loading image",
            )
            for route in delayed:
                route.continue_()
            page.unroute("**/pages/1/image", delay_image)
            wait_until(
                page,
                "() => { const image = document.querySelector('.upload-image');"
                "return image.naturalWidth > 0 && "
                "parseFloat(image.style.width) === image.naturalWidth * 2; }",
            )
            page.locator('[data-upload-zoom="2"]').first.click()
            clean()
        require("BUYER-MARKER-NAME" not in page.content(), "Filename leaked into page")
        hashes = rows(directory, "SELECT pdf_page,png_sha256 FROM source_upload_pages")
        submit("propose", redirect=False)
        proposal = page.locator('form[data-form="exclude"]').first
        require(proposal.get_attribute("data-fingerprint"), "Proposal fingerprint missing")
        require(
            "Affected pages: 1 2 3" in page.locator("#repeated-text").inner_text(),
            "Repeated-text pages missing",
        )
        require(
            all(proposal.locator(f'[name="exclude_{n}"]').is_checked() for n in range(1, 4)),
            "Proposal page selection changed",
        )
        proposal.locator('[name="rationale"]').fill("RATIONALE-PRIVATE-MARKER")
        submit("exclude")
        require(page.locator(".exclusion-mask").count() == 3, "Confirmed overlays missing")
        for mask in page.locator(".exclusion-mask").all():
            require(
                mask.evaluate("e => getComputedStyle(e).fillOpacity") == "1",
                "Exclusion overlay is not opaque",
            )
        require(
            rows(directory, "SELECT pdf_page,png_sha256 FROM source_upload_pages") == hashes,
            "Exclusion modified original PNGs",
        )
        # Numeric adjustment is exercised with JavaScript enabled and disabled.
        editor = page.locator('form[data-form="exclude"]')
        page.locator("#repeated-text details summary").click()
        if javascript:
            region = editor.locator("[data-region-editor]").first
            region.locator("[data-region-keyboard]").click()
            handle = region.locator('[data-corner="0"]')
            before = float(editor.locator('[name="box_1_0"]').input_value())
            handle.press("ArrowRight")
            require(
                float(editor.locator('[name="box_1_0"]').input_value()) == before + 1,
                "Shared region keyboard handle lost displayed coordinates",
            )
        editor.locator('[name="box_1_0"]').fill("4")
        editor.locator('[name="rationale"]').fill("Synthetic numeric adjustment")
        submit("exclude")
        require(
            rows(directory, "SELECT revision FROM source_upload_exclusions ORDER BY revision")
            == [(1,), (2,)],
            "Replacement did not append history",
        )
        require(page.locator(".exclusion-mask").first.get_attribute("x") == "4", "Adjust lost")
        page.locator('form[data-form="remove-exclusion"] [name="rationale"]').fill(
            "Synthetic removal"
        )
        submit("remove-exclusion")
        require(page.locator(".exclusion-mask").count() == 0, "Removal kept overlay")
        require(
            rows(directory, "SELECT rationale FROM source_upload_exclusions WHERE revision=1")
            == [("RATIONALE-PRIVATE-MARKER",)],
            "Rationale row changed",
        )
        for table in rows(directory, "SELECT name FROM sqlite_schema WHERE type='table'"):
            name = table[0].replace('"', '""')
            records = rows(directory, f'SELECT * FROM "{name}"')
            rendered = repr(records).lower()
            require("private-text-marker" not in rendered, "Staged text leaked into database")
            require("buyer-marker-name" not in rendered, "Filename leaked into database")
            if name != "source_upload_exclusions":
                require("rationale-private-marker" not in rendered, "Rationale escaped its row")
        submit("discard")
        require(
            rows(directory, "SELECT count(*) FROM source_uploads") == [(0,)], "Discard kept rows"
        )
        require(not list((directory / "staging").iterdir()), "Discard kept staged files")
        require(
            "BUYER-MARKER-NAME" not in (directory / "server.log").read_text(),
            "Filename leaked into log",
        )
        logged = (directory / "server.log").read_text().lower()
        for marker in ("private-text-marker", "rationale-private-marker"):
            require(marker not in logged, "Private text leaked into server/worker stderr capture")
    finally:
        context.close()
        if process.poll() is None:
            stop(process, log)
        elif not log.closed:
            log.close()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path)
    args = parser.parse_args(argv)
    binary = args.binary.resolve(strict=True)
    from playwright.sync_api import sync_playwright

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(args=["--disable-background-networking"])
        try:
            for viewport in ({"width": 1280, "height": 720}, {"width": 390, "height": 844}):
                for javascript in (True, False):
                    with tempfile.TemporaryDirectory(prefix="mimik-upload-browser-") as temp:
                        directory = fresh_directory(Path(temp) / "data")
                        passwords = demo_passwords()
                        create_accounts(binary, directory, passwords)
                        registry_cli(
                            binary,
                            directory,
                            "register-ruleset",
                            "--game-id",
                            "mork-borg",
                            "--game-label",
                            "MÖRK BORG",
                            "--edition-id",
                            "fixture",
                            "--edition-label",
                            "fixture-edition",
                        )
                        scenario(browser, binary, directory, viewport, javascript, passwords[OWNER])
                    print(f"Source upload passed: {viewport['width']} px, JavaScript={javascript}")
        finally:
            browser.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
