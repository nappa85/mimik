"""Offline Chromium checks of the shipped artifact, with synthetic inputs only."""

import argparse
import signal
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qsl, urlsplit

from admin_ruleset_browser import run_admin_ruleset_browser
from catalogue_browser import run_catalogue_browser
from material_browser import run_material_browser
from smoke_release import (
    AUTH_DENIED_PATHS,
    prepare_auth_player,
    prepare_unknown_registry_source,
    prepare_workspace_materials,
    publish_assist_definition,
    registry_cli,
    require,
    require_secret_free,
    smoke_accounts,
    smoke_environment,
    start,
    stop,
)


def prepare_browser_registry(binary, directory, fixture):
    prepare_unknown_registry_source(binary, directory, fixture)
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
        "Fixture edition",
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


def no_overflow(page, label):
    sizes = page.evaluate(
        "({width:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth})"
    )
    require(sizes["scroll"] <= sizes["width"] + 2, f"Document overflow on {label}: {sizes}")


def run_scenario(browser, viewport):
    from playwright.sync_api import expect

    context = browser.new_context(viewport=viewport)
    context.set_default_timeout(15_000)
    context.add_init_script("window.__synthetic_xss = false")
    page = context.new_page()
    forbidden, dialogs, errors, posts, navigations = [], [], [], [], []

    def inspect_request(request):
        url = urlsplit(request.url)
        if url.scheme in {"http", "https", "ws", "wss"} and (
            url.hostname not in {"127.0.0.1", "localhost"} or url.port != 3000
        ):
            forbidden.append(request.url)
        if request.method == "POST":
            posts.append((request.url, request.headers))

    page.on("request", inspect_request)
    page.on("websocket", lambda socket: forbidden.append(socket.url))
    page.on("dialog", lambda dialog: (dialogs.append(dialog.message), dialog.dismiss()))
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on(
        "framenavigated",
        lambda frame: navigations.append(frame.url) if frame == page.main_frame else None,
    )
    try:
        page.goto("http://127.0.0.1:3000")
        page.wait_for_function(
            "document.querySelector('.brand img').complete && document.querySelector('.brand img').naturalWidth > 0"
        )
        expect(page.get_by_role("link", name="Mimik home", exact=True)).to_be_visible()
        require(page.locator(".brand").inner_text() == "", "Redundant header title remains")
        require(
            page.locator(".brand img").evaluate(
                "image=>{const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);return ctx.getImageData(0,0,1,1).data[3]===0 && getComputedStyle(image).backgroundColor==='rgba(0, 0, 0, 0)'}"
            ),
            "Logo alpha or transparent CSS background lost",
        )
        name = "LongCharacterName" * 5
        page.locator("#name").fill(name)
        page.locator("#adapter").select_option("mork-borg-it-2025")
        page.get_by_role("button", name="Create character", exact=True).click()
        page.wait_for_url("**/characters/*/mork-borg")
        page.get_by_role("link", name="Identity and notes", exact=False).click()
        character_url = page.url
        notes = "\n".join(["Synthetic long-text line: " + "words " * 20] * 120)
        require(len(notes) < 20_000, "Fixture exceeds valid note size")
        before = page.locator("#notes").evaluate("e=>e.clientHeight")
        page.locator("#notes").fill(notes)
        after = page.locator("#notes").evaluate(
            "e=>({height:e.clientHeight,scroll:e.scrollHeight})"
        )
        require(
            after["height"] > before * 2 and after["scroll"] <= after["height"] + 2,
            "Textarea did not expand without clipping",
        )
        no_overflow(page, "identity with long text")
        page.evaluate("window.__document_probe='kept'")
        count = len(navigations)
        initial = int(page.locator('input[name="revision"]').input_value())
        page.get_by_role("button", name="Save changes", exact=True).click()
        expect(page.get_by_role("status")).to_contain_text("Saved on this device")
        require(
            page.evaluate("window.__document_probe") == "kept" and len(navigations) == count,
            "Save reloaded instead of swapping the fragment",
        )
        require(
            any(
                url == character_url and headers.get("hx-request") == "true"
                for url, headers in posts
            ),
            "Real htmx POST was not observed",
        )
        require(
            int(page.locator('input[name="revision"]').input_value()) == initial + 1,
            "Identity revision increment failed",
        )
        require(page.locator("#notes").input_value() == notes, "Swap changed saved notes")
        page.reload()
        require(page.locator("#notes").input_value() == notes, "Notes did not persist")
        page.get_by_role("link", name="Open MÖRK BORG stats and rules").click()
        page.locator("#strength").fill("-3")
        page.get_by_text("Original worn tier and carrying count", exact=True).click()
        page.locator("#worn_armor").fill("3")
        page.locator("#protection").fill("1")
        page.locator("#standard_items").fill("6")
        page.locator("#description").fill("Synthetic description\n" * 80)
        page.locator("#class_notes").fill("Class notes are not implemented class rules")
        page.locator("#powers").fill("Power notes")
        page.locator("#weapon_one").fill("Sword notes")
        page.locator("#weapon_two").fill("Bow notes")
        page.locator("#campaign_miseries").fill("0")
        page.get_by_role("button", name="Save stats and refresh rules").click()
        expect(page.get_by_role("status")).to_contain_text("Sheet saved")
        require(page.locator("#weapon_two").input_value() == "Bow notes", "Sheet weapon notes lost")
        require(
            page.locator("#campaign_miseries").input_value() == "0",
            "Explicit zero miseries became unknown",
        )
        current_sheet_revision = page.locator('#mork-editor input[name="revision"]').input_value()
        page.locator("#hp").fill("not-a-number")
        page.locator("#weapon_one").fill("Uncommitted weapon draft")
        with page.expect_response(
            lambda response: response.request.method == "POST"
            and response.url.endswith("/mork-borg")
            and response.status == 422
        ):
            page.get_by_role("button", name="Save stats and refresh rules").click()
        expect(page.get_by_role("status")).to_contain_text("Current HP must be an integer")
        require(
            page.locator("#hp").input_value() == "not-a-number",
            "Numeric error draft was not swapped/preserved",
        )
        require(
            page.locator("#weapon_one").input_value() == "Uncommitted weapon draft",
            "Other draft values lost",
        )
        require(
            page.locator('#mork-editor input[name="revision"]').input_value()
            == current_sheet_revision,
            "Error changed expected revision",
        )
        page.reload()
        require(
            page.locator("#hp").input_value() == "1"
            and page.locator("#weapon_one").input_value() == "Sword notes",
            "Invalid save mutated canonical values",
        )
        for extra_name in ["unexpected_field", "hp"]:
            page.locator("#weapon_one").fill("Transport rejection draft")
            page.locator("#mork-editor form").evaluate(
                "(form,name)=>{const field=document.createElement('input');field.type='hidden';field.name=name;field.value='4';form.append(field)}",
                extra_name,
            )
            with page.expect_response(
                lambda response: response.request.method == "POST"
                and response.url.endswith("/mork-borg")
                and response.status == 422
            ):
                page.get_by_role("button", name="Save stats and refresh rules").click()
            expect(page.get_by_role("status")).to_contain_text("Submission format invalid")
            require(
                page.locator("#mork-editor").count() == 1
                and page.locator("#weapon_one").input_value() == "Transport rejection draft",
                "Transport error replaced editor or lost draft",
            )
            require(
                page.locator('#mork-editor input[name="revision"]').input_value()
                == current_sheet_revision,
                "Transport error changed revision",
            )
            page.reload()
            require(
                page.locator("#weapon_one").input_value() == "Sword notes",
                "Transport rejection mutated data",
            )
        for headers, status, message in [
            ({"origin": "https://example.com"}, 403, ""),
            ({"sec-fetch-site": "cross-site"}, 403, ""),
            ({"content-type": "application/json"}, 415, "Submission format invalid"),
        ]:
            page.locator("#weapon_one").fill("Rejected origin/content-type draft")

            def change_headers(route, request, overrides=headers):
                response = route.fetch(headers={**request.headers, **overrides})
                route.fulfill(response=response)

            page.route("**/mork-borg", change_headers)
            with page.expect_response(
                lambda response, expected=status: response.request.method == "POST"
                and response.url.endswith("/mork-borg")
                and response.status == expected
            ):
                page.get_by_role("button", name="Save stats and refresh rules").click()
            expect(page.get_by_role("status")).not_to_be_empty()
            if message:
                expect(page.get_by_role("status")).to_contain_text(message)
            require(
                page.locator("#mork-editor").count() == 1
                and page.locator("#weapon_one").input_value()
                == "Rejected origin/content-type draft"
                and page.locator('#mork-editor input[name="revision"]').input_value()
                == current_sheet_revision,
                "403/415 rejection replaced editor, draft, or revision",
            )
            page.unroute("**/mork-borg", change_headers)
            page.reload()
            require(
                page.locator("#weapon_one").input_value() == "Sword notes",
                "403/415 rejection mutated canonical data",
            )
        # A second tab changes identity; the original sheet draft must remain stale.
        stale_revision = page.locator('#mork-editor input[name="revision"]').input_value()
        other = context.new_page()
        other.goto(character_url)
        other.locator("#name").fill("Updated identity in another tab")
        other.get_by_role("button", name="Save changes", exact=True).click()
        expect(other.get_by_role("status")).to_contain_text("Saved on this device")
        other.close()
        page.locator("#sheet-name").fill("Stale sheet name draft")
        with page.expect_response(
            lambda response: response.request.method == "POST"
            and response.url.endswith("/mork-borg")
            and response.status == 409
        ):
            page.get_by_role("button", name="Save stats and refresh rules").click()
        expect(page.get_by_role("status")).to_contain_text("another tab")
        require(
            page.locator("#sheet-name").input_value() == "Stale sheet name draft",
            "Conflict name draft lost",
        )
        require(
            page.locator('#mork-editor input[name="revision"]').input_value() == stale_revision,
            "Conflict silently refreshed stale token",
        )
        page.reload()
        require(
            page.locator("#sheet-name").input_value() == "Updated identity in another tab",
            "Stale sheet overwrote name",
        )
        expect(page.get_by_text("Defense: d20", exact=False)).to_contain_text("DR 16")
        no_overflow(page, "stats with long name")
        page.get_by_role("link", name="Open contextual base-rule checklist", exact=False).click()
        context_revision = page.locator('input[name="revision"]').input_value()
        for name in [
            "base",
            "can_act",
            "movement_remaining",
            "melee_reach",
            "ranged_ready",
            "ranged_in_range",
            "incoming_attack",
            "scroll_available",
            "blocking_armor",
            "route_clear",
        ]:
            page.locator(f"#{name}").select_option("yes")
        for name in ["action_remaining", "zweihander", "disoriented"]:
            page.locator(f"#{name}").select_option("no")
        page.locator("#power_uses").fill("3")
        page.get_by_role("button", name="Explain the standard options").click()
        expect(page.locator('[data-option="melee"] h2')).to_contain_text("blocked standard path")
        expect(page.locator('[data-option="move"] h2')).to_contain_text("standard check supported")
        expect(page.locator('[data-option="defense"]')).to_contain_text("DR 16")
        require(
            page.locator('input[name="revision"]').input_value() == context_revision,
            "Advice changed sheet revision",
        )
        no_overflow(page, "context checklist")
        page.goto("http://127.0.0.1:3000/sources")
        expect(page.get_by_text("synthetic-game / first", exact=True)).to_be_visible()
        no_overflow(page, "source list with hash")
        page.locator("section.panel").filter(
            has=page.get_by_role("heading", name="Synthetic source", exact=True)
        ).get_by_role("link", name="PDF page 1", exact=False).click()
        page.wait_for_function(
            "document.querySelector('.source-original img').complete && document.querySelector('.source-original img').naturalWidth > 0"
        )
        preview_url = page.locator(".source-region img").first.get_attribute("src")
        attempts = {}

        def transient_region_failure(route):
            key = urlsplit(route.request.url).query.split("&region-retry=")[0]
            attempts[key] = attempts.get(key, 0) + 1
            if attempts[key] == 1:
                route.fulfill(status=503, headers={"Retry-After": "1"}, body="Busy")
            else:
                route.continue_()

        page.route("**/units/*/image?*", transient_region_failure)
        page.locator(".source-region").first.evaluate(
            "(figure,url)=>{for(let i=0;i<3;i++){const clone=figure.cloneNode(true);clone.classList.add('retry-probe');const img=clone.querySelector('img');img.loading='eager';img.src=url+'?probe='+i;figure.parentNode.insertBefore(clone,figure)}}",
            preview_url,
        )
        page.wait_for_function(
            "[...document.querySelectorAll('.retry-probe img')].length===3 && [...document.querySelectorAll('.retry-probe img')].every(img=>img.complete && img.naturalWidth===9)"
        )
        require(
            len(attempts) == 3 and all(value >= 2 for value in attempts.values()),
            "Overlapping busy previews did not recover",
        )
        page.unroute("**/units/*/image?*", transient_region_failure)
        page.locator(".retry-probe").evaluate_all(
            "figures=>figures.forEach(figure=>figure.remove())"
        )
        terminal_attempts = []

        def terminal_region_failure(route):
            terminal_attempts.append(route.request.url)
            route.fulfill(status=422, body="Region unavailable; use full page")

        page.route("**/units/*/image?terminal*", terminal_region_failure)
        page.locator(".source-region").first.evaluate(
            "(figure,url)=>{const clone=figure.cloneNode(true);clone.classList.add('terminal-probe');const img=clone.querySelector('img');img.loading='eager';img.src=url+'?terminal=1';figure.parentNode.insertBefore(clone,figure)}",
            preview_url,
        )
        expect(page.locator(".terminal-probe .region-status")).to_contain_text(
            "unavailable", timeout=15_000
        )
        require(len(terminal_attempts) == 5, "Region retries did not respect four-retry ceiling")
        page.wait_for_timeout(1500)
        require(len(terminal_attempts) == 5, "Exhausted region kept retrying")
        page.unroute("**/units/*/image?terminal*", terminal_region_failure)
        page.locator(".terminal-probe").evaluate_all(
            "figures=>figures.forEach(figure=>figure.remove())"
        )
        page.locator(".source-region img").first.scroll_into_view_if_needed()
        page.wait_for_function(
            "document.querySelector('.source-region img').complete && document.querySelector('.source-region img').naturalWidth === 9 && document.querySelector('.source-region img').naturalHeight === 3"
        )
        with page.expect_download() as download_info:
            page.get_by_role("link", name="Download region PNG", exact=True).first.click()
        with tempfile.TemporaryDirectory(prefix="mimik-region-download-") as directory:
            destination = Path(directory) / "region.png"
            download_info.value.save_as(destination)
            require(
                destination.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"),
                "Region download is not a PNG",
            )
        require(not page.evaluate("window.__synthetic_xss"), "Synthetic XSS executed")
        bad = page.locator(".source-extract").evaluate_all(
            "elements=>elements.some(e=>e.querySelector('script,iframe,object,embed,img,link,style')||[...e.querySelectorAll('*')].some(x=>[...x.attributes].some(a=>a.name.startsWith('on'))))"
        )
        require(not bad, "Forbidden content in sanitized DOM")
        no_overflow(page, "source page")
        search_url = "http://127.0.0.1:3000/sources/search?system=mork-borg&edition=fixture-edition&query=capacity"
        source_url = page.url
        page.goto(search_url)
        expect(page.get_by_text("No reviewed match", exact=False)).to_be_visible()
        page.goto(source_url)
        unit_form = page.locator('form[action*="/units/"]').first
        unit_form.locator('select[name="state"]').select_option("accepted")
        unit_form.locator('input[name="actor"]').fill("synthetic-browser-reviewer")
        unit_form.locator('textarea[name="rationale"]').fill(
            "Synthetic state-machine verification, not real source approval"
        )
        unit_form.get_by_role("button", name="Record decision").click()
        page.goto(search_url)
        expect(page.get_by_text("No reviewed match", exact=False)).to_be_visible()
        page.goto(source_url)
        complete_form = page.locator('form[action="' + urlsplit(source_url).path + '"]')
        complete_form.locator('input[name="printed_label"]').fill("1")
        complete_form.locator('input[name="actor"]').fill("synthetic-browser-reviewer")
        complete_form.locator('textarea[name="rationale"]').fill(
            "Synthetic completeness state check"
        )
        complete_form.get_by_role("button", name="Confirm inspected page completeness").click()
        page.goto(search_url)
        expect(page.locator("pre.evidence-text")).to_contain_text("Strength capacity")
        no_overflow(page, "lookup")
        page.goto("http://127.0.0.1:3000/sources/search")
        page.locator("#system").fill("mork-borg")
        page.locator("#edition-id").fill("fixture")
        page.locator("#query").fill("capacity")
        page.get_by_role("button", name="Look up evidence", exact=True).click()
        expect(page.locator("pre.evidence-text")).to_contain_text("Strength capacity")
        require(
            page.locator("#edition").input_value() == "", "Stable selector guessed a legacy label"
        )
        page.goto(search_url.replace("fixture-edition", "wrong-edition"))
        expect(page.get_by_text("Unsupported source/edition", exact=False)).to_be_visible()
        require(
            not forbidden and not dialogs and not errors,
            f"Browser violations: {forbidden}, {dialogs}, {errors}",
        )
    finally:
        context.close()


