"""Profile the sheet's name suggestions (assisted sheet step 4; docs/catalogues.md).

Prototype tier: synthetic data only, no model, no network beyond the loopback server it is
pointed at. Builds a synthetic catalogue at the entry cap (6,144 entries; names up to 100
scalars, shared prefixes, Latin, Cyrillic, Greek, Hangul and CJK names, several entry types,
duplicate names), seeds it through the real CLI (`import-catalogue-draft`, `compare-catalogue`,
`publish-catalogue`), and times warm requests per mix against a running release binary:

- `prefix`: a 2-scalar prefix many entries share;
- `substring`: a 4-scalar infix (no prefix match, so the whole name list is scanned);
- `typo`: the worst `typo=1` query, a 6-scalar near-miss of nothing, so every entry whose
  length is within 2 scalars gets an edit-distance comparison.

Cold loads (the first request of a mix after the name cache was empty) are reported apart
from the warm p50/p95. The gate (`--max-p95-ms`, default 50) exits 1 when a mix's warm p95
exceeds it. The server, the account, the sheet and its session cookie are prepared by the
caller (the cookie is read from the environment variable `SUGGEST_COOKIE`, never from an
argument); `--url` names the route's address. The ignored Rust test
`measure_the_worst_suggest_request` (src/campaigns/suggest_tests.rs) measures the same mixes
in process; this tool adds the real binary and the real HTTP stack.

    python tools/suggest_profile.py --write-draft draft.json
    python tools/suggest_profile.py --seed target/release/mimik scratch.sqlite3
    SUGGEST_COOKIE='id=...' python tools/suggest_profile.py \
        --url http://127.0.0.1:3000/private-sheets/1/suggest/1/0
"""

import argparse
import json
import math
import os
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

GAME = "synthetic-game"
EDITION = "first"
CATALOGUE = "gear"
ENTRIES = 6_144
MAX_NAME = 100
TYPES = ["tool", "weapon", "armor", "potion", "scroll", "relic"]
SHORT = ["Rope", "Lamp", "Flask", "Oil"]
LATIN = ["Lantern", "Lanterna", "Longsword", "Lamp", "Ladder", "Flask", "Rope", "Shield"]
OTHER_SCRIPTS = [
    "Фонарь",  # Cyrillic
    "Φανάρι",  # Greek
    "한글이름",  # Hangul
    "長剣の鍵",  # CJK
]
QUERIES = {
    "prefix": {"q": "la"},
    "substring": {"q": "nter"},
    "typo": {"q": "qzxwvu", "typo": "1"},
}


