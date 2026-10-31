"""Offline Chromium source-review checks, using only the synthetic source_fixture output.

Run in mimik-browser:
  python /checks/review_browser.py /app/mimik --fixture /fixture
Each viewport/JavaScript combination gets its own database and Owner session.
"""

import argparse
import json
import re
import sqlite3
import tempfile
from pathlib import Path
from urllib.parse import urljoin, urlsplit

from assist_demo import OWNER, create_accounts, demo_passwords, fresh_directory
from browser_smoke import CSP_PROBE, csp_clean, no_overflow
from smoke_release import registry_cli, require, start, stop

REGION_CROP = re.compile(r"/admin/materials/pages/\d+/units/\d+/image")
ORIGIN = "http://127.0.0.1:3000"
COUNTS = {"correction": 4, "reject": 1, "merge": 1, "missing_text": 1, "note": 2}


def form(page, kind, target=None):
    selector = f'form[data-form="{kind}"]'
    if target is not None:
        selector += f'[data-target="{target}"]'
    return page.locator(selector)


def field(container, name):
    return container.locator(f'[name="{name}"]')


def rows(directory, sql, parameters=()):
    """Read back durable state without changing the application's database."""
    path = directory / "smoke.sqlite3"
    with sqlite3.connect(f"{path.as_uri()}?mode=ro", uri=True) as db:
        return db.execute(sql, parameters).fetchall()


def prepare(binary, directory, fixture):
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
        "Synthetic edition",
    )
    registry_cli(
        binary,
        directory,
        "register-ruleset-alias",
        "--game-id",
        "mork-borg",
        "--edition-id",
        "fixture",
        "--legacy-edition",
        "fixture-edition",
    )
    # A first reading makes choosing the review reading a real primary change.
    for bundle in ("bundle", "review-bundle"):
        registry_cli(
            binary,
            directory,
            "import-source",
            "--bundle",
            str(fixture / bundle),
            "--pdf",
            str(fixture / "original.pdf"),
        )
    source, page_id = rows(
        directory,
        "SELECT p.source_id,p.id FROM source_pages p JOIN sources s ON s.id=p.source_id "
        "WHERE s.source_id='synthetic-review'",
    )[0]
    command = (
        "import-suggestions",
        "--source",
        str(source),
        "--report",
        str(fixture / "review-report.v2.json"),
    )
    first = json.loads(registry_cli(binary, directory, *command))
    repeated = json.loads(registry_cli(binary, directory, *command))
    require(
        first["counts"] == COUNTS and first["binding"] == "bundle" and not first["repeat"],
        f"Unexpected report import: {first}",
    )
    require(
        repeated["repeat"] and repeated["report_id"] == first["report_id"],
        "Second report import did not identify a repeat",
    )
    units = {
        int(block.rsplit("/", 1)[1]): unit
        for unit, block in rows(
            directory,
            "SELECT id,block_id FROM source_units WHERE page_id=? ORDER BY id",
            (page_id,),
        )
    }
    suggestions = [
        row[0] for row in rows(directory, "SELECT id FROM source_suggestions ORDER BY id")
    ]
    require(len(units) == 60 and len(suggestions) == 9, "Fixture cardinality changed")
    return source, page_id, units, suggestions