FORMULA_DERIVED = ("pace", "echo", "marker")


def unit_save(page, trigger, status=303, path=None, key="Enter"):
    """Saves one unit of a saved runtime sheet (assisted sheet slice 1.5a): Enter in its
    control, or a click on its Save button (`key=None`). The POST goes to the unit's own
    route with exactly csrf, base and value (plus null for a nullable field)."""
    with page.expect_response(lambda response: response.request.method == "POST") as response:
        with page.expect_navigation(wait_until="domcontentloaded"):
            if key is None:
                trigger.click()
            else:
                trigger.press(key)
    request = response.value.request
    keys = [key for key, _ in parse_qsl(request.post_data or "", keep_blank_values=True)]
    require(response.value.status == status, f"Unit save status differs ({request.url})")
    if path is not None:
        require(
            urlsplit(request.url).path.endswith(path)
            and sorted(keys) in (["base", "csrf", "value"], ["base", "csrf", "null", "value"]),
            f"Unit save posted {keys} to {request.url}, not a unit form to {path}",
        )
    return keys


def submitted_keys(request):
    return {key for key, _ in parse_qsl(request.post_data or "", keep_blank_values=True)}


def run_formula_browser(page, campaign, version, submit):
    """Published scalar formula sheet: computed values, advisory checks, reflow, keyboard."""
    page.goto(campaign + f"/sheets/new?version={version}")
    main = page.locator("main")
    require(
        "Derived values are computed after the first save." in main.inner_text(),
        "Formula create page evaluated or hid the first-save note",
    )
    for derived in FORMULA_DERIVED:
        require(
            page.locator(f'[name="value-{derived}"], [name="null-{derived}"]').count() == 0,
            "Derived field rendered as a form control",
        )
    page.locator('input[name="name"]').fill("Formula browser private")
    page.locator('input[name="value-name"]').fill("Formula <b>name</b>")
    page.locator('input[name="value-vitality"]').fill("4")
    page.locator('select[name="null-hit-points"]').select_option("known")
    page.locator('input[name="value-hit-points"]').fill("7")
    page.locator('textarea[name="value-story"]').fill("unbroken" * 400)
    with page.expect_request(lambda request: request.method == "POST") as request:
        submit(page, page.get_by_role("button", name="Save full private sheet", exact=True))
    keys = submitted_keys(request.value)
    require(
        not any(key.endswith(tuple(f"-{derived}" for derived in FORMULA_DERIVED)) for key in keys)
        and "value-hit-points" in keys,
        "Saved formula form carried a derived name",
    )
    formula_sheet = page.url
    pace = page.locator("dl.derived", has=page.locator("#label-pace"))
    require(
        pace.locator("dd").inner_text() == "2" and "Computed" in pace.inner_text(),
        "Derived value missing after save",
    )
    require(
        page.locator("dl.derived", has=page.locator("#label-echo")).locator("dd").inner_text()
        == "unbroken" * 400,
        "Derived long text differs",
    )
    require(
        "0 violated, 0 not evaluated" in page.locator("#advisory-checks").inner_text(),
        "Advisory checks missing after save",
    )
    # Saved: each field saves on its own (slice 1.5a); no full-sheet Save is offered.
    require(
        page.get_by_role("button", name="Save full private sheet").count() == 0,
        "A saved runtime sheet still offers the full-sheet Save",
    )
    page.locator("#v1").fill("11")
    unit_save(page, page.locator("#v1"), path="/fields/vitality")
    checks = page.locator("#advisory-checks")
    require(
        "Violated" in checks.inner_text()
        and "Vitality is at most 10." in checks.inner_text()
        and page.get_by_role("link", name="Advisory checks: 1 violated, 0 not evaluated").count()
        == 1,
        "Violation not visible after a violating save",
    )
    require(
        page.locator("dl.derived", has=page.locator("#label-pace")).locator("dd").inner_text()
        == "-5",
        "Derived value not recomputed on save",
    )
    no_overflow(page, "formula sheet")
    # A multiline field and a nullable field save with their Save button (no Enter).
    story = page.locator("#v3")
    story.fill("first line\nsecond line")
    unit_save(
        page,
        page.get_by_role("button", name="Save Story", exact=True),
        key=None,
        path="/fields/story",
    )
    require(
        page.locator("#v3").input_value() == "first line\nsecond line"
        and page.locator("#v1").input_value() == "11",
        "The multiline field did not save on its own",
    )
    hit_points = page.locator('form[action$="/fields/hit-points"]')
    hit_points.locator('select[name="null"]').select_option("unknown")
    unit_save(
        page,
        page.get_by_role("button", name="Save Hit points", exact=True),
        key=None,
        path="/fields/hit-points",
    )
    require(
        page.locator('form[action$="/fields/hit-points"] select[name="null"]').input_value()
        == "unknown",
        "The nullable field's state did not save with its button",
    )
    no_overflow(page, "formula sheet with unit forms")
    for target in (formula_sheet, formula_sheet + "/history/1"):
        page.goto(target)
        marker = page.locator("details", has=page.locator("pre", has_text="name=="))
        summary = marker.locator("summary")
        summary.focus()
        page.keyboard.press("Enter")
        require(
            marker.evaluate("details => details.open")
            and len(marker.locator("pre").inner_text()) == 1024,
            "Expression disclosure is not keyboard-operable",
        )
        no_overflow(page, "formula sheet with an open 1,024-byte expression")
    require(
        page.locator('input[name="value-vitality"]').input_value() == "4"
        and page.locator('input[name="value-vitality"]').is_disabled()
        and page.locator("dl.derived", has=page.locator("#label-pace")).locator("dd").inner_text()
        == "2",
        "Formula history did not recompute the historical revision",
    )


