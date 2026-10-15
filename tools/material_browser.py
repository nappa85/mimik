"""Native Owner material review acceptance, shared by loopback and LAN/proxy gates."""

from urllib.parse import urlsplit

from smoke_release import require


def run_material_browser(page, origin):
    """Use only generated native-adapter evidence, never published runtime mappings."""
    page.goto(origin + "/admin/materials")
    article = page.locator("article").filter(has_text="mork-borg / it-2025").first
    article.get_by_role("link").click()
    page.get_by_role("link", name="PDF page", exact=False).first.click()
    path = urlsplit(page.url).path
    page.wait_for_function("document.querySelector('.source-original img').naturalWidth > 0")
    page.wait_for_function("document.querySelector('.source-region img').naturalWidth > 0")
    forms = page.locator('form[action*="/units/"]')
    first = forms.first
    action = first.get_attribute("action")
    revision = first.locator('[name="revision"]').input_value()
    stale = page.context.new_page()
    try:
        stale.goto(origin + path)
        first.locator('[name="state"]').select_option("corrected")
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
            first.get_by_role("button", name="Record decision", exact=True).click()
        require(submitted.value.status == 303, "Maximum Unicode material review failed")
        page.wait_for_url(origin + path)
        stale_form = stale.locator(f'form[action="{action}"]')
        stale_form.locator('[name="state"]').select_option("corrected")
        stale_form.locator('[name="correction"]').fill("<retained source draft>")
        stale_form.locator('[name="rationale"]').fill("<recheck original>")
        with stale.expect_response(
            lambda r: r.request.method == "POST" and urlsplit(r.url).path == action
        ) as conflict:
            stale_form.get_by_role("button", name="Record decision", exact=True).click()
        require(conflict.value.status == 409, "Stale material review was accepted")
        stale.get_by_role("alert").wait_for()
        retained = stale.locator(f'form[action="{action}"]')
        require(
            retained.locator('[name="correction"]').input_value() == "<retained source draft>"
            and retained.locator('[name="rationale"]').input_value() == "<recheck original>"
            and retained.locator('[name="revision"]').input_value() == revision,
            "Material conflict lost draft or silently refreshed revision",
        )
    finally:
        stale.close()
    page.goto(origin + path)
    for form in page.locator('form[action*="/units/"]').all():
        action = form.get_attribute("action")
        form.locator('[name="state"]').select_option("accepted")
        form.locator('[name="correction"]').fill("")
        form.locator('[name="rationale"]').fill("Compared generated original region")
        with page.expect_response(
            lambda r, action=action: r.request.method == "POST" and urlsplit(r.url).path == action
        ) as submitted:
            form.get_by_role("button", name="Record decision", exact=True).click()
        require(submitted.value.status == 303, "Material acceptance failed")
        page.wait_for_url(origin + path)
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
    require("account:" in page.locator("main").inner_text(), "Authenticated reviewer missing")
    sizes = page.evaluate(
        "({width:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth})"
    )
    require(sizes["scroll"] <= sizes["width"] + 2, "Material document overflows viewport")
