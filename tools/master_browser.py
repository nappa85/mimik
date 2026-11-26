"""Chromium check of the Master workspace (Master view slice c1, prototype tooling; CAMP-14).

Runs a built mimik (release or development, no model) over the synthetic fixture with an Owner,
two Masters (A creates the campaign, B is promoted), a player and an outsider, all synthetic
and enrolled through the CLI with generated passwords. Everything else is seeded over HTTP by
the script itself or driven through the normal pages. For each of the four combinations
1280 x 720 and 390 x 844 (touch on the narrow one) with JavaScript enabled and disabled:

With JavaScript:
- the workspace loads under the strict sheet policy (header equal to the sheet pages'), shows
  the polite status "Live", has no CSP violation or script error and no document overflow;
- an NPC is created through the Master-only link of the campaign page (the choice is the
  page's own `#npc-create`); another Master's list gains the row by push, without navigation,
  and its Open link keeps focus and the row element keeps its identity across later pushes;
- opening a sheet (the sheet page auto-submits the seen form) clears that Master's badge on
  their other tab and no other Master's, nor the badge of a sheet they have not opened; a
  later revision brings "Changed" back; an edit, a new sheet and a departure (freeze) arrive
  by push;
- a long name with bidirectional text and markup stays text, with no XSS;
- an unsaved editor draft survives a remote edit (no stream on sheet pages), and the stale
  save keeps the draft (409);
- two healthy list tabs: the displaced one shows "moved to another tab", offers Resume and
  never reclaims the slot, the newest stays live; Resume swaps them; a repeated `pageshow`
  opens no second stream, and a real navigation away and back opens exactly one stream;
- a graceful server restart is followed by a snapshot probe and a bounded retry that goes
  live again; a refused probe (503) shows "busy" with a bounded number of probes and no
  reconnecting flicker; sign-out shows a sign-in link; demotion
  clears the list and says access ended;
- both sections page at 50 on their own cursors: the Next link appears by push at the 51st
  sheet, a later page keeps its cursors and its "on this page" note;
- the module failing to load (blocked or broken) leaves the plain page, its Reload and Open
  links working;
- the player and the outsider are refused (403) on the workspace, its snapshot and its stream,
  and none of their network responses (documents, JSON, event streams) carries the NPC label
  or the NPC's name, and they never load the Master module.

Without JavaScript: the same page explains that a reload is needed, Reload shows new rows (a
GET reload never clears a badge), the Open link and the one-click seen form (303) work and the
badge clears; NPC creation works as plain forms.

The Master's home (`/`, server-rendered, checked with JavaScript on and off): the block "Latest
edited sheets in campaigns I run" lists a player's new sheet as Not opened, Opened after the
Master acknowledges it, Changed after a remote edit and Opened again after the next
acknowledgement, while the other Master's badge for it stays Not opened; no document overflow.

No request leaves 127.0.0.1:3000. Run inside the browser container
(containers/browser.Dockerfile), from the repository root:
  cargo run --example source_fixture -- /tmp/fixture
  python3 tools/master_browser.py target/release/mimik --fixture /tmp/fixture
"""

import argparse
import re
import secrets
import shutil
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from assist_demo import fresh_directory, write_private
from browser_smoke import CSP_PROBE, csp_clean, no_overflow
from smoke_release import (
    FormFields,
    auth_cookie,
    auth_fields,
    auth_login,
    auth_request,
    prepare_unknown_registry_source,
    publish_assist_definition,
    registry_cli,
    require,
    start,
    stop,
    version_link,
)

ORIGIN = "http://127.0.0.1:3000"
VIEWPORTS = [
    {"width": 1280, "height": 720, "touch": False},
    {"width": 390, "height": 844, "touch": True},
]
STRICT_CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none'; "
    "base-uri 'none'; form-action 'self'; frame-ancestors 'none'; "
    "require-trusted-types-for 'script'; trusted-types 'none'"
)
OWNER = "table-owner"
LOGINS = {"master-a": "Aria", "master-b": "Bruno", "table-player": "Pia", "outsider": "Otto"}
# A long name with bidirectional text and markup: it must stay text everywhere. The listing
# refuses names over 100 characters; this one is 86.
TRICKY = "<img src=x onerror=window.__xss=1> שלום &amp; " + "w" * 40
NPC_NAME = "Zorgnak the <b>Unseen</b> ‮bidi‬" + "n" * 20
STATUS = {"connecting", "live", "reconnecting", "offline", "busy", "replaced", "signin", "denied"}


def until(page, check, what, seconds=25):
    """Polls `check()` (a Python callable): Playwright's own polling evaluates strings, which
    the sheet policy refuses."""
    for _ in range(seconds * 20):
        if check():
            return
        page.wait_for_timeout(50)
    raise AssertionError(f"Timed out waiting for {what}")


def live_state(page):
    return page.evaluate("document.documentElement.dataset.masterLive || ''")


def wait_state(page, state, seconds=25):
    until(page, lambda: live_state(page) == state, f"live state {state!r}", seconds)


