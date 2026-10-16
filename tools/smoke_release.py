"""Real-process smoke check for the unpacked Linux release, without source/runtime assets."""

import argparse
import http.client
import json
import os
import re
import signal
import sqlite3
import stat
import subprocess
import tempfile
import time
from html.parser import HTMLParser
from http.cookies import SimpleCookie
from pathlib import Path
from urllib.parse import urlencode


def request(path, form=None, headers=None):
    connection = http.client.HTTPConnection("127.0.0.1", 3000, timeout=5)
    values = dict(headers or {})
    body = None
    if form is not None:
        body = urlencode(form)
        values.update(
            {
                "Content-Type": "application/x-www-form-urlencoded",
                "HX-Request": "true",
                "Origin": "http://127.0.0.1:3000",
            }
        )
    try:
        connection.request("POST" if form is not None else "GET", path, body, values)
        response = connection.getresponse()
        raw = response.read()
        body = raw if response.getheader("content-type", "").startswith("image/") else raw.decode()
        return response.status, dict(response.getheaders()), body
    finally:
        connection.close()


def revision(html):
    match = re.search(r'name="revision" value="(\d+)"', html)
    if not match:
        raise AssertionError("Rendered form lacks revision")
    return int(match[1])


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def registry_cli(binary, directory, *arguments, success=True, error=None, secrets=None):
    result = subprocess.run(
        [str(binary), "--database", str(directory / "smoke.sqlite3"), *arguments],
        cwd=directory,
        env=smoke_environment(directory),
        capture_output=True,
        timeout=30,
    )
    if secrets is not None:
        require_secret_free(result.stdout + result.stderr, secrets)
    require((result.returncode == 0) == success, "Registry CLI exit status unexpected")
    if error is not None:
        require(
            not success
            and result.returncode == 1
            and result.stdout == b""
            and result.stderr.strip() == f"Error: {error}".encode(),
            f"Expected typed CLI failure {error}; unexpected diagnostics redacted",
        )
    if secrets is not None and success:
        require(result.stderr == b"", "Account CLI emitted unexpected diagnostics (redacted)")
    return result.stdout


def require_secret_free(output, secrets):
    require(
        not any(secret in output for secret in secrets)
        and re.search(rb"[0-9a-f]{64}", output) is None
        and b"$argon2" not in output,
        "Account CLI exposed a password, credential or verifier (redacted)",
    )


def smoke_accounts(binary, directory):
    """Local operator CLI only; synthetic private files, no DB/verifier inspection."""
    private = directory / "account-inputs"
    private.mkdir(mode=0o700)
    require(stat.S_IMODE(private.stat().st_mode) == 0o700, "Account directory is not private")
    passwords = (b"Synthetic initial password 2026!", b"Synthetic replacement password 2026!")
    password_paths = [private / name for name in ("initial.password", "replacement.password")]
    for path, password in zip(password_paths, passwords, strict=True):
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(password)
        require(stat.S_IMODE(path.stat().st_mode) == 0o600, "Password file is not private")

    def cli(*arguments, error=None):
        raw = registry_cli(
            binary,
            directory,
            *arguments,
            success=error is None,
            error=error,
            secrets=passwords,
        )
        return json.loads(raw) if error is None else None

    def summary(actual, login, display, state, epoch, owner=False, account_id=None):
        require(
            type(actual) is dict
            and set(actual) == {"id", "login", "display_name", "state", "epoch", "owner"}
            and type(actual["id"]) is int
            and actual["id"] > 0
            and (account_id is None or actual["id"] == account_id)
            and actual["login"] == login
            and actual["display_name"] == display
            and actual["state"] == state
            and type(actual["epoch"]) is int
            and actual["epoch"] == epoch
            and actual["owner"] is owner,
            "Account summary identity/state/epoch/Owner projection differs",
        )
        return actual

    def show(account):
        require(
            cli("show-account", "--id", str(account["id"])) == account,
            "Account summary differs from committed transition",
        )

    def token_output(name):
        path = private / name
        require(not path.exists(), "Credential output already exists")
        return path

    def issued(path):
        metadata = path.lstat()
        require(
            stat.S_ISREG(metadata.st_mode)
            and stat.S_IMODE(metadata.st_mode) == 0o600
            and metadata.st_uid == os.geteuid()
            and metadata.st_size == 64,
            "Credential output is not a private 64-byte regular file",
        )

    bootstrap = (
        "bootstrap-owner",
        "--login",
        "smoke-owner",
        "--display-name",
        "Synthetic Owner",
        "--password-file",
        str(password_paths[0]),
    )
    owner = summary(cli(*bootstrap), "smoke-owner", "Synthetic Owner", "enabled", 1, True)
    cli(*bootstrap, error='Invalid("Owner bootstrap has already occurred.")')
    show(owner)
    unavailable = 'Invalid("Credential is unavailable, expired or superseded.")'
    first_invite = token_output("invite-first.token")
    invite = (
        "invite-account",
        "--login",
        "smoke-player",
        "--display-name",
        "Synthetic Player",
        "--token-output",
    )
    pending = summary(
        cli(*invite, str(first_invite)), "smoke-player", "Synthetic Player", "pending", 1
    )
    issued(first_invite)
    show(pending)
    second_invite = token_output("invite-second.token")
    pending = summary(
        cli(*invite, str(second_invite)),
        "smoke-player",
        "Synthetic Player",
        "pending",
        2,
        account_id=pending["id"],
    )
    issued(second_invite)
    show(pending)

    def redeem(command, token, password=0, error=None):
        return cli(
            command,
            "--token-file",
            str(token),
            "--password-file",
            str(password_paths[password]),
            error=error,
        )

    redeem("enroll-account", first_invite, error=unavailable)
    redeem("complete-account-reset", second_invite, error=unavailable)
    show(pending)
    player = summary(
        redeem("enroll-account", second_invite),
        "smoke-player",
        "Synthetic Player",
        "enabled",
        3,
        account_id=pending["id"],
    )
    redeem("enroll-account", second_invite, error=unavailable)
    show(player)

    def verify(account, password, expected):
        result = cli(
            "verify-account-password",
            "--id",
            str(account["id"]),
            "--password-file",
            str(password_paths[password]),
        )
        require(
            result == {"matched": expected} and result["matched"] is expected,
            "Account password verification result differs",
        )

    def reset(account, path, epoch=None, error=None):
        return cli(
            "issue-account-reset",
            "--id",
            str(account["id"]),
            "--expected-epoch",
            str(account["epoch"] if epoch is None else epoch),
            "--token-output",
            str(path),
            error=error,
        )

    verify(player, 0, True)
    first_reset = token_output("reset-first.token")
    player = summary(
        reset(player, first_reset),
        "smoke-player",
        "Synthetic Player",
        "enabled",
        4,
        account_id=player["id"],
    )
    issued(first_reset)
    show(player)  # Issuance itself advances the epoch, before redemption.
    verify(player, 0, True)
    stale_reset = token_output("reset-stale.token")
    reset(player, stale_reset, epoch=3, error="Conflict")
    require(not stale_reset.exists(), "Stale reset produced a credential file")
    redeem("enroll-account", first_reset, error=unavailable)
    show(player)
    second_reset = token_output("reset-second.token")
    player = summary(
        reset(player, second_reset),
        "smoke-player",
        "Synthetic Player",
        "enabled",
        5,
        account_id=player["id"],
    )
    issued(second_reset)
    redeem("complete-account-reset", first_reset, password=1, error=unavailable)
    show(player)
    player = summary(
        redeem("complete-account-reset", second_reset, password=1),
        "smoke-player",
        "Synthetic Player",
        "enabled",
        6,
        account_id=player["id"],
    )
    redeem("complete-account-reset", second_reset, password=1, error=unavailable)
    show(player)
    verify(player, 0, False)
    verify(player, 1, True)
    final_reset = token_output("reset-disable.token")
    player = summary(
        reset(player, final_reset),
        "smoke-player",
        "Synthetic Player",
        "enabled",
        7,
        account_id=player["id"],
    )
    issued(final_reset)
    show(player)
    disable = ("disable-account", "--id", str(player["id"]), "--expected-epoch")
    cli(*disable, "6", error="Conflict")
    show(player)
    player = summary(
        cli(*disable, "7"),
        "smoke-player",
        "Synthetic Player",
        "disabled",
        8,
        account_id=player["id"],
    )
    redeem("complete-account-reset", final_reset, password=1, error=unavailable)
    verify(player, 0, False)
    verify(player, 1, False)
    show(player)
    show(owner)
    verify(owner, 0, True)
    snapshot = [owner, player]
    require(cli("list-accounts") == snapshot, "Account list or durable Owner binding differs")
    require(
        cli("list-accounts", "--after-id", str(owner["id"])) == [player],
        "Account list cursor differs",
    )
    return snapshot, passwords


