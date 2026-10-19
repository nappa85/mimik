"""Chromium acceptance of the read-only catalogue routes reached through a sheet.

Catalogue plan §3 step 3: keyboard-only search, no script and no page images, and owner text
holding an unterminated right-to-left override rendered in an isolated bidi context
(`<bdi>`, `white-space: pre-wrap`) that cannot reorder the surrounding name, citation or
controls. Synthetic original text only (`smoke_release.synthetic_catalogue`).
"""

from urllib.parse import urlsplit

from smoke_release import CATALOGUE_RLO, require

ORIGIN = "http://127.0.0.1:3000"

# For each element: is its first visible character before its last one in reading order
# (left of it on the same line, or on an earlier line)? Single-character runs are skipped.
LTR_ORDER = """(selector) => [...document.querySelectorAll(selector)].map((element) => {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  const nodes = [];
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (node.data.trim() && node.parentElement.closest('select, option') === null) nodes.push(node);
  }
  if (nodes.length === 0) return true;
  const first = nodes[0];
  const last = nodes[nodes.length - 1];
  const start = first.data.search(/\\S/);
  const end = last.data.trimEnd().length - 1;
  if (first === last && start >= end) return true;
  const range = document.createRange();
  range.setStart(first, start); range.setEnd(first, start + 1);
  const a = range.getBoundingClientRect();
  range.setStart(last, end); range.setEnd(last, end + 1);
  const b = range.getBoundingClientRect();
  return a.top + 2 < b.top || (Math.abs(a.top - b.top) <= 2 && a.left < b.left);
})"""

# Element boxes in document order must also read left-to-right, line by line.
BOX_ORDER = """(selector) => {
  const boxes = [...document.querySelectorAll(selector)].map((e) => e.getBoundingClientRect());
  return boxes.every((box, i) => i === 0 || boxes[i - 1].top + 2 < box.top
    || (Math.abs(boxes[i - 1].top - box.top) <= 2 && boxes[i - 1].left < box.left));
}"""


def _ltr(page, selector, label):
    order = page.evaluate(LTR_ORDER, selector)
    require(order and all(order), f"Catalogue {label} is not in reading order: {order}")


def _no_script_or_image(page, label):
    require(
        page.locator("script").count() == 0 and page.locator("img").count() == 0,
        f"Catalogue {label} loads a script or shows an image",
    )


def run_catalogue_browser(page, sheet, outsider, no_overflow):
    """`sheet` is the URL of a runtime sheet whose definition links `spells`; `outsider`
    is a page signed in as a user who cannot read that sheet."""
    page.goto(sheet)
    link = page.get_by_role("link", name="Synthetic spells", exact=True)
    require(
        link.get_attribute("target") == "_blank" and link.get_attribute("rel") == "noopener",
        "Catalogue link would replace the sheet tab and its unsaved draft",
    )
    browse = ORIGIN + link.get_attribute("href")
    require(outsider.goto(browse).status == 403, "Catalogue readable without the sheet")
    require(page.goto(browse).status == 200, "Catalogue browse unavailable")
    _no_script_or_image(page, "browse page")
    no_overflow(page, "catalogue browse")
    # Keyboard only: focus the search field, type, and submit with Enter.
    page.get_by_label("Search names").focus()
    page.keyboard.type("ward")
    with page.expect_navigation(wait_until="domcontentloaded"):
        page.keyboard.press("Enter")
    require(urlsplit(page.url).query.startswith("q=ward"), "Enter did not submit the search")
    results = page.locator("ul.catalogue-results li")
    require(
        results.count() == 1 and "Ward Ink" in results.first.inner_text(),
        "Catalogue name search differs",
    )
    _ltr(page, "main > h1, nav.catalogue-nav, ul.catalogue-results li", "browse text")
    require(
        page.evaluate(BOX_ORDER, "form.catalogue-search input, form.catalogue-search button"),
        "Catalogue search controls are out of order",
    )
    page.get_by_role("link", name="Ward Ink", exact=True).focus()
    with page.expect_navigation(wait_until="domcontentloaded"):
        page.keyboard.press("Enter")
    _no_script_or_image(page, "entry page")
    no_overflow(page, "catalogue owner-written entry")
    paragraph = page.locator("#description p.catalogue-text")
    isolate = paragraph.locator("bdi")
    require(
        paragraph.count() == 1
        and isolate.count() == 1
        and isolate.evaluate("(e) => getComputedStyle(e).unicodeBidi") == "isolate"
        and paragraph.evaluate("(e) => getComputedStyle(e).whiteSpace") == "pre-wrap",
        "Owner text is not in an isolated pre-wrap bidi context",
    )
    text = isolate.evaluate("(e) => e.textContent")
    require(
        CATALOGUE_RLO in text and "\n" in text and "Owner-written" in page.content(),
        "Owner text, its line break or its badge is missing",
    )
    _ltr(page, "main > h1, nav.catalogue-nav, main > p, dl.catalogue-properties", "entry text")
    require(
        page.evaluate(BOX_ORDER, "nav.catalogue-nav a"),
        "Catalogue navigation controls are out of order",
    )
    # The paragraph after the override still reads in document order.
    require(
        page.evaluate(
            """() => {
              const heading = [...document.querySelectorAll('h2')].at(-1)
                .getBoundingClientRect();
              const text = document.querySelector('#description').getBoundingClientRect();
              return heading.bottom <= text.top + 1;
            }"""
        ),
        "Description escaped its block",
    )
    # A unit-sourced entry: badge, live citation, no page image.
    page.goto(browse.rsplit("?", 1)[0] + "/lantern-flare")
    _no_script_or_image(page, "unit-sourced entry")
    badge = page.locator("main > p .badge").first.inner_text()
    require(
        badge in {"Unchecked source text", "Reviewed extraction"}
        and page.locator("ul.citations li").count() == 1,
        "Unit-sourced entry badge or citation missing",
    )
    _ltr(page, "main > h1, nav.catalogue-nav, main > p, ul.citations li", "citation")
    no_overflow(page, "catalogue unit-sourced entry")
    print(
        "Catalogue Chromium stage passed: keyboard search, no script or image, isolated owner text."
    )