def rows(page, section):
    """(id, name, custodian, revision, badge) of every row of a section, from the DOM."""
    return page.evaluate(
        """(section) => [...document.querySelectorAll(`#${section}-rows > article`)].map(
          (a) => [a.dataset.id, a.querySelector('a.open bdi').textContent,
                  a.querySelector('.custodian').textContent,
                  a.querySelector('.revision').textContent,
                  a.querySelector('.badge').textContent,
                  a.querySelector('.state').textContent])""",
        section,
    )


def badge_of(page, section, sheet):
    for row in rows(page, section):
        if row[0] == str(sheet):
            return row[4]
    return None


class Client:
    """One signed-in account over plain HTTP (independent of every browser context)."""

    def __init__(self, login, password):
        self.login = login
        for attempt in range(6):
            try:
                response = auth_request("/auth/login")
                self.cookie = auth_login(
                    auth_cookie(response), auth_fields(response, "/auth/login"), login, password
                )
                return
            except AssertionError:
                # The login throttle (one process-wide quota) answers 429 for a few seconds.
                if attempt == 5:
                    raise
                time.sleep(6)

    def get(self, path):
        return auth_request(path, cookie=self.cookie)

    def form(self, path, action=None):
        response = self.get(path)
        require(response[0] == 200, f"{path} answered {response[0]}")
        parser = FormFields()
        parser.feed(response[2])
        actions = [a for a in parser.forms if a not in ("/auth/logout",) and "/seen" not in a]
        action = action or actions[0]
        require(action in parser.forms, f"{path} has no form {action}")
        return action, dict(parser.forms[action]), response[2]

    def post(self, path, fields, **updates):
        response = auth_request(path, cookie=self.cookie, form=dict(fields, **updates))
        return response

    def submit(self, path, fields, **updates):
        response = self.post(path, fields, **updates)
        require(response[0] == 303, f"POST {path} answered {response[0]}")
        return response[1]["location"]


def enroll(binary, directory, passwords):
    """Bootstraps the Owner and enrolls every other account; the password files and the
    invitation tokens exist only while the CLI reads them."""
    private = directory / "account-inputs"
    private.mkdir(mode=0o700)
    hidden = tuple(passwords.values())
    try:
        files = {}
        for login, password in passwords.items():
            files[login] = private / f"{login}.password"
            write_private(files[login], password)
        registry_cli(
            binary,
            directory,
            "bootstrap-owner",
            "--login",
            OWNER,
            "--display-name",
            "Table Owner",
            "--password-file",
            str(files[OWNER]),
            secrets=hidden,
        )
        for login, display in LOGINS.items():
            token = private / f"{login}-invite.token"
            registry_cli(
                binary,
                directory,
                "invite-account",
                "--login",
                login,
                "--display-name",
                display,
                "--token-output",
                str(token),
                secrets=hidden,
            )
            registry_cli(
                binary,
                directory,
                "enroll-account",
                "--token-file",
                str(token),
                "--password-file",
                str(files[login]),
                secrets=hidden,
            )
    finally:
        shutil.rmtree(private)


def account_ids(binary, directory):
    import json

    listing = json.loads(registry_cli(binary, directory, "list-accounts"))
    return {account["login"]: account["id"] for account in listing}


class Table:
    """One campaign: A created it, B is a promoted Master, the player is accepted."""

    def __init__(self, clients, ids, version, title):
        self.clients, self.ids, self.version = clients, ids, version
        a, b, player = (clients[login] for login in ("master-a", "master-b", "table-player"))
        action, fields, _ = a.form("/campaigns", "/campaigns")
        self.path = a.submit(
            "/campaigns", fields, title=title, game_id="synthetic-game", edition_id="first"
        )
        self.id = int(self.path.rsplit("/", 1)[1])
        for who in (b, player):
            action, fields, _ = who.form(self.path, f"{self.path}/join")
            who.submit(action, fields)
        for login in ("master-b", "table-player"):
            action, fields, _ = a.form(self.path, f"{self.path}/requests/{ids[login]}")
            a.submit(action, fields, action="accept")
        action, fields, _ = a.form(self.path, f"{self.path}/members/{ids['master-b']}")
        a.submit(action, fields, action="promote")

    @property
    def master(self):
        return f"{self.path}/master"

    def create_sheet(self, who, name, npc=False):
        """A sheet through the plain create form; returns its path."""
        html = self.clients[who].get(self.path)[2]
        link = version_link(html, self.path, self.version)
        if npc:
            link += "&npc=1"
        action, fields, _ = self.clients[who].form(link)
        return self.clients[who].submit(action, fields, name=name).split("#")[0]

    def edit(self, who, sheet, name):
        """A save of the sheet's Name unit at the current revision (a new revision)."""
        action, fields, _ = self.clients[who].form(sheet, f"{sheet}/meta/name")
        return self.clients[who].submit(action, fields, value=name)

    def sheet_id(self, sheet):
        return int(sheet.rsplit("/", 1)[1])


