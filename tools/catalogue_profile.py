"""Profile catalogue publication through the real CLI (plan docs/catalogue-plan.md R8, §9).

Synthetic data only. Seeds a scratch database with one finalized synthetic source
(1,024 pages x 16 units), imports a catalogue draft at the plan's maxima, and measures
`import-catalogue-draft`, `compare-catalogue`, `publish-catalogue`, `show-catalogue-version`,
`verify-catalogues` and `rebuild-catalogue-index`, each in a fresh wrapper process so peak RSS
is that command's own (RUSAGE_CHILDREN, as tools/definition_profile.py). Every publish runs
under a lock probe (`LockProbe`, `publish_span`) that bounds its write-lock hold between the
last free probe that read the old version count and the first that read the new one, with a
contender that writes like the server (BEGIN IMMEDIATE, 5 s busy timeout) while the publish
holds the lock. R8 hold evidence comes from `--no-contender` runs (conservative upper bound);
runs with the contender report its wait and success, and loose bounds. Cases: v1, v2
(update) and v3 of the calibrated shape, then five
shapes at the caps, each published and updated: maximum fan-out, FTS-heavy names (4- and
3-character tokens), 64-scalar keys and declaration-heavy types (plan docs/catalogue-plan.md
§12-§14). They are the worst measured cases on the profiling host, not a proof that no
admitted shape costs more. The R8 gate (`--max-hold-ms`, finite and positive, 2,000 by
default) exits 1 when no publication was gated, a hold's upper bound exceeds the budget or
cannot be bounded, the contender was requested and did not wait through every publish and
succeed, or the run is not complete at the caps (Rust entry and reference caps, every shape,
the calibrated cases) unless `--allow-partial` (cap selection; never R8 evidence). A
`--no-contender` run gates the hold only; R8 needs a passing run of each kind.
"""

