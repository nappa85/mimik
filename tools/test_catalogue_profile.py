"""The catalogue profile draft must sit at the plan's maxima without exceeding any limit."""

import argparse
import json
import re
import sqlite3
import tempfile
import threading
import time
import unittest
from pathlib import Path

from catalogue_profile import (
    MAX_DISTINCT_UNITS,
    MAX_DRAFT_BYTES,
    MAX_ENTRIES,
    MAX_ID,
    MAX_NAME,
    MAX_NODES,
    MAX_OWNER_TEXT,
    MAX_TEXT_PROPERTY,
    MAX_UNIT_REFERENCES,
    OWNER_TOTAL,
    RUST_LIMITS,
    SHAPES,
    ProbeError,
    Rng,
    declaration_heavy,
    draft,
    encoded,
    fanout,
    fts3_heavy,
    fts_heavy,
    hold_budget,
    long_keys,
    probe_publish,
    publish_span,
    r8_failures,
    r8_gate,
    span_ms,
)

ROOT = Path(__file__).resolve().parent.parent


def rust_limits():
    """Every `const MAX_*: usize` of the catalogue decoder and admission, evaluated."""
    found = {}
    for name in ("decode.rs", "admission.rs"):
        text = (ROOT / "src" / "catalogues" / name).read_text()
        for const, expression in re.findall(r"const (MAX_\w+): usize = ([0-9_ *]+);", text):
            value = 1
            for factor in expression.split("*"):
                value *= int(factor.strip().replace("_", ""))
            if const in found:
                raise AssertionError(f"{const} is defined in more than one catalogue module")
            found[const] = value
    return found


class RustLimitTests(unittest.TestCase):
    def test_profiler_limits_match_the_rust_constants(self):
        # A Rust-only cap change must fail here, not silently profile a non-worst shape.
        rust = rust_limits()
        self.assertGreaterEqual(len(rust), 15)
        for name, value in RUST_LIMITS.items():
            self.assertIn(name, rust)
            self.assertEqual(value, rust[name], name)


class DraftShapeTests(unittest.TestCase):
    def test_capacity_drafts_fit_every_limit_and_grow_append_only(self):
        for capacity in (4096, MAX_ENTRIES):
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
            self.assertLessEqual(len(units), MAX_DISTINCT_UNITS)
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
        first, second = fanout(MAX_ENTRIES, 0), fanout(MAX_ENTRIES, 1)
        for value in (first, second):
            self.assertEqual(len(value["entries"]), MAX_ENTRIES)
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
        self.assertEqual(len(value["entries"]), MAX_ENTRIES)
        self.assertLessEqual(nodes(value), MAX_NODES)
        references = sum(len(e["description"].get("units", [])) for e in value["entries"])
        self.assertLessEqual(references, MAX_UNIT_REFERENCES)
        owner = sum(len(e["description"].get("owner_text", "").encode()) for e in value["entries"])
        self.assertLessEqual(owner, OWNER_TOTAL)
        keys = [entry["key"] for entry in value["entries"]]
        self.assertEqual(len(keys), len(set(keys)))
        for entry in value["entries"]:
            self.assertLessEqual(len(entry["name"]), MAX_NAME)
            self.assertLessEqual(len(entry["key"]), MAX_ID)
            self.assertLessEqual(len(entry["description"].get("owner_text", "")), MAX_OWNER_TEXT)
            for item in entry["properties"].values():
                if isinstance(item, str):
                    self.assertLessEqual(len(item), MAX_TEXT_PROPERTY)
        return references

    def test_every_shape_fits_at_4096_entries_too(self):
        # Fewer entries leave more bytes for text property values: they must stay ASCII so
        # the byte budget holds (a 4,096-entry run once overflowed 4 MiB).
        for build in (fanout, fts_heavy, declaration_heavy, fts3_heavy, long_keys):
            data = encoded(build(4096, 0))
            # The declaration-heavy shape has no text properties, so at 4,096 entries its
            # owner text (1 MiB total) cannot take every spare byte.
            self.assertGreater(len(data), MAX_DRAFT_BYTES - 256 * 1024)

    def test_fts_heavy_names_are_long_and_every_token_is_distinct(self):
        first, second = fts_heavy(MAX_ENTRIES, 0), fts_heavy(MAX_ENTRIES, 1)
        for value in (first, second):
            self.assertEqual(self.common(value), MAX_UNIT_REFERENCES)
            tokens = [token for e in value["entries"] for token in e["name"].split()]
            self.assertEqual(len(tokens), len(set(tokens)))
            self.assertGreaterEqual(len(tokens), MAX_ENTRIES * 20)
            self.assertTrue(all(len(e["name"]) >= 99 for e in value["entries"]))
        self.assertNotEqual(encoded(first), encoded(second))

    def test_fts3_names_use_three_character_tokens_shared_a_few_times(self):
        first, second = fts3_heavy(MAX_ENTRIES, 0), fts3_heavy(MAX_ENTRIES, 1)
        for value in (first, second):
            self.assertEqual(self.common(value), MAX_UNIT_REFERENCES)
            tokens = [token for e in value["entries"] for token in e["name"].split()]
            self.assertEqual(len(tokens), MAX_ENTRIES * 25)
            self.assertTrue(all(len(token) == 3 for token in tokens))
            self.assertEqual(len(set(tokens)), 36**3)
            self.assertTrue(all(len(e["name"]) == 99 for e in value["entries"]))
        self.assertNotEqual(encoded(first), encoded(second))

    def test_long_keys_sit_at_the_id_cap_on_fts_heavy_names(self):
        first, second = long_keys(MAX_ENTRIES, 0), long_keys(MAX_ENTRIES, 1)
        for value in (first, second):
            self.assertEqual(self.common(value), MAX_UNIT_REFERENCES)
            self.assertTrue(all(len(e["key"]) == MAX_ID for e in value["entries"]))
            self.assertTrue(all(len(e["name"]) >= 99 for e in value["entries"]))
        self.assertEqual(
            [(e["key"], e["type"]) for e in first["entries"]],
            [(e["key"], e["type"]) for e in second["entries"]],
        )
        self.assertNotEqual(encoded(first), encoded(second))

    def test_declaration_heavy_declares_every_type_property_and_option(self):
        first, second = declaration_heavy(MAX_ENTRIES, 0), declaration_heavy(MAX_ENTRIES, 1)
        for value in (first, second):
            # At the reference cap every reference fits beside the declarations.
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