class Watch:
    """Requests and responses of one browser context: external traffic, stream opens and every
    response's text (for the label-absence checks)."""

    def __init__(self, context):
        self.external, self.events, self.texts, self.scripts = [], [], [], []
        context.on("request", self._request)
        context.on("response", self._response)

    def _request(self, request):
        url = urlsplit(request.url)
        if url.scheme in {"http", "https", "ws", "wss"} and (
            url.hostname not in {"127.0.0.1", "localhost"} or url.port != 3000
        ):
            self.external.append(request.url)
        if url.path.endswith("/master/events"):
            self.events.append(request.url)
        if url.path.endswith("/assets/master-live.js"):
            self.scripts.append(request.url)

    def _response(self, response):
        kind = response.headers.get("content-type", "")
        if kind.startswith(("text/html", "application/json")):
            try:
                self.texts.append((response.url, response.text()))
            except Exception:  # a navigation may have discarded the body
                pass


def open_context(browser, viewport, cookie, *, javascript=True):
    context = browser.new_context(
        viewport={"width": viewport["width"], "height": viewport["height"]},
        has_touch=viewport["touch"],
        java_script_enabled=javascript,
    )
    context.set_default_timeout(15_000)
    name, value = cookie.split("=", 1)
    context.add_cookies([{"name": name, "value": value, "url": ORIGIN}])
    if javascript:
        context.add_init_script(CSP_PROBE)
        context.add_init_script("window.__xss = false")
    return context


def instrument(page):
    consoles, errors = [], []
    page.on("console", lambda m: consoles.append(m.text) if m.type == "error" else None)
    page.on("pageerror", lambda e: errors.append(str(e)))
    return consoles, errors


def go_master(page, table, query=""):
    response = page.goto(f"{ORIGIN}{table.master}{query}")
    require(response.status == 200, f"The workspace answered {response.status}")
    require(
        response.headers.get("content-security-policy") == STRICT_CSP,
        "The workspace does not carry the strict sheet policy",
    )
    return response


def players(page):
    return rows(page, "players")