class Checks:
    """Check every visited document; keep console/script failures across navigations."""

    def __init__(self, context, javascript):
        self.context = context
        self.javascript = javascript
        self.pages = []
        self.consoles = []
        self.errors = []
        self.expected_conflicts = set()

    def page(self):
        page = self.context.new_page()
        page.add_init_script(CSP_PROBE)
        page.on(
            "console",
            lambda message: self.consoles.append(
                (message.type, message.text, message.location.get("url", ""))
            ),
        )
        page.on("pageerror", lambda error: self.errors.append(str(error)))
        self.pages.append(page)
        return page

    def clean(self, page):
        no_overflow(page, f"source review {page.url}")
        console_text = [text for _, text, _ in self.consoles]
        if self.javascript:
            csp_clean(page, console_text, self.errors)
        else:
            require(
                not self.errors
                and not any(
                    "Content Security Policy" in text or "Trusted Type" in text
                    for text in console_text
                ),
                f"No-JS CSP/script failure: {console_text} {self.errors}",
            )
        expected_message = (
            "Failed to load resource: the server responded with a status of 409 (Conflict)"
        )
        # One region crop runs at a time and a busy one answers 503 Retry-After: 1 (SRC-8);
        # source-regions.js retries it, so those console lines are expected.
        busy_crop = "Failed to load resource: the server responded with a status of 503 (Service Unavailable)"
        unexpected = [
            (text, url)
            for kind, text, url in self.consoles
            if kind == "error"
            and not (url in self.expected_conflicts and text == expected_message)
            and not (text == busy_crop and REGION_CROP.fullmatch(urlsplit(url).path))
        ]
        require(not unexpected, f"Console errors: {unexpected}")

    def goto(self, page, path):
        if page.url != "about:blank":
            self.clean(page)
        base = ORIGIN if page.url == "about:blank" else page.url
        response = page.goto(urljoin(base, path), wait_until="load")
        if response is None:
            # Same document, another fragment: Playwright returns no response, so reload to
            # check this request's status and policy too.
            response = page.reload(wait_until="load")
        require(response.status == 200, f"GET {path}: {response.status}")
        if urlsplit(response.url).path.startswith("/admin/materials"):
            require(
                "script-src 'self'" in response.headers.get("content-security-policy", ""),
                "Review document lost its script CSP",
            )
        self.clean(page)

    def submit(self, page, container, button=None, status=303):
        self.clean(page)
        action = urljoin(page.url, container.get_attribute("action"))
        if status == 409:
            self.expected_conflicts.add(action)
        with page.expect_navigation(wait_until="load"):
            with page.expect_response(
                lambda response: response.url == action and response.request.method == "POST"
            ) as response:
                (button if button is not None else container.locator("button")).click()
        require(
            response.value.status == status,
            f"POST {action}: expected {status}, got {response.value.status}",
        )
        if status == 409:
            require(page.locator('[role="alert"]').count() > 0, "Conflict has no announced error")
        self.clean(page)


def overlay(checks, page, path, manifest, units):
    checks.goto(page, path)
    require(
        page.locator("#review-image > img").evaluate(
            "image => image.complete && image.naturalWidth > 0"
        ),
        "Original page render did not load",
    )
    require(
        page.locator("[data-overlay-unit]").count() == 60, "Overlay must include all card pages"
    )
    if checks.javascript:
        # Busy crops are retried by source-regions.js; the first preview must arrive.
        page.locator(".source-region img").first.scroll_into_view_if_needed()
        page.wait_for_function(
            "() => { const i = document.querySelector('.source-region img');"
            " return i.complete && i.naturalWidth > 0; }"
        )
    modes = ("fit", "one", "two") if checks.javascript else ("fit",)
    polygons = {
        str(units[index + 1]): unit["polygon"]
        for index, unit in enumerate(manifest["pages"][0]["units"])
    }
    for mode in modes:
        if checks.javascript:
            page.locator(f'[data-zoom="{mode}"]').click()
            page.wait_for_function(
                "mode => document.querySelector(`[data-zoom=${mode}]`).getAttribute('aria-pressed') === 'true'",
                arg=mode,
            )
        actual = page.locator("#review-overlay").evaluate("""svg => ({
            image: (() => { const r = document.querySelector('#review-image > img').getBoundingClientRect();
                return {x:r.x,y:r.y,width:r.width,height:r.height}; })(),
            boxes: [...svg.querySelectorAll('[data-overlay-unit]')].map(box => {
                const r=box.getBoundingClientRect();
                return {id:box.dataset.overlayUnit,x:r.x,y:r.y,width:r.width,height:r.height};
            })})""")
        rendered = actual["image"]
        if mode != "fit":
            require(
                abs(rendered["width"] - (600 if mode == "one" else 1200)) <= 1,
                f"{mode} zoom did not use the rendered image's natural size: {rendered}",
            )
        for box in actual["boxes"]:
            points = polygons[box["id"]]
            xs, ys = [p[0] for p in points], [p[1] for p in points]
            expected = {
                "x": rendered["x"] + min(xs) * rendered["width"] / 200,
                "y": rendered["y"] + min(ys) * rendered["height"] / 300,
                "width": (max(xs) - min(xs)) * rendered["width"] / 200,
                "height": (max(ys) - min(ys)) * rendered["height"] / 300,
            }
            require(
                all(abs(box[key] - value) <= 1 for key, value in expected.items()),
                f"Overlay misaligned at {mode}: {box}, expected {expected}",
            )
        checks.clean(page)
    # Actual SVG clicks exercise navigation, including a card absent from this card page.
    if checks.javascript:
        page.locator('[data-zoom="fit"]').click()
    for index in (1, 51):
        checks.goto(page, path)
        box = page.locator(f'[data-overlay-unit="{units[index]}"]')
        link = box.locator("..")
        href = link.get_attribute("href")
        if index == 51:
            require("units=" in href, "Off-card-page overlay link has no units cursor")
        with page.expect_navigation(wait_until="load"):
            box.click()
        page.locator(f"#unit-{units[index]}").wait_for(state="visible")
        require(
            urlsplit(page.url).fragment == f"unit-{units[index]}",
            "Overlay opened the wrong card",
        )
        if index == 51:
            require("units=" in urlsplit(page.url).query, "Card cursor was lost")
        checks.clean(page)