def run_list_browser(page, campaign, version, submit):
    """Published list sheet: draft ops by pointer and keyboard on the create page (M4); once
    saved (assisted sheet slice 1.5b), Enter in a row cell saves that cell alone, the
    new-row form adds a row, Move works without script, Remove shows "Row removed · Undo"
    and Undo restores the row; change summary and read-only history."""

    def active():
        # Autofocus runs at the next rendering opportunity, after DOMContentLoaded.
        try:
            page.wait_for_function(
                "document.activeElement && document.activeElement !== document.body",
                timeout=3_000,
            )
        except Exception:  # noqa: BLE001 - report what has focus instead
            pass
        return page.evaluate("document.activeElement ? document.activeElement.id : ''")

    def press(key, status=200):
        with page.expect_response(lambda response: response.request.method == "POST") as response:
            with page.expect_navigation(wait_until="domcontentloaded"):
                page.keyboard.press(key)
        require(response.value.status == status, f"List keyboard op status differs ({key})")
        return response.value.request

    def button(name):
        return page.get_by_role("button", name=name, exact=True)

    def posted(request, path, keys):
        sent = sorted(key for key, _ in parse_qsl(request.post_data or "", keep_blank_values=True))
        require(
            urlsplit(request.url).path.endswith(path) and sent == sorted(keys),
            f"List save posted {sent} to {request.url}, not {keys} to {path}",
        )

    def click(name, path, keys, status=303):
        with page.expect_response(lambda response: response.request.method == "POST") as response:
            with page.expect_navigation(wait_until="domcontentloaded"):
                button(name).click()
        require(response.value.status == status, f"List button {name} status differs")
        posted(response.value.request, path, keys)

    page.goto(campaign + f"/sheets/new?version={version}")
    require(page.locator("script").count() == 0, "List sheet page carries a script")
    require("No rows yet." in page.locator("#list-1").inner_text(), "Empty list note missing")
    # Draft operations on the create page (the full form): Add before Name is filled
    # (formnovalidate), focus, moves by keyboard, edge buttons, Remove.
    submit(page, button("Add Equipment row"), 200)
    focused = active()
    require(focused == "row-1-0-0", f"Add did not focus the new row's first control ({focused!r})")
    require(
        "Row added to Equipment. Not saved yet." in page.get_by_role("status").inner_text(),
        "Add status message missing",
    )
    page.locator('input[name="name"]').fill("List browser private")
    page.locator('input[name="value-name"]').fill("Lister")
    page.locator('input[name="row-1-0-0"]').fill("Rope")
    page.locator('input[name="row-1-0-2"]').fill("2")
    page.locator('input[name="row-1-0-3"]').fill("3")
    page.locator('select[name="row-1-0-4"]').select_option("false")
    submit(page, button("Add Equipment row"), 200)
    page.locator('input[name="row-1-1-0"]').fill("Torch")
    page.locator('input[name="row-1-1-2"]').fill("1")
    page.locator('input[name="row-1-1-3"]').fill("1")
    page.locator('select[name="row-1-1-4"]').select_option("true")
    # Leading newlines survive every round trip (a parser drops the first newline after
    # <textarea>; the template emits a sacrificial one).
    page.locator('select[name="set-1-0-5"]').select_option("present")
    page.locator('textarea[name="row-1-0-5"]').fill("\n\nkept note")
    page.locator('textarea[name="row-1-1-5"]').focus()
    page.keyboard.press("Tab")
    require(active() == "up-1-1", "Tab did not reach Move row 2 up")
    press("Enter")
    require(active() == "down-1-0", "Move up to the top did not focus its Move down")
    require(
        page.locator('input[name="row-1-0-0"]').input_value() == "Torch",
        "Move up did not reorder the draft",
    )
    require(
        page.locator("#up-1-0").is_disabled() and page.locator("#down-1-1").is_disabled(),
        "Edge move buttons are not disabled",
    )
    press("Space")
    require(active() == "up-1-1", "Move down to the end did not focus its Move up")
    require(
        page.locator('input[name="row-1-1-0"]').input_value() == "Torch"
        and page.locator('textarea[name="row-1-0-5"]').input_value() == "\n\nkept note",
        "Move down did not carry the row and its note",
    )
    submit(page, button("Add Equipment row"), 200)
    submit(page, button("Remove row 3 of Equipment"), 200)
    require(active() == "add-1", "Remove did not focus the Add button")
    # Enter in a text field uses the first submitter: the top Save.
    page.locator('input[name="row-1-0-0"]').focus()
    with page.expect_request(lambda request: request.method == "POST") as request:
        press("Enter", 303)
    require(
        ("op", "save") in parse_qsl(request.value.post_data or "", keep_blank_values=True),
        "Enter did not save through the op=save button",
    )
    list_sheet = page.url
    require(
        "ID r1-1" in page.locator("#row-1-0 legend").inner_text()
        and "ID r1-2" in page.locator("#row-1-1 legend").inner_text(),
        "Saved rows did not get the host IDs r1-1 and r1-2",
    )
    load = page.locator("dl.derived", has=page.locator("#label-load")).locator("dd")
    require(load.inner_text() == "7", "List-derived load missing after save")
    require(
        page.get_by_role("button", name="Save list Equipment").count() == 0
        and page.get_by_role("button", name="Add Equipment row").count() == 0,
        "A saved list still has its list form",
    )
    # Slice 1.5b: Enter in row 2's Quantity saves that cell alone (csrf, base, value).
    quantity = page.locator("#row-1-1-2")
    quantity.fill("4")
    with page.expect_request(lambda request: request.method == "POST") as request:
        with page.expect_navigation(wait_until="domcontentloaded"):
            quantity.press("Enter")
    posted(request.value, "/cells/1/1/2", ["csrf", "base", "value"])
    require(
        load.inner_text() == "10" and page.locator("#row-1-0-2").input_value() == "2",
        "The Quantity cell did not save alone",
    )
    require(
        page.locator("#row-1-0-5").input_value() == "\n\nkept note",
        "Leading newlines lost after a save",
    )
    # A multiline cell saves with its Save button, named by its cell and row: a leading
    # newline and 3,000 characters.
    long_note = "\n" + "long note " * 300
    page.locator("#set-1-1-5").select_option("present")
    page.locator("#row-1-1-5").fill(long_note)
    click("Save Item note Equipment Row 2", "/cells/1/1/5", ["csrf", "base", "set", "value"])
    require(
        page.locator("#row-1-1-5").input_value() == long_note,
        "The multiline cell did not save with its button",
    )
    no_overflow(page, "list sheet with a 3,000-character note")
    # The new-row form: Enter in its first control adds the row (lists/1/rows).
    page.locator("#row-new-1-0").fill("Lamp")
    page.locator("#row-new-1-2").fill("1")
    page.locator("#row-new-1-3").fill("0")
    page.locator("#row-new-1-4").select_option("false")
    with page.expect_request(lambda request: request.method == "POST") as request:
        with page.expect_navigation(wait_until="domcontentloaded"):
            page.locator("#row-new-1-0").press("Enter")
    posted(
        request.value,
        "/lists/1/rows",
        ["csrf", "base", "row-0", "set-1", "row-1", "row-2", "row-3", "row-4", "set-5", "row-5"],
    )
    require(
        page.locator("#row-1-2-0").input_value() == "Lamp"
        and "ID r4-1" in page.locator("#row-1-2 legend").inner_text()
        and load.inner_text() == "10",
        "The new row was not added with the host ID r4-1",
    )
    no_overflow(page, "list sheet with cell forms, row actions and a new-row form")
    # Move without script, by keyboard: Lamp up to row 2 (csrf and base only).
    page.locator("#up-1-2").focus()
    request = press("Enter", 303)
    posted(request, "/lists/1/rows/2/up", ["csrf", "base"])
    # The moved row's Move up keeps focus, so the next Enter moves it on (review round 1).
    focused = active()
    require(focused == "up-1-1", f"Move up did not focus the moved row's Move up ({focused!r})")
    require(
        page.locator("#row-1-1-0").input_value() == "Lamp"
        and page.locator("#row-1-2-0").input_value() == "Torch",
        "Move up did not reorder the saved rows",
    )
    require(
        page.locator("#up-1-0").is_disabled() and page.locator("#down-1-2").is_disabled(),
        "Saved edge move buttons are not disabled",
    )
    # Remove happens at once, with "Row removed · Undo" right after (D-V); Undo restores it.
    click("Remove Row 2 Equipment", "/lists/1/rows/1/remove", ["csrf", "base"])
    removed = page.locator("#removed-1")
    require(
        removed.inner_text().strip() == "Row removed · Undo"
        and page.locator("#row-1-2").count() == 0,
        "Remove did not remove the row with Undo offered",
    )
    no_overflow(page, "list sheet after Remove")
    click("Undo Row removed Equipment", "/undo", ["csrf", "base", "note"])
    require(
        page.locator("#row-1-1-0").input_value() == "Lamp"
        and page.locator("#removed-1").count() == 0
        and "ID r7-1" in page.locator("#row-1-1 legend").inner_text(),
        "Undo did not restore the removed row with a new host ID",
    )
    # 390 px: the 32-property Traits row through its new-row form.
    for p in range(32):
        control = page.locator(f"#row-new-2-{p}")
        tag = control.evaluate("element => element.tagName")
        if tag == "SELECT":
            control.select_option(index=1)
        else:
            control.fill("1" if p % 5 == 1 else "trait")
    no_overflow(page, "list sheet with a 32-property new row")
    click(
        "Add row Traits",
        "/lists/2/rows",
        ["csrf", "base"] + [f"row-{p}" for p in range(32)] + [f"set-{p}" for p in range(0, 32, 3)],
    )
    require(
        page.locator("#row-2-0-1").input_value() == "1" and load.inner_text() == "10",
        "The Traits row was not added on its own",
    )
    page.reload()
    require(
        page.locator("#row-1-0-5").input_value() == "\n\nkept note",
        "Leading newlines lost after a reload",
    )
    # Then a violation through a cell (revision 9).
    page.locator("#row-1-2-2").fill("9")
    with page.expect_navigation(wait_until="domcontentloaded"):
        page.locator("#row-1-2-2").press("Enter")
    require(
        "Load is at most 10." in page.locator("#advisory-checks").inner_text(),
        "List advisory violation not shown",
    )
    page.goto(list_sheet + "/history/9")
    summary = page.locator("section#change-summary")
    require(
        summary.get_by_role("heading", name="Changes in revision 9").count() == 1
        and "equipment/r1-2/quantity" in summary.inner_text(),
        "History change summary missing",
    )
    require(
        page.get_by_role("button").count() == 0
        and page.locator('input[name="row-1-0-0"]').is_disabled(),
        "List history is not read-only",
    )
    no_overflow(page, "list history with its change summary")