def run_js(browser, viewport, passwords, ids, version, restart):
    clients = {login: Client(login, passwords[login]) for login in LOGINS}
    label = f"{viewport['width']}x{viewport['height']} JS on"
    a, b, player, outsider = (
        clients[login] for login in ("master-a", "master-b", "table-player", "outsider")
    )
    empty = Table(clients, ids, version, f"Empty table {label}")
    table = Table(clients, ids, version, f"Table {label}")
    ctx = {}
    watches = {}
    for login, client in clients.items():
        ctx[login] = open_context(browser, viewport, client.cookie)
        watches[login] = Watch(ctx[login])

    # Empty state, then a player sheet arrives by push.
    list_empty = ctx["master-a"].new_page()
    consoles, errors = instrument(list_empty)
    go_master(list_empty, empty)
    wait_state(list_empty, "live")
    require(
        "No player sheets." in list_empty.locator("#players-rows").inner_text()
        and "No NPC sheets." in list_empty.locator("#npcs-rows").inner_text(),
        f"{label}: empty state differs",
    )
    status = list_empty.locator("#live-status")
    require(status.get_attribute("role") == "status", "The status region is not role=status")
    require("Live" in status.inner_text(), f"{label}: status text {status.inner_text()!r}")
    no_overflow(list_empty, f"{label} empty workspace")
    navigations = []
    list_empty.on(
        "framenavigated",
        lambda f: navigations.append(f.url) if f == list_empty.main_frame else None,
    )
    empty.create_sheet("table-player", "First arrival")
    until(list_empty, lambda: len(players(list_empty)) == 1, "the first player row by push")
    require(
        "No player sheets" not in list_empty.locator("#players-rows").inner_text(),
        f"{label}: the empty note stayed",
    )
    require(not navigations, f"{label}: the list navigated: {navigations}")
    csp_clean(list_empty, consoles, errors)
    list_empty.close()

    # The player sheet with the tricky name, before anyone opens the workspace.
    player_sheet = table.create_sheet("table-player", TRICKY)
    player_id = table.sheet_id(player_sheet)

    # Master B's list tab, and a second tab of B for opening sheets.
    list_b = ctx["master-b"].new_page()
    consoles_b, errors_b = instrument(list_b)
    go_master(list_b, table)
    wait_state(list_b, "live")
    work_b = ctx["master-b"].new_page()
    instrument(work_b)
    # Master A's list tab; A works in a second tab.
    list_a = ctx["master-a"].new_page()
    consoles_a, errors_a = instrument(list_a)
    go_master(list_a, table)
    wait_state(list_a, "live")
    work_a = ctx["master-a"].new_page()
    consoles_work, errors_work = instrument(work_a)
    require(
        [r[1] for r in players(list_a)] == [TRICKY] and players(list_a)[0][2] == "Pia",
        f"{label}: the tricky name is not shown as text: {players(list_a)}",
    )
    require(
        list_a.evaluate("window.__xss") is False
        and list_a.locator("#players-rows img").count() == 0,
        "Markup in a name became DOM",
    )
    require(badge_of(list_a, "players", player_id) == "Not opened", "A's first badge differs")
    no_overflow(list_a, f"{label} workspace with a long name")

    # A creates an NPC through the Master-only link; B's list gains it by push.
    watch_nav_b = []
    list_b.on(
        "framenavigated", lambda f: watch_nav_b.append(f.url) if f == list_b.main_frame else None
    )
    work_a.goto(f"{ORIGIN}{table.path}")
    work_a.get_by_role("link", name=re.compile(r"^New NPC")).first.click()
    require(work_a.locator("#npc-create").count() == 1, "The create page lacks the NPC notice")
    work_a.locator("input[name=name]").first.fill(NPC_NAME)
    work_a.get_by_role("button", name="Save full private sheet").first.click()
    work_a.wait_for_url(re.compile(r"/private-sheets/\d+"))
    npc_id = int(urlsplit(work_a.url).path.rsplit("/", 1)[1])
    until(list_b, lambda: len(rows(list_b, "npcs")) == 1, "the NPC row on B's list by push")
    require(rows(list_b, "npcs")[0][1] == NPC_NAME, f"{label}: NPC name differs")
    require(badge_of(list_b, "npcs", npc_id) == "Not opened", "B's NPC badge differs")
    until(
        list_a, lambda: badge_of(list_a, "npcs", npc_id) == "Opened", "A's own creation is opened"
    )
    require(not watch_nav_b, f"{label}: B's list navigated: {watch_nav_b}")
    require(
        list_b.locator("#npcs-rows").inner_text().count("No NPC sheets") == 0,
        "The NPC empty note stayed",
    )

    # Identity and focus: mark B's NPC row, focus its Open link, then push changes.
    list_b.evaluate("document.querySelector('#npcs-rows article').__mark = 'kept'")
    list_b.locator("#npcs-rows a.open").focus()
    table.edit("master-a", f"/private-sheets/{npc_id}", NPC_NAME + " v2")
    until(list_b, lambda: rows(list_b, "npcs")[0][3] == "2", "the NPC's second revision by push")
    require(
        list_b.evaluate(
            "document.querySelector('#npcs-rows article').__mark === 'kept' "
            "&& document.activeElement === document.querySelector('#npcs-rows a.open')"
        ),
        f"{label}: the row element or its focus was replaced",
    )
    require(rows(list_b, "npcs")[0][4] == "Not opened", "Not opened turned into another badge")

    # B opens the NPC in its other tab: the sheet page acknowledges by itself, B's list tab
    # follows, A's badge is unchanged.
    work_b.goto(f"{ORIGIN}/private-sheets/{npc_id}?players_after=0&npcs_after=0")
    until(
        work_b, lambda: work_b.locator("#seen-state").count() == 1, "the automatic acknowledgement"
    )
    require(
        "Opened (revision 2)" in work_b.locator("#seen-state").inner_text(),
        "The acknowledgement text differs",
    )
    until(list_b, lambda: badge_of(list_b, "npcs", npc_id) == "Opened", "B's badge clearing")
    require(badge_of(list_a, "npcs", npc_id) == "Opened", "A's badge moved with B's open")
    # Only the opening Master: B has not opened the player's sheet, so it stays "Not opened"
    # for B while A opens it and A's badge clears.
    work_a.goto(f"{ORIGIN}{player_sheet}")
    until(
        work_a, lambda: work_a.locator("#seen-state").count() == 1, "A's automatic acknowledgement"
    )
    until(list_a, lambda: badge_of(list_a, "players", player_id) == "Opened", "A's player badge")
    list_b.wait_for_timeout(1000)
    require(
        badge_of(list_b, "players", player_id) == "Not opened",
        "A's open cleared B's badge for a sheet B never opened",
    )
    # A later revision (A's own edit) brings "Changed" back for B only where seen.
    table.edit("master-a", f"/private-sheets/{npc_id}", NPC_NAME + " v3")
    until(list_b, lambda: badge_of(list_b, "npcs", npc_id) == "Changed", "Changed returning on B")
    require(badge_of(list_a, "npcs", npc_id) == "Opened", "A's own save left A opened")

    # Keyboard: Tab reaches the Open links and Reload in DOM order.
    list_b.locator("#reload").focus()
    list_b.keyboard.press("Tab")
    require(
        list_b.evaluate("document.activeElement.classList.contains('open')"),
        "Tab from Reload does not reach the first Open link",
    )

    # A freeze arrives by push: the player leaves and keeps no active sheet.
    player_form = player.form(table.path, f"{table.path}/members/{ids['table-player']}")
    player.submit(player_form[0], player_form[1], action="leave")
    until(list_a, lambda: players(list_a)[0][5] != "active", "the freeze by push")
    # (the player left: the sheet stays in the list, frozen)
    states = {row[5] for row in players(list_a)}
    require(states and "active" not in states, f"{label}: states after the freeze: {states}")

    # An unsaved draft survives a remote edit; the stale save keeps it (409).
    draft_sheet = f"/private-sheets/{npc_id}"
    work_b.goto(f"{ORIGIN}{draft_sheet}")
    field = work_b.locator("input[name=name], input[id*=name]").first
    field.fill("Draft typed by B")
    events_before = len(watches["master-b"].events)
    table.edit("master-a", draft_sheet, NPC_NAME + " v4")
    work_b.wait_for_timeout(1500)
    require(
        field.input_value() == "Draft typed by B"
        and len(watches["master-b"].events) == events_before,
        f"{label}: the draft or the sheet page's streams changed",
    )
    # The stale save is refused (409) and the module keeps the draft in the field.
    field.press("Enter")
    until(work_b, lambda: work_b.locator("[role=alert]").count() >= 1, "the conflict alert")
    require(field.input_value() == "Draft typed by B", "The refused save lost the draft")
    csp_clean(work_b, [], [])

    # Two healthy list tabs of one Master: the displaced tab is terminal.
    second_b = ctx["master-b"].new_page()
    instrument(second_b)
    go_master(second_b, table)
    wait_state(second_b, "live")
    wait_state(list_b, "replaced")
    require(
        "another tab" in list_b.locator("#live-status").inner_text()
        and list_b.get_by_role("button", name="Resume live updates").count() == 1,
        f"{label}: the replaced tab's status differs",
    )
    first_events = len([u for u in watches["master-b"].events])
    list_b.wait_for_timeout(7000)
    require(
        live_state(list_b) == "replaced"
        and live_state(second_b) == "live"
        and len(watches["master-b"].events) == first_events,
        f"{label}: the replaced tab reclaimed the slot",
    )
    # A repeated pageshow on a live tab opens no second stream.
    count = len(watches["master-b"].events)
    second_b.evaluate(
        "window.dispatchEvent(new PageTransitionEvent('pageshow', {persisted: true}))"
    )
    second_b.wait_for_timeout(500)
    require(len(watches["master-b"].events) == count, "A pageshow opened a duplicate stream")
    list_b.get_by_role("button", name="Resume live updates").click()
    wait_state(list_b, "live")
    wait_state(second_b, "replaced")
    second_b.close()

    # Server restart: both Master A and B follow back to live by themselves.
    restart()
    wait_state(list_a, "live", 60)
    wait_state(list_b, "live", 60)
    table.edit("master-a", f"/private-sheets/{npc_id}", NPC_NAME + " v5")
    until(list_b, lambda: rows(list_b, "npcs")[0][3] == "5", "a push after the restart")

    # A refused probe shows "busy" without a reconnect storm (routes are the test's own).
    probes = []

    def refuse(route):
        probes.append(route.request.url)
        route.fulfill(status=503, headers={"retry-after": "3"}, body="")

    list_a.route("**/master/snapshot*", refuse)
    list_a.route("**/master/events*", lambda route: route.abort())
    # The refused stream and probe make the failure; the state must stay "busy" (no
    # announcement flicker) and the probes must stay bounded by the 3 s floor and the backoff.
    list_a.reload()
    wait_state(list_a, "busy", 30)
    seen_states = set()
    for _ in range(40):
        seen_states.add(live_state(list_a))
        list_a.wait_for_timeout(200)
    require(seen_states == {"busy"}, f"{label}: states during the outage {seen_states}")
    require(1 <= len(probes) <= 4, f"{label}: {len(probes)} probes in about 8 s")
    list_a.unroute("**/master/snapshot*")
    list_a.unroute("**/master/events*")
    list_a.reload()
    wait_state(list_a, "live")
    # A real navigation away and back leaves exactly one new stream (Chromium may restore the
    # page from its cache or reload it; the `no-store` pages normally prevent the former).
    opened = len(watches["master-a"].events)
    list_a.goto(f"{ORIGIN}/")
    list_a.go_back()
    wait_state(list_a, "live")
    list_a.wait_for_timeout(500)
    require(
        len(watches["master-a"].events) == opened + 1,
        f"{label}: navigating away and back opened {len(watches['master-a'].events) - opened}",
    )

    # The player and the outsider: refused everywhere, nothing of the NPC in their responses.
    for who in ("table-player", "outsider"):
        page = ctx[who].new_page()
        for path in (table.master, f"{table.master}/snapshot", f"{table.master}/events"):
            response = page.request.get(f"{ORIGIN}{path}")
            require(response.status == 403, f"{who} got {response.status} on {path}")
            lowered = response.text().lower()
            require("npc" not in lowered and "zorgnak" not in lowered, f"{who} saw {path}")
        page.goto(f"{ORIGIN}{table.path}")
        page.goto(f"{ORIGIN}/private-sheets")
        page.close()
    for who in ("table-player", "outsider"):
        watch = watches[who]
        require(not watch.scripts, f"{who} loaded the Master module")
        for url, text in watch.texts:
            lowered = text.lower()
            require(
                "npc" not in lowered and "zorgnak" not in lowered,
                f"{who}'s response {url} carries an NPC label or name",
            )

    # B signs out in another tab: B's list shows a sign-in link and stops.
    work_b.goto(f"{ORIGIN}/")
    sign_out = work_b.get_by_role("button", name="Sign out")
    work_b.locator("details.account summary").click()
    sign_out.click()
    wait_state(list_b, "signin", 40)
    require(
        list_b.get_by_role("link", name="Sign in").get_attribute("href") == "/auth/login",
        "The sign-in link differs",
    )
    count = len(watches["master-b"].events)
    list_b.wait_for_timeout(4000)
    require(len(watches["master-b"].events) == count, "A signed-out tab kept retrying")
    csp_clean(list_a, consoles_a, errors_a)
    csp_clean(list_b, consoles_b, errors_b)

    # The module failing to load (blocked, then broken) leaves the plain page usable. A fresh
    # B session: the first one signed out.
    b2 = Client("master-b", passwords["master-b"])
    plain = open_context(browser, viewport, b2.cookie)
    broken = plain.new_page()
    broken_errors = []
    broken.on("pageerror", lambda e: broken_errors.append(str(e)))
    broken.route("**/assets/master-live.js", lambda route: route.abort())
    broken.route("**/assets/sheet-seen.js", lambda route: route.abort())
    go_master(broken, table)
    broken.wait_for_timeout(500)
    require(
        live_state(broken) == "" and "JavaScript" in broken.locator("#live-status").inner_text(),
        f"{label}: the page changed without its module",
    )
    broken.locator("#reload").click()
    broken.wait_for_url(f"{ORIGIN}{table.master}?players_after=0&npcs_after=0")
    broken.locator("#npcs-rows a.open").click()
    broken.wait_for_url(re.compile(r"/private-sheets/\d+"))
    broken.locator("#seen-button").click()
    require(
        "Opened (revision" in broken.locator("#seen-state").inner_text(),
        f"{label}: the one-click acknowledgement failed",
    )
    broken.locator("#master-link").click()
    require(badge_of(broken, "npcs", npc_id) == "Opened", "The one click did not clear the badge")
    broken.unroute("**/assets/master-live.js")
    broken.route(
        "**/assets/master-live.js",
        lambda route: route.fulfill(
            status=200, content_type="text/javascript", body="this is { not javascript"
        ),
    )
    go_master(broken, table)
    broken.wait_for_timeout(500)
    require(broken_errors and live_state(broken) == "", "A broken module changed the page")
    require(badge_of(broken, "npcs", npc_id) == "Opened", "The plain page lost its rows")
    no_overflow(broken, f"{label} plain page")
    broken.unroute("**/assets/master-live.js")

    # Access revocation, with a Master on the create page (the choice survives only for a
    # still-authorized Master): A demotes B; B's live list clears and says access ended, the
    # create is refused and makes no sheet, and a reload is the uniform 403.
    go_master(broken, table)
    wait_state(broken, "live")
    create = plain.new_page()
    create.goto(f"{ORIGIN}{table.path}")
    create.get_by_role("link", name=re.compile(r"^New NPC")).first.click()
    require(create.locator("#npc-create").count() == 1, "B lacks the NPC create page")
    create.locator("input[name=name]").first.fill("Refused NPC")
    action, fields, _ = a.form(table.path, f"{table.path}/members/{ids['master-b']}")
    a.submit(action, fields, action="demote")
    wait_state(broken, "denied", 40)
    require(
        rows(broken, "players") == []
        and rows(broken, "npcs") == []
        and "no longer have access" in broken.locator("#live-status").inner_text(),
        f"{label}: the list was not cleared on revocation",
    )
    with create.expect_response(lambda r: r.request.method == "POST") as sent:
        create.get_by_role("button", name="Save full private sheet").first.click()
    require(sent.value.status in {403, 404}, f"The revoked create answered {sent.value.status}")
    require(
        all(row[1] != "Refused NPC" for row in rows(list_a, "npcs")),
        "A refused create made a sheet",
    )
    broken.reload()
    require(
        broken.evaluate("document.body.innerText.includes('Refused NPC')") is False,
        "The refused name leaked",
    )
    for context in (*ctx.values(), plain):
        context.close()
    for watch in watches.values():
        require(not watch.external, f"{label}: external requests {watch.external}")
    print(f"Master workspace with JavaScript passed at {label}.")