OLD, NEW = 4, 5  # catalogue versions before and after the publish


def observe(schedule, publish, others=()):
    """Probes of a lock held by the publish over `publish` = (acquired, released) and by
    other transactions over `others` (the store-open lock, the contender), adding no version:
    each `(start, executed)` is a probe recorded at `start` whose BEGIN IMMEDIATE and read
    actually ran at `executed` (a thread descheduled in between), completing just after. Every
    probe reads the version count (the publish's row is visible from its release)."""
    holds = [publish, *others]
    return [
        (
            start,
            executed + 1e-4,
            not any(acquired <= executed < released for acquired, released in holds),
            NEW if executed >= publish[1] else OLD,
        )
        for start, executed in schedule
    ]


class PublishSpanTests(unittest.TestCase):
    def assert_upper(self, probes, hold):
        span = publish_span(probes, NEW)
        self.assertTrue(span["bounded"], span)
        self.assertGreaterEqual(span["upper"], hold[1] - hold[0] - 1e-9)
        return span

    def test_delayed_probes_still_bracket_the_hold(self):
        # Sol round 2: the free probe after the hold was recorded before the release but ran
        # after it; the first refused probe was recorded before the acquisition.
        probes = observe(
            [(0.0, 0.0), (0.99, 0.995), (0.999, 1.4), (2.0, 2.0), (2.5, 3.2), (3.5, 3.5)],
            (1.0, 3.0),
        )
        span = self.assert_upper(probes, (1.0, 3.0))
        self.assertLessEqual(span["lower"], 2.0)

    def test_startup_contender_then_publish_with_missed_free_gaps(self):
        # Sol round 3: the store-open lock [0.02, 0.08), then the contender [0.096, 0.1),
        # then the publish [0.12, 2.12), with no free probe in either gap. Capping the
        # release at the contender's acquisition (0.096) bounded a 2 s hold by 0.1 s.
        probes = observe(
            [(0.0, 0.0), (0.05, 0.05), (0.5, 0.5), (1.0, 1.0), (2.0, 2.0), (2.2, 2.2)],
            (0.12, 2.12),
            [(0.02, 0.08), (0.096, 0.1)],
        )
        self.assert_upper(probes, (0.12, 2.12))

    def test_observed_startup_with_unobserved_publication(self):
        # The store-open lock is seen; the poller is then descheduled across the whole
        # publish. The startup run must not be reported as the publish's hold.
        probes = observe(
            [(0.0, 0.0), (0.05, 0.05), (0.1, 0.1), (0.11, 2.5), (2.6, 2.6)],
            (0.12, 2.12),
            [(0.02, 0.08)],
        )
        span = self.assert_upper(probes, (0.12, 2.12))
        self.assertEqual(span["refused"], 0)
        self.assertEqual(span["lower"], 0.0)

    def test_random_schedules_always_bound_the_hold_from_above(self):
        rng = Rng(42)
        bounded = 0
        for trial in range(500):
            startup = 0.5 + rng.below(500) / 1000
            startup_end = startup + 0.03 + rng.below(20) / 1000
            acquired = startup_end + rng.below(300) / 1000
            released = acquired + 0.05 + rng.below(3000) / 1000
            others = [(startup, startup_end)]
            if trial % 2:  # the contender, before or after the publish
                at = acquired - 0.002 if trial % 4 == 1 else released
                others.append((at - 0.001, at))
                if trial % 4 == 3:
                    others[-1] = (released, released + 0.002)
            schedule, at = [], 0.0
            while at < released + 0.5:
                executed = at + rng.below(400) / 1000 * (rng.below(4) == 0)
                schedule.append((at, executed))
                at = executed + 1e-4 + rng.below(20) / 1000
            probes = observe(schedule, (acquired, released), others)
            span = publish_span(probes, NEW)
            if not span["bounded"]:
                # Only when no free probe ran after the publish released.
                self.assertFalse(any(p[2] and p[3] == NEW for p in probes))
                continue
            self.assertGreaterEqual(span["upper"], released - acquired - 1e-9)
            bounded += 1
        self.assertGreater(bounded, 450)

    def test_an_unbounded_hold_is_reported_and_fails_the_gate(self):
        # Space Bunny M1: no free probe after the release must never yield a null bound.
        probes = observe([(0.0, 0.0), (0.5, 0.5), (1.5, 1.5), (2.0, 2.0)], (1.0, 9.0))
        span = publish_span(probes, NEW)
        self.assertFalse(span["bounded"])
        self.assertIn("published version", span["reason"])
        shown = span_ms(span, 0.0, False)
        self.assertEqual(shown, {"bounded": False, "reason": span["reason"]})
        failures = r8_failures("case", {"lock": {"publish_hold": shown}}, 2000.0)
        self.assertEqual(len(failures), 1)
        self.assertIn("not bounded", failures[0])
        # Nor without a free probe before it that read the old count.
        probes = observe([(1.5, 1.5), (9.5, 9.5)], (1.0, 9.0))
        self.assertFalse(publish_span(probes, NEW)["bounded"])

    def test_bounds_round_outward(self):
        # Sol round 3: an exact 1,819.049 ms upper bound printed as 1,819.0 was below it.
        span = {
            "bounded": True,
            "acquired": (10.00004, 10.00106),
            "released": (11.81904, 11.819091),
            "upper": 1.819049,
            "lower": 1.81799,
            "refused": 3,
        }
        shown = span_ms(span, 10.0, False)
        self.assertEqual(shown["hold_ms_upper"], 1819.1)
        self.assertEqual(shown["hold_ms_lower"], 1817.9)
        # Refused probes are not attributed to the publish, so its endpoints are indicative;
        # the outer span (last old-count free probe to first new-count one) is the bound.
        self.assertNotIn("acquired_after_ms", shown)
        self.assertEqual(shown["indicative_acquired_after_ms"], [0.0, 1.1])
        self.assertEqual(shown["indicative_released_after_ms"], [1819.0, 1819.1])
        self.assertEqual(shown["span_after_ms"], [0.0, 1819.1])
        self.assertIn("conservative", shown["bounds"])
        self.assertIn("loose", span_ms(span, 10.0, True)["bounds"])