def run_assist_browser(page, campaign, version):
    """Assisted sheet slices 1.1 to 1.4: a sum typed into Current HP saves its result and
    shows "84 − 15 = 69" next to the cell, a saved negative survives the re-post, an
    unreadable entry keeps the value with its scribble (inside `<bdi>`), an unchanged save
    writes nothing (same revision), integer inputs keep the plain keyboard (no `inputmode`,
    which on phones hides minus and space), "-15" is applied literally with the
    "Did you mean" switch, which can be hidden, applied and undone, a blanked number is
    unknown ("?") with Undo, "lots" in a new row's Quantity keeps the row as unknown
    with its scribble while Load says it is not computed, and (slice 1.6) "3 torches" and
    "healing potion x2" in the quick-add line each add a row."""

    def enter(field, status=303):
        with page.expect_response(lambda response: response.request.method == "POST") as response:
            with page.expect_navigation(wait_until="domcontentloaded"):
                field.press("Enter")
        require(response.value.status == status, "Assisted sheet Enter status differs")

    def press(name, status=303):
        button = page.get_by_role("button", name=name, exact=True)
        with page.expect_response(lambda response: response.request.method == "POST") as response:
            with page.expect_navigation(wait_until="domcontentloaded"):
                button.click()
        require(response.value.status == status, f"Assisted sheet button {name} status differs")

    def current_hp():
        # By id after the first save: the switch and Undo buttons name themselves with the
        # cell's label (aria-labelledby), which get_by_label also matches. Saved, HP is the
        # control of its own form (slice 1.5a), with the DOM id of its field index.
        return page.locator("#v0")

    def revision():
        return page.locator('form[action$="/fields/hp"] input[name="base"]').input_value()

    def note():
        notes = page.locator("#note-0")
        return notes.inner_text().strip() if notes.count() else None

    page.goto(campaign + f"/sheets/new?version={version}")
    page.locator('input[name="name"]').fill("Assisted browser private")
    page.locator('input[name="value-max_hp"]').fill("84")
    page.locator('input[name="value-armour"]').fill("-1")
    page.locator('select[name="null-luck"]').select_option("unknown")
    hp = page.get_by_label("Current HP", exact=True)
    hp.fill("84")
    enter(hp)
    assisted = page.url
    for key in ("v0", "v1", "v2", "v3"):
        require(
            page.locator(f"#{key}").get_attribute("inputmode") is None,
            f"Runtime integer input {key} restricts the phone keyboard",
        )
    require(
        page.get_by_role("button", name="Save full private sheet").count() == 0
        and page.locator('form[action$="/fields/hp"]').get_by_role("button").count() == 1,
        "Current HP is not in its own form with one Save button",
    )
    # D-AA without JavaScript: leaving a changed field sends nothing; Enter is the only way.
    posted = []
    page.on("request", lambda request: posted.append(request) if request.method == "POST" else None)
    current_hp().fill("71")
    current_hp().press("Tab")
    page.wait_for_timeout(500)
    require(not posted, "Leaving a changed field posted without JavaScript")
    page.goto(assisted)
    require(current_hp().input_value() == "84", "A field left without Enter was saved")
    # Slice 1.5a: Enter in Current HP saves HP alone (csrf, base, value to fields/hp).
    page.locator("#v2").fill("7")
    hp = current_hp()
    hp.fill("")
    hp.press_sequentially("84 -15")
    unit_save(page, hp, path="/fields/hp")
    page.goto(assisted)
    require(
        current_hp().input_value() == "69" and page.locator("#v2").input_value() == "-1",
        "The typed sum 84 -15 did not save 69 alone, or the saved negative changed",
    )
    require(
        "0 violated, 0 not evaluated" in page.locator("#advisory-checks").inner_text(),
        "Assisted sheet advisory check differs",
    )
    # Slice 1.7 (RR-3): every check passed, so no validity flag.
    require(page.locator("#validity").count() == 0, "A passing assisted sheet is flagged")
    require(note() == "84 \u2212 15 = 69", "The sum note 84 − 15 = 69 is not shown")
    no_overflow(page, "assisted sheet after a typed sum")
    # Slice 1.2: an unreadable entry keeps the saved value and shows the scribble.
    hp = current_hp()
    hp.fill("lots")
    enter(hp)
    page.goto(assisted)
    require(
        note() == "kept 69 \u00b7 \u2018lots\u2019 not understood"
        and page.locator("#note-0 bdi").inner_text() == "lots"
        and current_hp().input_value() == "lots",
        "The scribble lots did not keep 69 with its note",
    )
    no_overflow(page, "assisted sheet with a scribble")
    # Pressing Enter on the unchanged page writes nothing (D-U): the revision stays.
    unchanged = revision()
    hp = current_hp()
    enter(hp, status=200)
    require(
        "No change" in page.locator('[role="status"]').inner_text(),
        "An unchanged assisted save did not say No change",
    )
    page.goto(assisted)
    require(revision() == unchanged, "An unchanged assisted save wrote a revision")
    # A later number replaces the scribble.
    hp = current_hp()
    hp.fill("70")
    enter(hp)
    page.goto(assisted)
    require(
        note() is None and current_hp().input_value() == "70",
        "A new number did not replace the scribble",
    )
    # Slice 1.3: "-15" is applied literally (D-Q) and the switch is offered.
    hp = current_hp()
    hp.fill("-15")
    enter(hp)
    page.goto(assisted)
    offer = page.locator("#offer-0")
    require(
        current_hp().input_value() == "-15"
        and "Did you mean 70 \u2212 15 = 55?" in offer.inner_text(),
        "The sign-led entry -15 was not applied literally with the switch offered",
    )
    no_overflow(page, "assisted sheet with a switch offer")
    require(
        page.get_by_role("textbox", name="Current HP", exact=True).count() == 1,
        "Current HP lost its label name next to the switch and Undo buttons",
    )
    # D-X4: the offer is non-blocking; Hide removes it without script, a reload brings it back.
    page.locator('label[for="dismiss-0"]').click()
    require(not offer.is_visible(), "Hide did not hide the switch offer")
    page.goto(assisted)
    require(page.locator("#offer-0").is_visible(), "The offer did not come back on reload")
    # The buttons are named by reference to the cell's label (aria-labelledby).
    press("Yes Current HP")
    page.goto(assisted)
    require(
        current_hp().input_value() == "55"
        and note() == "70 \u2212 15 = 55"
        and page.locator("#offer-0").count() == 0,
        "The switch did not store 70 - 15 = 55",
    )
    require(
        page.locator("#was-0").inner_text() == "Last change: was -15",
        "Undo does not say what it restores",
    )
    press("Undo Current HP")
    page.goto(assisted)
    require(
        current_hp().input_value() == "-15",
        "Undo did not restore -15",
    )
    no_overflow(page, "assisted sheet after Undo")
    # Slice 1.4: a blank number is erased to unknown, shown as "?", and can be undone (D-X1).
    hp = current_hp()
    hp.fill("")
    enter(hp)
    page.goto(assisted)
    checks = page.locator("#advisory-checks").inner_text()
    require(
        current_hp().input_value() == ""
        and current_hp().get_attribute("placeholder") == "?"
        and current_hp().get_attribute("aria-describedby") == "unknown-hint"
        and page.locator("#unknown-hint").text_content() == "Unknown value"
        and page.locator("#was-0").inner_text() == "Last change: was -15"
        and "1 not evaluated" in checks
        and "a value it uses is unknown (null)" in checks,
        "The blanked Current HP is not unknown with Undo",
    )
    # Slice 1.7 (RR-3): the check that reads the unknown HP was not evaluated, so the header
    # says "Not validated" in text (not colour alone), and the value can still be saved.
    flag = page.locator("#validity")
    require(
        flag.is_visible()
        and flag.locator("strong").inner_text() == "Not validated"
        and "1 check not evaluated" in flag.inner_text()
        and "never blocks saving" in flag.inner_text(),
        "The validity flag is missing for a check that was not evaluated",
    )
    no_overflow(page, "assisted sheet with the validity flag")
    no_overflow(page, "assisted sheet with an erased value")
    press("Undo Current HP")
    page.goto(assisted)
    require(current_hp().input_value() == "-15", "Undo did not restore the erased -15")
    # The nullable Luck saves its Known/Unknown state and value with its Save button.
    luck = page.locator('form[action$="/fields/luck"]')
    luck.locator('select[name="null"]').select_option("known")
    page.locator("#v3").fill("3")
    unit_save(
        page,
        page.get_by_role("button", name="Save Luck", exact=True),
        key=None,
        path="/fields/luck",
    )
    require(
        page.locator("#v3").input_value() == "3" and current_hp().input_value() == "-15",
        "Luck did not save on its own with its button",
    )
    no_overflow(page, "assisted sheet unit forms")
    # "lots" in a new row's Quantity keeps the row: unknown, with the scribble (D-M). The
    # row comes from the list's new-row form (slice 1.5b).
    page.locator("#row-new-4-0").fill("Torch")
    page.locator("#row-new-4-2").fill("1")
    quantity = page.locator("#row-new-4-1")
    quantity.fill("lots")
    enter(quantity)
    page.goto(assisted)
    row_note = page.locator("#note-4-0-1")
    require(
        page.locator("#row-4-0-1").input_value() == "lots"
        and row_note.count() == 1
        and row_note.inner_text().strip() == "? (unknown) \u00b7 \u2018lots\u2019 not understood"
        and page.locator("#note-4-0-1 bdi").inner_text() == "lots",
        "The new row's unreadable Quantity is not unknown with its scribble",
    )
    require(
        page.locator("dl.derived dd").inner_text().strip()
        == "Not computed: a value it uses is unknown (null). null_value",
        "Load does not say a value it uses is unknown",
    )
    no_overflow(page, "assisted sheet with an unknown row value")

    # Slice 1.6: "3 torches" + Enter in Equipment's quick-add line adds torches x3 (the
    # weight unknown) and comes back with the line focused; "healing potion x2" adds two.
    def focused():
        # Autofocus runs at the next rendering opportunity, after DOMContentLoaded.
        try:
            page.wait_for_function(
                "document.activeElement && document.activeElement !== document.body",
                timeout=3_000,
            )
        except Exception:  # noqa: BLE001 - report what has focus instead
            pass
        return page.evaluate("document.activeElement ? document.activeElement.id : ''")

    for typed, r, name, quantity in (
        ("3 torches", 1, "torches", "3"),
        ("healing potion x2", 2, "healing potion", "2"),
    ):
        line = page.get_by_role("textbox", name="Quick add Equipment", exact=True)
        line.fill(typed)
        enter(line)
        require(
            page.url.endswith("?focus=quick-4")
            and focused() == "quick-4"
            and page.locator("#quick-4").input_value() == "",
            f"The quick-add line is not focused and empty after {typed!r}",
        )
        require(
            page.locator(f"#row-4-{r}-0").input_value() == name
            and page.locator(f"#row-4-{r}-1").input_value() == quantity
            and page.locator(f"#row-4-{r}-2").input_value() == ""
            and page.locator(f"#note-4-{r}-1").count() == 0,
            f"Quick add {typed!r} did not add {name} x{quantity} with an unknown weight",
        )
    require(
        page.get_by_role("button", name="Add Equipment", exact=True).count() == 1,
        "The quick-add button is not named Add Equipment",
    )
    no_overflow(page, "assisted sheet after a quick-add")
    # Review round 1: a refused entry with a line break comes back in a textarea, so the
    # control really holds the break and a retry posts what was typed (an input strips it).
    typed = "3 torches\nand rope"
    with page.expect_response(lambda response: response.request.method == "POST") as response:
        with page.expect_navigation(wait_until="domcontentloaded"):
            page.evaluate(
                """(typed) => {
                    const form = document.querySelector('form.quick-add');
                    const area = document.createElement('textarea');
                    area.name = 'text';
                    area.value = typed;
                    form.querySelector('#quick-4').replaceWith(area);
                    form.requestSubmit();
                }""",
                typed,
            )
    kept = page.locator("#quick-4")
    require(
        response.value.status == 422
        and kept.evaluate("element => element.tagName") == "TEXTAREA"
        and kept.input_value() == typed
        and focused() == "quick-4",
        "A refused quick-add entry with a line break was not kept with its break",
    )
    no_overflow(page, "assisted sheet with a refused multi-line quick-add entry")


SHEET_CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none'; "
    "base-uri 'none'; form-action 'self'; frame-ancestors 'none'; "
    "require-trusted-types-for 'script'; trusted-types 'none'"
)

CSP_PROBE = """
window.__csp = [];
document.addEventListener('securitypolicyviolation', (event) => {
  window.__csp.push(`${event.violatedDirective} ${event.blockedURI}`);
});
"""