import argparse
import json
import math
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
UNITS_PER_ENTRY = 1
TYPES = 8
OWNER_EVERY = 8
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
# Admission limits, copied from src/catalogues/{decode,admission}.rs (the shapes sit at all
# of them except MAX_DISTINCT_UNITS, which the reference cap keeps out of reach).
# `RUST_LIMITS` names each one's Rust constant; tools/test_catalogue_profile.py parses the
# Rust sources and fails when they differ, so a Rust-only cap change cannot silently profile
# a non-worst shape (plan §14).
MAX_DRAFT_BYTES = 4 * 1024 * 1024
MAX_NODES = 524_288
MAX_ENTRIES = 6_144
MAX_TYPES = 32
MAX_PROPERTIES = 16
MAX_OPTIONS = 128
MAX_UNITS_PER_ENTRY = 16
MAX_NAME = 100
MAX_ID = 64
MAX_TEXT_PROPERTY = 1_000
MAX_OWNER_TEXT = 20_000
MAX_UNIT_REFERENCES = 6_144
MAX_DISTINCT_UNITS = 16_384
OWNER_TOTAL = 1024 * 1024
RUST_LIMITS = {
    "MAX_BYTES": MAX_DRAFT_BYTES,
    "MAX_NODES": MAX_NODES,
    "MAX_ENTRIES": MAX_ENTRIES,
    "MAX_TYPES": MAX_TYPES,
    "MAX_PROPERTIES": MAX_PROPERTIES,
    "MAX_OPTIONS": MAX_OPTIONS,
    "MAX_UNITS": MAX_UNITS_PER_ENTRY,
    "MAX_NAME": MAX_NAME,
    "MAX_ID": MAX_ID,
    "MAX_TEXT_PROPERTY": MAX_TEXT_PROPERTY,
    "MAX_OWNER_TEXT": MAX_OWNER_TEXT,
    "MAX_UNIT_REFERENCES": MAX_UNIT_REFERENCES,
    "MAX_DISTINCT_UNITS": MAX_DISTINCT_UNITS,
    "MAX_OWNER_TEXT_TOTAL": OWNER_TOTAL,
}
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
    sized so `capacity` entries hold 1 MiB in total), the others citing 1 unit (5,376
    references at 6,144 entries, under the 6,144 reference cap; plan §14)."""
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
    owner_len = min(OWNER_TOTAL // max(owners, 1), MAX_OWNER_TEXT)
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


def unique_token(counter, letters="abcdefghijklmnopqrstuvwxyz012345"):
    """A 4-character token no other counter value below 32**4 produces: the counter's base-32
    digits, least significant first, so neighbouring tokens differ in their first character
    and the 3-character prefix index spreads too. ASCII only: the index folds diacritics, so
    accented letters would merge tokens (and cost a second byte each)."""
    digits = []
    for _ in range(4):
        counter, digit = divmod(counter, len(letters))
        digits.append(letters[digit])
    return "".join(digits)


def fts_name(_rng, index):
    """A 100-scalar name of distinct tokens (plan §13 FTS-heavy shape): every token appears
    once in the whole catalogue, so the name index holds one posting list per token."""
    return " ".join(unique_token(index * 20 + k) for k in range(20))[:MAX_NAME]


def short_name(rng, index):
    return f"{words(rng, 3)} {index}"


def capped(
    entries=MAX_ENTRIES,
    variant=0,
    catalogue_id="fanout",
    references=MAX_UNIT_REFERENCES,
    *,
    types=TYPES,
    controls=("integer", "boolean", "enum", "text"),
    options=8,
    name=short_name,
    option_id=lambda n: f"o{n}",
    option_label=lambda n: f"Option {n}",
    property_id=lambda n: f"p{n:02}",
    entry_key=lambda index: f"entry-{index:05}",
):
    """A catalogue at the caps (plan §12, §13): `types` types x 16 properties (controls in
    turn; enums with `options` choices), `references / 16` evenly spread entries citing 16
    units each (units numbered in turn, so each is cited at most once), the rest
    owner-written (up to 1 MiB in all), and owner text then text property values sized so the
    draft is just under the 4 MiB cap. `variant` 1 changes one property value: an update with
    every key and type kept."""
    choices = [{"id": option_id(n), "label": option_label(n)} for n in range(options)]
    declared = []
    for t in range(types):
        properties = []
        for n in range(MAX_PROPERTIES):
            control = controls[n % len(controls)]
            item = {"id": property_id(n), "label": f"Property {n}", "control": control}
            if control == "enum":
                item["options"] = choices
            properties.append(item)
        declared.append({"id": f"type-{t}", "label": f"Type {t}", "properties": properties})
    citing = references // MAX_UNITS_PER_ENTRY
    cited = [index for index in range(entries) if (index * citing) % entries < citing]
    order = {index: number for number, index in enumerate(cited)}
    owners = max(entries - len(cited), 1)
    units_total = PAGES * UNITS_PER_PAGE
    text_properties = sum(1 for n in range(MAX_PROPERTIES) if controls[n % len(controls)] == "text")

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
                control = controls[n % len(controls)]
                key = property_id(n)
                if control == "integer":
                    properties[key] = rng.below(1000)
                elif control == "boolean":
                    properties[key] = rng.below(2) == 1
                elif control == "enum":
                    choice = rng.below(options)
                    if variant and index == 0 and n == 0:
                        choice = (choice + 1) % options
                    properties[key] = option_id(choice)
                else:
                    # ASCII, so the byte budget above is exact (one byte per scalar).
                    text = ("v" + words(rng, 40)).encode("ascii", "replace").decode()
                    properties[key] = text[:text_len].ljust(text_len, "x")
            if variant and index == 0 and controls[0] == "integer":
                properties[property_id(0)] = 1000  # integers are drawn below 1,000
            listed.append(
                {
                    "key": entry_key(index),
                    "type": f"type-{index % types}",
                    "name": name(rng, index),
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
            "entry_types": declared,
            "entries": listed,
        }

    # References and properties are at their caps; the remaining bytes go to owner text (up
    # to its 1 MiB total), then to the text property values.
    base = len(json.dumps(build(1, 1), ensure_ascii=False, separators=(",", ":")).encode())
    room = MAX_DRAFT_BYTES - FANOUT_HEADROOM - base
    if room < 0:
        raise AssertionError(f"Skeleton exceeds the 4 MiB cap by {-room} bytes")
    owner_len = max(1, min(MAX_OWNER_TEXT, OWNER_TOTAL // owners, 1 + room // owners))
    room -= (owner_len - 1) * owners
    text_len = 1
    if text_properties:
        text_len = max(1, min(MAX_TEXT_PROPERTY, 1 + room // (entries * text_properties)))
    return build(text_len, owner_len)


def fanout(entries=MAX_ENTRIES, variant=0, catalogue_id="fanout", references=MAX_UNIT_REFERENCES):
    """Maximum fan-out at the caps (plan §12): 8 types x 16 properties (integer, boolean,
    enum with 8 options, text), 3-word names, every reference used."""
    return capped(entries, variant, catalogue_id, references)


def fts_heavy(entries=MAX_ENTRIES, variant=0, catalogue_id="names", references=MAX_UNIT_REFERENCES):
    """FTS-heavy (plan §13): the fan-out shape with 100-scalar names of tokens used once each
    in the catalogue (large vocabulary), every reference used; owner text gets fewer bytes."""
    return capped(entries, variant, catalogue_id, references, name=fts_name)


def short_token(counter, letters="abcdefghijklmnopqrstuvwxyz0123456789"):
    """A 3-character token, base-36 digits least significant first (46,656 distinct)."""
    digits = []
    for _ in range(3):
        counter, digit = divmod(counter, len(letters))
        digits.append(letters[digit])
    return "".join(digits)


def fts3_name(_rng, index):
    """A 99-scalar name of 25 three-character tokens (plan §14): every token is its own
    3-character prefix, and at 6,144 entries the 153,600 tokens cycle through all 46,656,
    so each appears three or four times (more postings per token than `fts_name`)."""
    return " ".join(short_token((index * 25 + k) % 36**3) for k in range(25))[:MAX_NAME]


def fts3_heavy(
    entries=MAX_ENTRIES, variant=0, catalogue_id="names3", references=MAX_UNIT_REFERENCES
):
    """FTS-heavy with 3-character tokens (plan §14, Sonnet round 3)."""
    return capped(entries, variant, catalogue_id, references, name=fts3_name)


def long_key(index):
    """A 64-scalar entry key (the id cap), distinct per index."""
    return f"entry-{index:05}-".ljust(MAX_ID, "k")


def long_keys(entries=MAX_ENTRIES, variant=0, catalogue_id="keys", references=MAX_UNIT_REFERENCES):
    """64-scalar keys on the FTS-heavy shape (plan §14, Sonnet round 3): keys are stored in
    entry, unit, seal and name index rows; every reference used."""
    return capped(entries, variant, catalogue_id, references, name=fts_name, entry_key=long_key)


def compact_id(n):
    """`a`..`z`, then `a1`..`z1` and so on: the shortest ids, so declarations fit 4 MiB."""
    return f"{'abcdefghijklmnopqrstuvwxyz'[n % 26]}{n // 26 or ''}"


def declaration_heavy(
    entries=MAX_ENTRIES, variant=0, catalogue_id="declarations", references=MAX_UNIT_REFERENCES
):
    """Declaration-heavy (plan §13): 32 types x 16 enum properties x 128 options (65,536
    declared choices, compact ids and labels), every entry holding 16 enum values, and the
    most references that fit 4 MiB beside them, up to `references` (a multiple of 16, found
    by bisection; 9,056 fit at 8,192 entries, so at the 6,144 reference cap every
    reference is used)."""

    def build(count):
        return capped(
            entries,
            variant,
            catalogue_id,
            count,
            types=MAX_TYPES,
            controls=("enum",),
            options=MAX_OPTIONS,
            option_id=compact_id,
            option_label=lambda n: compact_id(n).upper(),
            property_id=compact_id,
        )

    low, high = 0, references // MAX_UNITS_PER_ENTRY
    best = None
    while low <= high:
        middle = (low + high) // 2
        try:
            best = build(middle * MAX_UNITS_PER_ENTRY)
            low = middle + 1
        except AssertionError:
            high = middle - 1
    if best is None:
        raise AssertionError("Declaration-heavy skeleton exceeds the 4 MiB cap")
    return best


SHAPES = {
    "fanout": fanout,
    "fts": fts_heavy,
    "declarations": declaration_heavy,
    "fts3": fts3_heavy,
    "keys": long_keys,
}
CATALOGUE_IDS = {
    "fanout": "fanout",
    "fts": "names",
    "declarations": "declarations",
    "fts3": "names3",
    "keys": "keys",
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
        [str(binary), "--database", str(database), *args],
        capture_output=True,
        timeout=600,
    )
    if check and result.returncode != 0:
        raise AssertionError(f"{args[0]} failed: {result.stderr.decode(errors='replace')[:400]}")
    return result


def measured(binary, database, args):
    """Run one CLI command in a fresh wrapper process; return its wall time and peak RSS."""
    result = subprocess.run(
        [
            sys.executable,
            __file__,
            "--measure",
            str(binary),
            str(database),
            "--",
            *args,
        ],
        capture_output=True,
        timeout=900,
        check=True,
    )
    return json.loads(result.stdout)


def measure_child(binary, database, args):
    started = time.perf_counter()
    result = subprocess.run(
        [str(binary), "--database", str(database), *args],
        capture_output=True,
        timeout=600,
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


class ProbeError(Exception):
    """The lock probe could not observe the lock (it failed or never saw it free)."""


# Primary SQLite result codes meaning another connection holds the lock.
CONTENTION = {5, 6}  # SQLITE_BUSY, SQLITE_LOCKED


def contended(error):
    """True when `error` is lock contention, not any other operational failure."""
    return (
        isinstance(error, sqlite3.OperationalError)
        and (getattr(error, "sqlite_errorcode", 0) & 0xFF) in CONTENTION
    )


def publish_span(probes, published):
    """Bound the publish's write-lock hold from complete probe windows (plan §13, §14).

    `probes` are `(start, end, free, versions)` in order from one thread, so their windows do
    not overlap; `versions` is the catalogue version count the probe read (None when it could
    not read). A free probe holds the write lock itself when it reads, so:

    - a free probe that read fewer than `published` versions (the count after the publish)
      took the lock before the publish did: the publish acquired after its start;
    - a free probe that read `published` took the lock after the publish released: the
      publish released before its end.

    The publish is identified by the version it adds, not by which refusal run looks like
    it: between the last free probe that read the old count (`before`) and the first that read
    the new one (`after`) every probe is refused, and the hold `[A, R)` lies inside
    `(before.start, after.end)`. **`after.end - before.start` is a conservative upper bound**
    of the hold whatever else ran between them (the CLI's store-open lock, the contender):
    other transactions can only widen the span. The lower bound `last.start - first.end`
    over the refused probes between them (0 when none or negative) is indicative only: a
    refusal by another transaction in the same span (the store-open lock when no free probe
    landed in the gap after it, or the contender) inflates it.

    Returns `bounded` False with a `reason` when either free probe is missing (never a null
    bound); otherwise `acquired` and `released` as (earliest, latest), `upper`, `lower` and
    `refused` (the refused probes inside the span), times in seconds."""

    def read(probe, newer):
        if not probe[2] or probe[3] is None or published is None:
            return False
        return probe[3] >= published if newer else probe[3] < published

    after = next((i for i, probe in enumerate(probes) if read(probe, True)), None)
    if after is None:
        return {"bounded": False, "reason": "no free probe read the published version"}
    before = next((i for i in range(after - 1, -1, -1) if read(probes[i], False)), None)
    if before is None:
        return {"bounded": False, "reason": "no free probe read the previous version count"}
    refused = [probe for probe in probes[before + 1 : after] if not probe[2]]
    first_end = refused[0][1] if refused else probes[after][1]
    last_start = refused[-1][0] if refused else probes[before][0]
    return {
        "bounded": True,
        "acquired": (probes[before][0], first_end),
        "released": (last_start, probes[after][1]),
        "upper": probes[after][1] - probes[before][0],
        "lower": max(0.0, last_start - first_end) if refused else 0.0,
        "refused": len(refused),
    }


def floor_ms(seconds):
    """Seconds to milliseconds, rounded down to 0.1 ms (lower bounds round outward)."""
    return math.floor(seconds * 10_000) / 10


def ceil_ms(seconds):
    """Seconds to milliseconds, rounded up to 0.1 ms (upper bounds round outward)."""
    return math.ceil(seconds * 10_000) / 10


def span_ms(span, origin, contender):
    """`publish_span` in milliseconds, times relative to `origin` (the launch), every bound
    rounded outward. With the contender the bounds are labelled loose (it may run inside)."""
    if not span["bounded"]:
        return {"bounded": False, "reason": span["reason"]}
    return {
        "bounded": True,
        "bounds": "loose: the contender may hold the lock inside the span"
        if contender
        else "conservative upper; lower indicative",
        # The outer span (last free probe reading the old version count to the first reading
        # the new one) contains the publish by construction. The endpoint intervals use the
        # first and last refused probes, which may belong to the store-open lock or the
        # contender, so they are indicative only (Sol round 4).
        "span_after_ms": [
            floor_ms(span["acquired"][0] - origin),
            ceil_ms(span["released"][1] - origin),
        ],
        "indicative_acquired_after_ms": [
            floor_ms(span["acquired"][0] - origin),
            ceil_ms(span["acquired"][1] - origin),
        ],
        "indicative_released_after_ms": [
            floor_ms(span["released"][0] - origin),
            ceil_ms(span["released"][1] - origin),
        ],
        "hold_ms_lower": floor_ms(span["lower"]),
        "hold_ms_upper": ceil_ms(span["upper"]),
        "refused_probes": span["refused"],
    }


class LockProbe:
    """Probe the write lock without waiting from a thread (`BEGIN IMMEDIATE` with a zero busy
    timeout, a read of `marker`, then `ROLLBACK`), recording each probe's start, completion
    and marker value (refused probes try the read too). Once a refusal
    has lasted `contend_after` seconds (past the ~30-40 ms lock every CLI process takes when
    it opens the store), a contender thread runs the server's write pattern on its own
    connection: `BEGIN IMMEDIATE` with the server's 5 s busy timeout, a read of `marker`, one
    insert, `COMMIT` (the lock-level equivalent of a sheet save). Any failure other than lock contention, in
    the poller or the contender, is kept and raised as `ProbeError` by `start`/`check`."""

    def __init__(self, database, contend_after=0.05, ready_probes=20, marker=None):
        self.database = database
        # A query whose result grows when the publish commits (see `publish_hold`).
        self.marker = marker
        self.contend_after = contend_after
        self.ready_probes = ready_probes
        self.probes = []
        self.contender = {}
        self.failure = None
        self.ready = threading.Event()
        self.halt = threading.Event()
        self.thread = threading.Thread(target=self.poll, daemon=True)

    def contend(self):
        db = None
        try:
            db = sqlite3.connect(self.database, timeout=5.0, isolation_level=None)
            self.contender["begin"] = time.perf_counter()
            try:
                db.execute("BEGIN IMMEDIATE")
            except sqlite3.OperationalError as error:
                if not contended(error):
                    raise
                self.contender.update(success=False, error="busy after the 5 s timeout")
                return
            self.contender["acquired"] = time.perf_counter()
            if self.marker is not None:
                self.contender["versions"] = db.execute(self.marker).fetchone()[0]
            db.execute("INSERT INTO profile_writes(at) VALUES('contended save')")
            db.execute("COMMIT")
            self.contender["success"] = True
        except Exception as error:  # handed to the main thread as ProbeError
            self.contender.update(success=False, error=repr(error))
            self.failure = self.failure or error
        finally:
            if db is not None:
                db.close()

    def poll(self):
        db = None
        contender = None
        try:
            db = sqlite3.connect(self.database, timeout=0, isolation_level=None)
            free_count = 0
            first_refused_end = None
            while not self.halt.is_set():
                start = time.perf_counter()
                versions = None
                try:
                    db.execute("BEGIN IMMEDIATE")
                except sqlite3.OperationalError as error:
                    if not contended(error):
                        raise
                    free = False
                    versions = self.read_marker(db)
                else:
                    try:
                        versions = self.read_marker(db)
                    finally:
                        db.execute("ROLLBACK")
                    free = True
                end = time.perf_counter()
                self.probes.append((start, end, free, versions))
                if free:
                    first_refused_end = None
                    free_count += 1
                    if free_count >= self.ready_probes:
                        self.ready.set()
                else:
                    first_refused_end = first_refused_end or end
                    # Held at some instant of the first refused window and of this one.
                    if (
                        contender is None
                        and self.contend_after is not None
                        and self.ready.is_set()
                        and start - first_refused_end >= self.contend_after
                    ):
                        contender = threading.Thread(target=self.contend, daemon=True)
                        contender.start()
                time.sleep(0.0005)
        except Exception as error:  # handed to the main thread as ProbeError
            self.failure = self.failure or error
        finally:
            if db is not None:
                db.close()
            if contender is not None:
                contender.join(timeout=30)
                if contender.is_alive():
                    self.failure = self.failure or ProbeError("contender did not finish")
            self.ready.set()

    def read_marker(self, db):
        """The marker's value now, or None when the read itself meets the lock (the commit
        phase of a rollback-journal writer refuses readers)."""
        if self.marker is None:
            return None
        try:
            return db.execute(self.marker).fetchone()[0]
        except sqlite3.OperationalError as error:
            if not contended(error):
                raise
            return None

    def settle(self, published, timeout):
        """Wait at most `timeout` seconds for a free probe that read `published` versions, so
        the span has its closing probe (`publish_span`); False if none came."""
        deadline = time.perf_counter() + timeout
        seen = 0
        while time.perf_counter() < deadline and self.thread.is_alive():
            probes = self.probes[seen:]
            seen += len(probes)
            if any(p[2] and p[3] is not None and p[3] >= published for p in probes):
                return True
            time.sleep(0.001)
        return False

    def check(self):
        if self.failure is not None:
            raise ProbeError(f"lock probe failed: {self.failure!r}") from self.failure

    def start(self, timeout):
        """Start polling; wait at most `timeout` seconds for `ready_probes` free probes."""
        self.thread.start()
        if not self.ready.wait(timeout):
            self.stop()
            raise ProbeError(f"lock probe saw no free lock within {timeout} s")
        if self.failure is not None:
            self.stop()
            self.check()

    def stop(self):
        self.halt.set()
        self.thread.join(timeout=60)
        if self.thread.is_alive():
            self.failure = self.failure or ProbeError("poller did not stop")


VERSIONS = "SELECT count(*) FROM catalogue_versions"


def probe_publish(
    binary,
    database,
    command,
    contend_after=0.05,
    run=None,
    ready_timeout=10.0,
    marker=VERSIONS,
):
    """Run one publish (`run`, by default `measured`) under a `LockProbe` and report its
    write-lock hold (`publish_span`: a conservative upper bound without the contender, loose
    bounds with it) and the contender's result (plan §12, §13, §14)."""
    run = measured if run is None else run
    probe = LockProbe(database, contend_after, marker=marker)
    probe.start(ready_timeout)
    published = None
    try:
        launched = time.perf_counter()
        measured_result = run(binary, database, command)
        if marker is not None:
            db = sqlite3.connect(database, timeout=5.0)
            try:
                published = db.execute(marker).fetchone()[0]
            finally:
                db.close()
            probe.settle(published, 5.0)
    finally:
        probe.stop()
    probe.check()
    contender = probe.contender
    span = publish_span(probe.probes, published)
    after = [p for p in probe.probes if p[0] >= launched]
    lock = {
        "probes": len(after),
        "probe_window_max_ms": ceil_ms(max((p[1] - p[0] for p in after), default=0)),
        "probe_gap_max_ms": ceil_ms(
            max((b[0] - a[1] for a, b in zip(after, after[1:], strict=False)), default=0)
        ),
        "publish_hold": span_ms(span, launched, bool(contender)),
    }
    if contend_after is not None:
        lock["contender"] = contender_result(contender, probe.probes, published, launched)
    measured_result["lock"] = lock
    return measured_result