def decide(checks, page, path, unit, text="", rationale="Synthetic owner comparison"):
    checks.goto(page, path)
    decision = form(page, "unit", unit)
    field(decision, "state").select_option("corrected" if text else "accepted")
    field(decision, "correction").fill(text)
    field(decision, "rationale").fill(rationale)
    checks.submit(page, decision)


def applied(checks, page, path, suggestion, unit=None, text=None):
    checks.goto(page, path)
    card = page.locator(f'[data-suggestion="{suggestion}"]').first
    require("— applied" in card.locator("h3").inner_text(), "Suggestion not shown as applied")
    if unit is not None:
        require(
            page.locator(f"#unit-{unit} pre.evidence-text").inner_text() == text,
            "Card lookup does not contain the applied text",
        )
        require(
            page.locator(f'[data-lookup-unit="{unit}"]').inner_text() == text,
            "Lookup panel does not contain the applied text",
        )


def shortcuts(checks, page, path, units):
    checks.goto(page, path)
    first = page.locator(f"#unit-{units[1]}")
    second = page.locator(f"#unit-{units[2]}")
    first.focus()
    page.keyboard.press("j")
    require(second.evaluate("node => node === document.activeElement"), "j did not focus next card")
    page.keyboard.press("k")
    require(
        first.evaluate("node => node === document.activeElement"), "k did not focus previous card"
    )
    posts = []
    page.on(
        "request", lambda request: posts.append(request.url) if request.method == "POST" else None
    )
    page.keyboard.press("a")
    rationale = first.locator("[data-accept-rationale]")
    require(
        rationale.evaluate("node => node === document.activeElement"), "a did not focus rationale"
    )
    rationale.fill("Synthetic shortcut draft ")
    for key in ("j", "k", "a"):
        page.keyboard.press(key)
        require(
            rationale.evaluate("node => node === document.activeElement"), "Shortcut escaped field"
        )
    require(
        rationale.input_value() == "Synthetic shortcut draft jka" and not posts,
        "Shortcut submitted or swallowed input",
    )
    field(form(page, "unit", units[1]), "state").focus()
    page.keyboard.press("j")
    require(
        field(form(page, "unit", units[1]), "state").evaluate(
            "node => node === document.activeElement"
        ),
        "Shortcut escaped select",
    )
    page.get_by_text("Review keyboard help", exact=True).click()
    page.locator("#review-shortcuts").uncheck()
    checks.goto(page, path)
    require(
        not page.locator("#review-shortcuts").is_checked(), "Shortcut off switch did not persist"
    )
    first.focus()
    for key in ("j", "k", "a"):
        page.keyboard.press(key)
        require(
            first.evaluate("node => node === document.activeElement"),
            "Disabled shortcut moved focus",
        )
    require(not posts, "Shortcut submitted a decision")