def run_assist_js_browser(page, campaign, version):
    """Step 2a with JavaScript: the sheet module saves Name, fields and cells without a
    reload, on Enter and when a changed field is left (D-AA), answers in JSON with exact
    integers, keeps drafts typed while a save is on its way, serialises saves per unit,
    orders derived values and checks by revision, keeps the other units' `base`, and runs
    under the strict sheet CSP with no violation."""
    context = page.context
    consoles, errors, navigations = [], [], []
    page.add_init_script(CSP_PROBE)
    page.on(
        "console",
        lambda message: consoles.append(message.text) if message.type == "error" else None,
    )
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on(
        "framenavigated",
        lambda frame: navigations.append(frame.url) if frame == page.main_frame else None,
    )
    posts = []
    page.on("request", lambda request: posts.append(request) if request.method == "POST" else None)

    def until(tab, expression):
        # Polled from here: Playwright's own polling evaluates strings, which the sheet's
        # CSP (rightly) refuses.
        for _ in range(300):
            if tab.evaluate(expression):
                return
            tab.wait_for_timeout(50)
        raise AssertionError(f"Timed out waiting for {expression}")

    def ready(tab):
        until(tab, "document.documentElement.dataset.sheetAssist === 'ready'")

    def settled(tab):
        until(tab, "!document.querySelector('form[aria-busy]')")

    def base(tab, action):
        return int(tab.locator(f'form[action$="{action}"] input[name="base"]').input_value())

    def saved_by(tab, trigger, action, status=200):
        """Runs `trigger`, which must post exactly one JSON save to `action`, without a
        reload; returns the response."""
        before, moved = len(posts), len(navigations)
        with tab.expect_response(
            lambda response: response.request.method == "POST"
            and urlsplit(response.url).path.endswith(action)
        ) as response:
            trigger()
        settled(tab)
        tab.wait_for_timeout(300)
        request = response.value.request
        require(
            response.value.status == status
            and request.headers.get("accept") == "application/json"
            and request.headers.get("content-type", "").startswith(
                "application/x-www-form-urlencoded"
            )
            and response.value.headers.get("content-type") == "application/json"
            and response.value.headers.get("content-security-policy") == SHEET_CSP,
            f"The module's save to {action} differs ({response.value.status})",
        )
        if tab is page:
            require(len(posts) - before == 1, f"Saving {action} posted {len(posts) - before} times")
            require(len(navigations) == moved, f"Saving {action} reloaded the page")
        return response.value

    def quiet(trigger, label):
        before = len(posts)
        trigger()
        page.wait_for_timeout(500)
        require(len(posts) == before, f"{label} posted a save")

    page.goto(campaign + f"/sheets/new?version={version}")
    page.locator('input[name="name"]').fill("Assisted JavaScript private")
    page.locator('input[name="value-max_hp"]').fill("84")
    page.locator('input[name="value-armour"]').fill("-1")
    page.locator('select[name="null-luck"]').select_option("unknown")
    hp = page.get_by_label("Current HP", exact=True)
    hp.fill("84")
    with page.expect_navigation(wait_until="domcontentloaded"):
        hp.press("Enter")
    sheet = page.url.split("#")[0]
    response = page.goto(sheet)
    require(
        response.headers.get("content-security-policy") == SHEET_CSP,
        "The sheet page lacks the strict CSP",
    )
    ready(page)
    require(
        page.locator('script[type="module"][src="/assets/sheet-assist.js"]').count() == 1
        and page.locator("script").count() == 1
        and page.locator("[style]").count() == 0,
        "The sheet page loads other scripts or has inline styles",
    )
    hp = page.locator("#v0")
    start = base(page, "/fields/hp")

    # D-AA: Tab out of a changed HP saves once, without a reload, and shows the result.
    hp.fill("84 -15")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    require(
        hp.input_value() == "69"
        and page.locator("#note-0").inner_text() == "84 − 15 = 69"
        and page.locator("#note-0 bdi").inner_text() == "84 − 15"
        and page.locator("#was-0").inner_text() == "Last change: was 84"
        and base(page, "/fields/hp") == start + 1,
        "Leaving a changed HP did not save 84 -15 as 69 with its note and Undo",
    )
    require(
        page.locator('main [role="status"]').inner_text() == "Current HP saved.",
        "The live region did not announce the save",
    )
    # Retyping the saved sum is compared with the acknowledged 69, not the raw last
    # request, so leaving the field sends it again (as Enter would).
    hp.fill("84 -15")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    hp.fill("69")
    # Tab out of an unchanged HP sends nothing.
    quiet(lambda: (hp.focus(), hp.press("Tab")), "Leaving an unchanged HP")
    # Enter followed by leaving the field saves once; focus stays in the cell.
    hp.fill("70")
    saved_by(page, lambda: hp.press("Enter"), "/fields/hp")
    require(
        page.evaluate("document.activeElement.id") == "v0" and hp.input_value() == "70",
        "Enter moved the focus out of the cell",
    )
    quiet(lambda: hp.press("Tab"), "Leaving HP after Enter")
    # Save clicked right after typing: the save starts on leaving the field, and its answer
    # returns focus to the cell.
    armour_field = page.locator("#v2")
    armour_field.fill("-2")
    saved_by(
        page,
        lambda: page.get_by_role("button", name="Save Armour modifier", exact=True).click(),
        "/fields/armour",
    )
    require(
        page.evaluate("document.activeElement.id") == "v2",
        "Clicking Save did not return focus to the cell",
    )
    # A Save clicked while an earlier save is on its way is queued with its button, so its
    # answer returns focus to the cell too.
    queue = []
    page.route("**/fields/armour", lambda route: queue.append(route))

    def wait_queue(count):
        for _ in range(100):
            if len(queue) >= count:
                return
            page.wait_for_timeout(50)
        raise AssertionError(f"Armour save {count} was not sent")

    armour_field.fill("-3")
    armour_field.press("Tab")
    wait_queue(1)
    armour_field.fill("-4")
    page.get_by_role("button", name="Save Armour modifier", exact=True).click()
    queue[0].continue_()
    wait_queue(2)
    queue[1].continue_()
    settled(page)
    page.unroute("**/fields/armour")
    require(
        page.evaluate("document.activeElement.id") == "v2" and armour_field.input_value() == "-4",
        "A queued Save did not return focus to the cell",
    )

    # The switch offer and Undo come with the answer and post through their own forms.
    hp.fill("-15")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    offer = page.locator("#offer-0")
    require(
        hp.input_value() == "-15"
        and "Did you mean 70 − 15 = 55?" in offer.inner_text()
        and hp.get_attribute("aria-describedby") == "offer-0",
        "The sign-led -15 did not offer the switch",
    )
    for name in ("Yes Current HP", "Undo Current HP"):
        size = page.get_by_role("button", name=name, exact=True).bounding_box()
        require(size["width"] >= 24 and size["height"] >= 24, f"The {name} button is under 24 px")
    no_overflow(page, "JavaScript sheet with a switch offer")
    csp_clean(page, consoles, errors)
    with page.expect_navigation(wait_until="domcontentloaded"):
        page.get_by_role("button", name="Yes Current HP", exact=True).click()
    ready(page)
    require(page.locator("#v0").input_value() == "55", "The JavaScript-built switch did not apply")
    hp = page.locator("#v0")
    hp.fill("50")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    with page.expect_navigation(wait_until="domcontentloaded"):
        page.get_by_role("button", name="Undo Current HP", exact=True).click()
    ready(page)
    hp = page.locator("#v0")
    require(hp.input_value() == "55", "The JavaScript-built Undo did not restore 55")

    # 9007199254740993 round-trips exactly (integers are strings in the answer).
    armour = page.locator("#v2")
    armour.fill("9007199254740993")
    saved_by(page, lambda: armour.press("Tab"), "/fields/armour")
    require(armour.input_value() == "9007199254740993", "A large integer lost precision")
    page.reload()
    ready(page)
    require(
        page.locator("#v2").input_value() == "9007199254740993",
        "A large integer was not stored exactly",
    )
    hp, armour = page.locator("#v0"), page.locator("#v2")

    # A row from the quick-add line (still a page post), then the derived Load and the
    # check move with each save, and the validity flag follows.
    line = page.get_by_role("textbox", name="Quick add Equipment", exact=True)
    line.fill("2 rope")
    with page.expect_navigation(wait_until="domcontentloaded"):
        line.press("Enter")
    ready(page)
    hp, armour = page.locator("#v0"), page.locator("#v2")
    weight = page.locator("#row-4-0-2")
    load = page.locator("#label-load + dd")
    require(
        load.inner_text().startswith("Not computed")
        and "1 derived value not computed" in page.locator("#validity").inner_text(),
        "The unknown weight does not flag Load",
    )
    weight.fill("3")
    saved_by(page, lambda: weight.press("Tab"), "/cells/4/0/2")
    require(
        load.inner_text() == "6"
        and page.locator("#not-computed").count() == 0
        and page.locator("#validity").count() == 0,
        "Saving the weight did not update Load and the flag",
    )
    hp.fill("90")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    flag = page.locator("#validity")
    revision = base(page, "/fields/hp")
    require(
        "1 check violated" in flag.inner_text()
        and f"(saved revision {revision})" in flag.inner_text()
        and page.locator("#checks-counts").inner_text() == "1 violated, 0 not evaluated, 0 passed."
        and page.locator("#checks-link").inner_text()
        == "Advisory checks: 1 violated, 0 not evaluated"
        and page.locator("#advisory-checks > ul > li strong").inner_text() == "Violated"
        and page.locator("#derived-revision").inner_text() == str(revision),
        "HP above Max HP did not flag the check",
    )
    no_overflow(page, "JavaScript sheet with the validity flag")

    # Another tab changes the weight; this tab's HP save brings Load and the check from the
    # new revision without touching any other input or its base.
    other = context.new_page()
    other.goto(sheet)
    ready(other)
    other_weight = other.locator("#row-4-0-2")
    other_weight.fill("5")
    saved_by(other, lambda: other_weight.press("Tab"), "/cells/4/0/2")
    other.close()
    untouched = {
        action: (
            page.locator(f'form[action$="{action}"] [name="value"]').input_value(),
            base(page, action),
        )
        for action in (
            "/fields/max_hp",
            "/fields/armour",
            "/fields/luck",
            "/cells/4/0/2",
            "/cells/4/0/1",
        )
    }
    hp.fill("80")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    require(
        load.inner_text() == "10"
        and page.locator("#validity").count() == 0
        and page.locator("#checks-counts").inner_text() == "0 violated, 0 not evaluated, 1 passed.",
        "The HP save did not bring Load and the check of the new revision",
    )
    require(
        all(
            (
                page.locator(f'form[action$="{action}"] [name="value"]').input_value(),
                base(page, action),
            )
            == shown
            for action, shown in untouched.items()
        ),
        "An HP save moved another input or its base",
    )

    # Two tabs: tab B saves Armour; tab A has a dirty Armour and saves HP. The HP answer
    # leaves Armour's base alone, so Armour is a conflict, kept, and only Enter overwrites.
    other = context.new_page()
    other.goto(sheet)
    ready(other)
    other_armour = other.locator("#v2")
    other_armour.fill("7")
    saved_by(other, lambda: other_armour.press("Tab"), "/fields/armour")
    other.close()
    armour_base = base(page, "/fields/armour")
    armour.evaluate(
        "(field) => { field.value = '4'; field.dispatchEvent(new Event('input', {bubbles: true})); }"
    )
    hp.fill("81")
    saved_by(page, lambda: hp.press("Tab"), "/fields/hp")
    require(
        base(page, "/fields/armour") == armour_base and armour.input_value() == "4",
        "The HP answer moved the dirty Armour's base or value",
    )
    saved_by(page, lambda: (armour.focus(), armour.press("Enter")), "/fields/armour", status=409)
    alert = page.locator('form[action$="/fields/armour"] .entry-alert[role="alert"]')
    require(
        armour.input_value() == "4"
        and "Armour modifier not saved: it changed since this page was loaded" in alert.inner_text()
        and page.evaluate("document.activeElement.id") == "v2",
        "The stale Armour was not refused with its draft kept",
    )
    no_overflow(page, "JavaScript sheet with a refused entry")
    quiet(lambda: armour.press("Tab"), "Leaving a refused Armour")
    # Even an edited draft is not sent by leaving the field after a refusal (ASSIST-23).
    armour.fill("5")
    quiet(lambda: armour.press("Tab"), "Leaving an edited refused Armour")
    saved_by(page, lambda: (armour.focus(), armour.press("Enter")), "/fields/armour")
    require(alert.count() == 0 and armour.input_value() == "5", "Enter did not overwrite Armour")

    # Draft generations: a delayed HP answer keeps the 60 typed meanwhile and moves the base.
    held = []
    page.route("**/fields/hp", lambda route: held.append(route))

    def wait_held(count):
        for _ in range(100):
            if len(held) >= count:
                return
            page.wait_for_timeout(50)
        raise AssertionError(f"HP save {count} was not sent")

    def release(route):
        """Lets a held save through; returns the revision its answer carries."""
        with page.expect_response(
            lambda response: response.request.method == "POST"
            and urlsplit(response.url).path.endswith("/fields/hp")
        ) as response:
            route.continue_()
        return int(response.value.json()["revision"])

    hp.fill("50")
    hp.press("Enter")
    wait_held(1)
    hp.fill("60")
    before = base(page, "/fields/hp")
    revision = release(held[0])
    settled(page)
    require(
        hp.input_value() == "60" and revision > before and base(page, "/fields/hp") == revision,
        "A late HP answer replaced the draft typed meanwhile or left the base",
    )
    # Repeated with a queued second save: the second goes after the first answer, with its
    # base, and its answer applies.
    hp.fill("61")
    hp.press("Enter")
    wait_held(2)
    hp.fill("62")
    hp.press("Tab")
    page.wait_for_timeout(300)
    require(len(held) == 2, "A second HP save was sent before the first answer")
    revision = release(held[1])
    wait_held(3)
    second = dict(parse_qsl(held[2].request.post_data))
    require(
        second["value"] == "62" and int(second["base"]) == revision and hp.input_value() == "62",
        "The queued HP save did not carry the latest value and the new base",
    )
    revision = release(held[2])
    settled(page)
    require(
        hp.input_value() == "62" and base(page, "/fields/hp") == revision,
        "The queued HP save was not acknowledged",
    )
    page.unroute("**/fields/hp")

    # Projection ordering: HP (revision n) and the weight (n + 1) answer in reverse; both
    # units acknowledge, Load and the checks stay at n + 1. Reloaded first: the weight
    # changed in the other tab, so this tab's weight would rightly conflict.
    page.reload()
    ready(page)
    routes = {}
    page.route("**/fields/hp", lambda route: routes.setdefault("hp", route))
    page.route("**/cells/4/0/2", lambda route: routes.setdefault("weight", route))
    hp.fill("90")
    hp.press("Tab")
    weight.fill("4")
    weight.press("Tab")
    for _ in range(100):
        if len(routes) == 2:
            break
        page.wait_for_timeout(50)
    require(len(routes) == 2, "The HP and weight saves were not both sent")
    first = routes["hp"].fetch()
    # Between the two, another tab raises Max HP: the HP answer (n) has the check violated,
    # the weight answer (n + 2) has it passed.
    other = context.new_page()
    other.goto(sheet)
    ready(other)
    other_max = other.locator("#v1")
    other_max.fill("100")
    saved_by(other, lambda: other_max.press("Tab"), "/fields/max_hp")
    other.close()
    last = routes["weight"].fetch()
    routes["weight"].fulfill(response=last)
    until(page, "!document.querySelector('form[action$=\"/cells/4/0/2\"][aria-busy]')")
    newest = base(page, "/cells/4/0/2")
    routes["hp"].fulfill(response=first)
    settled(page)
    page.unroute("**/fields/hp")
    page.unroute("**/cells/4/0/2")
    require(
        base(page, "/fields/hp") == newest - 2
        and hp.input_value() == "90"
        and weight.input_value() == "4"
        and load.inner_text() == "8"
        and page.locator("#derived-revision").inner_text() == str(newest)
        and page.locator("#validity").count() == 0
        and page.locator("#checks-counts").inner_text() == "0 violated, 0 not evaluated, 1 passed."
        and page.locator("#checks-link").inner_text()
        == "Advisory checks: 0 violated, 0 not evaluated"
        and page.locator("#advisory-checks > ul > li").count() == 0
        and page.locator("#advisory-checks > details li strong").text_content() == "Pass",
        "A late HP answer moved Load, the checks or the flag back",
    )
    no_overflow(page, "JavaScript sheet after reordered answers")
    csp_clean(page, consoles, errors)
    page.reload()
    ready(page)
    require(
        page.locator("#v0").input_value() == "90"
        and page.locator("#row-4-0-2").input_value() == "4",
        "The reordered saves were not both stored",
    )
    csp_clean(page, consoles, errors)

    def count_posts(action):
        return len([request for request in posts if urlsplit(request.url).path.endswith(action)])

    # Review round 1. A save queued with Enter before a conflict was shown is dropped: only
    # a new Enter overwrites. The refusal shows the saved value.
    other = context.new_page()
    other.goto(sheet)
    ready(other)
    other_armour = other.locator("#v2")
    other_armour.fill("8")
    saved_by(other, lambda: other_armour.press("Tab"), "/fields/armour")
    other.close()
    held = []
    page.route("**/fields/armour", lambda route: held.append(route))
    armour.fill("1")
    armour.press("Enter")
    for _ in range(100):
        if held:
            break
        page.wait_for_timeout(50)
    armour.fill("2")
    armour.press("Enter")
    sent = count_posts("/fields/armour")
    held[0].continue_()
    settled(page)
    page.wait_for_timeout(600)
    for late in held[1:]:
        late.abort()
    page.unroute("**/fields/armour")
    alert = page.locator('form[action$="/fields/armour"] .entry-alert')
    require(
        count_posts("/fields/armour") == sent
        and armour.input_value() == "2"
        and "Saved value now: \u201c8\u201d" in alert.inner_text()
        and page.locator('form[action$="/fields/armour"] .entry-actions').count() == 0
        and page.locator('form[action$="/fields/armour"] .entry-note').count() == 0,
        "An Enter queued before the conflict overwrote it, or the alert hid the saved value",
    )
    saved_by(page, lambda: (armour.focus(), armour.press("Enter")), "/fields/armour")
    # After a 422 (no conflict) a corrected entry saves when the field is left.
    armour.evaluate(
        "(field) => { field.value = '6\\u0007'; field.dispatchEvent(new Event('input', {bubbles: true})); }"
    )
    saved_by(page, lambda: (armour.focus(), armour.press("Enter")), "/fields/armour", status=422)
    armour.fill("7")
    saved_by(page, lambda: armour.press("Tab"), "/fields/armour")
    require(armour.input_value() == "7", "A corrected entry after a 422 was not saved")
    # A 409 whose body cannot be read is still a conflict: leaving the field sends nothing.
    page.route(
        "**/fields/armour",
        lambda route: route.fulfill(status=409, content_type="application/json", body="{"),
    )
    armour.fill("11")
    saved_by_any = count_posts("/fields/armour")
    armour.press("Enter")
    page.wait_for_timeout(500)
    page.unroute("**/fields/armour")
    armour.fill("12")
    armour.press("Tab")
    page.wait_for_timeout(500)
    require(
        count_posts("/fields/armour") == saved_by_any + 1
        and "Not saved" in page.locator('form[action$="/fields/armour"] .entry-alert').inner_text(),
        "A 409 with an unreadable body did not hold the field",
    )
    saved_by(page, lambda: (armour.focus(), armour.press("Enter")), "/fields/armour")
    armour.fill("7")
    saved_by(page, lambda: armour.press("Enter"), "/fields/armour")

    # Undo clicked while that field's save is on its way: the save replaces the change it
    # would undo, so nothing is undone (never another note's Undo), and the page says so.
    held = []
    page.route("**/fields/hp", lambda route: held.append(route))
    hp.fill("91")
    moved = len(navigations)
    page.get_by_role("button", name="Undo Current HP", exact=True).click()
    for _ in range(100):
        if held:
            break
        page.wait_for_timeout(50)
    held[0].continue_()
    settled(page)
    page.wait_for_timeout(800)
    page.unroute("**/fields/hp")
    require(
        len(navigations) == moved
        and hp.input_value() == "91"
        and "Not undone" in page.locator('main [role="status"]').inner_text()
        and not errors,
        f"An Undo clicked during its field's save was retargeted or failed: {errors}",
    )
    # Undo of another field clicked while a save is on its way still applies after it.
    held = []
    page.route("**/fields/hp", lambda route: held.append(route))
    hp.fill("92")
    page.get_by_role("button", name="Undo Armour modifier", exact=True).click()
    for _ in range(100):
        if held:
            break
        page.wait_for_timeout(50)
    with page.expect_navigation(wait_until="domcontentloaded"):
        held[0].continue_()
    page.unroute("**/fields/hp")
    ready(page)
    require(
        page.locator("#v0").input_value() == "92" and page.locator("#v2").input_value() == "12",
        "The Armour Undo clicked during the HP save did not apply after it",
    )

    # A malformed answer leaves no save pending: a page post still goes.
    page.route(
        "**/fields/max_hp",
        lambda route: route.fulfill(
            status=200,
            content_type="application/json",
            body='{"status":"saved","revision":"9999","message":"x","unit":{},'
            '"derived":[{}],"validity":{}}',
        ),
    )
    max_hp = page.locator("#v1")
    max_hp.fill("101")
    max_hp.press("Tab")
    page.wait_for_timeout(500)
    page.unroute("**/fields/max_hp")
    require(
        "Not saved" in page.locator('form[action$="/fields/max_hp"] .entry-alert').inner_text(),
        "A malformed answer was not reported",
    )
    line = page.get_by_role("textbox", name="Quick add Equipment", exact=True)
    line.fill("1 lamp")
    with page.expect_navigation(wait_until="domcontentloaded", timeout=5_000):
        line.press("Enter")
    ready(page)

    # An orphaned cell: another tab removes the row; the refused entry stays to copy and
    # can no longer be posted (never to the row now at that index).
    # A yes/no cell of that row is saved meanwhile: its selects become unchangeable too,
    # still showing the entry.
    held = []
    page.route("**/cells/4/0/3", lambda route: held.append(route))
    broken = page.locator("#row-4-0-3")
    broken.select_option("true")
    for _ in range(100):
        if held:
            break
        page.wait_for_timeout(50)
    other = context.new_page()
    other.goto(sheet)
    ready(other)
    with other.expect_navigation(wait_until="domcontentloaded"):
        other.get_by_role("button", name="Remove Row 1 Equipment", exact=True).click()
    other.close()
    held[0].continue_()
    settled(page)
    page.unroute("**/cells/4/0/3")
    require(
        broken.is_disabled()
        and broken.input_value() == "true"
        and page.locator("#set-4-0-3").is_disabled()
        and "row was removed"
        in page.locator('form[action$="/cells/4/0/3"] .entry-alert').inner_text(),
        "An orphaned yes/no cell stayed changeable or lost its entry",
    )
    orphan = page.locator("#row-4-0-2")
    # Enter, then a second Enter queued while the first is on its way: the 409 without a
    # route drops the queued one and leaves no save pending, so a page post still goes.
    queue = []
    page.route("**/cells/4/0/2", lambda route: queue.append(route))
    orphan.fill("9")
    orphan.press("Enter")
    for _ in range(100):
        if queue:
            break
        page.wait_for_timeout(50)
    orphan.fill("10")
    orphan.press("Enter")
    sent = count_posts("/cells/4/0/2")
    queue[0].continue_()
    settled(page)
    page.wait_for_timeout(300)
    page.unroute("**/cells/4/0/2")
    orphan.press("Enter")
    page.wait_for_timeout(500)
    require(
        count_posts("/cells/4/0/2") == sent
        and orphan.input_value() == "10"
        and orphan.get_attribute("readonly") is not None
        and orphan.get_attribute("aria-describedby") == "alert-row-4-0-2"
        and page.locator('form[action$="/cells/4/0/2"] button:not([form])').is_disabled()
        and "row was removed"
        in page.locator('form[action$="/cells/4/0/2"] .entry-alert').inner_text(),
        "An orphaned cell entry could still be posted, or was not kept to copy",
    )
    line = page.get_by_role("textbox", name="Quick add Equipment", exact=True)
    line.fill("1 cup")
    with page.expect_navigation(wait_until="domcontentloaded", timeout=5_000):
        line.press("Enter")
    ready(page)
    require(
        page.locator("#row-4-0-0").input_value() == "lamp"
        and page.locator("#row-4-0-2").input_value() == ""
        and page.locator("#row-4-1-0").input_value() == "cup",
        "The orphaned entry was written to another row, or a page post waited forever",
    )

    # The row removed between the write and its live read: the answer has no route; the
    # cell becomes copy-only too.
    def no_route(route):
        response = route.fetch()
        answer = response.json()
        answer["unit"]["action"] = None
        route.fulfill(response=response, json=answer)

    page.route("**/cells/4/0/1", no_route)
    quantity = page.locator("#row-4-0-1")
    quantity.fill("3")
    saved_by(page, lambda: quantity.press("Tab"), "/cells/4/0/1")
    page.unroute("**/cells/4/0/1")
    sent = count_posts("/cells/4/0/1")
    quantity.press("Enter")
    page.wait_for_timeout(500)
    require(
        count_posts("/cells/4/0/1") == sent and quantity.get_attribute("readonly") is not None,
        "A cell whose row left its route could still be posted",
    )
    no_overflow(page, "JavaScript sheet with an orphaned cell")
    csp_clean(page, consoles, errors)