def name(index):
    """The synthetic name of entry `index`: shared prefixes, mixed scripts, some long names
    and some duplicates (every 64th entry repeats the previous one's name)."""
    if index % 64 == 63:
        return name(index - 1)
    if index % 11 == 0:
        base = OTHER_SCRIPTS[index // 11 % len(OTHER_SCRIPTS)]
        return f"{base} {index}"
    if index % 3 == 1:
        # Short names, so many norms fall within 2 scalars of the typo query's length.
        return f"{SHORT[index % len(SHORT)]} {index % 100}"
    base = LATIN[index % len(LATIN)]
    if index % 7 == 0:
        filler = " of the long winding road beyond the silver mountains and the sleeping sea and on"
        return (f"{base}{filler} {index}")[:MAX_NAME]
    return f"{base} {index // len(LATIN)} mark {index % 97}"


def draft(entries=ENTRIES):
    """The catalogue draft: owner-written entries only (no source needed), several types."""
    return {
        "contract_version": 1,
        "game_id": GAME,
        "edition_id": EDITION,
        "catalogue_id": CATALOGUE,
        "title": "Synthetic suggestion profile",
        "sources": [],
        "entry_types": [
            {"id": kind, "label": kind.capitalize(), "properties": []} for kind in TYPES
        ],
        "entries": [
            {
                "key": f"e{index}",
                "type": TYPES[index % len(TYPES)],
                "name": name(index),
                "properties": {},
                "description": {"owner_text": f"Synthetic text {index}."},
            }
            for index in range(entries)
        ],
    }


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def cli(binary, database, *args):
    result = subprocess.run(
        [str(binary), "--database", str(database), *args],
        capture_output=True,
        timeout=600,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"{args[0]} failed: {result.stderr.decode(errors='replace')[:300]}")
    return json.loads(result.stdout)


def seed(binary, database, value, scratch):
    """Imports and publishes `value` through the CLI into `database` (which must already hold
    the pair's registry rows, as tools/catalogue_profile.py seeds them)."""
    path = Path(scratch) / "suggest-profile-draft.json"
    path.write_bytes(encoded(value))
    imported = cli(
        binary, database, "import-catalogue-draft", "--file", str(path), "--expected-revision", "0"
    )
    draft_id = str(imported["id"])
    compared = cli(binary, database, "compare-catalogue", "--id", draft_id)
    cli(
        binary,
        database,
        "publish-catalogue",
        "--id",
        draft_id,
        "--comparison-hash",
        compared["comparison_hash"],
        "--actor",
        "profile",
        "--rationale",
        "Synthetic profile",
    )


def percentile(samples, fraction):
    """The nearest-rank percentile of `samples` (`fraction` in (0, 1])."""
    if not samples:
        raise ValueError("no samples")
    if not 0 < fraction <= 1:
        raise ValueError("fraction outside (0, 1]")
    ordered = sorted(samples)
    return ordered[max(math.ceil(fraction * len(ordered)) - 1, 0)]


def summary(cold, warm):
    """Cold load apart; p50 and p95 of the warm requests, in milliseconds."""
    return {
        "cold_ms": round(cold, 3),
        "warm_count": len(warm),
        "p50_ms": round(statistics.median(warm), 3),
        "p95_ms": round(percentile(warm, 0.95), 3),
        "max_ms": round(max(warm), 3),
    }


def failures(report, max_p95):
    """The mixes whose warm p95 exceeds the budget."""
    if not report:
        return ["no mix was measured"]
    return [
        f"{mix}: p95 {row['p95_ms']} ms exceeds {max_p95} ms"
        for mix, row in report.items()
        if row["p95_ms"] > max_p95
    ]


def request(url, cookie, query):
    http = urllib.request.Request(
        f"{url}?{urllib.parse.urlencode(query)}",
        headers={"Accept": "application/json", "Cookie": cookie},
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(http, timeout=10) as response:  # noqa: S310 loopback only
            body = json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"HTTP {error.code} for {query}") from error
    elapsed = (time.perf_counter() - started) * 1000
    if body.get("status") != "ok":
        raise RuntimeError(f"unexpected answer for {query}")
    return elapsed


def profile(url, cookie, count=1000):
    report = {}
    for mix, query in QUERIES.items():
        cold = request(url, cookie, query)
        warm = [request(url, cookie, query) for _ in range(count)]
        report[mix] = summary(cold, warm)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--write-draft", type=Path, help="write the synthetic draft and exit")
    parser.add_argument("--seed", nargs=2, metavar=("BINARY", "DATABASE"))
    parser.add_argument("--url", help="the suggest route of a prepared sheet, without a query")
    parser.add_argument("--count", type=int, default=1000)
    parser.add_argument("--max-p95-ms", type=float, default=50.0)
    args = parser.parse_args(argv)
    value = draft()
    if args.write_draft:
        args.write_draft.write_bytes(encoded(value))
        return 0
    if args.seed:
        seed(args.seed[0], args.seed[1], value, Path(args.seed[1]).parent)
        return 0
    if not args.url:
        parser.error("one of --write-draft, --seed or --url is required")
    cookie = os.environ.get("SUGGEST_COOKIE", "")
    if not cookie:
        parser.error("set SUGGEST_COOKIE to the prepared session's cookie header value")
    report = profile(args.url, cookie, args.count)
    print(json.dumps(report, indent=2))
    problems = failures(report, args.max_p95_ms)
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