def contender_result(contender, probes, published, origin):
    """Whether the contender waited through the publish and succeeded (R8). It began before
    the publish released when some probe that started no earlier than its `BEGIN` still read
    the old version count, and it acquired after the release when its own read inside its
    transaction saw `published` versions."""
    if "begin" not in contender:
        return {"started": False, "success": False, "waited_through_publish": False}
    begin = contender["begin"]
    began_before_release = published is not None and any(
        p[0] >= begin and p[3] is not None and p[3] < published for p in probes
    )
    saw_published = published is not None and contender.get("versions", -1) >= published
    return {
        "started": True,
        "success": contender.get("success", False),
        "waited_through_publish": began_before_release and saw_published,
        "began_after_ms": floor_ms(begin - origin),
        "wait_ms": None if "acquired" not in contender else floor_ms(contender["acquired"] - begin),
        "error": contender.get("error"),
    }


def r8_failures(name, publish, max_hold_ms, contender_expected=False):
    """R8 gate (plan §14) for one publish: a bounded hold whose upper bound is within
    `max_hold_ms`, and, when the contender ran or was requested, a contender that waited
    through the publish and succeeded. Returns the failures as text."""
    failures = []
    hold = publish["lock"]["publish_hold"]
    if not hold["bounded"]:
        failures.append(f"{name}: hold not bounded ({hold['reason']})")
    elif hold["hold_ms_upper"] > max_hold_ms:
        failures.append(f"{name}: hold upper bound {hold['hold_ms_upper']} ms > {max_hold_ms}")
    contender = publish["lock"].get("contender")
    if contender is None and contender_expected:
        failures.append(f"{name}: the contender was requested but did not run")
    elif contender is not None:
        if not contender["success"]:
            failures.append(f"{name}: contender did not succeed ({contender.get('error')})")
        elif not contender["waited_through_publish"]:
            failures.append(f"{name}: contender did not wait through the publish")
    return failures