def run_paging(browser, viewport, passwords, ids, version):
    """Both sections page at 50 on their own cursors: the live list gains the Next link when
    the 51st sheet arrives, a later page keeps its cursors, and the "on this page" note shows."""
    label = f"{viewport['width']}x{viewport['height']} paging"
    clients = {login: Client(login, passwords[login]) for login in LOGINS}
    table = Table(clients, ids, version, f"Paged table {label}")
    # 50 player sheets from two custodians (32 sheets at most per custodian).
    for index in range(30):
        table.create_sheet("table-player", f"Player sheet {index}")
    for index in range(20):
        table.create_sheet("master-a", f"Master sheet {index}")
    context = open_context(browser, viewport, clients["master-a"].cookie)
    watch = Watch(context)
    page = context.new_page()
    consoles, errors = instrument(page)
    go_master(page, table)
    wait_state(page, "live")
    first = players(page)
    require(len(first) == 50, f"{label}: first page has {len(first)} rows")
    require(page.locator("#players-next").count() == 0, f"{label}: Next before the 51st sheet")
    last = first[-1][0]
    table.create_sheet("master-a", "Master sheet 20")
    until(page, lambda: page.locator("#players-next").count() == 1, "the Next link by push")
    href = page.locator("#players-next").get_attribute("href")
    require(
        href == f"{table.master}?players_after={last}&npcs_after=0" and len(players(page)) == 50,
        f"{label}: Next link {href}",
    )
    # The second page, with the NPC section on a cursor past its last row.
    go_master(page, table, f"?players_after={last}&npcs_after=999999")
    wait_state(page, "live")
    require(len(players(page)) == 1, f"{label}: second page rows {len(players(page))}")
    require(
        "No NPC sheets on this page." in page.locator("#npcs-rows").inner_text(),
        f"{label}: the paged empty note differs",
    )
    require(page.locator("a", has_text="First player sheets").count() == 1, "No First link")
    table.create_sheet("master-a", "Master sheet 21")
    until(page, lambda: len(players(page)) == 2, "the second page's new row by push")
    require(
        "No NPC sheets on this page." in page.locator("#npcs-rows").inner_text()
        and page.locator("#players-next").count() == 0,
        f"{label}: the paged state changed after a push",
    )
    no_overflow(page, f"{label} second page")
    csp_clean(page, consoles, errors)
    require(not watch.external, f"{label}: external requests {watch.external}")
    context.close()
    print(f"Master workspace paging passed at {label}.")


