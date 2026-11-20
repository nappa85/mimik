"""Chromium check of name suggestions and "Which one?" (assisted sheet step 4, prototype tooling).

Runs a built mimik (release or development, no model, no assistant flags) over the synthetic
fixture, on the demo sheet of tools/assist_demo.py: its Equipment list's item name is
catalogue-backed (`assist-gear`: Shield, Rope, Healing draught, and two entries named Sword of
different types), and the rows are "Rope" and "Torch" (no entry: the rule-4 warning). Everything
else is seeded through HTTP by the script itself. At 1280 x 720 and 390 x 844 (touch on the
narrow one), with synthetic data only:

- the name inputs are comboboxes; typing "Sw" shows two options with the count in the status
  line, the duplicates' accessible names carry their type and hint;
- keyboard: ArrowDown moves, Enter fills the name and does NOT save (no request, stored value
  unchanged), the pick among duplicates is held as `entry`, Enter again saves it with the key
  (the entry link points at the picked entry, the held input is gone);
- pointer and touch: picking an option keeps focus in the input, so the half-typed text is
  never posted and the stored value stays until the player saves;
- a saved name with two same-named entries shows "Several catalogue entries have this name."
  and "Which one?": inline (the list under the warning, "Use this one" saves the pick through
  the unit) and as its own page (a form per candidate, no script needed);
- a new row's name takes a pick among duplicates and the row is created with its key;
- "Did you mean...?" runs the typo pass;
- no CSP violation, no console error, no horizontal overflow.

Run inside the browser container (containers/browser.Dockerfile), from the repository root:
  cargo run --example source_fixture -- /tmp/fixture
  python3 tools/suggest_browser.py target/release/mimik --fixture /tmp/fixture
"""

import argparse
import sys
import tempfile
from pathlib import Path

from assist_demo import (
    PLAYER,
    create_accounts,
    create_sheet,
    demo_passwords,
    fresh_directory,
)
from browser_smoke import CSP_PROBE, csp_clean, no_overflow
from smoke_release import (
    auth_cookie,
    auth_fields,
    auth_login,
    auth_request,
    prepare_unknown_registry_source,
    publish_assist_definition,
    require,
    start,
    stop,
)

ORIGIN = "http://127.0.0.1:3000"
LIST = 4  # the Equipment list is field 4 of the assist definition
VIEWPORTS = [
    {"width": 1280, "height": 720, "touch": False},
    {"width": 390, "height": 844, "touch": True},
]


def until(page, check, what):
    """Polls `check()` (a Python callable) for up to 20 s."""
    for _ in range(400):
        if check():
            return
        page.wait_for_timeout(50)
    raise AssertionError(f"Timed out waiting for {what}")


def stored(page, sheet, row):
    """The saved name of Equipment row `row`, read from a fresh page load (never the DOM the
    module edits), so a half-typed value that was posted would show."""
    html = page.context.request.get(f"{ORIGIN}{sheet}").text()
    marker = f'id="row-{LIST}-{row}-0" name="value" value="'
    start_at = html.index(marker) + len(marker)
    return html[start_at : html.index('"', start_at)]


def options(page, key):
    return page.locator(f"#suggest-{key} [role=option]")


def cell_post(page, row):
    return page.expect_response(
        lambda response: response.request.method == "POST"
        and f"/cells/{LIST}/{row}/0" in response.url
    )


def save(page, row, text):
    """Types `text` into the row's name and saves it with Enter (no option is active)."""
    control = page.locator(f"#row-{LIST}-{row}-0")
    control.fill(text)
    with cell_post(page, row) as sent:
        control.press("Enter")
    require(sent.value.status == 200, f"Saving {text!r} answered {sent.value.status}")
    control.evaluate("element => element.blur()")
    page.wait_for_timeout(200)


def ready(page):
    until(
        page,
        lambda: page.evaluate("document.documentElement.dataset.sheetAssist === 'ready'"),
        "the sheet module",
    )


def link_ends(page, row, tail):
    link = page.locator(f"#entry-{LIST}-{row}-0")
    return link.count() == 1 and link.get_attribute("href").endswith(tail)


def typed(page, key, row_input, text):
    control = page.locator(row_input)
    control.click()
    control.fill("")
    control.press_sequentially(text, delay=30)
    return control