def add_text(checks, page, path, directory):
    checks.goto(page, path)
    owner = form(page, "owner")
    if checks.javascript:
        page.locator("#draw-region").click()
        page.locator("#review-overlay").scroll_into_view_if_needed()
        rectangle = page.locator("#review-image > img").bounding_box()
        x, y = rectangle["x"], rectangle["y"]
        width, height = rectangle["width"], rectangle["height"]
        page.mouse.move(x + width * 0.2, y + height * 0.2)
        page.mouse.down()
        page.mouse.move(x + width * 0.4, y + height * 0.3, steps=4)
        page.mouse.up()
        polygon = json.loads(field(owner, "polygon").input_value())
        expected = [[40, 60], [80, 60], [80, 90], [40, 90]]
        require(
            all(
                abs(a - b) <= 1
                for p, q in zip(polygon, expected, strict=True)
                for a, b in zip(p, q, strict=True)
            ),
            f"Pointer box coordinates differ: {polygon}",
        )
        handle = page.locator('[data-corner="0"]')
        handle.focus()
        handle.press("ArrowRight")
        handle.press("ArrowDown")
        adjusted = json.loads(field(owner, "polygon").input_value())
        require(
            abs(adjusted[0][0] - polygon[0][0] - 1) < 0.001
            and abs(adjusted[0][1] - polygon[0][1] - 1) < 0.001
            and adjusted[2] == polygon[2],
            "Keyboard did not adjust the drawn top-left corner in PDF points",
        )
        page.locator('[data-corner="2"]').press("Shift+ArrowRight")
        final_polygon = json.loads(field(owner, "polygon").input_value())
        require(
            abs(final_polygon[2][0] - adjusted[2][0] - 10) < 0.001,
            "Keyboard coarse adjustment did not use ten PDF points",
        )
        text = "Synthetic pointer and keyboard owner text"
    else:
        final_polygon = [[20, 280], [80, 280], [80, 290], [20, 290]]
        require(not page.locator("#draw-region").is_visible(), "No-JS drawing control is visible")
        field(owner, "polygon").fill(json.dumps(final_polygon))
        text = "Synthetic no-JavaScript corner text"
    field(owner, "text").fill(text)
    field(owner, "rationale").fill("Synthetic missing-text comparison")
    checks.submit(page, owner)
    saved = rows(
        directory,
        "SELECT id,polygon,correction FROM source_units WHERE origin='owner' AND correction=?",
        (text,),
    )
    require(
        len(saved) == 1 and json.loads(saved[0][1]) == final_polygon, "Owner region/text not stored"
    )
    # Follow its overlay link to the owner card, which is beyond the original 50 cards.
    checks.goto(page, path)
    link = page.locator(f'[data-overlay-unit="{saved[0][0]}"]').locator("..")
    checks.goto(page, link.get_attribute("href"))
    require(
        text == page.locator(f"#unit-{saved[0][0]} pre.evidence-text").inner_text(),
        "New owner text not shown on its card",
    )


def conflicts(checks, page, path, units, suggestions):
    other = checks.page()
    # Unit decision: both pages read revision 1; the first commit invalidates the draft.
    checks.goto(page, path)
    draft = form(page, "unit", units[9])
    field(draft, "state").select_option("corrected")
    field(draft, "correction").fill("Synthetic unit draft <b>&\nsecond line")
    field(draft, "rationale").fill("Synthetic unit draft reason")
    revision = field(draft, "revision").input_value()
    decide(checks, other, path, units[9])
    checks.submit(page, draft, status=409)
    kept = form(page, "unit", units[9])
    require(
        field(kept, "state").input_value() == "corrected"
        and field(kept, "correction").input_value() == "Synthetic unit draft <b>&\nsecond line"
        and field(kept, "rationale").input_value() == "Synthetic unit draft reason"
        and field(kept, "revision").input_value() == revision,
        "409 lost a unit draft field/token",
    )
    # Accept/Edit: a unit decision in the other tab makes this open edit stale.
    suggestion = suggestions[3]
    checks.goto(page, f"/admin/materials/suggestions/{suggestion}#edit-{suggestion}")
    draft = form(page, "accept", suggestion)
    field(draft, "edited_text").fill("Synthetic accept draft <b>&\nsecond line")
    field(draft, "rationale").fill("Synthetic accept draft reason")
    decide(checks, other, path, units[8])
    checks.submit(
        page, draft, draft.get_by_role("button", name="Accept my edit", exact=True), status=409
    )
    kept = form(page, "accept", suggestion)
    require(
        field(kept, "edited_text").input_value() == "Synthetic accept draft <b>&\nsecond line"
        and field(kept, "rationale").input_value() == "Synthetic accept draft reason",
        "409 lost an accept/edit draft",
    )
    # Dismiss has no unit revision: a competing dismissal conflicts on suggestion state.
    suggestion = suggestions[8]
    detail = f"/admin/materials/suggestions/{suggestion}"
    checks.goto(page, detail)
    draft = form(page, "dismiss", suggestion)
    field(draft, "reason").fill("Synthetic dismissal draft <b>&\nsecond line")
    checks.goto(other, detail)
    field(form(other, "dismiss", suggestion), "reason").fill("Synthetic competing dismissal")
    checks.submit(other, form(other, "dismiss", suggestion))
    checks.submit(page, draft, status=409)
    require(
        field(form(page, "dismiss", suggestion), "reason").input_value()
        == "Synthetic dismissal draft <b>&\nsecond line",
        "409 lost dismissal draft",
    )
    checks.clean(other)