def run_plain(browser, viewport, clients, ids, version):
    """No JavaScript: plain pages, Reload, Open, the one-click form and plain NPC creation."""
    label = f"{viewport['width']}x{viewport['height']} JS off"
    table = Table(clients, ids, version, f"Plain table {label}")
    contexts = {
        login: open_context(browser, viewport, clients[login].cookie, javascript=False)
        for login in ("master-a", "master-b")
    }
    watches = {login: Watch(context) for login, context in contexts.items()}
    page_a, page_b = (contexts[login].new_page() for login in ("master-a", "master-b"))
    go_master(page_b, table)
    text = page_b.locator("#live-status").inner_text()
    require("JavaScript" in text and "Reload" in text, f"{label}: status text {text!r}")
    require(
        page_b.locator("#players-rows .empty").count() == 1
        and page_b.locator("#npcs-rows .empty").count() == 1,
        f"{label}: the empty state differs",
    )
    # A creates an NPC through plain forms.
    page_a.goto(f"{ORIGIN}{table.path}")
    page_a.get_by_role("link", name=re.compile(r"^New NPC")).first.click()
    require(page_a.locator("#npc-create").count() == 1, f"{label}: no NPC create notice")
    page_a.locator("input[name=name]").first.fill(NPC_NAME)
    page_a.get_by_role("button", name="Save full private sheet").first.click()
    page_a.wait_for_url(re.compile(r"/private-sheets/\d+"))
    npc_id = int(urlsplit(page_a.url).path.rsplit("/", 1)[1])
    player_sheet = table.create_sheet("table-player", TRICKY)
    # B's page is still the old one until the Reload link is followed.
    require(not page_b.locator("#npcs-rows article").count(), "The plain page updated itself")
    page_b.locator("#reload").click()
    require(
        page_b.locator("#npcs-rows article").count() == 1,
        f"{label}: Reload shows no NPC",
    )
    badges = (
        page_b.locator("#npcs-rows .badge").inner_text(),
        page_b.locator("#players-rows .badge").inner_text(),
    )
    require(badges == ("Not opened", "Not opened"), f"{label}: badges {badges}")
    page_b.locator("#reload").click()
    require(
        page_b.locator("#npcs-rows .badge").inner_text() == "Not opened",
        f"{label}: a GET reload cleared a badge",
    )
    no_overflow(page_b, f"{label} plain workspace")
    # Open (a GET) clears nothing; the one click does, for B only.
    page_b.locator("#npcs-rows a.open").click()
    page_b.wait_for_url(re.compile(r"/private-sheets/\d+\?players_after=0&npcs_after=0"))
    page_b.locator("#master-link").click()
    require(
        page_b.locator("#npcs-rows .badge").inner_text() == "Not opened",
        f"{label}: opening by GET cleared the badge",
    )
    page_b.locator("#npcs-rows a.open").click()
    with page_b.expect_response(lambda r: r.request.method == "POST") as sent:
        page_b.locator("#seen-button").click()
    require(sent.value.status == 303, f"{label}: the seen form answered {sent.value.status}")
    require(
        "Opened (revision" in page_b.locator("#seen-state").inner_text(),
        f"{label}: the one click left no mark",
    )
    page_b.locator("#master-link").click()
    require(page_b.locator("#npcs-rows .badge").inner_text() == "Opened", "B's badge stayed")
    # A later revision reappears on the next reload; A's own list never had "Changed".
    table.edit("master-a", f"/private-sheets/{npc_id}", NPC_NAME + " v2")
    page_b.locator("#reload").click()
    require(page_b.locator("#npcs-rows .badge").inner_text() == "Changed", "Changed did not return")
    page_a.goto(f"{ORIGIN}{table.master}")
    require(page_a.locator("#npcs-rows .badge").inner_text() == "Opened", "A's badge differs")
    require(
        page_a.locator("#players-rows article a.open bdi").inner_text() == TRICKY
        and page_a.locator("#players-rows img").count() == 0,
        f"{label}: the tricky name is not text",
    )
    require(player_sheet, "no player sheet")
    for context in contexts.values():
        context.close()
    for watch in watches.values():
        require(not watch.external, f"{label}: external requests {watch.external}")
        require(not watch.events and not watch.scripts, f"{label}: a script or stream ran")
    print(f"Master workspace without JavaScript passed at {label}.")


