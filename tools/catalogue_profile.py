"""Profile catalogue publication through the real CLI (plan docs/catalogue-plan.md R8, §9).

Synthetic data only. Seeds a scratch database with one finalized synthetic source
(1,024 pages x 16 units), imports a catalogue draft at the plan's maxima, and measures
`import-catalogue-draft`, `compare-catalogue`, `publish-catalogue`, `show-catalogue-version`,
`verify-catalogues` and `rebuild-catalogue-index`, each in a fresh wrapper process so peak RSS
is that command's own (RUSAGE_CHILDREN, as tools/definition_profile.py). Every publish runs
under a lock probe that measures its write-lock hold with both endpoints and a contender that
writes like the server (BEGIN IMMEDIATE, 5 s busy timeout) while the publish holds the lock.
Cases: v1, v2 (update) and v3 of the calibrated shape, then a maximum fan-out catalogue at the
caps, published and updated (plan docs/catalogue-plan.md §12).
"""

import argparse
import json
import resource
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

GAME = "synthetic-game"
EDITION = "first"
PAGES = 1024
UNITS_PER_PAGE = 16
UNITS_PER_ENTRY = 3
TYPES = 8
OWNER_EVERY = 8
OWNER_TOTAL = 1024 * 1024
WORDS = [
    "lantern",
    "ombra",
    "città",
    "flare",
    "veil",
    "ferro",
    "glimmer",
    "crypt",
    "salt",
    "ember",
    "nebbia",
    "warden",
    "spina",
    "hollow",
    "mörk",
    "tide",
]
ZEROS = "0" * 64
MAX_DRAFT_BYTES = 4 * 1024 * 1024
MAX_PROPERTIES = 16
MAX_UNITS_PER_ENTRY = 16
MAX_UNIT_REFERENCES = 24_576
FANOUT_HEADROOM = 16 * 1024


class Rng:
    """Deterministic LCG, so every run profiles the same draft."""

    def __init__(self, seed):
        self.state = seed

    def below(self, bound):
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        return (self.state >> 33) % bound


def words(rng, count):
    return " ".join(WORDS[rng.below(len(WORDS))] for _ in range(count))


