"""The catalogue profile draft must sit at the plan's maxima without exceeding any limit."""

import json
import sqlite3
import tempfile
import threading
import time
import unittest
from pathlib import Path

from catalogue_profile import (
    MAX_DRAFT_BYTES,
    MAX_UNIT_REFERENCES,
    OWNER_TOTAL,
    ProbeError,
    Rng,
    declaration_heavy,
    draft,
    encoded,
    fanout,
    fts_heavy,
    probe_publish,
    publish_hold,
)


class DraftShapeTests(unittest.TestCase):
    def test_capacity_drafts_fit_every_limit_and_grow_append_only(self):
        for capacity in (4096, 8192):
            versions = [draft(capacity - 2, extra, capacity=capacity) for extra in (0, 1, 2)]
            last = versions[-1]
            self.assertEqual(len(last["entries"]), capacity)
            self.assertLessEqual(len(encoded(last)), MAX_DRAFT_BYTES)
            owner = sum(
                len(entry["description"]["owner_text"].encode())
                for entry in last["entries"]
                if "owner_text" in entry["description"]
            )
            self.assertLessEqual(owner, OWNER_TOTAL)
            keys = [entry["key"] for entry in last["entries"]]
            self.assertEqual(len(keys), len(set(keys)))
            units = {
                unit["unit_id"]
                for entry in last["entries"]
                for unit in entry["description"].get("units", [])
            }
            self.assertLessEqual(len(units), 16_384)
            references = sum(
                len(entry["description"].get("units", [])) for entry in last["entries"]
            )
            self.assertLessEqual(references, MAX_UNIT_REFERENCES)
            # Each version keeps every earlier key with the same type.
            for older, newer in zip(versions, versions[1:], strict=False):
                kept = {e["key"]: e["type"] for e in newer["entries"]}
                for entry in older["entries"]:
                    self.assertEqual(kept[entry["key"]], entry["type"])

    def test_drafts_are_deterministic(self):
        self.assertEqual(json.dumps(draft(64)), json.dumps(draft(64)))

    def test_oversized_drafts_are_refused_before_any_run(self):
        value = draft(8)
        value["title"] = "x" * MAX_DRAFT_BYTES
        with self.assertRaises(AssertionError):
            encoded(value)

    def test_fanout_sits_at_the_reference_property_and_byte_caps(self):
        first, second = fanout(8192, 0), fanout(8192, 1)
        for value in (first, second):
            self.assertEqual(len(value["entries"]), 8192)
            self.assertLessEqual(len(encoded(value)), MAX_DRAFT_BYTES)
            self.assertGreater(len(encoded(value)), MAX_DRAFT_BYTES - 64 * 1024)
            references = [
                (unit["page_id"], unit["unit_id"])
                for entry in value["entries"]
                for unit in entry["description"].get("units", [])
            ]
            self.assertEqual(len(references), MAX_UNIT_REFERENCES)
            self.assertEqual(len(set(references)), MAX_UNIT_REFERENCES)
            self.assertTrue(all((unit - 1) // 16 + 1 == page for page, unit in references))
            self.assertTrue(all(len(t["properties"]) == 16 for t in value["entry_types"]))
            owner = sum(
                len(entry["description"].get("owner_text", "").encode())
                for entry in value["entries"]
            )
            self.assertLessEqual(owner, OWNER_TOTAL)
        # The update keeps every key and type and changes content.
        self.assertEqual(
            [(e["key"], e["type"]) for e in first["entries"]],
            [(e["key"], e["type"]) for e in second["entries"]],
        )
        self.assertNotEqual(encoded(first), encoded(second))


def nodes(value):
    """Decoder nodes as `src/catalogues/decode.rs` counts them: every JSON value."""
    if isinstance(value, dict):
        return 1 + sum(nodes(child) for child in value.values())
    if isinstance(value, list):
        return 1 + sum(nodes(child) for child in value)
    return 1


class WorstShapeTests(unittest.TestCase):
    """The plan §13 shapes stay inside every admission limit and keep their stress."""

    def common(self, value):
        data = encoded(value)
        self.assertLessEqual(len(data), MAX_DRAFT_BYTES)
        self.assertGreater(len(data), MAX_DRAFT_BYTES - 64 * 1024)
        self.assertEqual(len(value["entries"]), 8192)
        self.assertLessEqual(nodes(value), 524_288)
        references = sum(len(e["description"].get("units", [])) for e in value["entries"])
        self.assertLessEqual(references, MAX_UNIT_REFERENCES)
        owner = sum(len(e["description"].get("owner_text", "").encode()) for e in value["entries"])
        self.assertLessEqual(owner, OWNER_TOTAL)
        for entry in value["entries"]:
            self.assertLessEqual(len(entry["name"]), 100)
        return references

    def test_every_shape_fits_at_4096_entries_too(self):
        # Fewer entries leave more bytes for text property values: they must stay ASCII so
        # the byte budget holds (a 4,096-entry run once overflowed 4 MiB).
        for build in (fanout, fts_heavy, declaration_heavy):
            data = encoded(build(4096, 0))
            self.assertGreater(len(data), MAX_DRAFT_BYTES - 128 * 1024)

    def test_fts_heavy_names_are_long_and_every_token_is_distinct(self):
        first, second = fts_heavy(8192, 0), fts_heavy(8192, 1)
        for value in (first, second):
            self.assertEqual(self.common(value), MAX_UNIT_REFERENCES)
            tokens = [token for e in value["entries"] for token in e["name"].split()]
            self.assertEqual(len(tokens), len(set(tokens)))
            self.assertGreaterEqual(len(tokens), 8192 * 20)
            self.assertTrue(all(len(e["name"]) >= 99 for e in value["entries"]))
        self.assertNotEqual(encoded(first), encoded(second))

    def test_declaration_heavy_declares_every_type_property_and_option(self):
        first, second = declaration_heavy(8192, 0), declaration_heavy(8192, 1)
        for value in (first, second):
            # At the 8,192 cap every reference fits beside the declarations.
            self.assertEqual(self.common(value), MAX_UNIT_REFERENCES)
            self.assertEqual(len(value["entry_types"]), 32)
            for declared in value["entry_types"]:
                self.assertEqual(len(declared["properties"]), 16)
                for item in declared["properties"]:
                    self.assertEqual(item["control"], "enum")
                    ids = [option["id"] for option in item["options"]]
                    self.assertEqual(len(set(ids)), 128)
        self.assertEqual(
            [(e["key"], e["type"]) for e in first["entries"]],
            [(e["key"], e["type"]) for e in second["entries"]],
        )
        self.assertNotEqual(encoded(first), encoded(second))


def observe(hold, schedule):
    """Probes of a lock held over `hold` = (acquired, released): each `(start, executed)` is a
    probe recorded at `start` whose BEGIN IMMEDIATE actually ran at `executed` (a thread
    descheduled in between), completing just after."""
    acquired, released = hold
    return [
        (start, executed + 1e-4, not acquired <= executed < released, None)
        for start, executed in schedule
    ]


def longest(runs):
    return max(runs, key=lambda run: run["hold"][0])


class LockProbeTests(unittest.TestCase):
    def assert_brackets(self, runs, true_hold):
        lower, upper = longest(runs)["hold"]
        self.assertLessEqual(lower, true_hold + 1e-9)
        self.assertIsNotNone(upper)
        self.assertGreaterEqual(upper, true_hold - 1e-9)

    def test_delayed_probes_still_bracket_the_hold(self):
        # Sol's scenario: the free probe after the hold was recorded before the release but
        # ran after it; the first refused probe was recorded before the acquisition. Start
        # times alone put the upper bound at 1.51 s for a 2 s hold.
        probes = observe(
            (1.0, 3.0),
            [
                (0.0, 0.0),
                (0.99, 0.995),
                (0.999, 1.4),
                (2.0, 2.0),
                (2.5, 3.2),
                (3.5, 3.5),
            ],
        )
        self.assert_brackets(publish_hold(probes), 2.0)

    def test_random_delays_always_bracket_the_hold(self):
        rng = Rng(42)
        checked = 0
        for _ in range(300):
            acquired = 1.0 + rng.below(1000) / 1000
            released = acquired + 0.05 + rng.below(3000) / 1000
            schedule, at = [], 0.0
            while at < released + 0.5:
                executed = at + rng.below(400) / 1000 * (rng.below(4) == 0)
                schedule.append((at, executed))
                at = executed + 1e-4 + rng.below(20) / 1000
            runs = publish_hold(observe((acquired, released), schedule))
            if any(acquired <= executed < released for _, executed in schedule):
                checked += 1
                self.assert_brackets(runs, released - acquired)
            else:  # no probe ran inside the hold: nothing is attributed to it
                self.assertEqual(runs, [])
        self.assertGreater(checked, 250)

    def test_probes_refused_by_the_contender_are_not_attributed(self):
        # The publish holds [1, 3); the contender began waiting at 1.5, took the lock at 3.0
        # (its BEGIN returned at 3.05) and held it to 3.3. The probe recorded at 3.04 ran at
        # 3.25 and was refused by the contender, not the publish.
        def executed_free(executed):
            return not (1.0 <= executed < 3.3)

        schedule = [
            (0.0, 0.0),
            (0.9, 0.9),
            (1.0, 1.0),
            (2.0, 2.0),
            (3.04, 3.25),
            (3.4, 3.4),
        ]
        probes = [(s, e + 1e-4, executed_free(e), None) for s, e in schedule]
        runs = publish_hold(probes, contender=(1.5, 3.05))
        self.assert_brackets(runs, 2.0)

    def test_uncommitted_reads_keep_the_lower_bound_tight_under_contention(self):
        # As above, but each refused probe also read the version count: 4 before the publish
        # committed, 5 after. The probe at 2.0 read 4, so the release came after its start
        # although the contender was already waiting; the one the contender refused read 5.
        schedule = [(0.0, 0.0), (0.9, 0.9), (1.0, 1.0), (2.0, 2.0), (3.04, 3.25), (3.4, 3.4)]
        probes = [
            (s, e + 1e-4, not (1.0 <= e < 3.3), None if not 1.0 <= e < 3.3 else 4 + (e >= 3.0))
            for s, e in schedule
        ]
        lower, upper = longest(publish_hold(probes, (1.5, 3.05), published=5))["hold"]
        self.assertAlmostEqual(lower, 2.0 - 1.0001)
        self.assertAlmostEqual(upper, 3.05 - 0.9)
        # Without the reads only the probes before the contender's wait count.
        lower, _ = longest(publish_hold(probes, (1.5, 3.05)))["hold"]
        self.assertEqual(lower, 0.0)

    def test_a_hold_without_a_later_free_probe_has_no_upper_bound(self):
        probes = observe((1.0, 9.0), [(0.0, 0.0), (0.5, 0.5), (1.5, 1.5), (2.0, 2.0)])
        self.assertEqual(len(probes[0]), 4)
        lower, upper = longest(publish_hold(probes))["hold"]
        self.assertIsNone(upper)
        self.assertLessEqual(lower, 8.0)


class ProbeLifecycleTests(unittest.TestCase):
    def test_poller_failures_raise_promptly_without_running_the_command(self):
        ran = []
        with tempfile.TemporaryDirectory() as directory:
            garbage = Path(directory) / "garbage.sqlite3"
            garbage.write_bytes(b"not a database" * 512)
            for database in (Path(directory) / "missing" / "db.sqlite3", garbage):
                started = time.perf_counter()
                with self.assertRaises(ProbeError):
                    probe_publish(
                        None,
                        database,
                        ["publish"],
                        run=lambda *args: ran.append(args),
                        ready_timeout=5.0,
                        marker=None,
                    )
                self.assertLess(time.perf_counter() - started, 4.0)
        self.assertEqual(ran, [])

    def test_a_lock_never_free_times_out_instead_of_hanging(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "db.sqlite3"
            holder = sqlite3.connect(database, isolation_level=None)
            holder.execute("CREATE TABLE t(x)")
            holder.execute("BEGIN IMMEDIATE")
            try:
                started = time.perf_counter()
                with self.assertRaises(ProbeError):
                    probe_publish(
                        None, database, ["publish"], run=None, ready_timeout=0.5, marker=None
                    )
                self.assertLess(time.perf_counter() - started, 3.0)
            finally:
                holder.execute("ROLLBACK")
                holder.close()

    def test_a_failing_command_stops_the_poller(self):
        def failing(*_args):
            raise RuntimeError("command failed")

        before = threading.active_count()
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "db.sqlite3"
            sqlite3.connect(database).close()
            with self.assertRaises(RuntimeError):
                probe_publish(
                    None, database, ["publish"], run=failing, ready_timeout=5.0, marker=None
                )
        self.assertEqual(threading.active_count(), before)

    def test_a_real_hold_is_bracketed_and_the_contender_waits(self):
        hold = {}

        def publish(_binary, database, _command):
            db = sqlite3.connect(database, isolation_level=None)
            hold["begin"] = time.perf_counter()
            db.execute("BEGIN IMMEDIATE")
            hold["acquired"] = time.perf_counter()
            db.execute("INSERT INTO catalogue_versions VALUES(1)")
            time.sleep(0.3)
            hold["released"] = time.perf_counter()
            db.execute("COMMIT")
            hold["committed"] = time.perf_counter()
            db.close()
            time.sleep(0.1)
            return {"success": True}

        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "db.sqlite3"
            db = sqlite3.connect(database)
            db.execute("CREATE TABLE profile_writes(id INTEGER PRIMARY KEY, at TEXT)")
            db.execute("CREATE TABLE catalogue_versions(id INTEGER)")
            db.close()
            result = probe_publish(None, database, ["publish"], run=publish, ready_timeout=5.0)
        lock = result["lock"]
        lower, upper = (
            lock["publish_hold"]["hold_ms_lower"],
            lock["publish_hold"]["hold_ms_upper"],
        )
        # The true hold lies between these two spans.
        self.assertLessEqual(lower, (hold["committed"] - hold["begin"]) * 1000)
        self.assertGreaterEqual(upper, (hold["released"] - hold["acquired"]) * 1000)
        # The uncommitted reads keep the lower bound informative although the contender
        # began waiting 50 ms into the hold.
        self.assertGreater(lower, 200.0)
        self.assertLess(lock["contender"]["began_after_ms"], 200.0)
        self.assertTrue(lock["contender"]["success"])


if __name__ == "__main__":
    unittest.main()