HOME_HEADING = "Latest edited sheets in campaigns I run"


def home_badge(page, sheet, label):
    """The badge of `sheet`'s row in the home block, read from a fresh load of `/`."""
    response = page.goto(f"{ORIGIN}/")
    require(response.status == 200, f"{label}: the home answered {response.status}")
    row = page.locator(
        f'section.home-block:has(h3:text-is("{HOME_HEADING}")) li:has(a[href="{sheet}"])'
    )
    require(row.count() == 1, f"{label}: no home row for {sheet}")
    found = re.search(r"· (Changed|Not opened|Opened)\b", row.inner_text())
    require(found, f"{label}: no badge in {row.inner_text()!r}")
    no_overflow(page, f"{label} home")
    return found.group(1)


def run_home(browser, viewport, clients, ids, version, *, javascript):
    """The Master's home block: badges per Master through a remote edit and acknowledgements."""
    label = f"{viewport['width']}x{viewport['height']} JS {'on' if javascript else 'off'}"
    table = Table(clients, ids, version, f"Home table {label}")
    sheet = table.create_sheet("table-player", "Home sheet")
    contexts = {
        login: open_context(browser, viewport, clients[login].cookie, javascript=javascript)
        for login in ("master-a", "master-b")
    }
    page_a, page_b = (contexts[login].new_page() for login in ("master-a", "master-b"))

    def acknowledge(login):
        action, fields, _ = clients[login].form(sheet, f"{sheet}/seen")
        clients[login].submit(action, fields)

    def badges():
        return home_badge(page_a, sheet, label), home_badge(page_b, sheet, label)

    got = badges()
    require(got == ("Not opened", "Not opened"), f"{label}: first badges {got}")
    acknowledge("master-a")
    got = badges()
    require(got == ("Opened", "Not opened"), f"{label}: after A's open {got}")
    table.edit("table-player", sheet, "Home sheet v2")
    got = badges()
    require(got == ("Changed", "Not opened"), f"{label}: after the edit {got}")
    acknowledge("master-a")
    got = badges()
    require(got == ("Opened", "Not opened"), f"{label}: after A's second open {got}")
    for context in contexts.values():
        context.close()
    print(f"Master home block passed at {label}.")