def csp_clean(page, consoles, errors):
    """No CSP or Trusted Types violation and no script error on the page so far."""
    require(
        page.evaluate("window.__csp") == []
        and not [
            text for text in consoles if "Content Security Policy" in text or "Trusted Type" in text
        ]
        and not errors,
        f"CSP or script errors on the sheet: {consoles} {errors}",
    )


def run_workspace_browser(
    browser, page, viewport, binary, directory, collaborator, passwords, versions
):
    """Real campaign/native/runtime forms with an ordinary independent collaborator."""
    version, formula_version, list_version, assist_version = versions

    def submit(tab, button, status=303):
        with tab.expect_response(lambda response: response.request.method == "POST") as response:
            with tab.expect_navigation(wait_until="domcontentloaded"):
                button.click()
        require(response.value.status == status, "Workspace browser mutation status differs")

    def create_campaign(tab, title, game, edition):
        tab.goto("http://127.0.0.1:3000/campaigns")
        tab.locator('input[name="title"]').fill(title)
        tab.locator('select[name="game_id"]').select_option(game)
        tab.locator('select[name="edition_id"]').select_option(edition)
        submit(tab, tab.get_by_role("button", name="Create campaign", exact=True))
        return tab.url

    owner_context = browser.new_context(viewport=viewport)
    owner_context.set_default_timeout(15_000)
    owner_page = owner_context.new_page()
    stage = "ordinary collaborator login"
    try:
        owner_page.goto("http://127.0.0.1:3000/auth/login")
        owner_page.locator('input[name="login"]').fill("table-helper")
        owner_page.locator('input[name="password"]').fill(passwords[0].decode())
        submit(owner_page, owner_page.get_by_role("button", name="Sign in", exact=True))
        stage = "campaign and native creation"
        campaign = create_campaign(page, "Browser table campaign", "mork-borg", "it-2025")
        page.get_by_role("link", name="New native exploration sheet", exact=True).click()
        page.locator('input[name="name"]').fill("Browser player private")
        page.locator('textarea[name="description"]').fill("Long private description " * 500)
        page.locator('input[name="strength"]').fill("-3")
        page.get_by_text("Original worn tier and carrying count", exact=True).click()
        page.locator('input[name="standard_items"]').fill("6")
        submit(page, page.get_by_role("button", name="Save full private sheet", exact=True))
        sheet = page.url
        require("DR 14" in page.locator("main").inner_text(), "Native private calculations missing")
        no_overflow(page, "private native sheet")
        require(
            owner_page.goto(sheet).status == 403,
            "Collaborator implicitly read another player's sheet",
        )
        stage = "joining and acceptance"
        owner_page.goto(campaign)
        require(
            "Browser player private" not in owner_page.content(), "Directory leaked private name"
        )
        submit(owner_page, owner_page.get_by_role("button", name="Request to join", exact=True))
        owner = collaborator
        stage = "Master promotion"
        page.goto(campaign)
        submit(
            page,
            page.locator(f'form[action$="/requests/{owner["id"]}"]').get_by_role(
                "button", name="Accept as player", exact=True
            ),
        )
        require(owner_page.goto(sheet).status == 403, "Ordinary member read another player's sheet")
        page.goto(campaign)
        submit(
            page,
            page.locator(f'form[action$="/members/{owner["id"]}"]').get_by_role(
                "button", name="Grant additional Master role", exact=True
            ),
        )
        require(
            owner_page.goto(sheet).status == 200, "Explicit Master grant did not provide access"
        )
        stage = "Master edit and stale draft"
        owner_page.locator('textarea[name="notes"]').fill("Master revision")
        submit(
            owner_page, owner_page.get_by_role("button", name="Save full private sheet", exact=True)
        )
        page.goto(sheet)
        page.locator('input[name="revision"]').evaluate('(field) => field.value = "1"')
        page.locator('input[name="name"]').fill("Stale <script>draft</script>")
        submit(page, page.get_by_role("button", name="Save full private sheet", exact=True), 409)
        require(
            page.locator('input[name="name"]').input_value() == "Stale <script>draft</script>"
            and page.locator('input[name="revision"]').input_value() == "1",
            "Browser conflict discarded raw draft or stale token",
        )
        no_overflow(page, "private conflict")
        page.goto(campaign)
        submit(
            page,
            page.locator(f'form[action$="/members/{owner["id"]}"]').get_by_role(
                "button", name="Demote to player", exact=True
            ),
        )
        require(owner_page.goto(sheet).status == 403, "Demotion retained private access")
        owner_page.goto(campaign)
        stage = "personal sheet and departure"
        owner_page.get_by_role("link", name="New native exploration sheet", exact=True).click()
        owner_page.locator('input[name="name"]').fill("Browser collaborator personal")
        submit(
            owner_page, owner_page.get_by_role("button", name="Save full private sheet", exact=True)
        )
        owned = owner_page.url
        owner_page.goto(campaign)
        submit(
            owner_page,
            owner_page.get_by_role("button", name="Leave and freeze my active sheets", exact=True),
        )
        for tab in (page, owner_page):
            require(
                tab.goto(owned).status == 200
                and tab.locator("fieldset[disabled]").count() == 1
                and tab.get_by_role("button", name="Save full private sheet", exact=True).count()
                == 0,
                "Frozen sheet is editable by player or Master",
            )
        owner_page.goto(campaign)
        submit(owner_page, owner_page.get_by_role("button", name="Request to join", exact=True))
        page.goto(campaign)
        submit(
            page,
            page.locator(f'form[action$="/requests/{owner["id"]}"]').get_by_role(
                "button", name="Accept as player", exact=True
            ),
        )
        owner_page.goto(owned)
        require(
            owner_page.locator("fieldset[disabled]").count() == 1, "Rejoin thawed private sheet"
        )
        owner_page.get_by_role("link", name="Full history", exact=True).click()
        require(
            "freeze" in owner_page.locator("main").inner_text(), "Browser freeze history missing"
        )
        stage = "runtime creation and history"
        generic = create_campaign(page, "Runtime browser campaign", "synthetic-game", "first")
        # Step 2a: sheet pages load a module that saves without reloading. The PRG flows
        # (D-G) run in a context with JavaScript disabled, signed in with the same session.
        plain_context = browser.new_context(viewport=viewport, java_script_enabled=False)
        plain_context.set_default_timeout(15_000)
        plain_context.add_cookies(page.context.cookies())
        plain = plain_context.new_page()
        sheet_page, page = page, plain
        page.goto(generic + f"/sheets/new?version={version}")
        page.locator('input[name="name"]').fill("Runtime browser private")
        page.locator('select[name="null-name"]').select_option("known")
        page.locator('input[name="value-name"]').fill("Original pinned field")
        submit(page, page.get_by_role("button", name="Save full private sheet", exact=True))
        runtime_sheet = page.url
        page.locator("#v0").fill("Edited pinned field")
        unit_save(page, page.locator("#v0"), path="/fields/name")
        no_overflow(page, "private runtime sheet")
        page.get_by_role("link", name="Full history", exact=True).click()
        page.locator('a[href$="/history/1"]').click()
        require(
            page.locator('input[name="value-name"]').input_value() == "Original pinned field",
            "Browser history did not reconstruct immutable values",
        )
        require(
            owner_page.goto(runtime_sheet).status == 403,
            "Cross-campaign collaborator access leaked",
        )
        stage = "formula sheet"
        run_formula_browser(page, generic, formula_version, submit)
        stage = "list sheet"
        run_list_browser(page, generic, list_version, submit)
        stage = "assisted sheet"
        run_assist_browser(page, generic, assist_version)
        page = sheet_page
        plain_context.close()
        stage = "catalogue reading"
        run_catalogue_browser(page, runtime_sheet, owner_page, no_overflow)
        stage = "assisted sheet with JavaScript"
        run_assist_js_browser(page, generic, assist_version)
        page.goto("http://127.0.0.1:3000/auth/session")
    except AssertionError:
        raise
    except Exception:
        raise AssertionError(
            f"Workspace browser operation failed at {stage} (diagnostics redacted)"
        ) from None
    finally:
        owner_context.close()


class FrameProbeHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'<iframe src="http://127.0.0.1:3000/admin"></iframe>'
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass  # Fixed synthetic fixture; browser assertions capture the result.


def run_account_admin_browser(browser, owner_page, viewport, directory):
    """Copy an invitation link with the button and redeem in a separate jar."""
    owner_page.goto("http://127.0.0.1:3000/admin")
    owner_page.locator('form[action="/admin/invite"] input[name="login"]').fill("browser-enrolled")
    owner_page.locator('form[action="/admin/invite"] input[name="display_name"]').fill(
        "Browser enrolled player"
    )
    owner_page.get_by_role("button", name="Create invitation", exact=True).click()
    require(
        owner_page.get_by_role("button", name="Copy link", exact=True).count() == 1,
        "Invitation lacks a Copy link button",
    )
    require(owner_page.locator(".invitation-qr svg").count() == 1, "Invitation QR is missing")
    require(
        "Unix" not in owner_page.locator("main").inner_text(), "Expiry is not a readable datetime"
    )
    owner_page.context.grant_permissions(["clipboard-read", "clipboard-write"])
    owner_page.get_by_role("button", name="Copy link", exact=True).click()
    owner_page.wait_for_function(
        'document.getElementById("copy-status").textContent === "Link copied."'
    )
    link = owner_page.evaluate("navigator.clipboard.readText()")
    token = urlsplit(link).fragment.removeprefix("token=")
    require(
        len(token) == 64 and all(c in "0123456789abcdef" for c in token),
        "Visible credential could not be copied",
    )
    no_overflow(owner_page, "issued credential")
    context = browser.new_context(viewport=viewport)
    context.set_default_timeout(15_000)
    page = context.new_page()
    stage = "enrollment"
    try:
        page.goto(link)
        require(
            not urlsplit(page.url).fragment
            and page.locator('input[name="token"]').input_value() == token,
            "Invitation link did not prefill and clear the fragment",
        )
        for field in ("password", "confirmation"):
            page.locator(f'input[name="{field}"]').fill("abcdefghijkl")
        page.get_by_role("button", name="Complete enroll", exact=True).click()
        page.wait_for_url("**/auth/login")
        stage = "player sign-in"
        page.locator('input[name="login"]').fill("browser-enrolled")
        page.locator('input[name="password"]').fill("abcdefghijkl")
        page.get_by_role("button", name="Sign in", exact=True).click()
        page.wait_for_url("**/auth/session")
        require(
            page.goto("http://127.0.0.1:3000/admin").status == 403,
            "Enrolled player gained administration",
        )
        owner_page.goto("http://127.0.0.1:3000/admin")
        row = owner_page.locator("article").filter(has_text="browser-enrolled")
        row.get_by_role("button", name="Issue password reset", exact=True).click()
        owner_page.get_by_role("button", name="Copy link", exact=True).click()
        owner_page.wait_for_function(
            'document.getElementById("copy-status").textContent === "Link copied."'
        )
        reset_link = owner_page.evaluate("navigator.clipboard.readText()")
        reset = urlsplit(reset_link).fragment.removeprefix("token=")
        stage = "reset form"
        page.goto(reset_link)
        require(
            urlsplit(page.url).path == "/auth/reset", "Revoked session lost the password reset form"
        )
        require(
            not urlsplit(page.url).fragment
            and page.locator('input[name="token"]').input_value() == reset,
            "Revoked-session reset link lost prefilled token",
        )
        for field in ("password", "confirmation"):
            page.locator(f'input[name="{field}"]').fill("mnopqrstuvwx")
        page.get_by_role("button", name="Complete reset", exact=True).click()
        stage = "reset completion"
        page.wait_for_url("**/auth/login")
        owner_page.goto("http://127.0.0.1:3000/admin")
        stage = "disablement"
        owner_page.locator("article").filter(has_text="browser-enrolled").get_by_role(
            "button", name="Disable account permanently", exact=True
        ).click()
        owner_page.wait_for_url("**/admin")
        require(
            "disabled"
            in owner_page.locator("article").filter(has_text="browser-enrolled").inner_text(),
            "Disabled account state missing",
        )
        require(
            not owner_page.locator('input[id="credential-link"]').count(),
            "GET retrieved a credential",
        )
        no_overflow(owner_page, "Owner directory")
    except AssertionError:
        raise
    except Exception as exception:
        raise AssertionError(
            f"Account browser failed at {stage} ({type(exception).__name__}; diagnostics redacted)"
        ) from None
    finally:
        context.close()
    attacker_context = browser.new_context(viewport=viewport)
    attacker_context.add_cookies(owner_page.context.cookies())
    probe = HTTPServer(("127.0.0.1", 0), FrameProbeHandler)
    probe_thread = threading.Thread(target=probe.serve_forever, daemon=True)
    probe_thread.start()
    try:
        attacker = attacker_context.new_page()
        positive = attacker.goto("http://127.0.0.1:3000/admin")
        require(
            positive.status == 200 and attacker.locator("form").count() > 0,
            "Framing test lacked authenticated positive control",
        )
        require(
            positive.headers.get("content-security-policy") == "frame-ancestors 'none'",
            "Framing policy missing",
        )
        blocked = []
        attacker.on(
            "requestfailed",
            lambda request: blocked.append(
                (request.failure or "").startswith("net::ERR_BLOCKED_BY_RESPONSE")
            ),
        )
        attacker.on(
            "console",
            lambda message: blocked.append(
                "frame-ancestors" in message.text or "X-Frame-Options" in message.text
            ),
        )
        attacker.goto(f"http://127.0.0.1:{probe.server_port}/")
        require(any(blocked), "Browser did not report a blocked framing attempt")
        require(
            not any(frame.url == "http://127.0.0.1:3000/admin" for frame in attacker.frames),
            "Another loopback origin framed usable administration controls",
        )
    except AssertionError:
        raise
    except Exception as exception:
        raise AssertionError(
            f"Account framing browser failed ({type(exception).__name__}; diagnostics redacted)"
        ) from None
    finally:
        attacker_context.close()
        probe.shutdown()
        probe.server_close()
        probe_thread.join(timeout=5)
        require(not probe_thread.is_alive(), "Framing fixture did not stop")
    require_secret_free(
        (directory / "server.log").read_bytes(),
        (token.encode(), reset.encode(), b"abcdefghijkl", b"mnopqrstuvwx"),
    )


