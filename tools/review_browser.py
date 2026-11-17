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
from smoke_release import registry_cli, require, start, stop, wait_until

REGION_CROP = re.compile(r"/admin/materials/pages/\d+/units/\d+/image")
ORIGIN = "http://127.0.0.1:3000"
COUNTS = {"correction": 4, "reject": 1, "merge": 1, "missing_text": 1, "note": 2}
STRICT_CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none'; "
    "base-uri 'none'; form-action 'self'; frame-ancestors 'none'; "
    "require-trusted-types-for 'script'; trusted-types 'none'"
)


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
        self.injected_crops = set()
        self.recovered_crops = set()
        self.busy_crops = set()
        self.nojs_busy = {}
        self.exhausted_crop = None
        # Main-frame documents per tab: a busy crop of a document already left cannot finish
        # its retry, and that is expected (SRC-8).
        self.document = {}
        self.left_busy = set()

    def page(self):
        page = self.context.new_page()
        page.add_init_script(CSP_PROBE)
        page.on(
            "console",
            lambda message: self.consoles.append(
                (message.type, message.text, message.location.get("url", ""), page)
            ),
        )
        page.on("pageerror", lambda error: self.errors.append(str(error)))
        page.on("response", lambda response: self.crop_response(page, response))
        page.on(
            "request",
            lambda request: self.new_document(page)
            if request.is_navigation_request() and request.frame == page.main_frame
            else None,
        )
        self.pages.append(page)
        return page

    def new_document(self, page):
        self.nojs_busy[page] = 0
        self.document[page] = self.document.get(page, 0) + 1
        busy = "Failed to load resource: the server responded with a status of 503 (Service Unavailable)"
        for index, (kind, text, url, tab) in enumerate(self.consoles):
            if (
                tab is page
                and kind == "error"
                and text == busy
                and REGION_CROP.fullmatch(urlsplit(url).path)
            ):
                self.left_busy.add(index)

    def crop_response(self, page, response):
        path = urlsplit(response.url).path
        if not REGION_CROP.fullmatch(path):
            return
        if response.status == 503:
            self.busy_crops.add(response.url)
            self.recovered_crops.discard(path)
            if not self.javascript:
                self.nojs_busy[page] = self.nojs_busy.get(page, 0) + 1

    def crops_ready(self, page):
        if self.javascript:
            wait_until(
                page,
                """({exhausted, contended}) => [...document.querySelectorAll('.source-region img')]
                .filter(i => i.currentSrc).every(i => {
                  const status = i.closest('figure').querySelector('.region-status')?.textContent.trim();
                  // Under contention (one crop job, SRC-8) retries may run out: the documented
                  // fallback is a settled state too. The injected cases prove retry and fallback.
                  const fallback = status ===
                    'Region preview unavailable; inspect the full-page image or open the region link for details.';
                  return (i.complete && i.naturalWidth > 0 && status === 'Region preview loaded.') ||
                    (fallback && (contended || new URL(i.src).pathname === exhausted));
                })""",
                arg={"exhausted": self.exhausted_crop, "contended": len(self.pages) > 1},
                timeout=20000,
            )
            self.recovered_crops.update(
                page.locator(".source-region img").evaluate_all(
                    "images => images.filter(i => (i.complete && i.naturalWidth > 0) ||"
                    " i.closest('figure').querySelector('.region-status')?.textContent.trim()"
                    " === 'Region preview unavailable; inspect the full-page image or open the"
                    " region link for details.')"
                    ".map(i => new URL(i.src).pathname)"
                )
            )
        else:
            require(self.nojs_busy.get(page, 0) <= 60, "Unbounded no-JS busy crops")
            original = page.locator("#review-image > img")
            if original.count():
                require(
                    original.evaluate("i => i.complete && i.naturalWidth > 0"),
                    "No-JS full-page fallback did not load",
                )

    def clean(self, page):
        # Console events are shared across competing tabs; settle every open document first.
        for tab in self.pages:
            self.crops_ready(tab)
        no_overflow(page, f"source review {page.url}")
        console_text = [text for _, text, _, _ in self.consoles]
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
        busy_crop = "Failed to load resource: the server responded with a status of 503 (Service Unavailable)"

        def unexpected():
            return [
                (text, url)
                for index, (kind, text, url, _) in enumerate(self.consoles)
                if kind == "error"
                and index not in self.left_busy
                and not (url in self.expected_conflicts and text == expected_message)
                and not (
                    text == busy_crop
                    and (
                        # Injected by request routing (no server response event is relied on).
                        urlsplit(url).path in self.injected_crops
                        or (
                            url in self.busy_crops
                            and (urlsplit(url).path in self.recovered_crops or not self.javascript)
                        )
                    )
                )
            ]

        # A lazy crop can be requested (and found busy) after the settle above, e.g. when
        # the overflow check scrolls; settle again until only real errors remain.
        for _ in range(3):
            left = unexpected()
            if not left or not self.javascript or any(text != busy_crop for text, _ in left):
                break
            for tab in self.pages:
                self.crops_ready(tab)
        left = unexpected()
        require(not left, f"Console errors: {left}")

    def goto(self, page, path):
        if page.url != "about:blank":
            self.clean(page)
        base = ORIGIN if page.url == "about:blank" else page.url
        response = page.goto(urljoin(base, path), wait_until="load")
        if response is None:
            self.clean(page)
            return
        require(response.status == 200, f"GET {path}: {response.status}")
        if urlsplit(response.url).path.startswith("/admin/materials"):
            require(
                response.headers.get("content-security-policy") == STRICT_CSP,
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
        require(
            response.value.headers.get("content-security-policy") == STRICT_CSP,
            "Material POST lost its exact CSP",
        )
        if status == 409:
            require(page.locator('[role="alert"]').count() > 0, "Conflict has no announced error")
        self.clean(page)


def crop_retries(checks, page, path, units):
    once = f"{path}/units/{units[1]}/image"
    always = f"{path}/units/{units[2]}/image"
    attempts = {once: [], always: []}
    checks.injected_crops.update(attempts)
    checks.exhausted_crop = always

    def route_crop(route):
        crop = urlsplit(route.request.url).path
        if crop not in attempts:
            route.continue_()
            return
        attempts[crop].append(route.request.url)
        if crop == always or len(attempts[crop]) == 1:
            route.fulfill(status=503, headers={"Retry-After": "1"}, body="Synthetic busy crop")
        else:
            route.continue_()

    page.route(REGION_CROP, route_crop)
    checks.goto(page, path)
    for unit in (units[1], units[2]):
        page.locator(f"#unit-{unit} .source-region img").scroll_into_view_if_needed()
    wait_until(
        page,
        """({once, always}) => {
          const status = unit => document.querySelector(`#unit-${unit} .region-status`)?.textContent.trim();
          return status(once) === 'Region preview loaded.' && status(always) ===
            'Region preview unavailable; inspect the full-page image or open the region link for details.';
        }""",
        arg={"once": units[1], "always": units[2]},
        timeout=20000,
    )
    checks.crops_ready(page)
    require(
        len(attempts[once]) >= 2 and "region-retry=1" in attempts[once][1],
        "Busy crop did not make its retry request",
    )
    require(
        page.locator(f"#unit-{units[1]} .source-region img").evaluate(
            "i => i.complete && i.naturalWidth > 0"
        ),
        "Retried crop did not decode",
    )
    require(len(attempts[always]) == 5, f"Retry bound changed: {attempts[always]}")
    require(
        "Region preview unavailable; inspect the full-page image"
        in page.locator(f"#unit-{units[2]} .region-status").inner_text(),
        "Exhausted retry has no fallback",
    )
    checks.clean(page)
    page.unroute(REGION_CROP, route_crop)
    # Leave the deliberately broken document before demanding normal crop recovery again.
    checks.goto(page, "/admin/materials")
    checks.exhausted_crop = None
    checks.injected_crops.clear()


def overlay(checks, page, path, manifest, units, suggestions):
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
        wait_until(
            page,
            "() => { const i = document.querySelector('.source-region img');"
            " return i.complete && i.naturalWidth > 0; }",
        )
    modes = ("fit", "one", "two") if checks.javascript else ("fit",)
    polygons = {
        str(units[index + 1]): unit["polygon"]
        for index, unit in enumerate(manifest["pages"][0]["units"])
    }
    polygons[f"suggestion-{suggestions[6]}"] = [[140, 285], [195, 285], [195, 295], [140, 295]]
    require(page.locator("[data-overlay-suggestion]").count() == 1, "Missing-text overlay absent")
    for mode in modes:
        if checks.javascript:
            page.locator(f'[data-zoom="{mode}"]').click()
            wait_until(
                page,
                "mode => document.querySelector(`[data-zoom=${mode}]`).getAttribute('aria-pressed') === 'true'",
                arg=mode,
            )
        actual = page.locator("#review-overlay").evaluate("""svg => ({
            image: (() => { const r = document.querySelector('#review-image > img').getBoundingClientRect();
                return {x:r.x,y:r.y,width:r.width,height:r.height}; })(),
            boxes: [...svg.querySelectorAll('[data-overlay-unit], [data-overlay-suggestion]')].map(box => {
                const r=box.getBoundingClientRect();
                return {id:box.dataset.overlayUnit || `suggestion-${box.dataset.overlaySuggestion}`,
                    x:r.x,y:r.y,width:r.width,height:r.height};
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
    checks.goto(page, path)
    documents = []
    page.on(
        "request",
        lambda request: documents.append(request.url)
        if request.is_navigation_request() and request.frame == page.main_frame
        else None,
    )
    draft = field(form(page, "unit", units[2]), "rationale")
    draft.fill("Synthetic unsaved fragment draft")
    for link in (
        page.locator(f'[data-overlay-unit="{units[3]}"]').locator(".."),
        page.get_by_role("link", name="Next unreviewed unit", exact=True),
    ):
        href = link.get_attribute("href")
        require(href.startswith("#"), f"Expected same-document link: {href}")
        before = len(documents)
        link.click()
        page.wait_for_url(lambda url, fragment=href[1:]: urlsplit(str(url)).fragment == fragment)
        require(
            len(documents) == before and draft.input_value() == "Synthetic unsaved fragment draft",
            "Fragment navigation lost draft or fetched document",
        )
        checks.clean(page)
    missing = page.locator(f'[data-overlay-suggestion="{suggestions[6]}"]').locator("..")
    href = missing.get_attribute("href")
    # Listed on this page's notes panel: a same-document link to its row; otherwise its detail.
    if href.startswith("#"):
        require(
            href == f"#suggestion-{suggestions[6]}-unit-0" and page.locator(href).count() == 1,
            f"Missing-text box links to the wrong row: {href}",
        )
        before = len(documents)
        missing.click()
        page.wait_for_url(lambda url: urlsplit(str(url)).fragment == href[1:])
        require(len(documents) == before, "Missing-text row link fetched a document")
        detail = f"/admin/materials/suggestions/{suggestions[6]}"
        checks.goto(page, detail)
    else:
        require(
            href == f"/admin/materials/suggestions/{suggestions[6]}",
            f"Missing-text box links to the wrong suggestion: {href}",
        )
        with page.expect_navigation(wait_until="load") as navigation:
            missing.click()
        require(
            navigation.value.status == 200
            and navigation.value.headers.get("content-security-policy") == STRICT_CSP,
            "Missing-text detail lost its status/policy",
        )
    require(
        f"Suggestion #{suggestions[6]} — missing_text" in page.locator("h1").inner_text(),
        "Missing-text box did not open its suggestion detail",
    )
    checks.clean(page)
    for index in (51,):
        checks.goto(page, path)
        box = page.locator(f'[data-overlay-unit="{units[index]}"]')
        link = box.locator("..")
        href = link.get_attribute("href")
        if index == 51:
            require("units=" in href, "Off-card-page overlay link has no units cursor")
        with page.expect_request(
            lambda request: request.is_navigation_request()
            and "units=" in urlsplit(request.url).query
        ):
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
        draw_box(page)
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


def keyboard_boxes(checks, page, path, directory):
    for typed in (False, True):
        checks.goto(page, path)
        owner = form(page, "owner")
        require(field(owner, "polygon").input_value() == "", "Box did not start empty")
        expected = [[50, 75], [100, 75], [100, 150], [50, 150]]
        if typed:
            expected = [[20, 30], [80, 30], [80, 90], [20, 90]]
            field(owner, "polygon").fill(json.dumps(expected))
        button = page.locator("#keyboard-region")
        require("replaces entered polygon" in button.inner_text(), "Replacement warning missing")
        button.focus()
        page.keyboard.press("Enter")
        require(
            json.loads(field(owner, "polygon").input_value()) == expected,
            "Keyboard starter/entered polygon changed",
        )
        drawn_matches(page, expected, "Keyboard starter box")
        top = page.locator('[data-corner="0"]')
        require(
            top.evaluate("node => node === document.activeElement"),
            "Keyboard box did not focus top-left handle",
        )
        page.keyboard.press("ArrowRight")
        page.keyboard.press("ArrowDown")
        page.keyboard.press("Tab")
        require(
            page.locator('[data-corner="2"]').evaluate("node => node === document.activeElement"),
            "Tab did not reach bottom-right handle",
        )
        page.keyboard.press("Shift+ArrowRight")
        page.keyboard.press("ArrowDown")
        final = [
            [expected[0][0] + 1, expected[0][1] + 1],
            [expected[2][0] + 10, expected[0][1] + 1],
            [expected[2][0] + 10, expected[2][1] + 1],
            [expected[0][0] + 1, expected[2][1] + 1],
        ]
        require(
            json.loads(field(owner, "polygon").input_value()) == final,
            "Keyboard corners did not move independently in PDF points",
        )
        drawn_matches(page, final, "Keyboard-moved box")
        text = f"Synthetic keyboard-only owner text typed={typed}"
        field(owner, "text").fill(text)
        field(owner, "rationale").fill("Synthetic keyboard-only comparison")
        checks.submit(page, owner)
        saved = rows(
            directory,
            "SELECT polygon FROM source_units WHERE origin='owner' AND correction=?",
            (text,),
        )
        require(
            len(saved) == 1 and json.loads(saved[0][0]) == final,
            "Keyboard-only polygon not durably stored",
        )


def values(container):
    """Draft controls and hidden tokens; submit buttons are checked by their call sites."""
    return container.evaluate("""form => [...form.elements].filter(e => e.name &&
        e.tagName !== 'BUTTON' && !e.disabled &&
        (!['checkbox','radio'].includes(e.type) || e.checked))
        .map(e => [e.name, e.value])""")


def stored_review(directory):
    return {
        table: rows(directory, f"SELECT * FROM {table} ORDER BY 1")
        for table in (
            "source_units",
            "source_pages",
            "source_review_events",
            "source_web_events",
            "source_search_text",
            "source_primary_pages",
            "source_primary_events",
            "source_primary_web_events",
            "source_suggestions",
            "source_suggestion_applications",
            "source_suggestion_dismissals",
            "source_suggestion_web_events",
        )
    }


def form_conflicts(checks, page, other, path, directory, source, units):
    # Each stale form has a real competing commit, then a complete database snapshot.
    for kind in ("owner", "primary", "primary-pages", "complete"):
        destination = f"/admin/materials/sources/{source}" if kind == "primary-pages" else path
        checks.goto(page, destination)
        draft = form(page, kind)
        if kind == "owner":
            if checks.javascript:
                draw_box(page)
            else:
                field(draft, "polygon").fill("[[20,30],[80,30],[80,90],[20,90]]")
            field(draft, "text").fill("Synthetic owner conflict <b>&\nsecond line")
        if kind == "complete":
            field(draft, "printed_label").fill("Synthetic draft label")
        if kind == "primary-pages":
            field(draft, "page_1").check()
        field(draft, "rationale").fill(f"Synthetic {kind} conflict <b>&\nsecond line")
        submitted = values(draft)
        if kind in ("owner", "complete"):
            decide(checks, other, path, units[10], rationale=f"Competing {kind} revision")
        else:
            checks.goto(other, path)
            competing = form(other, "primary")
            field(competing, "rationale").fill(f"Competing {kind} primary event")
            checks.submit(other, competing)
        before = stored_review(directory)
        checks.submit(page, draft, status=409)
        kept = form(page, kind)
        require(values(kept) == submitted, f"409 lost {kind} values/tokens: {values(kept)}")
        require(stored_review(directory) == before, f"Rejected {kind} changed stored state")
        if kind == "owner" and checks.javascript:
            polygon = json.loads(field(kept, "polygon").input_value())
            rectangle = page.locator("#drawn-region")
            require(rectangle.is_visible(), "409 owner rectangle not restored")
            actual = [float(rectangle.get_attribute(key)) for key in ("x", "y", "width", "height")]
            expected = [
                polygon[0][0],
                polygon[0][1],
                polygon[2][0] - polygon[0][0],
                polygon[2][1] - polygon[0][1],
            ]
            require(actual == expected, f"409 rectangle differs from draft: {actual}")


def drawn_matches(page, polygon, what):
    rectangle = page.locator("#drawn-region")
    require(rectangle.is_visible(), f"{what}: drawn box not visible")
    actual = [float(rectangle.get_attribute(key)) for key in ("x", "y", "width", "height")]
    expected = [
        polygon[0][0],
        polygon[0][1],
        polygon[2][0] - polygon[0][0],
        polygon[2][1] - polygon[0][1],
    ]
    require(actual == expected, f"{what}: drawn box {actual} differs from {expected}")


def draw_box(page):
    page.locator("#draw-region").click()
    page.locator("#review-overlay").scroll_into_view_if_needed()
    box = page.locator("#review-image > img").bounding_box()
    page.mouse.move(box["x"] + box["width"] * 0.2, box["y"] + box["height"] * 0.2)
    page.mouse.down()
    page.mouse.move(box["x"] + box["width"] * 0.4, box["y"] + box["height"] * 0.3, steps=4)
    page.mouse.up()
    require(page.locator("#drawn-region").is_visible(), "Drawn box missing")


def conflicts(checks, page, path, units, suggestions, directory):
    other = checks.page()
    # Unit decision: both pages read revision 1; the first commit invalidates the draft.
    checks.goto(page, path)
    draft = form(page, "unit", units[9])
    field(draft, "state").select_option("corrected")
    field(draft, "correction").fill("Synthetic unit draft <b>&\nsecond line")
    field(draft, "rationale").fill("Synthetic unit draft reason")
    revision = field(draft, "revision").input_value()
    submitted = values(draft)
    decide(checks, other, path, units[9])
    before = stored_review(directory)
    checks.submit(page, draft, status=409)
    kept = form(page, "unit", units[9])
    require(values(kept) == submitted, "409 lost unit values/tokens")
    require(stored_review(directory) == before, "Rejected unit decision changed stored state")
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
    submitted = values(draft)
    decide(checks, other, path, units[8])
    before = stored_review(directory)
    checks.submit(
        page, draft, draft.get_by_role("button", name="Accept my edit", exact=True), status=409
    )
    kept = form(page, "accept", suggestion)
    require(values(kept) == submitted, "409 lost accept/edit values/tokens")
    require(stored_review(directory) == before, "Rejected acceptance changed stored state")
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
    submitted = values(draft)
    checks.goto(other, detail)
    field(form(other, "dismiss", suggestion), "reason").fill("Synthetic competing dismissal")
    checks.submit(other, form(other, "dismiss", suggestion))
    before = stored_review(directory)
    checks.submit(page, draft, status=409)
    require(
        values(form(page, "dismiss", suggestion)) == submitted, "409 lost dismiss values/tokens"
    )
    require(stored_review(directory) == before, "Rejected dismissal changed stored state")
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
    if javascript:
        crop_retries(checks, page, path, units)
    overlay(checks, page, path, manifest, units, suggestions)
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
    if javascript:
        keyboard_boxes(checks, page, path, directory)
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
    conflicts(checks, page, path, units, suggestions, directory)
    form_conflicts(checks, page, checks.page(), path, directory, source, units)
    require(
        rows(directory, "SELECT count(*) FROM source_units WHERE origin='owner'")[0][0]
        == (5 if javascript else 3),
        "Unexpected extra owner units",
    )
    for tab in checks.pages:
        checks.clean(tab)
    if javascript:
        # After every competing tab is closed, crops still load (no fallback allowed).
        for tab in checks.pages[1:]:
            tab.close()
        del checks.pages[1:]
        checks.goto(page, path)
        for index in (1, 2, 3, 4, 5):
            image = page.locator(f"#unit-{units[index]} .source-region img")
            image.scroll_into_view_if_needed()
            wait_until(
                page,
                "unit => { const f = document.querySelector(`#unit-${unit}`);"
                " const i = f.querySelector('.source-region img');"
                " return i.complete && i.naturalWidth > 0 &&"
                " f.querySelector('.region-status')?.textContent.trim() === 'Region preview loaded.'; }",
                arg=units[index],
                timeout=20000,
            )
        checks.clean(page)
    if not javascript:
        print(
            f"No-JS busy crop URLs (L-SRC-11; full-page fallback checked): {len(checks.busy_crops)}"
        )


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
