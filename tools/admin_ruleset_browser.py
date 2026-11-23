"""Owner browser ruleset registration acceptance with synthetic identities only."""

from urllib.parse import urlsplit

from smoke_release import require

GAME_LABEL = "Browser <script>game</script>"


def _submit(page, form, path):
    with page.expect_response(
        lambda r: r.request.method == "POST" and urlsplit(r.url).path == path
    ) as submitted:
        form.get_by_role("button").click()
    return submitted.value.status


def _fill(form, values):
    for name, value in values.items():
        form.locator(f'[name="{name}"]').fill(value)


def _wait_notice(page, origin, notice):
    page.wait_for_url(lambda url: url == f"{origin}/admin/rulesets?notice={notice}")


def run_admin_ruleset_browser(page, origin):
    """Register, repeat, collide and alias through native forms; never touch real manuals."""
    response = page.goto(origin + "/admin")
    require(response.status == 200, "Owner administration unavailable")
    page.get_by_role("link", name="Register game editions and exact legacy aliases").click()
    page.wait_for_url(origin + "/admin/rulesets")
    register = page.locator('form[action="/admin/rulesets"]')
    edition = {
        "game_id": "browser-game",
        "game_label": GAME_LABEL,
        "edition_id": "first",
        "edition_label": "First 🦀 edition",
    }
    _fill(register, edition)
    require(_submit(page, register, "/admin/rulesets") == 303, "Browser registration failed")
    _wait_notice(page, origin, "registered")
    main = page.locator("main").inner_text()
    require(GAME_LABEL in main and "browser-game" in main, "Registered edition not listed")
    require(page.locator("main script").count() == 0, "Registry label rendered as markup")
    register = page.locator('form[action="/admin/rulesets"]')
    _fill(register, edition)
    require(_submit(page, register, "/admin/rulesets") == 303, "Exact repeat was not idempotent")
    _wait_notice(page, origin, "unchanged")
    register = page.locator('form[action="/admin/rulesets"]')
    collision = dict(edition, edition_id="second")
    _fill(register, collision)
    require(_submit(page, register, "/admin/rulesets") == 422, "Label collision was accepted")
    page.get_by_role("alert").wait_for()
    retained = page.locator('form[action="/admin/rulesets"]')
    require(
        all(retained.locator(f'[name="{k}"]').input_value() == v for k, v in collision.items()),
        "Rejected registration lost the submitted input",
    )
    alias = page.locator('form[action="/admin/rulesets/aliases"]')
    _fill(alias, {"game_id": "browser-game", "edition_id": "missing", "legacy_edition": "Shadow"})
    require(
        _submit(page, alias, "/admin/rulesets/aliases") == 422,
        "An alias to an unregistered edition was accepted",
    )
    page.get_by_role("alert").wait_for()
    alias = page.locator('form[action="/admin/rulesets/aliases"]')
    _fill(
        alias,
        {"game_id": "browser-game", "edition_id": "first", "legacy_edition": "Old browser label"},
    )
    require(_submit(page, alias, "/admin/rulesets/aliases") == 303, "Exact alias failed")
    _wait_notice(page, origin, "alias-registered")
    sizes = page.evaluate(
        "({width:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth})"
    )
    require(sizes["scroll"] <= sizes["width"] + 2, "Ruleset document overflows viewport")
