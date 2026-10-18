"""Profile catalogue publication through the real CLI (plan docs/catalogue-plan.md R8, §9).

Synthetic data only. Seeds a scratch database with one finalized synthetic source
(1,024 pages x 16 units), imports a catalogue draft at the plan's maxima, and measures
`import-catalogue-draft`, `compare-catalogue`, `publish-catalogue`, `show-catalogue-version`,
`verify-catalogues` and `rebuild-catalogue-index`, each in a fresh wrapper process so peak RSS
is that command's own (RUSAGE_CHILDREN, as tools/definition_profile.py). It then measures
separate-process contention: a second process runs a write transaction (BEGIN IMMEDIATE, 5 s
busy timeout, like the server) while a publish holds the write lock.
"""

import argparse
import json
import resource
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

GAME = "synthetic-game"
EDITION = "first"
PAGES = 1024
UNITS_PER_PAGE = 16
UNITS_PER_ENTRY = 4
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
    sized so `capacity` entries hold 1 MiB in total), every other entry citing 4 units."""
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
            }
        )
    )


def contended_write(database, delay):
    """Separate process. After `delay` seconds (past the publish process's start-up and draft
    load), probe the write lock without waiting until the publish holds it; then run one
    IMMEDIATE write transaction with the server's 5 s busy timeout. `wait_ms` is the time
    from the first refused probe to acquiring the lock: the publish's remaining lock hold,
    within about a millisecond."""
    time.sleep(delay)
    probe = sqlite3.connect(database, timeout=0, isolation_level=None)
    deadline = time.perf_counter() + 60
    while True:
        try:
            probe.execute("BEGIN IMMEDIATE")
            probe.execute("ROLLBACK")
        except sqlite3.OperationalError:
            break
        if time.perf_counter() > deadline:
            print(json.dumps({"started": False}))
            return
        time.sleep(0.0005)
    first_refused = time.perf_counter()
    probe.close()
    db = sqlite3.connect(database, timeout=5.0, isolation_level=None)
    try:
        db.execute("BEGIN IMMEDIATE")
        acquired = time.perf_counter()
        db.execute("INSERT INTO profile_writes(at) VALUES('contended save')")
        db.execute("COMMIT")
        print(
            json.dumps(
                {
                    "started": True,
                    "success": True,
                    "wait_ms": round((acquired - first_refused) * 1000, 1),
                    "total_ms": round((time.perf_counter() - first_refused) * 1000, 1),
                }
            )
        )
    except sqlite3.OperationalError as error:
        print(
            json.dumps(
                {
                    "started": True,
                    "success": False,
                    "wait_ms": round((time.perf_counter() - first_refused) * 1000, 1),
                    "error": str(error),
                }
            )
        )
    finally:
        db.close()


def publish_case(binary, database, value, name, root):
    path = root / f"{name}.json"
    path.write_bytes(encoded(value))
    revision = json.loads(
        cli(binary, database, "show-catalogue-draft", "--id", "1", check=False).stdout
        or b'{"revision":0}'
    )["revision"]
    rows = {"draft_bytes": path.stat().st_size}
    rows["import"] = measured(
        binary,
        database,
        ["import-catalogue-draft", "--file", str(path), "--expected-revision", str(revision)],
    )
    rows["compare"] = measured(binary, database, ["compare-catalogue", "--id", "1"])
    ticket = json.loads(rows["compare"]["stdout"])["comparison_hash"]
    return rows, ticket


def publish_args(ticket):
    return [
        "publish-catalogue",
        "--id",
        "1",
        "--comparison-hash",
        ticket,
        "--actor",
        "profile",
        "--rationale",
        "Synthetic profile",
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--entries", type=int, default=8192)
    parser.add_argument("--measure", action="store_true")
    parser.add_argument("--contend", action="store_true")
    parser.add_argument("rest", nargs="*")
    args = parser.parse_args()
    if args.measure:
        database, *command = args.rest
        measure_child(args.binary, database, command)
        return
    if args.contend:
        database, delay = args.rest
        contended_write(database, float(delay))
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
        report["v1"], ticket = publish_case(
            args.binary, database, draft(first, capacity=args.entries), "v1", root
        )
        report["v1"]["publish"] = measured(args.binary, database, publish_args(ticket))
        report["v2"], ticket = publish_case(
            args.binary, database, draft(first, 1, capacity=args.entries), "v2", root
        )
        report["v2"]["publish"] = measured(args.binary, database, publish_args(ticket))
        # Contention: v3 publish in one process, an IMMEDIATE write in another.
        report["v3"], ticket = publish_case(
            args.binary, database, draft(first, 2, capacity=args.entries), "v3", root
        )
        writer = subprocess.Popen(
            [sys.executable, __file__, "--contend", str(args.binary), str(database), "0.15"],
            stdout=subprocess.PIPE,
        )
        report["v3"]["publish"] = measured(args.binary, database, publish_args(ticket))
        out, _ = writer.communicate(timeout=120)
        report["v3"]["contended_write"] = json.loads(out)
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
                "spells",
            ],
        )
        report["verify"] = measured(args.binary, database, ["verify-catalogues"])
        report["rebuild"] = measured(args.binary, database, ["rebuild-catalogue-index"])
        for case in ("v1", "v2", "v3"):
            for step in ("import", "compare", "publish"):
                report[case][step].pop("stdout", None)
        for step in ("show", "verify", "rebuild"):
            report[step].pop("stdout", None)
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