def check_accounts_retained(binary, directory, snapshot, passwords):
    for arguments, expected in [
        (("list-accounts",), snapshot),
        *[(("show-account", "--id", str(account["id"])), account) for account in snapshot],
    ]:
        require(
            json.loads(registry_cli(binary, directory, *arguments, secrets=passwords)) == expected,
            "Account epochs/status/Owner binding lost after service restart",
        )


def prepare_unknown_registry_source(binary, directory, fixture):
    command = [
        "register-ruleset",
        "--game-id",
        "synthetic-game",
        "--game-label",
        "Synthetic game",
        "--edition-id",
        "first",
        "--edition-label",
        "First edition",
    ]
    registry_cli(binary, directory, *command)
    registry_cli(binary, directory, *command)
    registry_cli(binary, directory, *command[:-1], "Wrong label", success=False)
    listing = json.loads(registry_cli(binary, directory, "list-rulesets"))
    unknown = next(entry for entry in listing if entry["game_id"] == "synthetic-game")
    require(
        unknown["edition_id"] == "first"
        and unknown["native_adapter"] is None
        and not unknown["generic_sheet_available"],
        "Registry falsely enables unknown-game character support",
    )
    registry_cli(
        binary,
        directory,
        "import-source",
        "--bundle",
        str(fixture / "unknown-bundle"),
        "--pdf",
        str(fixture / "original.pdf"),
    )
    return listing


def prepare_definition_draft(binary, directory, fixture):
    command = [
        "import-definition-draft",
        "--file",
        str(fixture / "definition.json"),
        "--expected-revision",
        "0",
    ]
    created = json.loads(registry_cli(binary, directory, *command))
    require(
        created["status"] == "draft" and created["revision"] == 1,
        "Draft was not revision-one proposal",
    )
    registry_cli(binary, directory, *command, success=False, error="Conflict")
    changed = json.loads((fixture / "definition.json").read_text(encoding="utf-8"))
    changed["title"] = "Updated synthetic draft"
    path = directory / "updated-definition.json"
    path.write_text(json.dumps(changed), encoding="utf-8")
    update = ["import-definition-draft", "--file", str(path), "--expected-revision", "1"]
    saved = json.loads(registry_cli(binary, directory, *update))
    require(
        saved["id"] == created["id"] and saved["revision"] == 2 and saved["status"] == "draft",
        "Draft update identity/revision invalid",
    )
    registry_cli(binary, directory, *update, success=False, error="Conflict")
    loaded = json.loads(
        registry_cli(binary, directory, "show-definition-draft", "--id", str(saved["id"]))
    )
    require(loaded == saved, "Stored draft differs after stale rejection")
    return saved