def r8_gate(report, max_hold_ms, contender_expected, coverage=None, allow_partial=False):
    """The report's R8 gate (plan §14-§16), fail-closed: it fails when no publication was
    gated, when any publication fails `r8_failures` (including a requested contender that
    did not run for it), or, unless `allow_partial` (cap-selection and exploratory runs), when
    the run is not complete at the caps: `coverage` must show the Rust entry and reference
    caps, every shape in `SHAPES` and the calibrated v1-v3 cases. Runs without the contender
    gate the hold only; R8's "a concurrent save waits and succeeds" needs a passing run with
    the contender too."""
    failures = []
    coverage = coverage or {}
    at_caps = (
        coverage.get("entries") == MAX_ENTRIES
        and coverage.get("references") == MAX_UNIT_REFERENCES
        and sorted(coverage.get("shapes", ())) == sorted(SHAPES)
        and coverage.get("calibrated", True)
    )
    publications = 0
    contender_observed = False
    for name, case in report.items():
        if isinstance(case, dict) and isinstance(case.get("publish"), dict):
            publications += 1
            contender_observed |= case["publish"]["lock"].get("contender") is not None
            failures += r8_failures(name, case["publish"], max_hold_ms, contender_expected)
    if publications == 0:
        failures.append("no publication was gated")
    elif contender_expected and not contender_observed:
        failures.append("the contender was requested but never observed")
    if not at_caps and not allow_partial:
        failures.append("not a complete run at the caps (pass --allow-partial to gate it anyway)")
    return {
        "max_hold_ms": max_hold_ms,
        "coverage": coverage,
        "at_caps": at_caps,
        "partial_allowed": allow_partial,
        "publications": publications,
        "contender_observed": contender_observed,
        "scope": "hold and contender" if contender_expected else "hold only",
        "passed": not failures,
        "failures": failures,
    }