class R8GateTests(unittest.TestCase):
    def publish(self, upper, contender=None):
        lock = {"publish_hold": {"bounded": True, "hold_ms_upper": upper}}
        if contender is not None:
            lock["contender"] = contender
        return {"lock": lock}

    def test_the_gate_fails_over_the_budget_and_on_a_failed_contender(self):
        # Space Bunny N6: R8's criteria are checked, not read by eye.
        self.assertEqual(r8_failures("a", self.publish(2000.0), 2000.0), [])
        self.assertEqual(len(r8_failures("a", self.publish(2000.1), 2000.0)), 1)
        good = {"success": True, "waited_through_publish": True}
        self.assertEqual(r8_failures("a", self.publish(1.0, good), 2000.0), [])
        for bad in (
            {"success": False, "waited_through_publish": False, "error": "busy"},
            {"success": True, "waited_through_publish": False},
            {"started": False, "success": False, "waited_through_publish": False},
        ):
            self.assertEqual(len(r8_failures("a", self.publish(1.0, bad), 2000.0)), 1)

    def test_the_gate_fails_closed_without_publications_or_contender_evidence(self):
        # Sol round 4: zero gated publications must not pass, nor a run whose contender was
        # requested but never observed.
        good = {"success": True, "waited_through_publish": True}
        self.assertFalse(r8_gate({}, 2000.0, contender_expected=False)["passed"])
        self.assertFalse(r8_gate({"rebuild": {}}, 2000.0, contender_expected=True)["passed"])
        no_contender = {"v1": {"publish": self.publish(1.0)}}
        full = {"entries": MAX_ENTRIES, "references": MAX_UNIT_REFERENCES, "shapes": list(SHAPES)}
        gate = r8_gate(no_contender, 2000.0, False, coverage=full)
        self.assertTrue(gate["passed"])
        self.assertEqual(gate["publications"], 1)
        self.assertFalse(gate["contender_observed"])
        self.assertFalse(r8_gate(no_contender, 2000.0, contender_expected=True)["passed"])
        observed = {"v1": {"publish": self.publish(1.0, good)}}
        gate = r8_gate(observed, 2000.0, True, coverage=full)
        self.assertTrue(gate["passed"])
        self.assertTrue(gate["contender_observed"])

    def test_the_gate_requires_a_complete_run_at_the_caps(self):
        # Sonnet and Space Bunny round 5: a reduced profile (fewer entries or references, a
        # subset of shapes, no calibrated cases) must not pass as R8 evidence.
        full = {"entries": MAX_ENTRIES, "references": MAX_UNIT_REFERENCES, "shapes": list(SHAPES)}
        cases = {"v1": {"publish": self.publish(1.0)}}
        self.assertTrue(r8_gate(cases, 2000.0, False, coverage=full)["passed"])
        for partial in (
            {**full, "entries": 64},
            {**full, "references": 0},
            {**full, "shapes": ["fanout"]},
            {**full, "calibrated": False},
        ):
            gate = r8_gate(cases, 2000.0, False, coverage=partial)
            self.assertFalse(gate["passed"], partial)
            self.assertFalse(gate["at_caps"])
            self.assertTrue(r8_gate(cases, 2000.0, False, partial, allow_partial=True)["passed"])

    def test_a_requested_contender_is_required_on_every_publication(self):
        # Space Bunny round 5 F2: a publication without contender evidence fails on its own.
        good = {"success": True, "waited_through_publish": True}
        mixed = {
            "a": {"publish": self.publish(1.0, good)},
            "b": {"publish": self.publish(1.0)},
        }
        self.assertFalse(r8_gate(mixed, 2000.0, contender_expected=True)["passed"])
        self.assertEqual(len(r8_failures("b", self.publish(1.0), 2000.0, True)), 1)

    def test_the_budget_must_be_finite_and_positive(self):
        # Sol round 4: `hold > nan` is always false, so a NaN budget passed every hold.
        self.assertEqual(hold_budget("2000"), 2000.0)
        for bad in ("nan", "inf", "-inf", "0", "-1", "x"):
            with self.assertRaises(argparse.ArgumentTypeError):
                hold_budget(bad)


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
        def run(contend_after):
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
                return {"success": True}

            with tempfile.TemporaryDirectory() as directory:
                database = Path(directory) / "db.sqlite3"
                db = sqlite3.connect(database)
                db.execute("CREATE TABLE profile_writes(id INTEGER PRIMARY KEY, at TEXT)")
                db.execute("CREATE TABLE catalogue_versions(id INTEGER)")
                db.close()
                result = probe_publish(
                    None, database, ["publish"], contend_after, run=publish, ready_timeout=5.0
                )
            lock = result["lock"]
            self.assertTrue(lock["publish_hold"]["bounded"])
            # The true hold lies within the upper bound.
            self.assertGreaterEqual(
                lock["publish_hold"]["hold_ms_upper"], (hold["released"] - hold["acquired"]) * 1000
            )
            return lock, hold

        lock, hold = run(None)
        self.assertNotIn("contender", lock)
        self.assertLessEqual(
            lock["publish_hold"]["hold_ms_lower"], (hold["committed"] - hold["begin"]) * 1000
        )
        self.assertGreater(lock["publish_hold"]["hold_ms_lower"], 200.0)
        lock, _ = run(0.05)
        self.assertIn("loose", lock["publish_hold"]["bounds"])
        self.assertLess(lock["contender"]["began_after_ms"], 200.0)
        self.assertTrue(lock["contender"]["success"])
        self.assertTrue(lock["contender"]["waited_through_publish"])
        self.assertEqual(r8_failures("real", {"lock": lock}, 2000.0), [])


if __name__ == "__main__":
    unittest.main()