def scenario(page, sheet, mobile):
    consoles, errors, posts = [], [], []
    page.add_init_script(CSP_PROBE)
    page.on(
        "console",
        lambda message: consoles.append(message.text) if message.type == "error" else None,
    )
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on(
        "request",
        lambda request: posts.append(request.post_data or "") if request.method == "POST" else None,
    )
    page.goto(f"{ORIGIN}{sheet}")
    ready(page)

    # The Torch row has no entry: rule 4, with Browse; its name input is a combobox.
    warning = page.locator(f"#match-{LIST}-1-0")
    require(
        warning.count() == 1 and "Not in the catalogue." in warning.inner_text(),
        "The Torch row has no rule-4 warning",
    )
    torch = page.locator(f"#row-{LIST}-1-0")
    require(
        torch.get_attribute("role") == "combobox"
        and torch.get_attribute("aria-expanded") == "false"
        and torch.get_attribute("aria-autocomplete") == "list",
        "The name input is not a combobox",
    )

    # Keyboard: two Sword options, Enter fills without saving, the pick is held as `entry`.
    key = f"{LIST}-1-0"
    control = typed(page, key, f"#row-{key}", "Sw")
    until(page, lambda: options(page, key).count() == 2, "two Sword suggestions")
    require(control.get_attribute("aria-expanded") == "true", "The list is not announced open")
    require(
        page.locator(f"#suggest-{key}-status").inner_text() == "2 suggestions",
        "The status line does not give the count",
    )
    label = options(page, key).first.get_attribute("aria-label") or ""
    require(
        "Weapon" in label and "plain blade" in label,
        f"A duplicate's accessible name lacks its type and hint: {label!r}",
    )
    no_overflow(page, "the open suggestion list")
    before = len(posts)
    control.press("ArrowDown")
    require(
        control.get_attribute("aria-activedescendant") == f"suggest-{key}-0",
        "ArrowDown did not make the first option active",
    )
    control.press("Enter")
    held = page.locator(f"form:has(#row-{key}) input[name=entry]")
    require(
        control.input_value() == "Sword"
        and held.count() == 1
        and held.get_attribute("value") == "sword",
        "Enter did not fill the name and hold the pick",
    )
    require(page.locator(f"#suggest-{key}").is_hidden(), "The list stayed open after Enter")
    page.wait_for_timeout(700)
    require(len(posts) == before, "Enter on an option posted a save")
    require(stored(page, sheet, 1) == "Torch", "Enter on an option saved the name")
    with cell_post(page, 1) as sent:
        control.press("Enter")
    require(sent.value.status == 200, "Saving the pick failed")
    until(page, lambda: link_ends(page, 1, "/catalogue/assist-gear/sword"), "the picked entry link")
    require(
        page.locator(f"form:has(#row-{key}) input[name=entry]").count() == 0
        and page.locator(f"#match-{key}").count() == 0
        and stored(page, sheet, 1) == "Sword",
        "The held pick was not cleared, or the warning stayed, after the save",
    )

    # Pointer / touch: the pick keeps focus, the half-typed "Hea" is never posted.
    key = f"{LIST}-0-0"
    control = typed(page, key, f"#row-{key}", "Hea")
    until(page, lambda: options(page, key).count() == 1, "the Healing draught suggestion")
    before = len(posts)
    url_before = page.url
    rows_before = page.locator(f"input[id^='row-{LIST}-'][id$='-0']").count()
    option = options(page, key).first
    if mobile:
        option.tap()
    else:
        option.click()
    require(control.input_value() == "Healing draught", "The pointer pick did not fill the name")
    page.wait_for_timeout(700)
    require(
        stored(page, sheet, 0) == "Rope"
        and not any("value=Hea&" in body or body.endswith("value=Hea") for body in posts[before:]),
        "A pointer pick saved, or posted the half-typed text",
    )
    # The click that ends the pick must not land on what the closing list uncovers (Save, the
    # next row's input, Move, Remove, Add row): nothing was posted, nothing navigated, no row
    # changed, and focus is still in the name input.
    require(
        len(posts) == before
        and page.url == url_before
        and page.locator(f"input[id^='row-{LIST}-'][id$='-0']").count() == rows_before
        and page.evaluate("document.activeElement && document.activeElement.id") == f"row-{key}"
        and page.locator(f"form:has(#row-{key}) .entry-alert").count() == 0,
        "The pointer pick activated something else (a ghost click)",
    )
    with cell_post(page, 0) as sent:
        control.press("Enter")
    require(sent.value.status == 200, "Saving the pointer pick failed")
    until(page, lambda: link_ends(page, 0, "/catalogue/assist-gear/healing-draught"), "its link")
    require(stored(page, sheet, 0) == "Healing draught", "The picked name was not stored")

    # "Which one?" inline: two entries are called Sword and the row has no key.
    save(page, 0, "Sword")
    match = page.locator(f"#match-{key}")
    require(
        match.count() == 1 and "Several catalogue entries have this name." in match.inner_text(),
        "A Sword row has no rule-3 warning",
    )
    url = page.url
    match.locator("a.which").click()
    until(
        page,
        lambda: page.locator(f"form:has(#row-{key}) .which-list li").count() == 2,
        "candidates",
    )
    require(page.url == url, "Which one? navigated away with script on")
    no_overflow(page, "the inline Which one? list")
    with cell_post(page, 0) as sent:
        page.locator(f"form:has(#row-{key}) .which-list li").nth(1).get_by_role(
            "button", name="Use this one"
        ).click()
    require(sent.value.status == 200 and "entry=sword-prop" in posts[-1], "The pick was not posted")
    until(page, lambda: link_ends(page, 0, "/catalogue/assist-gear/sword-prop"), "the picked link")
    require(page.locator(f"#match-{key}").count() == 0, "The warning stayed after the pick")

    # "Which one?" as its own page (no script on it): a form per candidate.
    save(page, 1, "Torch")
    save(page, 1, "Sword")
    href = page.locator(f"#match-{LIST}-1-0 a.which").get_attribute("href")
    page.goto(f"{ORIGIN}{href}")
    require(
        page.locator("h1").inner_text() == "Which one?"
        and page.locator(".which-list form").count() == 2,
        "The Which one? page does not list both entries",
    )
    no_overflow(page, "the Which one? page")
    page.locator(".which-list form").first.get_by_role("button", name="Use this one").click()
    page.wait_for_url(f"{ORIGIN}{sheet}*")
    ready(page)
    require(link_ends(page, 1, "/catalogue/assist-gear/sword"), "The page pick did not link")

    # A new row takes a pick among duplicates and is created with its key.
    new = typed(page, "new-4-0", "#row-new-4-0", "Sw")
    until(page, lambda: options(page, "new-4-0").count() == 2, "new-row suggestions")
    new.press("ArrowDown")
    new.press("ArrowDown")
    new.press("Enter")
    require(
        new.input_value() == "Sword"
        and page.locator("form:has(#row-new-4-0) input[name=entry]").get_attribute("value")
        == "sword-prop",
        "The new row did not hold the pick",
    )
    page.locator(f"#add-{LIST}").click()
    page.wait_for_url(f"{ORIGIN}{sheet}*")
    ready(page)
    require(
        link_ends(page, 2, "/catalogue/assist-gear/sword-prop")
        and stored(page, sheet, 2) == "Sword",
        "The new row was not created with the picked entry",
    )

    # Did you mean: a transposed name shows the warning and its button runs the typo pass.
    save(page, 1, "Swrod")
    until(page, lambda: page.locator(f"#match-{LIST}-1-0 .did-you-mean").count() == 1, "the button")
    page.locator(f"#match-{LIST}-1-0 .did-you-mean").click()
    key = f"{LIST}-1-0"
    until(page, lambda: options(page, key).count() == 2, "typo suggestions")
    require(
        "Sword" in options(page, key).first.inner_text(),
        "The typo pass did not suggest Sword",
    )
    page.locator(f"#row-{key}").press("Escape")
    require(
        page.locator(f"#row-{key}").get_attribute("aria-expanded") == "false",
        "Escape did not close the list",
    )
    no_overflow(page, "the sheet after the scenario")
    csp_clean(page, consoles, errors)