def draft(entries, extra=0, catalogue_id="spells", capacity=None):
    """A catalogue at the plan's maxima: 8 types, every 8th entry owner-written (owner text
    sized so `capacity` entries hold 1 MiB in total), the others citing 3 units (21,504
    references at 8,192 entries, under the 24,576 cap)."""
    rng = Rng(1000)
    types = [
        {
            "id": f"type-{t}",
            "label": f"Type {t}",
            "properties": [
                {"id": "level", "label": "Level", "control": "integer"},
                {"id": "school", "label": "School", "control": "text"},
            ],
        }
        for t in range(TYPES)
    ]
    owners = -(-(capacity or entries + extra) // OWNER_EVERY)
    owner_len = min(OWNER_TOTAL // max(owners, 1), 20_000)
    units_total = PAGES * UNITS_PER_PAGE
    listed = []
    for index in range(entries + extra):
        if index % OWNER_EVERY == 0:
            description = {"owner_text": "h" * owner_len}
        else:
            units = []
            for k in range(UNITS_PER_ENTRY):
                unit = (index * UNITS_PER_ENTRY + k) % units_total + 1
                page = (unit - 1) // UNITS_PER_PAGE + 1
                units.append({"source_id": 1, "page_id": page, "unit_id": unit})
            description = {"units": units}
        listed.append(
            {
                "key": f"entry-{index:05}",
                "type": f"type-{index % TYPES}",
                "name": f"{words(rng, 3)} {index}",
                "properties": {"level": rng.below(10), "school": words(rng, 2)},
                "description": description,
            }
        )
    return {
        "contract_version": 1,
        "game_id": GAME,
        "edition_id": EDITION,
        "catalogue_id": catalogue_id,
        "title": "Synthetic spells",
        "sources": [{"source_id": 1, "pdf_sha256": ZEROS}],
        "entry_types": types,
        "entries": listed,
    }


def fanout(entries=8192, variant=0, catalogue_id="fanout", references=MAX_UNIT_REFERENCES):
    """Maximum fan-out at the caps (plan §12): 8 types x 16 properties (integer, boolean,
    enum, text), `references / 16` evenly spread entries citing 16 units each (the reference
    cap; units numbered in turn, so every one of the 16,384 units is cited), the rest
    owner-written (up to 1 MiB in all), and owner text then text property values sized so the
    draft is just under the 4 MiB cap. `variant` 1 changes one
    property value: an update with every key and type kept."""
    controls = ("integer", "boolean", "enum", "text")
    options = [{"id": f"o{n}", "label": f"Option {n}"} for n in range(8)]
    types = []
    for t in range(TYPES):
        properties = []
        for n in range(MAX_PROPERTIES):
            control = controls[n % 4]
            item = {"id": f"p{n:02}", "label": f"Property {n}", "control": control}
            if control == "enum":
                item["options"] = options
            properties.append(item)
        types.append({"id": f"type-{t}", "label": f"Type {t}", "properties": properties})
    citing = references // MAX_UNITS_PER_ENTRY
    cited = [index for index in range(entries) if (index * citing) % entries < citing]
    order = {index: number for number, index in enumerate(cited)}
    owners = max(entries - len(cited), 1)
    units_total = PAGES * UNITS_PER_PAGE

    def build(text_len, owner_len):
        rng = Rng(2000)
        listed = []
        for index in range(entries):
            if index in order:
                first = order[index] * MAX_UNITS_PER_ENTRY
                units = []
                for k in range(MAX_UNITS_PER_ENTRY):
                    unit = (first + k) % units_total + 1
                    page = (unit - 1) // UNITS_PER_PAGE + 1
                    units.append({"source_id": 1, "page_id": page, "unit_id": unit})
                description = {"units": units}
            else:
                description = {"owner_text": "h" * owner_len}
            properties = {}
            for n in range(MAX_PROPERTIES):
                control = controls[n % 4]
                if control == "integer":
                    properties[f"p{n:02}"] = rng.below(1000)
                elif control == "boolean":
                    properties[f"p{n:02}"] = rng.below(2) == 1
                elif control == "enum":
                    properties[f"p{n:02}"] = f"o{rng.below(8)}"
                else:
                    properties[f"p{n:02}"] = ("v" + words(rng, 40))[:text_len].ljust(text_len, "x")
            if variant and index == 0:
                properties["p00"] = 1000
            listed.append(
                {
                    "key": f"entry-{index:05}",
                    "type": f"type-{index % TYPES}",
                    "name": f"{words(rng, 3)} {index}",
                    "properties": properties,
                    "description": description,
                }
            )
        return {
            "contract_version": 1,
            "game_id": GAME,
            "edition_id": EDITION,
            "catalogue_id": catalogue_id,
            "title": "Synthetic fan-out",
            "sources": [{"source_id": 1, "pdf_sha256": ZEROS}],
            "entry_types": types,
            "entries": listed,
        }

    # References and properties are at their caps; the remaining bytes go to owner text (up
    # to its 1 MiB total), then to the text property values.
    text_properties = MAX_PROPERTIES // 4
    base = len(json.dumps(build(1, 1), ensure_ascii=False, separators=(",", ":")).encode())
    room = MAX_DRAFT_BYTES - FANOUT_HEADROOM - base
    if room < 0:
        raise AssertionError(f"Fan-out skeleton exceeds the 4 MiB cap by {-room} bytes")
    owner_len = max(1, min(20_000, OWNER_TOTAL // owners, 1 + room // owners))
    room -= (owner_len - 1) * owners
    text_len = max(1, min(1000, 1 + room // (entries * text_properties)))
    return build(text_len, owner_len)


def encoded(value):
    data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
    if len(data) > MAX_DRAFT_BYTES:
        raise AssertionError(f"Draft exceeds the 4 MiB cap: {len(data)} bytes")
    return data


def seed(database):
    """One finalized synthetic source of the pair, inserted directly (synthetic text)."""
    db = sqlite3.connect(database)
    db.execute("PRAGMA foreign_keys=ON")
    prepared = json.dumps({"contract_version": 1, "extraction_sha256": ZEROS}).encode()
    with db:
        db.execute(
            "INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,"
            "pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(1,?1,'fixture','Fixture',"
            "?2,'First','en',?1,x'',x'',?3,?4)",
            (ZEROS, GAME, prepared, PAGES),
        )
        db.execute("INSERT INTO source_rulesets VALUES(1,?1,?2)", (GAME, EDITION))
        rng = Rng(7)
        for page in range(1, PAGES + 1):
            db.execute(
                "INSERT INTO source_pages(id,source_id,pdf_page,png,printed_label) "
                "VALUES(?1,1,?1,x'',?2)",
                (page, str(page)),
            )
            for k in range(UNITS_PER_PAGE):
                unit = (page - 1) * UNITS_PER_PAGE + k + 1
                db.execute(
                    "INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) "
                    "VALUES(?1,?2,?3,'text','[[0,0],[1,0],[1,1],[0,1]]','',?4)",
                    (unit, page, f"b{k}", words(rng, 90)),
                )
        for page in range(1, PAGES + 1):
            db.execute("INSERT INTO source_page_hashes VALUES(?1,1,?2)", (page, ZEROS))
        db.execute("INSERT INTO source_finalizations VALUES(1,?1,?1)", (ZEROS,))
        db.execute("CREATE TABLE profile_writes(id INTEGER PRIMARY KEY, at TEXT)")
    db.close()


def cli(binary, database, *args, check=True):
    result = subprocess.run(
        [str(binary), "--database", str(database), *args], capture_output=True, timeout=600
    )
    if check and result.returncode != 0:
        raise AssertionError(f"{args[0]} failed: {result.stderr.decode(errors='replace')[:400]}")
    return result


def measured(binary, database, args):
    """Run one CLI command in a fresh wrapper process; return its wall time and peak RSS."""
    result = subprocess.run(
        [sys.executable, __file__, "--measure", str(binary), str(database), "--", *args],
        capture_output=True,
        timeout=900,
        check=True,
    )
    return json.loads(result.stdout)


def measure_child(binary, database, args):
    started = time.perf_counter()
    result = subprocess.run(
        [str(binary), "--database", str(database), *args], capture_output=True, timeout=600
    )
    print(
        json.dumps(
            {
                "command": args[0],
                "process_ms": round((time.perf_counter() - started) * 1000, 1),
                "peak_child_rss_kib": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,
                "success": result.returncode == 0,
                "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace")[:400],
            }
        )
    )


def lock_intervals(polls, cutoff=None):
    """Group non-blocking lock polls `(time, acquired)` (in time order) into refused intervals.

    Each interval is `(previous_free, first_refused, last_refused, next_free)`: the lock was
    taken in `(previous_free, first_refused]` and released in `(last_refused, next_free]`, so
    the hold lies between `last_refused - first_refused` and `next_free - previous_free`.
    `previous_free`/`next_free` are None when no free poll precedes or follows. Polls at or
    after `cutoff` (the contender's own acquisition) are ignored."""
    intervals = []
    previous_free = None
    current = None
    for at, free in polls:
        if cutoff is not None and at >= cutoff:
            break
        if free:
            if current is not None:
                intervals.append((current[0], current[1], current[2], at))
                current = None
            previous_free = at
        elif current is None:
            current = [previous_free, at, at]
        else:
            current[2] = at
    if current is not None:
        intervals.append((current[0], current[1], current[2], cutoff))
    return intervals


def hold_bounds(interval, origin):
    """Millisecond bounds of one refused interval relative to `origin` (the publish launch)."""
    previous_free, first_refused, last_refused, next_free = interval

    def ms(value):
        return None if value is None else round((value - origin) * 1000, 1)

    return {
        "acquired_after_ms": [ms(previous_free), ms(first_refused)],
        "released_after_ms": [ms(last_refused), ms(next_free)],
        "hold_ms_lower": round((last_refused - first_refused) * 1000, 1),
        "hold_ms_upper": None
        if previous_free is None or next_free is None
        else round((next_free - previous_free) * 1000, 1),
    }


def probe_publish(binary, database, command, contend_after=0.05):
    """Measure one publish's write-lock hold with both endpoints (plan §12).

    A poller thread probes the write lock without waiting (`BEGIN IMMEDIATE` with a zero busy
    timeout, then `ROLLBACK`) from before the publish process is launched until it exits, so
    the first refused probe marks the acquisition and the first free probe after it the
    release, each within one poll period. Once a refusal has lasted `contend_after` seconds
    (past any short lock taken at start-up), a contender thread runs the server's write
    pattern on its own connection: `BEGIN IMMEDIATE` with the server's 5 s busy timeout, one
    insert, `COMMIT` (the lock-level equivalent of a sheet save). Polls after the contender
    acquires the lock are not attributed to the publish."""
    polls = []
    stop = threading.Event()
    contender = {}

    def contend():
        started = time.perf_counter()
        db = sqlite3.connect(database, timeout=5.0, isolation_level=None)
        try:
            db.execute("BEGIN IMMEDIATE")
            contender["acquired"] = time.perf_counter()
            db.execute("INSERT INTO profile_writes(at) VALUES('contended save')")
            db.execute("COMMIT")
            contender.update(success=True, committed=time.perf_counter())
        except sqlite3.OperationalError as error:
            contender.update(success=False, error=str(error))
        finally:
            contender["started"] = started
            db.close()

    def poll():
        db = sqlite3.connect(database, timeout=0, isolation_level=None)
        refused_since = None
        thread = None
        while not stop.is_set():
            at = time.perf_counter()
            try:
                db.execute("BEGIN IMMEDIATE")
                db.execute("ROLLBACK")
                free = True
                refused_since = None
            except sqlite3.OperationalError:
                free = False
                refused_since = refused_since or at
            polls.append((at, free))
            if thread is None and refused_since and at - refused_since >= contend_after:
                thread = threading.Thread(target=contend)
                thread.start()
            time.sleep(0.0005)
        db.close()
        if thread is not None:
            thread.join(timeout=30)

    poller = threading.Thread(target=poll)
    poller.start()
    while sum(1 for _, free in polls if free) < 20:
        time.sleep(0.001)
    launched = time.perf_counter()
    measured_result = measured(binary, database, command)
    stop.set()
    poller.join(timeout=60)
    cutoff = contender.get("acquired")
    intervals = [i for i in lock_intervals(polls, cutoff) if i[1] >= launched]
    gaps = [b[0] - a[0] for a, b in zip(polls, polls[1:], strict=False) if a[0] >= launched]
    longest = max(intervals, key=lambda i: i[2] - i[1], default=None)
    lock = {
        "refused_intervals": len(intervals),
        "poll_gap_max_ms": round(max(gaps, default=0) * 1000, 2),
        "publish_hold": None if longest is None else hold_bounds(longest, launched),
        "other_intervals_ms": [
            round((i[2] - i[1]) * 1000, 1) for i in intervals if i is not longest
        ],
    }
    if contender:
        lock["contender"] = {
            "success": contender.get("success", False),
            "started_after_ms": round((contender["started"] - launched) * 1000, 1),
            "wait_ms": None
            if "acquired" not in contender
            else round((contender["acquired"] - contender["started"]) * 1000, 1),
            "error": contender.get("error"),
        }
    measured_result["lock"] = lock
    return measured_result


def publish_case(binary, database, value, name, root, revision):
    """Import `value` over draft `revision` (0 creates it) and compare it; returns the rows,
    the draft id and the ticket."""
    path = root / f"{name}.json"
    path.write_bytes(encoded(value))
    rows = {"draft_bytes": path.stat().st_size}
    rows["import"] = measured(
        binary,
        database,
        ["import-catalogue-draft", "--file", str(path), "--expected-revision", str(revision)],
    )
    if not rows["import"]["success"]:
        raise AssertionError(f"{name} import failed: {rows['import']['stderr']}")
    draft_id = json.loads(rows["import"]["stdout"])["id"]
    rows["compare"] = measured(binary, database, ["compare-catalogue", "--id", str(draft_id)])
    if not rows["compare"]["success"]:
        raise AssertionError(f"{name} compare failed: {rows['compare']['stderr']}")
    ticket = json.loads(rows["compare"]["stdout"])["comparison_hash"]
    return rows, draft_id, ticket


def publish_args(ticket, draft_id):
    return [
        "publish-catalogue",
        "--id",
        str(draft_id),
        "--comparison-hash",
        ticket,
        "--actor",
        "profile",
        "--rationale",
        "Synthetic profile",
    ]


def run_case(binary, database, value, name, root, revision):
    """Import, compare, then publish under the lock probe."""
    rows, draft_id, ticket = publish_case(binary, database, value, name, root, revision)
    rows["publish"] = probe_publish(binary, database, publish_args(ticket, draft_id))
    published = json.loads(rows["publish"]["stdout"])
    if not published.get("published"):
        raise AssertionError(f"{name} did not publish")
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--entries", type=int, default=8192)
    parser.add_argument("--measure", action="store_true")
    parser.add_argument("--no-fanout", action="store_true")
    parser.add_argument("--fanout-references", type=int, default=MAX_UNIT_REFERENCES)
    parser.add_argument("--fanout-only", action="store_true")
    parser.add_argument("rest", nargs="*")
    args = parser.parse_args()
    if args.measure:
        database, *command = args.rest
        measure_child(args.binary, database, command)
        return
    with tempfile.TemporaryDirectory(prefix="mimik-catalogue-profile-") as directory:
        root = Path(directory)
        database = root / "profile.sqlite3"
        cli(
            args.binary,
            database,
            "register-ruleset",
            "--game-id",
            GAME,
            "--game-label",
            "Synthetic game",
            "--edition-id",
            EDITION,
            "--edition-label",
            "First edition",
        )
        seed(database)
        cli(args.binary, database, "list-rulesets")  # search backfill, not measured
        report = {"entries_v1_v2_v3": [args.entries - 2, args.entries - 1, args.entries]}
        base = measured(args.binary, database, ["list-rulesets"])
        report["baseline"] = {k: base[k] for k in ("process_ms", "peak_child_rss_kib")}
        # v1 holds entries - 2; v2 and v3 each add one entry (append-only), ending at the cap.
        first = args.entries - 2
        for extra, name in enumerate(() if args.fanout_only else ("v1", "v2", "v3")):
            value = draft(first, extra, capacity=args.entries)
            report[name] = run_case(args.binary, database, value, name, root, extra)
        if not args.no_fanout:
            # Maximum fan-out at the caps: a second catalogue, published then updated.
            for variant, name in enumerate(("fanout_v1", "fanout_v2")):
                value = fanout(args.entries, variant, references=args.fanout_references)
                report[name] = run_case(args.binary, database, value, name, root, variant)
                report[name]["unit_references"] = sum(
                    len(entry["description"].get("units", [])) for entry in value["entries"]
                )
        report["show"] = measured(
            args.binary,
            database,
            [
                "show-catalogue-version",
                "--game",
                GAME,
                "--edition",
                EDITION,
                "--catalogue",
                "fanout" if args.fanout_only else "spells",
            ],
        )
        report["verify"] = measured(args.binary, database, ["verify-catalogues"])
        report["rebuild"] = measured(args.binary, database, ["rebuild-catalogue-index"])
        for case in ("v1", "v2", "v3", "fanout_v1", "fanout_v2"):
            for step in ("import", "compare", "publish"):
                report.get(case, {}).get(step, {}).pop("stdout", None)
                report.get(case, {}).get(step, {}).pop("stderr", None)
        for step in ("show", "rebuild"):
            report[step].pop("stdout", None)
            report[step].pop("stderr", None)
        report["verify"].pop("stderr", None)
        verified = json.loads(report["verify"].pop("stdout"))
        report["verify"]["status"] = verified["status"]
        report["verify"]["versions"] = verified["versions"]
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