def scenario(
    context, directory, fixture, source, page_id, units, suggestions, javascript, password
):
    checks = Checks(context, javascript)
    page = checks.page()
    checks.goto(page, "/auth/login")
    page.locator('[name="login"]').fill(OWNER)
    page.locator('[name="password"]').fill(password.decode())
    with page.expect_navigation(wait_until="load"):
        page.locator('form[action="/auth/login"] button').click()
    path = f"/admin/materials/pages/{page_id}"
    manifest = json.loads((fixture / "review-bundle/bundle.json").read_text())
    overlay(checks, page, path, manifest, units)
    if javascript:
        shortcuts(checks, page, path, units)
    # A stored correction, without supplying the suggestion text in the form.
    checks.goto(page, path)
    for suggestion in (suggestions[7], suggestions[8]):
        note = page.locator(f'[data-suggestion="{suggestion}"]')
        require(
            note.count() == 1 and note.locator('form[data-form="accept"]').count() == 0,
            "Unit/page note is missing or offers acceptance",
        )
    checks.submit(page, form(page, "accept", suggestions[0]))
    applied(checks, page, path, suggestions[0], units[1], "Synthetic corrected text 1")
    # The second card page's correction, reached through its actual Edit link.
    checks.goto(
        page,
        page.locator(f'[data-overlay-unit="{units[51]}"]').locator("..").get_attribute("href"),
    )
    second_path = page.url
    edit = page.locator(f'[data-suggestion="{suggestions[1]}"]').get_by_role(
        "link", name=f"Edit suggestion #{suggestions[1]}", exact=True
    )
    with page.expect_navigation(wait_until="load"):
        edit.click()
    edited = "Synthetic edited correction <b>& literal text"
    acceptance = form(page, "accept", suggestions[1])
    field(acceptance, "edited_text").fill(edited)
    field(acceptance, "rationale").fill("Synthetic edited comparison")
    checks.submit(
        page, acceptance, acceptance.get_by_role("button", name="Accept my edit", exact=True)
    )
    applied(checks, page, second_path, suggestions[1], units[51], edited)
    require(
        page.locator(f"#unit-{units[51]} pre.evidence-text b").count() == 0, "Edit became markup"
    )
    # Normal dismissal with a reason, and a separately stale correction.
    checks.goto(page, path)
    dismissal = form(page, "dismiss", suggestions[7])
    field(dismissal, "reason").fill("Synthetic normal dismissal reason")
    checks.submit(page, dismissal)
    checks.goto(page, f"/admin/materials/suggestions/{suggestions[7]}")
    require(
        "— dismissed" in page.locator("h1").inner_text()
        and rows(
            directory,
            "SELECT reason FROM source_suggestion_dismissals WHERE suggestion_id=?",
            (suggestions[7],),
        )
        == [("Synthetic normal dismissal reason",)],
        "Dismissal state/reason not recorded",
    )
    decide(checks, page, path, units[2])
    checks.goto(page, path)
    stale = page.locator(f'[data-suggestion="{suggestions[2]}"]')
    require(
        "— stale" in stale.locator("h3").inner_text()
        and form(page, "accept", suggestions[2]).locator("button").is_disabled()
        and stale.get_by_role("link", name=f"Edit suggestion #{suggestions[2]}", exact=True).count()
        == 0
        and form(page, "dismiss", suggestions[2]).locator("button").is_enabled(),
        "Stale card has wrong actions",
    )
    checks.goto(page, f"/admin/materials/suggestions/{suggestions[2]}")
    require(
        form(page, "accept").get_by_role("button", name="Accept my edit", exact=True).is_disabled()
        and form(page, "accept")
        .get_by_role("button", name="Accept as suggested", exact=True)
        .is_disabled()
        and field(form(page, "accept"), "edited_text").get_attribute("readonly") is not None
        and form(page, "dismiss").locator("button").is_enabled(),
        "Stale detail permits acceptance/edit",
    )
    checks.submit(page, form(page, "dismiss"))
    checks.goto(page, path)
    checks.submit(page, form(page, "accept", suggestions[4]))
    applied(checks, page, path, suggestions[4])
    require("rejected" in page.locator(f"#unit-{units[3]} h2").inner_text(), "Reject not applied")
    checks.submit(page, form(page, "accept", suggestions[5]))
    applied(checks, page, path, suggestions[5])
    for index in (4, 5):
        require(
            "rejected" in page.locator(f"#unit-{units[index]} h2").inner_text(),
            "Merge target not rejected",
        )
    merged = rows(
        directory,
        "SELECT id,polygon FROM source_units "
        "WHERE origin='owner' AND correction='Synthetic merged text'",
    )
    require(
        len(merged) == 1 and json.loads(merged[0][1]) == [[5, 19], [125, 19], [125, 29], [5, 29]],
        f"Merge did not create one bounded owner unit: {merged}",
    )
    checks.submit(page, form(page, "accept", suggestions[6]))
    applied(checks, page, path, suggestions[6])
    missing = rows(
        directory,
        "SELECT polygon FROM source_units "
        "WHERE origin='owner' AND correction='Synthetic missing text'",
    )
    require(
        len(missing) == 1
        and json.loads(missing[0][0]) == [[140, 285], [195, 285], [195, 295], [140, 295]],
        "Missing-text acceptance lost region/text",
    )
    add_text(checks, page, path, directory)
    checks.goto(page, f"/admin/materials/sources/{source}")
    primary = page.locator("#primary-pages")
    require(
        "another reading" in primary.inner_text(), "Primary fixture already selects this reading"
    )
    primary.locator('[name="page_1"]').check()
    field(primary, "rationale").fill("Synthetic primary reading comparison")
    checks.submit(page, primary)
    require(
        "this reading is primary" in page.locator("#primary-pages").inner_text(),
        "Page list did not record primary reading",
    )
    conflicts(checks, page, path, units, suggestions)
    require(
        rows(directory, "SELECT count(*) FROM source_units WHERE origin='owner'")[0][0] == 3,
        "Unexpected extra owner units",
    )
    for tab in checks.pages:
        checks.clean(tab)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path)
    parser.add_argument("--fixture", type=Path, required=True)
    args = parser.parse_args(argv)
    binary, fixture = args.binary.resolve(strict=True), args.fixture.resolve(strict=True)
    from playwright.sync_api import sync_playwright

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(args=["--disable-background-networking"])
        try:
            for viewport in ({"width": 1280, "height": 720}, {"width": 390, "height": 844}):
                for javascript in (True, False):
                    with tempfile.TemporaryDirectory(prefix="mimik-review-browser-") as temp:
                        directory = fresh_directory(Path(temp) / "data")
                        passwords = demo_passwords()
                        create_accounts(binary, directory, passwords)
                        prepared = prepare(binary, directory, fixture)
                        process, log = start(binary, directory, auth=True)
                        try:
                            context = browser.new_context(
                                viewport=viewport, java_script_enabled=javascript
                            )
                            try:
                                scenario(
                                    context,
                                    directory,
                                    fixture,
                                    *prepared,
                                    javascript,
                                    passwords[OWNER],
                                )
                            finally:
                                context.close()
                        finally:
                            stop(process, log)
                    print(f"Source review passed: {viewport['width']} px, JavaScript={javascript}")
        finally:
            browser.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