def shape_list(text):
    """`--shapes`: a comma-separated, non-empty list of known shapes."""
    shapes = [shape for shape in text.split(",") if shape]
    unknown = [shape for shape in shapes if shape not in SHAPES]
    if not shapes or unknown:
        raise argparse.ArgumentTypeError(f"choose from {', '.join(SHAPES)}")
    return shapes


def hold_budget(text):
    """`--max-hold-ms`: a finite, positive number of milliseconds (`hold > nan` is always
    false, so a NaN budget would pass every hold)."""
    try:
        value = float(text)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"not a number: {text!r}") from error
    if not math.isfinite(value) or value <= 0:
        raise argparse.ArgumentTypeError("the budget must be finite and positive")
    return value


def publish_case(binary, database, value, name, root, revision):
    """Import `value` over draft `revision` (0 creates it) and compare it; returns the rows,
    the draft id and the ticket."""
    path = root / f"{name}.json"
    path.write_bytes(encoded(value))
    rows = {"draft_bytes": path.stat().st_size}
    rows["import"] = measured(
        binary,
        database,
        [
            "import-catalogue-draft",
            "--file",
            str(path),
            "--expected-revision",
            str(revision),
        ],
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


def run_case(binary, database, value, name, root, revision, contend_after=0.05):
    """Import, compare, then publish under the lock probe (`contend_after` None: no
    contender)."""
    rows, draft_id, ticket = publish_case(binary, database, value, name, root, revision)
    rows["publish"] = probe_publish(binary, database, publish_args(ticket, draft_id), contend_after)
    published = json.loads(rows["publish"]["stdout"])
    if not published.get("published"):
        raise AssertionError(f"{name} did not publish")
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--entries", type=int, default=MAX_ENTRIES)
    parser.add_argument("--measure", action="store_true")
    parser.add_argument("--no-fanout", action="store_true")
    parser.add_argument(
        "--references",
        type=int,
        default=MAX_UNIT_REFERENCES,
        help="reference cap the shapes use (cap selection runs)",
    )
    parser.add_argument(
        "--no-contender",
        action="store_true",
        help="probe without the contender: tighter lower bounds, no wait/success result",
    )
    parser.add_argument(
        "--max-hold-ms",
        type=hold_budget,
        default=2000.0,
        help="R8 gate: fail when a publish's hold upper bound exceeds this (plan §14)",
    )
    parser.add_argument("--fanout-only", action="store_true")
    parser.add_argument(
        "--allow-partial",
        action="store_true",
        help="gate a run below the caps or with fewer shapes (cap selection); never R8 evidence",
    )
    parser.add_argument("--shapes", type=shape_list, default=list(SHAPES))
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
        contend = None if args.no_contender else 0.05
        report["contender"] = contend is not None
        base = measured(args.binary, database, ["list-rulesets"])
        report["baseline"] = {k: base[k] for k in ("process_ms", "peak_child_rss_kib")}
        # v1 holds entries - 2; v2 and v3 each add one entry (append-only), ending at the cap.
        first = args.entries - 2
        for extra, name in enumerate(() if args.fanout_only else ("v1", "v2", "v3")):
            value = draft(first, extra, capacity=args.entries)
            report[name] = run_case(args.binary, database, value, name, root, extra, contend)
        if not args.no_fanout:
            # Shapes at the caps (plan §12, §13), each a catalogue of its own, published then
            # updated (the update replaces that catalogue's name index rows).
            for shape in args.shapes:
                build = SHAPES[shape]
                for variant, step in enumerate(("v1", "v2")):
                    name = f"{shape}_{step}"
                    value = build(args.entries, variant, references=args.references)
                    report[name] = run_case(
                        args.binary, database, value, name, root, variant, contend
                    )
                    report[name]["unit_references"] = sum(
                        len(entry["description"].get("units", [])) for entry in value["entries"]
                    )
        catalogues = [] if args.fanout_only else ["spells"]
        if not args.no_fanout:
            catalogues += [CATALOGUE_IDS[shape] for shape in args.shapes]
        for catalogue in catalogues:
            show = measured(
                args.binary,
                database,
                [
                    "show-catalogue-version",
                    "--game",
                    GAME,
                    "--edition",
                    EDITION,
                    "--catalogue",
                    catalogue,
                ],
            )
            report[f"show_{catalogue}"] = {
                k: show[k] for k in ("process_ms", "peak_child_rss_kib", "success")
            }
        report["verify"] = measured(args.binary, database, ["verify-catalogues"])
        report["rebuild"] = measured(args.binary, database, ["rebuild-catalogue-index"])
        for case in report.values():
            for step in ("import", "compare", "publish"):
                if isinstance(case, dict) and isinstance(case.get(step), dict):
                    case[step].pop("stdout", None)
                    case[step].pop("stderr", None)
        for step in ("rebuild",):
            report[step].pop("stdout", None)
            report[step].pop("stderr", None)
        report["verify"].pop("stderr", None)
        verified = json.loads(report["verify"].pop("stdout"))
        report["verify"]["status"] = verified["status"]
        report["verify"]["versions"] = verified["versions"]
        coverage = {
            "entries": args.entries,
            "references": args.references,
            "shapes": [] if args.no_fanout else args.shapes,
            "calibrated": not args.fanout_only,
        }
        report["r8_gate"] = r8_gate(
            report, args.max_hold_ms, contend is not None, coverage, args.allow_partial
        )
        print(json.dumps(report, indent=2))
        if not report["r8_gate"]["passed"]:
            sys.exit(1)


if __name__ == "__main__":
    main()