def run(binary, fixture):
    from playwright.sync_api import sync_playwright

    with tempfile.TemporaryDirectory(prefix="mimik-suggest-browser-") as temp:
        directory = fresh_directory(Path(temp) / "data")
        passwords = demo_passwords()
        create_accounts(binary, directory, passwords)
        prepare_unknown_registry_source(binary, directory, fixture)
        version = publish_assist_definition(binary, directory, fixture)
        process, log = start(binary, directory, auth=True)
        try:
            response = auth_request("/auth/login")
            cookie = auth_login(
                auth_cookie(response),
                auth_fields(response, "/auth/login"),
                PLAYER,
                passwords[PLAYER],
            )
            name, value = cookie.split("=", 1)
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(args=["--disable-background-networking"])
                try:
                    for viewport in VIEWPORTS:
                        sheet = create_sheet(passwords[PLAYER], version)
                        context = browser.new_context(
                            viewport={"width": viewport["width"], "height": viewport["height"]},
                            has_touch=viewport["touch"],
                        )
                        context.add_cookies([{"name": name, "value": value, "url": ORIGIN}])
                        scenario(context.new_page(), sheet, viewport["touch"])
                        context.close()
                finally:
                    browser.close()
        finally:
            stop(process, log)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path, help="A built mimik (release or development)")
    parser.add_argument("--fixture", type=Path, required=True, help="source_fixture output")
    args = parser.parse_args(argv)
    run(args.binary.resolve(strict=True), args.fixture.resolve(strict=True))
    print("Suggestions Chromium scenario passed at 1280 and 390.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