class FormFields(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.values = {}
        self.textarea = None
        self.forms = {}
        self.active_form = None
        self.select = None
        self.first_option = False

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "form":
            self.active_form = attrs.get("action")
            self.forms[self.active_form] = {}
        if tag == "input" and "name" in attrs:
            self.values[attrs["name"]] = attrs.get("value", "")
            if self.active_form:
                self.forms[self.active_form][attrs["name"]] = attrs.get("value", "")
        if tag == "textarea" and "name" in attrs:
            self.textarea = attrs["name"]
            self.values[self.textarea] = ""
            if self.active_form:
                self.forms[self.active_form][self.textarea] = ""
        if tag == "select" and "name" in attrs:
            self.select = attrs["name"]
            self.first_option = True
        if tag == "option" and self.select:
            if self.first_option or "selected" in attrs:
                value = attrs.get("value", "")
                self.values[self.select] = value
                if self.active_form:
                    self.forms[self.active_form][self.select] = value
            self.first_option = False

    def handle_data(self, data):
        if self.textarea:
            self.values[self.textarea] += data
            if self.active_form:
                self.forms[self.active_form][self.textarea] += data

    def handle_endtag(self, tag):
        if tag == "textarea":
            self.textarea = None
        if tag == "form":
            self.active_form = None
        if tag == "select":
            self.select = None


def assert_fields(html, expected):
    fields = FormFields()
    fields.feed(html)
    for name, value in expected.items():
        require(fields.values.get(name) == str(value), f"Persisted field differs: {name}")


def smoke_publication(binary, directory, fixture, draft):
    """CLI wiring lifecycle with synthetic originals and real source-review HTTP mutations."""

    def cli(*arguments, **options):
        return registry_cli(binary, directory, *arguments, **options)

    def compare(target, published=False):
        return json.loads(
            cli("compare-definition", "--id", str(target), *(["--published"] if published else []))
        )

    def approve(ticket, stale=False):
        result = cli(
            "approve-definition",
            "--id",
            str(ticket["target_id"]),
            *(["--published"] if ticket["target"] == "version" else []),
            "--comparison-hash",
            ticket["comparison_hash"],
            "--actor",
            "smoke-reviewer",
            "--rationale",
            "Compared synthetic original page and mapped region",
            "--originals-compared",
            success=not stale,
            error="Conflict" if stale else None,
        )
        return None if stale else json.loads(result)

    def show(version):
        return json.loads(cli("show-definition-version", "--id", str(version["id"])))

    def immutable(actual, expected, status):
        require(actual["wiring_status"] == status, f"Expected publication status {status}")
        for key in (
            "id",
            "version",
            "content_hash",
            "definition",
            "unresolved_fields",
            "advisory_tasks",
            "deterministic_rules",
        ):
            require(actual[key] == expected[key], f"Published {key} was mutated")

    def withdraw(version, approval=None, stale=False):
        cli(
            "withdraw-definition",
            "--id",
            str(version["id"]),
            "--expected-approval",
            str(version["approval_id"] if approval is None else approval),
            "--actor",
            "smoke-reviewer",
            "--rationale",
            "Synthetic withdrawal",
            success=not stale,
            error="Conflict" if stale else None,
        )

    def edit_review(action, **values):
        fields = FormFields()
        response = request(page_path)
        require(response[0] == 200, "Mapped source review page unavailable")
        fields.feed(response[2])
        form = dict(
            fields.forms[action],
            actor="smoke-reviewer",
            rationale="Synthetic publication invalidation regression",
        )
        form.update(values)
        require(request(action, form)[0] == 303, "Mapped source review mutation failed")

    section = re.search(
        r"<h2>Unknown-game synthetic source</h2>.*?</section>", request("/sources")[2], re.DOTALL
    )
    require(section is not None, "Publication source absent")
    page_link = re.search(r'href="(/sources/pages/(\d+))"', section[0])
    require(page_link is not None, "Publication page link absent")
    page_path, page_id = page_link.groups()
    fields = FormFields()
    fields.feed(request(page_path)[2])
    unit_path = next(action for action in fields.forms if "/units/" in action)
    unit_id = int(unit_path.rsplit("/", 1)[1])
    # Import output supplies the database source identity; page/unit IDs come from the source UI.
    imported = cli(
        "import-source",
        "--bundle",
        str(fixture / "unknown-bundle"),
        "--pdf",
        str(fixture / "original.pdf"),
    )
    source_match = re.search(rb"Registered source (\d+)\.", imported)
    require(source_match is not None, "Source import identity missing")
    source_id = int(source_match[1])
    manifest = json.loads((fixture / "unknown-bundle" / "bundle.json").read_text(encoding="utf-8"))
    body = dict(draft["definition"])
    body["sources"] = [
        {"source_id": source_id, "pdf_sha256": manifest["source_sha256"], "roles": ["sheet"]}
    ]
    body["fields"][0]["mapping"] = {
        "state": "mapped",
        "regions": [{"source_id": source_id, "page_id": int(page_id), "unit_id": unit_id}],
    }
    file = directory / "mapped-definition.json"

    def save(expected):
        file.write_text(json.dumps(body), encoding="utf-8")
        return json.loads(
            cli(
                "import-definition-draft",
                "--file",
                str(file),
                "--expected-revision",
                str(expected),
            )
        )

    draft = save(draft["revision"])
    before_review = compare(draft["id"])
    edit_review(unit_path, state="accepted", correction="")
    edit_review(page_path, printed_label="1")
    approve(before_review, stale=True)
    ticket = compare(draft["id"])
    require(ticket["original_page_urls"] == [page_path + "/image"], "Wrong original page evidence")
    require(
        ticket["original_region_urls"] == [unit_path + "/image"], "Wrong mapped region evidence"
    )
    for url in ticket["original_page_urls"] + ticket["original_region_urls"]:
        status, headers, image = request(url)
        require(
            status == 200
            and headers.get("content-type") == "image/png"
            and image.startswith(b"\x89PNG\r\n\x1a\n"),
            "Publication original page/region PNG inaccessible",
        )
    first = approve(ticket)
    require(
        first["version"] == 1
        and first["wiring_status"] == "current"
        and first["definition"] == draft["definition"]
        and first["unresolved_fields"] == 0
        and first["advisory_tasks"] == "unsupported"
        and first["deterministic_rules"] == "unsupported",
        "Initial mapped publication/capability scope invalid",
    )
    approve(ticket, stale=True)
    require(show(first) == first, "Stale approval changed published state")
    reused = approve(compare(draft["id"]))
    immutable(reused, first, "current")
    require(reused["approval_id"] > first["approval_id"], "Same-content approval was not appended")

    before_change = compare(first["id"], published=True)
    edit_review(
        unit_path, state="corrected", correction="Publication synthetic evidence correction"
    )
    stale = show(first)
    immutable(stale, first, "stale")
    require(stale["approval_id"] == reused["approval_id"], "Evidence edit silently approved wiring")
    approve(before_change, stale=True)
    require(show(first) == stale, "Rejected evidence ticket changed publication")
    edit_review(page_path, printed_label="1")
    immutable(show(first), first, "stale")
    reapproved = approve(compare(first["id"], published=True))
    immutable(reapproved, first, "current")
    require(
        reapproved["approval_id"] > reused["approval_id"], "Version reapproval was not appended"
    )

    # Page metadata alone invalidates wiring, independently of a unit correction.
    before_page_review = compare(first["id"], published=True)
    edit_review(page_path, printed_label="synthetic revised label")
    page_stale = show(first)
    immutable(page_stale, first, "stale")
    require(
        page_stale["approval_id"] == reapproved["approval_id"],
        "Page review silently approved wiring",
    )
    approve(before_page_review, stale=True)
    require(show(first) == page_stale, "Rejected page ticket changed publication")
    reapproved = approve(compare(first["id"], published=True))
    immutable(reapproved, first, "current")
    require(
        reapproved["approval_id"] > page_stale["approval_id"],
        "Page-invalidated version reapproval was not appended",
    )

    before_draft_change = compare(draft["id"])
    body["title"] = "New immutable synthetic version"
    draft = save(draft["revision"])
    approve(before_draft_change, stale=True)
    require(show(first) == reapproved, "Stale draft ticket changed old publication")
    second = approve(compare(draft["id"]))
    require(
        second["version"] == first["version"] + 1
        and second["id"] != first["id"]
        and second["content_hash"] != first["content_hash"]
        and second["definition"] == draft["definition"],
        "Changed content did not allocate a new immutable version",
    )
    require(show(first) == reapproved, "New version changed old publication")
    before_withdrawal = compare(second["id"], published=True)
    withdraw(second, approval=reapproved["approval_id"], stale=True)
    require(show(second) == second, "Stale withdrawal changed publication")
    withdraw(second)
    withdrawn = show(second)
    immutable(withdrawn, second, "withdrawn")
    require(
        withdrawn["approval_id"] == second["approval_id"], "Withdrawal replaced approval history"
    )
    withdraw(second, stale=True)
    require(show(second) == withdrawn, "Duplicate withdrawal changed publication")
    approve(before_withdrawal, stale=True)
    require(show(second) == withdrawn, "Pre-withdrawal ticket changed withdrawn publication")
    restored = approve(compare(second["id"], published=True))
    immutable(restored, second, "current")
    require(
        restored["approval_id"] > second["approval_id"], "Withdrawn reapproval was not appended"
    )
    withdraw(second, stale=True)
    require(show(second) == restored, "Old approval withdrew reapproved publication")
    return draft, [reapproved, restored]


def smoke_sources():
    sources = request("/sources")[2]
    lookup = re.search(r"<h2>Synthetic source</h2>.*?</section>", sources, re.DOTALL)
    require(lookup is not None, "Lookup-only synthetic source missing")
    match = re.search(r'href="(/sources/pages/\d+)"', lookup[0])
    require(match is not None, "Imported synthetic source missing")
    path = match[1]
    fields = FormFields()
    html = request(path)[2]
    fields.feed(html)
    unit_path = next(action for action in fields.forms if "/units/" in action)
    query = "/sources/search?" + urlencode(
        {"system": "mork-borg", "edition": "fixture-edition", "query": "capacity"}
    )
    require("No reviewed match" in request(query)[2], "Unreviewed source entered search")
    image = request(path + "/image")
    require(
        image[0] == 200
        and image[1]["content-type"] == "image/png"
        and image[2].startswith(b"\x89PNG\r\n\x1a\n"),
        "Source image unavailable",
    )
    unit = dict(
        fields.forms[unit_path],
        state="accepted",
        correction="",
        actor="smoke-reviewer",
        rationale="Synthetic review state test",
    )
    require(request(unit_path, unit)[0] == 303, "Unit approval failed")
    fields = FormFields()
    fields.feed(request(path)[2])
    page = dict(
        fields.forms[path],
        printed_label="1",
        actor="smoke-reviewer",
        rationale="Synthetic completeness test",
    )
    require(request(path, page)[0] == 303, "Page completeness failed")
    require("Strength capacity" in request(query)[2], "Reviewed evidence lookup failed")
    require(
        request(unit_path, dict(unit, state="corrected", correction="stale correction"))[0] == 409,
        "Stale review accepted",
    )
    require("Strength capacity" in request(query)[2], "Stale review changed evidence")
    fields = FormFields()
    fields.feed(request(path)[2])
    correction = dict(
        fields.forms[unit_path],
        state="corrected",
        correction="Verified fixture correction",
        actor="smoke-reviewer",
        rationale="Synthetic correction test",
    )
    require(request(unit_path, correction)[0] == 303, "Correction failed")
    require("No reviewed match" in request(query)[2], "Changed page remained searchable")
    fields = FormFields()
    fields.feed(request(path)[2])
    page = dict(
        fields.forms[path],
        printed_label="1",
        actor="smoke-reviewer",
        rationale="Rechecked synthetic completeness",
    )
    require(request(path, page)[0] == 303, "Reconfirmation failed")
    correction_query = "/sources/search?" + urlencode(
        {
            "system": "mork-borg",
            "edition": "fixture-edition",
            "query": "Verified fixture correction",
        }
    )
    require(
        '<pre class="evidence-text">Verified fixture correction' in request(correction_query)[2],
        "Corrected evidence missing",
    )
    return correction_query


def smoke_positive_advice():
    status, headers, _ = request(
        "/characters",
        {
            "name": "Positive advice fixture",
            "adapter_id": "mork-borg-it-2025",
            "notes": "synthetic",
        },
    )
    require(status == 303, "Positive context record creation failed")
    path = headers["location"].removesuffix("/mork-borg")
    sheet = {
        "agility": 0,
        "presence": 0,
        "strength": -3,
        "toughness": 0,
        "hp": 3,
        "max_hp": 5,
        "worn_armor": 0,
        "protection": 0,
        "standard_items": 6,
        "silver": 10,
        "omens": 2,
        "equipment": "synthetic context",
        "revision": 1,
    }
    saved = request(path + "/mork-borg", sheet)
    current = revision(saved[2])
    context = {
        "revision": current,
        "base": "yes",
        "can_act": "yes",
        "action_remaining": "no",
        "movement_remaining": "yes",
        "route_clear": "yes",
        "melee_reach": "yes",
        "incoming_attack": "yes",
        "blocking_armor": "no",
        "disoriented": "no",
        "power_uses": "3",
    }
    html = request(path + "/context", context)[2]
    require(
        "Ordinary melee attack — blocked standard path" in html,
        "Exhausted ordinary action not blocked",
    )
    require(
        "Ordinary room movement — standard check supported" in html,
        "Ordinary budget incorrectly blocked movement",
    )
    require(
        "Triggered defense reference — standard check supported" in html,
        "Ordinary budget incorrectly blocked defense",
    )
    require("meet or exceed DR 14" in html, "Context target incorrect")
    assert_fields(request(path + "/mork-borg")[2], dict(sheet, revision=current))


def auth_request(path, *, cookie=None, form=None, body=None, headers=None):
    """No redirects, implicit cookies, htmx headers or origin rewriting."""
    values = {"Host": "127.0.0.1:3000"}
    if cookie:
        values["Cookie"] = cookie
    if form is not None:
        body = urlencode(form)
    if body is not None:
        values.update(
            {"Origin": "http://127.0.0.1:3000", "Content-Type": "application/x-www-form-urlencoded"}
        )
    values.update(headers or {})
    connection = http.client.HTTPConnection("127.0.0.1", 3000, timeout=7)
    try:
        connection.request("POST" if body is not None else "GET", path, body, values)
        response = connection.getresponse()
        return response.status, dict(response.getheaders()), response.read().decode()
    finally:
        connection.close()


def auth_error(response, status, text):
    require(
        response[0] == status
        and response[2] == text
        and response[1].get("cache-control") == "no-store"
        and "set-cookie" not in response[1],
        f"Auth rejection differs: expected HTTP{status}, received HTTP{response[0]} (diagnostics redacted)",
    )


def auth_cookie(response):
    cookies = SimpleCookie()
    cookies.load(response[1].get("set-cookie", ""))
    require(
        set(cookies) == {"mimik_session"} and response[1].get("cache-control") == "no-store",
        "Auth session cookie missing",
    )
    cookie = cookies["mimik_session"]
    require(
        cookie["httponly"]
        and cookie["samesite"] == "Strict"
        and cookie["path"] == "/"
        and not cookie["secure"]
        and 0 < int(cookie["max-age"] or "0") <= 28800,
        "Auth cookie attributes differ",
    )
    return "mimik_session=" + cookie.value


def auth_fields(response, action):
    require(
        response[0] == 200 and response[1].get("cache-control") == "no-store",
        "Auth form unavailable",
    )
    fields = FormFields()
    fields.feed(response[2])
    require(
        action in fields.forms and len(fields.forms[action].get("csrf", "")) == 64,
        "Auth form CSRF absent",
    )
    return fields.forms[action]


def prepare_auth_player(binary, directory, passwords, *, login="auth-player"):
    """Reuse synthetic account input files, never an operator's actual credentials."""
    private = directory / "account-inputs"
    token = private / f"{login}-invite.token"
    registry_cli(
        binary,
        directory,
        "invite-account",
        "--login",
        login,
        "--display-name",
        "SyntheticLongDisplay" * 5,
        "--token-output",
        str(token),
        secrets=passwords,
    )
    return json.loads(
        registry_cli(
            binary,
            directory,
            "enroll-account",
            "--token-file",
            str(token),
            "--password-file",
            str(private / "initial.password"),
            secrets=passwords,
        )
    )


def publish_definition(binary, directory, definition, name):
    path = directory / name
    path.write_text(json.dumps(definition))
    draft = json.loads(
        registry_cli(
            binary,
            directory,
            "import-definition-draft",
            "--file",
            str(path),
            "--expected-revision",
            "0",
        )
    )
    comparison = json.loads(
        registry_cli(binary, directory, "compare-definition", "--id", str(draft["id"]))
    )
    return json.loads(
        registry_cli(
            binary,
            directory,
            "approve-definition",
            "--id",
            str(draft["id"]),
            "--comparison-hash",
            comparison["comparison_hash"],
            "--actor",
            "Synthetic fixture reviewer",
            "--rationale",
            "Known generated white page and mapped region fixture",
            "--originals-compared",
        )
    )


def version_link(html, campaign, version_id):
    """The create link of exactly `version_id`, never chosen by its position on the page."""
    link = f"{campaign}/sheets/new?version={version_id}"
    require(f'href="{link}"' in html, f"Published runtime version {version_id} missing")
    return link


def latest_version(directory, definition_id):
    """The ID of the newest published version of `definition_id`."""
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        row = db.execute(
            "SELECT id FROM definition_versions WHERE definition_id=? ORDER BY version DESC LIMIT 1",
            (definition_id,),
        ).fetchone()
    require(row is not None, f"No published version of {definition_id}")
    return row[0]


def prepare_workspace_materials(binary, directory, fixture):
    """Original synthetic fixtures only; no real manual/model approval.

    Returns the published version IDs of the generic and the formula definition.
    """
    registry_cli(
        binary,
        directory,
        "import-source",
        "--bundle",
        str(fixture / "adapter-bundle"),
        "--pdf",
        str(fixture / "original.pdf"),
    )
    prepare_unknown_registry_source(binary, directory, fixture)
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        source, page, unit = db.execute(
            "SELECT s.id,p.id,u.id FROM sources s JOIN source_pages p ON p.source_id=s.id "
            "JOIN source_units u ON u.page_id=p.id WHERE s.source_id='synthetic-unknown-core'"
        ).fetchone()
    manifest = json.loads((fixture / "unknown-bundle/bundle.json").read_text())
    sources = [{"source_id": source, "pdf_sha256": manifest["source_sha256"], "roles": ["sheet"]}]
    mapping = {
        "state": "mapped",
        "regions": [{"source_id": source, "page_id": page, "unit_id": unit}],
    }
    definition = json.loads((fixture / "definition.json").read_text())
    definition["sources"] = sources
    definition["fields"][0]["mapping"] = mapping
    generic = publish_definition(
        binary, directory, definition, "synthetic-workspace-definition.json"
    )
    formula = json.loads((fixture / "formula-definition.json").read_text())
    formula["sources"] = sources
    formula["fields"][0]["mapping"] = mapping
    published = publish_definition(binary, directory, formula, "synthetic-formula-definition.json")
    require(
        published["deterministic_rules"] == "formulas-advisory",
        "Formula definition is not published as advisory",
    )
    return generic["id"], published["id"]


def auth_login(cookie, fields, login, password):
    response = auth_request(
        "/auth/login", cookie=cookie, form=dict(fields, login=login, password=password.decode())
    )
    if response[0] == 429:
        auth_error(response, 429, "Too many authentication requests.")
        require(response[1].get("retry-after") == "6", "Auth throttle retry bound differs")
        time.sleep(6)
        response = auth_request(
            "/auth/login", cookie=cookie, form=dict(fields, login=login, password=password.decode())
        )
    require(
        response[0] == 303 and response[1].get("location") == "/auth/session",
        "Auth login failed (diagnostics redacted)",
    )
    return auth_cookie(response)


AUTH_DENIED_PATHS = (
    "/characters",
    "/characters/1",
    "/characters/1/mork-borg",
    "/characters/1/context",
    "/sources",
    "/sources/setup",
    "/sources/search",
    "/sources/search?system=mork-borg&edition=fixture&query=capacity",
    "/sources/pages/1",
    "/sources/pages/1/image",
    "/sources/units/1",
    "/sources/units/1/image",
)


def workspace_form(path, cookie, action=None):
    response = auth_request(path, cookie=cookie)
    require(response[0] == 200, "Workspace page unavailable")
    parser = FormFields()
    parser.feed(response[2])
    if action is None:
        require(len(parser.forms) == 1, "Expected one private editor form")
        action = next(iter(parser.forms))
    require(action in parser.forms, "Scoped workspace form missing")
    return action, dict(parser.forms[action]), response[2]


def workspace_submit(cookie, path, fields, **updates):
    response = auth_request(path, cookie=cookie, form=dict(fields, **updates))
    require(response[0] == 303, "Workspace mutation did not commit")
    return response[1]["location"]


def smoke_workspace(binary, directory, cookie, player, passwords):
    """Real private native/runtime flows; no authorization bypass or fabricated evidence."""
    _, fields, _ = workspace_form("/campaigns", cookie, "/campaigns")
    campaign = workspace_submit(
        cookie,
        "/campaigns",
        fields,
        title="Table campaign",
        game_id="mork-borg",
        edition_id="it-2025",
    )
    action, form, _ = workspace_form(campaign + "/sheets/new", cookie)
    form.update(
        name="Player private sheet",
        description="Private long text " * 700,
        strength="-3",
        standard_items="6",
        notes="Personal campaign notes",
    )
    sheet = workspace_submit(cookie, action, form)
    action, current, html = workspace_form(sheet, cookie)
    require("DR 14" in html and current["revision"] == "1", "Private native calculations differ")
    response = auth_request("/auth/login")
    administrative = auth_login(
        auth_cookie(response), auth_fields(response, "/auth/login"), "smoke-owner", passwords[0]
    )
    auth_error(auth_request(sheet, cookie=administrative), 403, "Access denied.")
    auth_error(auth_request("/campaigns", cookie=administrative), 403, "Access denied.")
    require(
        auth_request("/admin", cookie=administrative)[0] == 200,
        "Owner administration landing missing",
    )
    owner = prepare_auth_player(binary, directory, passwords, login="table-helper")
    response = auth_request("/auth/login")
    owner_cookie = auth_login(
        auth_cookie(response), auth_fields(response, "/auth/login"), "table-helper", passwords[0]
    )
    _, joining, outsider = workspace_form(campaign, owner_cookie, campaign + "/join")
    require("Player private sheet" not in outsider, "Outsider saw private sheet metadata")
    workspace_submit(owner_cookie, campaign + "/join", joining)
    request_path = campaign + "/requests/" + str(owner["id"])
    _, deciding, _ = workspace_form(campaign, cookie, request_path)
    workspace_submit(cookie, request_path, deciding, action="accept")
    auth_error(auth_request(sheet + "/history", cookie=owner_cookie), 403, "Access denied.")
    member_path = campaign + "/members/" + str(owner["id"])
    _, member, _ = workspace_form(campaign, cookie, member_path)
    workspace_submit(cookie, member_path, member, action="promote")
    _, master_form, _ = workspace_form(sheet, owner_cookie)
    workspace_submit(owner_cookie, sheet, master_form, notes="Explicit Master edit")
    stale = auth_request(
        sheet, cookie=cookie, form=dict(current, name="Stale <script>draft</script>")
    )
    require(stale[0] == 409 and "set-cookie" not in stale[1], "Stale private save did not reject")
    parser = FormFields()
    parser.feed(stale[2])
    require(
        parser.values["name"] == "Stale <script>draft</script>"
        and parser.values["revision"] == "1"
        and "<script>draft" not in stale[2],
        "Private conflict lost or executed the submitted draft",
    )
    _, member, _ = workspace_form(campaign, cookie, member_path)
    workspace_submit(cookie, member_path, member, action="demote")
    auth_error(auth_request(sheet, cookie=owner_cookie), 403, "Access denied.")
    action, owned_form, _ = workspace_form(campaign + "/sheets/new", owner_cookie)
    owned = workspace_submit(owner_cookie, action, owned_form, name="Collaborator personal sheet")
    _, member, _ = workspace_form(campaign, owner_cookie, member_path)
    workspace_submit(owner_cookie, member_path, member, action="leave")
    _, frozen, html = workspace_form(owned, owner_cookie)
    require("frozen" in html and "Save full private sheet" not in html, "Departure did not freeze")
    _, master_frozen, _ = workspace_form(owned, cookie)
    auth_error(auth_request(owned, cookie=cookie, form=master_frozen), 403, "Access denied.")
    _, joining, _ = workspace_form(campaign, owner_cookie, campaign + "/join")
    workspace_submit(owner_cookie, campaign + "/join", joining)
    _, deciding, _ = workspace_form(campaign, cookie, request_path)
    workspace_submit(cookie, request_path, deciding, action="accept")
    require(
        "Save full private sheet" not in auth_request(owned, cookie=owner_cookie)[2],
        "Rejoining thawed a retained sheet",
    )
    history = auth_request(owned + "/history", cookie=owner_cookie)
    require(history[0] == 200 and "freeze" in history[2], "Complete departure history unavailable")
    _, fields, _ = workspace_form("/campaigns", cookie, "/campaigns")
    generic = workspace_submit(
        cookie,
        "/campaigns",
        fields,
        title="Runtime campaign",
        game_id="synthetic-game",
        edition_id="first",
    )
    html = auth_request(generic, cookie=cookie)[2]
    generic_version = latest_version(directory, "sheet")
    print(f"Generic runtime sheet uses published version ID {generic_version} (definition 'sheet')")
    action, generic_form, _ = workspace_form(version_link(html, generic, generic_version), cookie)
    generic_sheet = workspace_submit(
        cookie,
        action,
        generic_form,
        name="Runtime private sheet",
        **{"value-name": "Canonical field", "null-name": "known"},
    )
    _, generic_form, _ = workspace_form(generic_sheet, cookie)
    workspace_submit(cookie, generic_sheet, generic_form, **{"value-name": ""})
    require(
        auth_request(generic_sheet + "/history/1", cookie=cookie)[0] == 200,
        "Pinned runtime history unavailable",
    )
    auth_error(auth_request(generic_sheet, cookie=owner_cookie), 403, "Access denied.")
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        require(
            db.execute("SELECT count(*) FROM private_sheets").fetchone()[0] == 3,
            "Unexpected private sheet count",
        )
        require(
            db.execute("SELECT count(*) FROM private_sheet_history").fetchone()[0] == 6,
            "Private mutation history is incomplete",
        )
        require(
            db.execute(
                "SELECT count(*) FROM characters WHERE name IN ('Player private sheet','Collaborator personal sheet','Runtime private sheet')"
            ).fetchone()[0]
            == 0,
            "Authenticated sheets leaked into prototype storage",
        )
    return sheet, owned, generic_sheet


def smoke_auth(binary, directory, passwords):
    player = prepare_auth_player(binary, directory, passwords)
    process, log = start(binary, directory, auth=True)
    replay = []
    try:
        require(
            "Mimik auth mode:" in (directory / "server.log").read_text(),
            "Main auth mode startup log missing",
        )
        root = auth_request("/")
        require(
            root[0] == 303 and root[1].get("location") == "/auth/login",
            "Anonymous root did not redirect to sign-in",
        )
        auth_error(
            auth_request("/auth/login", headers={"Host": "attacker.example:3000"}),
            400,
            "Invalid request.",
        )
        auth_error(auth_request("/auth/session"), 401, "Invalid credentials or session.")
        response = auth_request("/auth/login")
        cookie = auth_cookie(response)
        with sqlite3.connect(directory / "smoke.sqlite3") as db:
            row = db.execute(
                "SELECT issued_at,expires_at,state FROM auth_sessions WHERE id=?",
                (cookie.split("=", 1)[1],),
            ).fetchone()
            require(
                row is not None and row[1] - row[0] == 600 and row[2] == "active",
                "Anonymous auth deadline differs",
            )
        fields = auth_fields(response, "/auth/login")
        require('type="password"' in response[2], "Auth password control is not masked")
        form = dict(fields, login="smoke-owner", password=passwords[0].decode())
        for overrides, body, expected, text in [
            ({"Origin": "http://localhost:3000"}, urlencode(form), 403, "Access denied."),
            ({"Origin": ""}, urlencode(form), 403, "Access denied."),
            ({"Content-Type": "application/json"}, urlencode(form), 415, "Use a URL-encoded form."),
            ({}, urlencode(dict(form, csrf="wrong")), 403, "Access denied."),
            ({}, urlencode(dict(form, password="x" * 9000)), 413, "Request body is too large."),
            ({}, urlencode(dict(form, unexpected="value")), 422, "Request failed."),
        ]:
            rejected = auth_request("/auth/login", cookie=cookie, body=body, headers=overrides)
            auth_error(rejected, expected, text)
            require(passwords[0].decode() not in rejected[2], "Auth password echoed")
        for login in ("smoke-owner", "auth-player", "smoke-owner", "auth-player"):
            replay.append(cookie)
            cookie = auth_login(cookie, fields, login, passwords[0])
            require(cookie != replay[-1], "Login did not rotate session identity")
            status = auth_request("/auth/session", cookie=cookie)
            fields = auth_fields(status, "/auth/logout")
            require(login in status[2], "Current auth principal differs")
            for old in replay:
                auth_error(
                    auth_request("/auth/session", cookie=old),
                    401,
                    "Invalid credentials or session.",
                )
            for path in AUTH_DENIED_PATHS:
                auth_error(auth_request(path, cookie=cookie), 403, "Access denied.")
                auth_error(auth_request(path, cookie=cookie, form={}), 403, "Access denied.")
        with sqlite3.connect(directory / "smoke.sqlite3") as db:
            row = db.execute(
                "SELECT issued_at,expires_at,state FROM auth_sessions WHERE id=?",
                (cookie.split("=", 1)[1],),
            ).fetchone()
            require(
                row is not None and row[1] - row[0] == 28800 and row[2] == "active",
                "Auth fixed deadline differs",
            )
            version = db.execute("PRAGMA data_version").fetchone()
            for _ in range(3):
                read = auth_request("/auth/session", cookie=cookie)
                auth_fields(read, "/auth/logout")
                require("set-cookie" not in read[1], "Status read extended cookie")
            require(
                db.execute("PRAGMA data_version").fetchone() == version,
                "Status read wrote session storage",
            )
            require(
                db.execute(
                    "SELECT issued_at,expires_at,state FROM auth_sessions WHERE id=?",
                    (cookie.split("=", 1)[1],),
                ).fetchone()
                == row,
                "Status read changed fixed deadline",
            )
        retained = smoke_workspace(binary, directory, cookie, player, passwords)
        smoke_account_administration(directory, passwords)
    finally:
        stop(process, log)
    process, log = start(binary, directory, auth=True)
    try:
        fields = auth_fields(auth_request("/auth/session", cookie=cookie), "/auth/logout")
        require(
            all(auth_request(path, cookie=cookie)[0] == 200 for path in retained),
            "Campaign/private sheet access did not persist across restart",
        )
        private = directory / "account-inputs"
        reset = private / "auth-reset.token"
        player = json.loads(
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
        )
        auth_error(
            auth_request("/auth/session", cookie=cookie), 401, "Invalid credentials or session."
        )
        player = json.loads(
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
        )
        response = auth_request("/auth/login")
        anonymous = auth_cookie(response)
        fields = auth_fields(response, "/auth/login")
        auth_error(
            auth_request(
                "/auth/login",
                cookie=anonymous,
                form=dict(fields, login="auth-player", password=passwords[0].decode()),
            ),
            401,
            "Invalid credentials or session.",
        )
        current = auth_login(anonymous, fields, "auth-player", passwords[1])
        fields = auth_fields(auth_request("/auth/session", cookie=current), "/auth/logout")
        require(
            auth_request("/auth/logout", cookie=current, form=fields)[0] == 303, "Logout failed"
        )
        for old in [*replay, cookie, anonymous, current]:
            auth_error(
                auth_request("/auth/session", cookie=old), 401, "Invalid credentials or session."
            )
        response = auth_request("/auth/login")
        anonymous = auth_cookie(response)
        fields = auth_fields(response, "/auth/login")
        current = auth_login(anonymous, fields, "auth-player", passwords[1])
        registry_cli(
            binary,
            directory,
            "disable-account",
            "--id",
            str(player["id"]),
            "--expected-epoch",
            str(player["epoch"]),
            secrets=passwords,
        )
        auth_error(
            auth_request("/auth/session", cookie=current), 401, "Invalid credentials or session."
        )
        response = auth_request("/auth/login")
        anonymous = auth_cookie(response)
        fields = auth_fields(response, "/auth/login")
        auth_error(
            auth_request(
                "/auth/login",
                cookie=anonymous,
                form=dict(fields, login="auth-player", password=passwords[1].decode()),
            ),
            401,
            "Invalid credentials or session.",
        )
    finally:
        stop(process, log)
    smoke_owner_development(binary, directory, passwords)
    require_secret_free((directory / "server.log").read_bytes(), passwords)
    print(
        "Auth process stage passed: strict forms/cookies, rotation, denial, fixed reads, restart, reset, disable, logout replay."
    )
    print(
        "Auth black-box limits: 503 contention and clock-expiry races require core controlled gates; not exercised here."
    )


def issued_credential(response):
    require(response[0] == 200, "Browser credential issuance failed")
    for name, value in {
        "cache-control": "no-store",
        "x-frame-options": "DENY",
        "content-security-policy": "frame-ancestors 'none'",
        "referrer-policy": "same-origin",
    }.items():
        require(response[1].get(name) == value, "Credential response protection missing")
    require("set-cookie" not in response[1], "Credential issuance stored session data")
    token = re.search(
        r'id="credential-link" readonly type="text" value="http://[^"#]+/auth/(?:enroll|reset)#token=([0-9a-f]{64})"',
        response[2],
    )
    require(token is not None, "Issued credential shape differs")
    return token.group(1)


def credential_retry(response, status, token):
    require(
        response[0] == status and "set-cookie" not in response[1], "Credential retry status differs"
    )
    require(response[1].get("cache-control") == "no-store", "Credential retry cache policy differs")
    require('name="token" value="' + token + '"' in response[2], "Credential retry lost the token")
    require(
        'name="password"' in response[2] and 'value="abcdefghijkl"' not in response[2],
        "Credential retry echoed a password",
    )


def account_form_page(path, cookie=None):
    response = auth_request(path, cookie=cookie)
    if response[0] == 429:
        auth_error(response, 429, "Too many authentication requests.")
        require(response[1].get("retry-after") == "6", "Form quota retry bound differs")
        time.sleep(6)
        response = auth_request(path, cookie=cookie)
    require(response[0] == 200, "Account form unavailable")
    return response


def account_redemption_request(path, cookie, form):
    response = auth_request(path, cookie=cookie, form=form)
    if response[0] == 429:
        credential_retry(response, 429, form["token"])
        require(response[1].get("retry-after") == "6", "Redemption quota retry bound differs")
        time.sleep(6)
        response = auth_request(path, cookie=cookie, form=form)
    return response


def smoke_material_administration(directory, owner_cookie):
    require(auth_request("/admin/materials", cookie=owner_cookie)[0] == 200, "Materials missing")
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        page, source = db.execute(
            "SELECT p.id,p.source_id FROM source_pages p JOIN source_rulesets r "
            "ON r.source_id=p.source_id WHERE r.game_id='mork-borg' AND r.edition_id='it-2025'"
        ).fetchone()
        units = db.execute(
            "SELECT id FROM source_units WHERE page_id=? ORDER BY id", (page,)
        ).fetchall()
    require(
        auth_request(f"/admin/materials/sources/{source}", cookie=owner_cookie)[0] == 200,
        "Material pages missing",
    )
    path = f"/admin/materials/pages/{page}"
    for (unit,) in units:
        action = f"{path}/units/{unit}"
        response = auth_request(path, cookie=owner_cookie)
        fields = auth_fields(response, action)
        fields.update(state="accepted", correction="", rationale="Compared generated original")
        require(
            auth_request(action, cookie=owner_cookie, form=fields)[0] == 303,
            "Attributed material decision failed",
        )
        rejected = auth_request(
            action,
            cookie=owner_cookie,
            form=dict(fields, correction="<stale draft>", state="corrected"),
        )
        require(
            rejected[0] == 409 and "stale draft" in rejected[2],
            "Stale material decision lost draft",
        )
        image = request(action + "/image", headers={"Cookie": owner_cookie})
        require(image[0] == 200 and image[2].startswith(b"\x89PNG"), "Original crop missing")
    fields = auth_fields(auth_request(path, cookie=owner_cookie), path)
    fields.update(printed_label="synthetic", rationale="Whole original checked for omissions")
    require(auth_request(path, cookie=owner_cookie, form=fields)[0] == 303, "Completion failed")
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        require(
            db.execute(
                "SELECT count(*) FROM source_web_events w JOIN source_review_events e "
                "ON e.id=w.event_id WHERE e.page_id=? AND e.actor='account:'||w.actor_id",
                (page,),
            ).fetchone()[0]
            == len(units) + 1,
            "Material attribution/persistence differs",
        )


def smoke_account_administration(directory, passwords):
    response = account_form_page("/auth/login")
    owner_cookie = auth_login(
        auth_cookie(response), auth_fields(response, "/auth/login"), "smoke-owner", passwords[0]
    )
    smoke_material_administration(directory, owner_cookie)
    _, fields, _ = workspace_form("/admin", owner_cookie, "/admin/invite")
    fields.update(login="browser-player", display_name="Browser player", epoch="0")
    first = issued_credential(auth_request("/admin/invite", cookie=owner_cookie, form=fields))
    auth_error(
        auth_request("/admin/invite", cookie=owner_cookie, form=fields), 409, "Request failed."
    )
    fields["epoch"] = "1"
    token = issued_credential(auth_request("/admin/invite", cookie=owner_cookie, form=fields))
    replacement_invitation = token
    require(first != token, "Reissue did not replace credential")
    response = account_form_page("/auth/enroll")
    anonymous = auth_cookie(response)
    redemption = auth_fields(response, "/auth/enroll")
    redemption.update(token=first, password="abcdefghijkl", confirmation="abcdefghijkl")
    credential_retry(account_redemption_request("/auth/enroll", anonymous, redemption), 422, first)
    redemption["token"] = token
    require(
        account_redemption_request("/auth/enroll", anonymous, redemption)[0] == 303,
        "Browser enrollment failed",
    )
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        account_id, epoch = db.execute(
            "SELECT id,epoch FROM accounts WHERE login='browser-player'"
        ).fetchone()
    csrf = auth_fields(auth_request("/auth/session", cookie=owner_cookie), "/auth/logout")["csrf"]
    path = f"/admin/accounts/{account_id}"
    token = issued_credential(
        auth_request(
            path, cookie=owner_cookie, form={"csrf": csrf, "epoch": str(epoch), "action": "reset"}
        )
    )
    response = auth_request("/auth/reset", cookie=anonymous)
    redemption = auth_fields(response, "/auth/reset")
    redemption.update(token=token, password="mnopqrstuvwx", confirmation="mnopqrstuvwx")
    require(
        account_redemption_request("/auth/reset", anonymous, redemption)[0] == 303,
        "Browser password reset failed",
    )
    require(
        auth_request(
            path,
            cookie=owner_cookie,
            form={"csrf": csrf, "epoch": str(epoch + 2), "action": "disable"},
        )[0]
        == 303,
        "Browser disable failed",
    )
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        require(
            db.execute("SELECT state FROM accounts WHERE id=?", (account_id,)).fetchone()
            == ("disabled",),
            "Disabled account state missing",
        )
        require(
            db.execute(
                "SELECT count(*) FROM account_web_events w JOIN account_events e ON e.id=w.event_id WHERE e.account_id=?",
                (account_id,),
            ).fetchone()[0]
            == 4,
            "Authenticated administration audit incomplete",
        )
    require(
        token not in auth_request("/admin", cookie=owner_cookie)[2], "GET retrieved issued token"
    )
    require_secret_free(
        (directory / "server.log").read_bytes(),
        (
            *passwords,
            b"abcdefghijkl",
            b"mnopqrstuvwx",
            first.encode(),
            replacement_invitation.encode(),
            token.encode(),
        ),
    )
    print(
        "Browser account process stage passed: invite/reissue/enroll/reset/disable, protected tokens and authenticated audit."
    )


def smoke_environment(directory):
    # Runtime children need executable lookup, locale and timezone only. Do not
    # forward arbitrary credential variables, loader hooks or application config.
    allowed = {"PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ"}
    env = {name: value for name, value in os.environ.items() if name in allowed}
    env["MIMIK_DATABASE"] = str(directory / "smoke.sqlite3")
    return env


def smoke_owner_development(binary, directory, passwords):
    """Trusted CLI override and retained Owner records across production restart."""
    untouched = directory / "invalid-flags" / "must-not-exist.sqlite3"
    for flags in [("--no-auth", "--dev-owner-play"), ("--loopback", "list-accounts"), ("--auth",)]:
        result = subprocess.run(
            [str(binary), "--database", str(untouched), *flags],
            cwd=directory,
            env=smoke_environment(directory),
            capture_output=True,
            timeout=10,
            check=False,
        )
        require(
            result.returncode != 0 and not untouched.parent.exists(),
            "Invalid CLI flags accessed database",
        )
    process, log = start(binary, directory, auth=True, development_owner_play=True)
    try:
        response = auth_request("/auth/login")
        cookie = auth_login(
            auth_cookie(response), auth_fields(response, "/auth/login"), "smoke-owner", passwords[0]
        )
        status = auth_request("/auth/session", cookie=cookie)
        require("Development" in status[2], "Development Owner warning missing")
        _, fields, _ = workspace_form("/campaigns", cookie, "/campaigns")
        campaign = workspace_submit(
            cookie,
            "/campaigns",
            fields,
            title="Retained Owner development",
            game_id="mork-borg",
            edition_id="it-2025",
        )
        action, fields, _ = workspace_form(campaign + "/sheets/new", cookie)
        sheet = workspace_submit(cookie, action, fields, name="Retained development sheet")
        require(
            auth_request(sheet + "/history", cookie=cookie)[0] == 200,
            "Development history unavailable",
        )
    finally:
        stop(process, log)
    with sqlite3.connect(directory / "smoke.sqlite3") as db:
        retained = db.execute(
            "SELECT snapshot FROM private_sheets WHERE id=?", (int(sheet.rsplit("/", 1)[1]),)
        ).fetchone()
        history = db.execute(
            "SELECT * FROM private_sheet_history WHERE sheet_id=? ORDER BY revision",
            (int(sheet.rsplit("/", 1)[1]),),
        ).fetchall()
        require(
            retained is not None and bool(history), "Development sheet/history baseline missing"
        )
    process, log = start(binary, directory, auth=True)
    try:
        require(
            auth_request("/admin", cookie=cookie)[0] == 200,
            "Owner session lost on production restart",
        )
        for path in (campaign, sheet, sheet + "/history"):
            auth_error(auth_request(path, cookie=cookie), 403, "Access denied.")
    finally:
        stop(process, log)
    process, log = start(binary, directory, auth=True, development_owner_play=True)
    try:
        require(auth_request(sheet, cookie=cookie)[0] == 200, "Retained development grant lost")
        require(
            auth_request(sheet + "/history", cookie=cookie)[0] == 200,
            "Retained development history lost",
        )
        with sqlite3.connect(directory / "smoke.sqlite3") as db:
            require(
                db.execute(
                    "SELECT snapshot FROM private_sheets WHERE id=?",
                    (int(sheet.rsplit("/", 1)[1]),),
                ).fetchone()
                == retained,
                "Owner sheet changed on mode restart",
            )
            require(
                db.execute(
                    "SELECT * FROM private_sheet_history WHERE sheet_id=? ORDER BY revision",
                    (int(sheet.rsplit("/", 1)[1]),),
                ).fetchall()
                == history,
                "History changed on mode restart",
            )
    finally:
        stop(process, log)
    print(
        "Owner development process stage passed: invalid flags, explicit grants, production denial, retained snapshots/history."
    )


def start(binary, directory, *, auth=False, development_owner_play=False):
    # A fresh working directory proves embedded assets/templates are sufficient.
    log = open(directory / "server.log", "a", encoding="utf-8")
    process = subprocess.Popen(
        [
            str(binary),
            *(["--loopback"] if auth else ["--no-auth"]),
            *(["--dev-owner-play"] if development_owner_play else []),
        ],
        cwd=directory,
        env=smoke_environment(directory),
        stdout=log,
        stderr=log,
    )
    try:
        for _ in range(100):
            if process.poll() is not None:
                raise AssertionError("Packaged server exited before readiness")
            try:
                if request("/auth/login" if auth else "/")[0] == 200:
                    return process, log
            except OSError:
                pass
            time.sleep(0.1)
        raise AssertionError("Server readiness timeout")
    except BaseException:
        stop(process, log)
        raise


def stop(process, log):
    try:
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
                raise AssertionError("Server did not shut down gracefully") from None
        require(process.returncode == 0, "Server shutdown failed")
    finally:
        log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--auth", action="store_true", help="Also run loopback auth boundary smoke")
    parser.add_argument(
        "--source-fixture", type=Path, help="Generated synthetic source inputs; required by CI"
    )
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    # Refuse to mistake an unrelated process for this binary.
    try:
        request("/")
    except OSError:
        pass
    else:
        raise AssertionError("Port 3000 already in use")
    with tempfile.TemporaryDirectory(prefix="mimik-smoke-") as temp:
        directory = Path(temp)
        if args.source_fixture:
            fixture = args.source_fixture.resolve(strict=True)
            registry_listing = prepare_unknown_registry_source(binary, directory, fixture)
            definition_snapshot = prepare_definition_draft(binary, directory, fixture)
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
        account_snapshot, account_passwords = smoke_accounts(binary, directory)
        process, log = start(binary, directory)
        try:
            if args.source_fixture:
                require(
                    "synthetic-game / first" in request("/sources")[2],
                    "Registered unknown source identity absent",
                )
                definition_snapshot, publication_snapshots = smoke_publication(
                    binary, directory, fixture, definition_snapshot
                )
            for path, content_type in [
                ("/assets/app.css", "text/css"),
                ("/assets/htmx.min.js", "text/javascript"),
            ]:
                status, headers, body = request(path)
                require(
                    status == 200
                    and headers.get("content-type", "").startswith(content_type)
                    and body,
                    "Bundled asset missing",
                )
            require(
                request("/", headers={"Host": "attacker.example:3000"})[0] == 400,
                "Foreign authority accepted",
            )
            form = {
                "name": "Smoke character",
                "adapter_id": "mork-borg-it-2025",
                "notes": "initial",
            }
            status, headers, _ = request("/characters", form)
            require(status == 303, "Character creation failed")
            path = headers["location"].removesuffix("/mork-borg")
            form = {
                "name": "Smoke character",
                "system_id": "mork-borg",
                "edition": "MÖRK BORG — IT 2025",
                "notes": "initial",
            }
            initial = revision(request(path)[2])
            form.update(
                {"name": "Edited character", "notes": "persisted identity", "revision": initial}
            )
            response = request(path, form)
            require(
                response[0] == 200 and "Saved on this device" in response[2], "Identity save failed"
            )
            current = revision(response[2])
            require(current == initial + 1, "Identity revision did not increment exactly once")
            identity_state = dict(form, revision=current)
            stale = dict(form, notes="stale edit")
            require("another tab" in request(path, stale)[2], "Stale identity accepted")
            assert_fields(request(path)[2], identity_state)
            invalid = dict(form, name="", revision=current)
            require("Name must contain" in request(path, invalid)[2], "Invalid identity accepted")
            assert_fields(request(path)[2], identity_state)
            require(revision(request(path)[2]) == current, "Rejected identity changed revision")
            sheet_path = path + "/mork-borg"
            sheet = {
                "agility": 0,
                "presence": 0,
                "strength": -3,
                "toughness": 0,
                "hp": 0,
                "max_hp": 5,
                "worn_armor": 3,
                "protection": 1,
                "standard_items": 6,
                "silver": 10,
                "omens": 2,
                "equipment": "persisted gear",
                "revision": current,
            }
            html = request(sheet_path, sheet)[2]
            require(
                "Sheet saved" in html and "against DR 16" in html and "Overburdened" in html,
                "Sheet save/calculation failed",
            )
            next_revision = revision(html)
            require(next_revision == current + 1, "Sheet revision did not increment exactly once")
            sheet_state = dict(sheet, revision=next_revision)
            require(
                "another tab" in request(path, dict(form, revision=current))[2],
                "Sheet revision did not protect identity",
            )
            assert_fields(request(path)[2], dict(identity_state, revision=next_revision))
            require(
                "another tab"
                in request(sheet_path, dict(sheet, equipment="stale gear", silver=999))[2],
                "Stale sheet accepted",
            )
            assert_fields(request(sheet_path)[2], sheet_state)
            require(
                "Base ability modifiers"
                in request(sheet_path, dict(sheet, strength=20, revision=next_revision))[2],
                "Invalid stats accepted",
            )
            assert_fields(request(sheet_path)[2], sheet_state)
            require(
                revision(request(sheet_path)[2]) == next_revision, "Rejected stats changed revision"
            )
            # Identity -> sheet protection in the other direction.
            form["revision"] = next_revision
            html = request(path, form)[2]
            final_revision = revision(html)
            require(
                final_revision == next_revision + 1,
                "Shared revision did not increment exactly once",
            )
            require(
                "another tab" in request(sheet_path, dict(sheet, revision=next_revision))[2],
                "Identity revision did not protect sheet",
            )
            assert_fields(request(sheet_path)[2], dict(sheet_state, revision=final_revision))
            context = {
                "revision": final_revision,
                "base": "yes",
                "can_act": "yes",
                "action_remaining": "no",
                "movement_remaining": "yes",
                "route_clear": "yes",
                "incoming_attack": "yes",
                "blocking_armor": "yes",
                "disoriented": "no",
                "power_uses": "3",
            }
            context_path = path + "/context"
            context_response = request(context_path, context)
            require(
                context_response[0] == 200
                and "broken outcome" in context_response[2]
                and "Saved-sheet snapshot" in context_response[2]
                and "meet or exceed DR" not in context_response[2],
                "Broken-context adjudication guard failed",
            )
            require(
                request(context_path, dict(context, revision=final_revision - 1))[0] == 409,
                "Stale context accepted",
            )
            assert_fields(request(sheet_path)[2], dict(sheet_state, revision=final_revision))
            smoke_positive_advice()
            correction_query = smoke_sources() if args.source_fixture else None
        finally:
            stop(process, log)
        process, log = start(binary, directory)
        try:
            check_accounts_retained(binary, directory, account_snapshot, account_passwords)
            identity_html = request(path)[2]
            if args.source_fixture:
                for snapshot in publication_snapshots:
                    require(
                        json.loads(
                            registry_cli(
                                binary,
                                directory,
                                "show-definition-version",
                                "--id",
                                str(snapshot["id"]),
                            )
                        )
                        == snapshot,
                        "Immutable publication/approval lost after service restart",
                    )
                require(
                    json.loads(registry_cli(binary, directory, "list-rulesets"))
                    == registry_listing,
                    "Registry changed after server restart",
                )
                require(
                    json.loads(
                        registry_cli(
                            binary,
                            directory,
                            "show-definition-draft",
                            "--id",
                            str(definition_snapshot["id"]),
                        )
                    )
                    == definition_snapshot,
                    "Definition draft lost after server restart",
                )
                require(
                    "synthetic-game / first" in request("/sources")[2],
                    "Unknown source association lost after restart",
                )
            sheet_html = request(sheet_path)[2]
            assert_fields(identity_html, dict(identity_state, revision=final_revision))
            assert_fields(sheet_html, dict(sheet_state, revision=final_revision))
            require(
                "Edited character" in identity_html
                and "persisted identity" in identity_html
                and "stale edit" not in identity_html,
                "Identity persistence failed",
            )
            require(
                "persisted gear" in sheet_html and 'name="hp" value="0"' in sheet_html,
                "Sheet persistence failed",
            )
            require(
                revision(identity_html) == final_revision
                and revision(sheet_html) == final_revision,
                "Revision persistence failed",
            )
            if correction_query:
                require(
                    '<pre class="evidence-text">Verified fixture correction'
                    in request(correction_query)[2],
                    "Reviewed evidence persistence failed",
                )
        finally:
            stop(process, log)
        if args.auth:
            smoke_auth(binary, directory, account_passwords)
    print("Unpacked release startup/edit/validation/conflict/restart/persistence smoke passed.")
    print(
        "Local operator account CLI lifecycle/typed rejection/restart smoke passed (no account UI)."
    )
    if args.source_fixture:
        print("Publication CLI lifecycle and synthetic source-review evidence smoke passed.")


if __name__ == "__main__":
    main()
