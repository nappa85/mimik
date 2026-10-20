"""Assisted-sheet demo launcher (dev tool, prototype tier; docs/assisted-entry.md "Demo").

Builds a fresh local data directory from the synthetic source fixture, publishes the
`assist-sheet` definition, creates a demo Owner and player with generated dummy local
passwords, creates a campaign and a sheet (HP 84, Max HP 84, Armour -1; Equipment has a
quick-add line), then serves the loopback auth mode on http://127.0.0.1:3000 until
interrupted. Synthetic data only: no manuals, no real accounts, no secrets from the
environment.
"""

import argparse
import os
import secrets
import shutil
import stat
import sys
from pathlib import Path

from smoke_release import (
    FormFields,
    auth_cookie,
    auth_fields,
    auth_login,
    auth_request,
    prepare_unknown_registry_source,
    publish_assist_definition,
    registry_cli,
    request,
    require,
    start,
    stop,
    version_link,
    workspace_form,
    workspace_submit,
)

OWNER = "demo-owner"
PLAYER = "demo-player"
# The equipment list is field 4 of the assist definition (hp, max_hp, armour, luck, ...).
EQUIPMENT = 4
ROWS = (("Rope", "1", "1"), ("Torch", "3", "1"))


def fresh_directory(path):
    """Create `path` with mode 0700, or accept it only when it is an empty directory (never
    reuse data). The database and server log stay readable by the current user only.

    An existing empty directory supplied by the caller is accepted and its mode is changed
    to 0700 as well, so `--data` may name a directory the operator created beforehand."""
    if path.exists():
        require(path.is_dir() and not any(path.iterdir()), f"{path} is not an empty directory")
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.mkdir(mode=0o700)
    path.chmod(0o700)
    return path.resolve(strict=True)


def write_private(path, data):
    """A new 0600 file; refuses to overwrite."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
    require(stat.S_IMODE(path.stat().st_mode) == 0o600, f"{path} is not private")


def demo_passwords():
    """Generated dummy local passwords (never an operator's real credentials)."""
    return {OWNER: secrets.token_urlsafe(18).encode(), PLAYER: secrets.token_urlsafe(18).encode()}


def create_accounts(binary, directory, passwords):
    """Bootstraps the Owner and enrolls the player. The password files and the invitation
    token exist only while the CLI reads them: the launcher prints the passwords, so nothing
    needs them afterwards, and they are removed even when a step fails."""
    private = directory / "account-inputs"
    private.mkdir(mode=0o700)
    try:
        enroll(binary, directory, private, passwords)
    finally:
        shutil.rmtree(private)


def enroll(binary, directory, private, passwords):
    files = {}
    for login, password in passwords.items():
        files[login] = private / f"{login}.password"
        write_private(files[login], password)
    hidden = tuple(passwords.values())
    registry_cli(
        binary,
        directory,
        "bootstrap-owner",
        "--login",
        OWNER,
        "--display-name",
        "Demo Owner",
        "--password-file",
        str(files[OWNER]),
        secrets=hidden,
    )
    token = private / f"{PLAYER}-invite.token"
    registry_cli(
        binary,
        directory,
        "invite-account",
        "--login",
        PLAYER,
        "--display-name",
        "Demo Player",
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
        str(files[PLAYER]),
        secrets=hidden,
    )


def sheet_form(form):
    """The create form filled with the demo values; `form` holds the rendered rows."""
    values = dict(
        form,
        name="Assisted demo sheet",
        notes="",
        **{
            "value-hp": "84",
            "value-max_hp": "84",
            "value-armour": "-1",
            "null-luck": "known",
            "value-luck": "3",
        },
    )
    for r, (name, quantity, weight) in enumerate(ROWS):
        require(f"id-{EQUIPMENT}-{r}" in values, f"Equipment row {r + 1} was not added")
        values.update(
            {
                f"row-{EQUIPMENT}-{r}-0": name,
                f"row-{EQUIPMENT}-{r}-1": quantity,
                f"row-{EQUIPMENT}-{r}-2": weight,
                f"set-{EQUIPMENT}-{r}-3": "absent",
            }
        )
    values.pop("op", None)
    return values


def create_sheet(password, version):
    """As the player: sign in, create a campaign and the demo sheet. Returns the sheet path."""
    response = auth_request("/auth/login")
    cookie = auth_login(
        auth_cookie(response), auth_fields(response, "/auth/login"), PLAYER, password
    )
    _, fields, _ = workspace_form("/campaigns", cookie, "/campaigns")
    campaign = workspace_submit(
        cookie,
        "/campaigns",
        fields,
        title="Assisted demo campaign",
        game_id="synthetic-game",
        edition_id="first",
    )
    html = auth_request(campaign, cookie=cookie)[2]
    action, form, _ = workspace_form(version_link(html, campaign, version), cookie)
    for _ in ROWS:
        response = auth_request(action, cookie=cookie, form=dict(form, op=f"add-{EQUIPMENT}"))
        require(response[0] == 200, "Adding an equipment row failed")
        parser = FormFields()
        parser.feed(response[2])
        form = dict(parser.forms[action])
    sheet = workspace_submit(cookie, action, sheet_form(form)).split("#")[0]
    html = auth_request(sheet, cookie=cookie)[2]
    require(
        f'action="{sheet}/lists/{EQUIPMENT}/quick"' in html,
        "The demo sheet has no quick-add line on Equipment",
    )
    return sheet


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="A built mimik binary")
    parser.add_argument("--fixture", type=Path, required=True, help="source_fixture output")
    parser.add_argument("--data", type=Path, required=True, help="New, empty data directory")
    parser.add_argument(
        "--exit-after-setup",
        action="store_true",
        help="Stop the server once the sheet exists (a startup check; does not serve)",
    )
    args = parser.parse_args(argv)
    binary = args.binary.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    try:
        request("/")
    except OSError:
        pass
    else:
        raise SystemExit("Port 3000 is already in use; stop the other server first.")
    directory = fresh_directory(args.data)
    passwords = demo_passwords()
    create_accounts(binary, directory, passwords)
    prepare_unknown_registry_source(binary, directory, fixture)
    version = publish_assist_definition(binary, directory, fixture)
    process, log = start(binary, directory, auth=True)
    try:
        sheet = create_sheet(passwords[PLAYER], version)
        print("Assisted sheet demo (synthetic data, dummy local passwords):")
        print(f"  Sheet:   http://127.0.0.1:3000{sheet}")
        print(f"  Player:  {PLAYER} / {passwords[PLAYER].decode()}")
        print(f"  Owner:   {OWNER} / {passwords[OWNER].decode()} (administration only)")
        print(f"  Data:    {directory} (server log: {directory / 'server.log'})")
        print("  Try:     type '3 torches' in Equipment's Quick add line and press Enter")
        print("           or set Current HP above Max HP: the header says 'Not validated'")
        if args.exit_after_setup:
            print("Setup complete; stopping (--exit-after-setup).")
            return
        print("Serving; press Ctrl-C to stop.", flush=True)
        try:
            process.wait()
        except KeyboardInterrupt:
            pass
    finally:
        stop(process, log)


if __name__ == "__main__":
    sys.exit(main())