def run_auth_scenario(browser, viewport, binary, directory, fixture):
    """Real forms and Chromium cookie jar; only known dummy account inputs."""
    _, passwords = smoke_accounts(binary, directory)
    player = prepare_auth_player(binary, directory, passwords)
    collaborator = prepare_auth_player(binary, directory, passwords, login="table-helper")
    version, formula_version, list_version = prepare_workspace_materials(binary, directory, fixture)
    assist_version = publish_assist_definition(binary, directory, fixture)
    process, log = start(binary, directory, auth=True)
    context = browser.new_context(viewport=viewport)
    context.set_default_timeout(15_000)
    page = context.new_page()
    violations = []

    def inspect(request):
        url = urlsplit(request.url)
        if url.scheme in {"http", "https", "ws", "wss"} and (
            url.hostname != "127.0.0.1" or url.port != 3000
        ):
            violations.append(True)

    context.on("request", inspect)
    page.on("pageerror", lambda _: violations.append(True))
    page.on("dialog", lambda dialog: (violations.append(True), dialog.dismiss()))
    page.on("websocket", lambda _: violations.append(True))

    def cookies():
        jar = context.cookies()
        require(
            len(jar) == 1
            and jar[0]["name"] == "mimik_session"
            and jar[0]["httpOnly"]
            and jar[0]["sameSite"] == "Strict"
            and jar[0]["path"] == "/"
            and not jar[0]["secure"]
            and jar[0]["expires"] > 0,
            "Chromium auth cookie attributes differ",
        )
        require(page.evaluate("document.cookie") == "", "HttpOnly cookie exposed to DOM")
        return jar

    def login(name, password):
        response = page.goto("http://127.0.0.1:3000/auth/login")
        require(response.status == 200, "Browser auth form unavailable")
        require(
            page.locator('input[name="password"]').get_attribute("type") == "password",
            "Browser password input is not masked",
        )
        require(
            len(page.locator('input[name="csrf"]').input_value()) == 64, "Browser form lacks CSRF"
        )
        no_overflow(page, "auth login")
        page.locator('input[name="login"]').fill(name)
        if name == "smoke-owner":
            page.locator('input[name="password"]').fill("Wrong synthetic owner password")
            with page.expect_response(
                lambda r: r.request.method == "POST" and urlsplit(r.url).path == "/auth/login"
            ) as failed:
                page.get_by_role("button", name="Sign in", exact=True).click()
            require(failed.value.status == 401, "Wrong credentials lost the login status")
            page.get_by_role("alert").wait_for()
            require(
                page.locator('form[action="/auth/login"]').count() == 1
                and page.locator('input[name="login"]').input_value() == name
                and page.locator('input[name="password"]').input_value() == "",
                "Inline sign-in failure lost the form/username or retained password",
            )
            no_overflow(page, "inline sign-in error")
            page.locator('input[name="login"]').fill("  SMOKE-OWNER  ")
        page.locator('input[name="password"]').fill(password.decode())
        with page.expect_response(
            lambda r: r.request.method == "POST" and urlsplit(r.url).path == "/auth/login"
        ) as submitted:
            page.get_by_role("button", name="Sign in", exact=True).click()
        require(
            submitted.value.status == 303,
            f"Browser login failed with HTTP{submitted.value.status} (diagnostics redacted)",
        )
        page.wait_for_url("**/auth/session")
        require(name in page.locator("main").inner_text(), "Browser current principal differs")
        require(
            all(password.decode() not in page.content() for password in passwords),
            "Browser auth DOM echoed password",
        )
        require(
            page.locator('input[type="password"]').count() == 0,
            "Password input remains on session page",
        )
        no_overflow(page, "auth session long display")
        if name == "smoke-owner":
            require(
                page.get_by_role("link", name="Open administration", exact=True).count() == 1
                and page.get_by_role("link", name="Open campaign workspace", exact=True).count()
                == 0,
                "Production Owner landing exposed gameplay",
            )
            require(
                page.goto("http://127.0.0.1:3000/campaigns").status == 403,
                "Owner could enter gameplay",
            )
            require(
                page.goto("http://127.0.0.1:3000/admin").status == 200,
                "Owner administration unavailable",
            )
            page.goto("http://127.0.0.1:3000/auth/session")
        return cookies()

    def reject_replay(jar):
        replay_context = browser.new_context(viewport=viewport)
        try:
            replay_context.add_cookies(jar)
            replay_page = replay_context.new_page()
            response = replay_page.goto("http://127.0.0.1:3000/auth/session")
            require(
                response.status == 200
                and urlsplit(replay_page.url).path == "/auth/login"
                and replay_page.locator('input[type="password"]').count() == 1,
                "Revoked browser session did not reach sign-in",
            )
        finally:
            replay_context.close()

    try:
        entry = page.goto("http://127.0.0.1:3000/")
        require(
            entry.status == 200 and urlsplit(page.url).path == "/auth/login",
            "Incognito entry did not reach sign-in",
        )
        anonymous = cookies()
        owner = login("smoke-owner", passwords[0])
        run_material_browser(page, "http://127.0.0.1:3000")
        run_admin_ruleset_browser(page, "http://127.0.0.1:3000")
        run_account_admin_browser(browser, page, viewport, directory)
        stop(process, log)
        process, log = start(binary, directory, auth=True)
        require("disabled" in page.reload().text(), "Browser account state lost on restart")
        current = login("auth-player", passwords[0])
        require(
            owner[0]["value"] != anonymous[0]["value"] and current[0]["value"] != owner[0]["value"],
            "Browser login ID not rotated",
        )
        reject_replay(anonymous)
        reject_replay(owner)
        run_workspace_browser(
            browser,
            page,
            viewport,
            binary,
            directory,
            collaborator,
            passwords,
            (version, formula_version, list_version, assist_version),
        )
        for path in AUTH_DENIED_PATHS:
            response = page.goto("http://127.0.0.1:3000" + path)
            require(
                response.status == 403
                and page.get_by_role("alert").count() == 1
                and page.locator("form").count() == 0,
                "Browser exposes campaign/source/gameplay DOM",
            )
        require(
            page.goto("http://127.0.0.1:3000/admin/rulesets").status == 403
            and page.locator("form").count() == 0,
            "Player reached ruleset administration",
        )
        page.goto("http://127.0.0.1:3000/auth/session")
        require(cookies() == current, "Browser status read extended cookie")
        stop(process, log)
        process, log = start(binary, directory, auth=True)
        response = page.reload()
        require(
            response.status == 200 and "auth-player" in page.locator("main").inner_text(),
            "Browser session lost across restart",
        )
        require(cookies() == current, "Restart/status read changed cookie")
        private = directory / "account-inputs"
        reset = private / "browser-reset.token"
        registry_cli(
            binary,
            directory,
            "issue-account-reset",
            "--id",
            str(player["id"]),
            "--expected-epoch",
            str(player["epoch"]),
            "--token-output",
            str(reset),
            secrets=passwords,
        )
        response = page.reload()
        require(
            response.status == 200 and urlsplit(page.url).path == "/auth/login",
            "Browser reset issuance did not return to sign-in",
        )
        reject_replay(current)
        registry_cli(
            binary,
            directory,
            "complete-account-reset",
            "--token-file",
            str(reset),
            "--password-file",
            str(private / "replacement.password"),
            secrets=passwords,
        )
        current = login("auth-player", passwords[1])
        with page.expect_response(
            lambda r: r.request.method == "POST" and urlsplit(r.url).path == "/auth/logout"
        ) as submitted:
            page.get_by_role("button", name="Sign out", exact=True).click()
        require(submitted.value.status == 303, "Browser logout failed")
        page.wait_for_url("**/auth/login")
        reject_replay(current)
        reject_replay(owner)
        require(not violations, "Auth browser violation (diagnostics redacted)")
    except AssertionError:
        raise
    except Exception:
        raise AssertionError("Auth Chromium operation failed (diagnostics redacted)") from None
    finally:
        context.close()
        stop(process, log)
    require_secret_free((directory / "server.log").read_bytes(), passwords)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--auth", action="store_true", help="Also exercise real auth forms/cookies")
    parser.add_argument("--source-fixture", required=True, type=Path)
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright

    signal.alarm(300 if args.auth else 180)
    binary = args.binary.resolve(strict=True)
    fixture = args.source_fixture.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="mimik-browser-") as temp:
        directory = Path(temp)
        subprocess.run(
            [
                str(binary),
                "--database",
                str(directory / "smoke.sqlite3"),
                "import-source",
                "--bundle",
                str(fixture / "bundle"),
                "--pdf",
                str(fixture / "original.pdf"),
            ],
            cwd=directory,
            env=smoke_environment(directory),
            check=True,
            capture_output=True,
            timeout=30,
        )
        subprocess.run(
            [
                str(binary),
                "--database",
                str(directory / "smoke.sqlite3"),
                "import-source",
                "--bundle",
                str(fixture / "adapter-bundle"),
                "--pdf",
                str(fixture / "original.pdf"),
            ],
            cwd=directory,
            env=smoke_environment(directory),
            check=True,
            capture_output=True,
            timeout=30,
        )
        prepare_browser_registry(binary, directory, fixture)
        process, log = start(binary, directory)
        try:
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(args=["--disable-background-networking"])
                try:
                    for viewport in [{"width": 1280, "height": 720}, {"width": 390, "height": 844}]:
                        # Give each scenario a new imported source decision state.
                        if viewport["width"] == 390:
                            stop(process, log)
                            (directory / "smoke.sqlite3").unlink()
                            subprocess.run(
                                [
                                    str(binary),
                                    "--database",
                                    str(directory / "smoke.sqlite3"),
                                    "import-source",
                                    "--bundle",
                                    str(fixture / "bundle"),
                                    "--pdf",
                                    str(fixture / "original.pdf"),
                                ],
                                cwd=directory,
                                env=smoke_environment(directory),
                                check=True,
                                capture_output=True,
                                timeout=30,
                            )
                            subprocess.run(
                                [
                                    str(binary),
                                    "--database",
                                    str(directory / "smoke.sqlite3"),
                                    "import-source",
                                    "--bundle",
                                    str(fixture / "adapter-bundle"),
                                    "--pdf",
                                    str(fixture / "original.pdf"),
                                ],
                                cwd=directory,
                                env=smoke_environment(directory),
                                check=True,
                                capture_output=True,
                                timeout=30,
                            )
                            prepare_browser_registry(binary, directory, fixture)
                            process, log = start(binary, directory)
                        run_scenario(browser, viewport)
                finally:
                    browser.close()
        finally:
            stop(process, log)
    if args.auth:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(args=["--disable-background-networking"])
            try:
                for viewport in [{"width": 1280, "height": 720}, {"width": 390, "height": 844}]:
                    with tempfile.TemporaryDirectory(prefix="mimik-auth-browser-") as temp:
                        run_auth_scenario(browser, viewport, binary, Path(temp), fixture)
            finally:
                browser.close()
        print(
            "Two auth Chromium stages passed: real forms, cookie/DOM denial, long display, restart, epoch revocation, logout replay."
        )
    print(
        "Two Chromium scenarios passed: real htmx, persistence, long text, responsive layout, source review/lookup and sanitizer."
    )


if __name__ == "__main__":
    main()