def run(binary, fixture):
    from playwright.sync_api import sync_playwright

    with tempfile.TemporaryDirectory(prefix="mimik-master-browser-") as temp:
        directory = fresh_directory(Path(temp) / "data")
        passwords = {login: secrets.token_urlsafe(18).encode() for login in (OWNER, *LOGINS)}
        enroll(binary, directory, passwords)
        ids = account_ids(binary, directory)
        prepare_unknown_registry_source(binary, directory, fixture)
        version = publish_assist_definition(binary, directory, fixture)
        state = {}

        def begin():
            state["process"], state["log"] = start(binary, directory, auth=True)

        def restart():
            stop(state["process"], state["log"])
            time.sleep(1)
            begin()

        begin()
        try:
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(args=["--disable-background-networking"])
                try:
                    for viewport in VIEWPORTS:
                        run_js(browser, viewport, passwords, ids, version, restart)
                        clients = {login: Client(login, passwords[login]) for login in LOGINS}
                        run_plain(browser, viewport, clients, ids, version)
                        run_paging(browser, viewport, passwords, ids, version)
                        for javascript in (True, False):
                            clients = {login: Client(login, passwords[login]) for login in LOGINS}
                            run_home(
                                browser, viewport, clients, ids, version, javascript=javascript
                            )
                finally:
                    browser.close()
        finally:
            stop(state["process"], state["log"])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path, help="A built mimik (release or development)")
    parser.add_argument("--fixture", type=Path, required=True, help="source_fixture output")
    args = parser.parse_args(argv)
    run(args.binary.resolve(strict=True), args.fixture.resolve(strict=True))
    print("Master workspace Chromium scenarios passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
