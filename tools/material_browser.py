"""Native Owner material review acceptance, shared by loopback and LAN/proxy gates."""

import re
import time
from urllib.parse import urlsplit

from smoke_release import require


def run_material_browser(page, origin):
    """Use only generated native-adapter evidence, never published runtime mappings."""
    page.goto(origin + "/admin/materials")
    article = page.locator("article").filter(has_text="mork-borg / it-2025").first
    article.get_by_role("link").click()
    page.get_by_role("link", name="PDF page", exact=False).first.click()
    path = urlsplit(page.url).path
    # Every unit card, whatever the agreement check would pick as the default tab.
    every = origin + path + "?show=all"
    page.goto(every)
    # The region crop is `loading="lazy"` inside a unit's closed "Details" disclosure: it loads
    # when the reader opens it, as here. Polled from here: `wait_for_function` with a string
    # needs eval, which the CSP refuses.
    page.locator("details.unit-details:has(.source-region img) > summary").first.click()
    for selector in (".source-original img", ".source-region img"):
        image = page.locator(selector).first
        deadline = time.monotonic() + 15
        while not image.evaluate("(img) => img.complete && img.naturalWidth > 0"):
            require(time.monotonic() < deadline, f"Material evidence image {selector} did not load")
            page.wait_for_timeout(100)
    # The review screen: each unit card has an "Accept as extracted" form and, behind a
    # "Correct…" disclosure, a correction form; each form carries its own hidden `state`.
    first = page.locator('form[id^="unit-correct-"]').first
    form_id = first.get_attribute("id")
    unit = form_id.removeprefix("unit-correct-")
    record = f"Record correction for unit {unit}"
    saved = re.compile(re.escape(origin + path) + r"\?.*saved=")
    action = first.get_attribute("action")
    revision = first.locator('[name="revision"]').input_value()
    stale = page.context.new_page()
    try:
        stale.goto(every)
        page.locator(f'details[data-decision="corrected"]:has(#{form_id}) > summary').click()
        # insert_text exercises user input and the native UTF-16 maxlength constraint.
        for name, count in (("correction", 20_000), ("rationale", 5_000)):
            field = first.locator(f'[name="{name}"]')
            field.fill("")
            field.focus()
            page.keyboard.insert_text("🦀" * count)
            require(field.input_value() == "🦀" * count, "Native form truncated Unicode scalars")
        with page.expect_response(
            lambda r: r.request.method == "POST" and urlsplit(r.url).path == action
        ) as submitted:
            first.get_by_role("button", name=record, exact=True).click()
        require(submitted.value.status == 303, "Maximum Unicode material review failed")
        page.wait_for_url(saved)
        stale.locator(f'details[data-decision="corrected"]:has(#{form_id}) > summary').click()
        stale_form = stale.locator(f"#{form_id}")
        stale_form.locator('[name="correction"]').fill("<retained source draft>")
        stale_form.locator('[name="rationale"]').fill("<recheck original>")
        with stale.expect_response(
            lambda r: r.request.method == "POST" and urlsplit(r.url).path == action
        ) as conflict:
            stale_form.get_by_role("button", name=record, exact=True).click()
        require(conflict.value.status == 409, "Stale material review was accepted")
        stale.get_by_role("alert").first.wait_for()
        retained = stale.locator(f"#{form_id}")
        require(
            retained.locator('[name="correction"]').input_value() == "<retained source draft>"
            and retained.locator('[name="rationale"]').input_value() == "<recheck original>"
            and retained.locator('[name="revision"]').input_value() == revision,
            "Material conflict lost draft or silently refreshed revision",
        )
    finally:
        stale.close()
    # Accept every other unit card, one post (and one fresh page) at a time.
    page.goto(every)
    cards = page.locator("[data-unit]").count()
    require(cards > 0, "Material page lists no unit cards")
    for n in range(cards):
        page.goto(every)
        card = page.locator("[data-unit]").nth(n)
        if card.get_attribute("data-unit") == unit:
            continue
        form = card.locator("form:has(button[data-accept-unit])")
        action = form.get_attribute("action")
        with page.expect_response(
            lambda r, action=action: r.request.method == "POST" and urlsplit(r.url).path == action
        ) as submitted:
            form.locator("button[data-accept-unit]").click()
        require(submitted.value.status == 303, "Material acceptance failed")
        page.wait_for_url(saved)
    page.goto(origin + path)
    completion = page.locator(f'form[action="{path}"]')
    completion.locator('[name="printed_label"]').fill("synthetic page")
    completion.locator('[name="rationale"]').fill("Entire generated original checked for omissions")
    with page.expect_response(
        lambda r: r.request.method == "POST" and urlsplit(r.url).path == path
    ) as submitted:
        completion.get_by_role("button").click()
    require(submitted.value.status == 303, "Material page completion failed")
    page.wait_for_url(origin + path)
    require("review complete" in page.locator("main").inner_text(), "Completed review not visible")
    # The review history (a disclosure) names the reviewer as `account:<id>` and shows the
    # long correction; open it so the overflow check below covers that text too.
    history = page.locator("details.page-task", has_text="Review history")
    history.locator("summary").click()
    shown = history.inner_text()
    require("account:" in shown, "Authenticated reviewer missing")
    require("corrected" in shown, "The corrected unit's decision is missing from the history")
    sizes = page.evaluate(
        "({width:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth})"
    )
    require(sizes["scroll"] <= sizes["width"] + 2, "Material document overflows viewport")
