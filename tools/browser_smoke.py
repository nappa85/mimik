"""Offline Chromium checks of the shipped artifact, with synthetic inputs only."""

import argparse
import signal
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from material_browser import run_material_browser
from smoke_release import (
    AUTH_DENIED_PATHS,
    prepare_auth_player,
    prepare_unknown_registry_source,
    prepare_workspace_materials,
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


def run_workspace_browser(
    browser, page, viewport, binary, directory, collaborator, passwords, version
):
    """Real campaign/native/runtime forms with an ordinary independent collaborator."""

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
        page.goto(generic + f"/sheets/new?version={version}")
        page.locator('input[name="name"]').fill("Runtime browser private")
        page.locator('select[name="null-name"]').select_option("known")
        page.locator('input[name="value-name"]').fill("Original pinned field")
        submit(page, page.get_by_role("button", name="Save full private sheet", exact=True))
        runtime_sheet = page.url
        page.locator('input[name="value-name"]').fill("Edited pinned field")
        submit(page, page.get_by_role("button", name="Save full private sheet", exact=True))
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
    version = prepare_workspace_materials(binary, directory, fixture)
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
            browser, page, viewport, binary, directory, collaborator, passwords, version
        )
        for path in AUTH_DENIED_PATHS:
            response = page.goto("http://127.0.0.1:3000" + path)
            require(
                response.status == 403
                and page.get_by_role("alert").count() == 1
                and page.locator("form").count() == 0,
                "Browser exposes campaign/source/gameplay DOM",
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
