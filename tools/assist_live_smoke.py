"""Live smoke of the interpretation assistant against a hand-started llama-server (step
2c-1, ASSIST-40; prototype tooling, reported as live inference).

Builds the demo sheet with a development (dev-endpoint) build whose assistant points at the
server, then through the sheet's own JSON routes: a scribble starts a job, the reply is read
and Use applies the first option (or the run reports none); a second job, whose cached
prefix tokens the caller reads from the server log; a job superseded mid-generation, then
one that must finish within its usual time. Synthetic phrases only. Prints one JSON object.

Run where 127.0.0.1:<port> reaches the server (the screen's container network namespace):
  python3 tools/assist_live_smoke.py <dev mimik> --fixture <dir> --endpoint http://127.0.0.1:8080 \
      --token-file <file> --data <new dir>
"""

import argparse
import json
import sys
import time
from pathlib import Path

from assist_demo import (
    PLAYER,
    create_accounts,
    create_sheet,
    demo_passwords,
    fresh_directory,
)
from smoke_release import (
    FormFields,
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

CSRF_FORM = "/fields/hp"


def unit_csrf(cookie, sheet):
    html = auth_request(sheet, cookie=cookie)[2]
    parser = FormFields()
    parser.feed(html)
    form = parser.forms[f"{sheet}{CSRF_FORM}"]
    return form["csrf"], form["base"]


def post_json(path, cookie, form):
    return auth_request(path, cookie=cookie, form=form, headers={"Accept": "application/json"})


def scribble(cookie, sheet, text):
    csrf, base = unit_csrf(cookie, sheet)
    status, _, body = post_json(
        f"{sheet}/fields/hp", cookie, {"csrf": csrf, "base": base, "value": text}
    )
    require(status == 200, f"scribble save answered {status}")
    answer = json.loads(body)
    return csrf, answer["unit"]["note"]["key"]


def start_job(cookie, sheet, csrf, key):
    status, _, body = post_json(f"{sheet}/interpretations", cookie, {"csrf": csrf, "note": key})
    require(status in (200, 202), f"start answered {status}: {body[:200]}")
    return json.loads(body)["poll"]


def wait(cookie, poll, limit=600):
    began = time.monotonic()
    while time.monotonic() - began < limit:
        status, _, body = auth_request(poll, cookie=cookie, headers={"Accept": "application/json"})
        answer = json.loads(body)
        if answer.get("state") != "thinking":
            return answer, time.monotonic() - began
        time.sleep(0.5)
    raise AssertionError("job did not finish")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", type=Path)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--token-file", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument(
        "--deadline",
        type=int,
        default=None,
        help="--assistant-dev-deadline for an unprimed server (its first request evaluates "
        "the whole fixed prefix)",
    )
    args = parser.parse_args(argv)
    binary = args.binary.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    directory = fresh_directory(args.data)
    passwords = demo_passwords()
    create_accounts(binary, directory, passwords)
    prepare_unknown_registry_source(binary, directory, fixture)
    version = publish_assist_definition(binary, directory, fixture)
    extra = (
        "--assistant",
        "--assistant-dev-endpoint",
        args.endpoint,
        "--assistant-dev-token-file",
        str(args.token_file.resolve(strict=True)),
        *(
            ("--assistant-dev-deadline", str(args.deadline), "--assistant-dev-queue-wait", "600")
            if args.deadline
            else ()
        ),
    )
    process, log = start(binary, directory, auth=True, extra=extra)
    out = {}
    try:
        sheet = create_sheet(passwords[PLAYER], version)
        response = auth_request("/auth/login")
        cookie = auth_login(
            auth_cookie(response), auth_fields(response, "/auth/login"), PLAYER, passwords[PLAYER]
        )
        # 1. One smoke: a scribble, a job, its options, Use.
        csrf, key = scribble(cookie, sheet, "took 15 damage")
        answer, seconds = wait(cookie, start_job(cookie, sheet, csrf, key))
        out["first"] = {"seconds": round(seconds, 2), "state": answer["state"]}
        if answer["state"] == "options":
            out["first"]["options"] = [
                {k: option[k] for k in ("expression", "value", "effect")}
                for option in answer["options"]
            ]
            csrf, _ = unit_csrf(cookie, sheet)
            status, _, body = post_json(answer["options"][0]["action"], cookie, {"csrf": csrf})
            out["first"]["apply"] = {"status": status, "value": json.loads(body)["unit"]["value"]}
        # 2. A second request of the session (the prefix comes from the prompt cache).
        csrf, key = scribble(cookie, sheet, "healed 4")
        answer, seconds = wait(cookie, start_job(cookie, sheet, csrf, key))
        out["second"] = {"seconds": round(seconds, 2), "state": answer["state"]}
        # 3. A job cancelled mid-generation by a newer save, then one that must finish in its
        # usual time: the superseded request's slot is free.
        csrf, key = scribble(cookie, sheet, "lost a third of it, rounded down, after the fall")
        first_poll = start_job(cookie, sheet, csrf, key)
        time.sleep(20)
        csrf, key = scribble(cookie, sheet, "took 3 damage")
        began = time.monotonic()
        answer, seconds = wait(cookie, start_job(cookie, sheet, csrf, key))
        superseded, _ = wait(cookie, first_poll, limit=5)
        out["after_cancel"] = {
            "seconds": round(time.monotonic() - began, 2),
            "state": answer["state"],
            "superseded": superseded["state"],
        }
        log_text = (directory / "server.log").read_text()
        for phrase in ("took 15 damage", "healed 4", "took 3 damage"):
            require(phrase not in log_text, "player text reached the server log")
    finally:
        stop(process, log)
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
