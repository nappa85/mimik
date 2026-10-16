"""Unattended queue (tools/run_queue.py): fake runner, fake clock (plan §11 item 26).

Failing-first: run before tools/run_queue.py existed (ModuleNotFoundError), and the review
regressions (ReviewFix* classes) against commit 7bb41d6; the mutation checks are listed in
docs/tool-loop-benchmark.md ("Run infrastructure").
"""

import datetime as dt
import json
import os
import signal
import stat
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

import model_registry
import run_queue as rq
import tool_loop_benchmark as tl

TOOLS = Path(__file__).resolve().parent
REGISTRY = model_registry.load()
SUBSET = TOOLS / "case-subsets" / "dev-screen-40.json"
HARNESS_SHA = tl.file_digest(TOOLS / "tool_loop_benchmark.py")
DEV_IDS = [f"c{n:03d}" for n in range(174)]
FLAG_ENV = {
    "NUDGE": ("nudge", "1"),
    "TURNS_REMAINING": ("turns_remaining", "0"),
    "SEARCH_NOTES": ("search_notes", "0"),
    "SANITIZE_ECHO": ("sanitize_echo", "1"),
    "PENDING_CURRENT": ("pending_current", "1"),
    "UNTRUSTED": ("untrusted", "0"),
}


def at(*args) -> float:
    """Epoch of a naive local time."""
    return dt.datetime(*args).timestamp()


# 2026-10-14 is a Wednesday; 2026-10-16 a Friday; 2026-10-17 a Saturday.
WED, THU, FRI, SAT, SUN, MON = 14, 15, 16, 17, 18, 19


class FakeClock:
    def __init__(self, start):
        self.now = start
        self.sleeps = []
        self.on_sleep = None

    def time(self):
        return self.now

    def sleep(self, seconds):
        if self.on_sleep:
            self.on_sleep(self)
        self.sleeps.append(seconds)
        self.now += seconds


class FakeHandle:
    def __init__(self, launcher, behaviour, env, argv):
        self.launcher, self.behaviour, self.env, self.argv = launcher, behaviour, env, argv
        self.start = launcher.clock.now
        self.rc = None
        self.out = Path(env["OUT_DIR"])

    def poll(self):
        if self.rc is None and self.behaviour.get("jump_to"):
            self.launcher.clock.now = self.behaviour.pop("jump_to")  # e.g. a suspended machine
        if self.rc is None and self.launcher.clock.now >= self.start + self.behaviour["seconds"]:
            self.rc = write_attempt(self.out, self.behaviour, self.env, self.argv)
        return self.rc

    def signal(self, sig):
        self.launcher.signals.append(sig)
        if self.rc is None and (sig == signal.SIGKILL or not self.behaviour.get("ignore_term")):
            with open(self.out / "provenance.txt", "a", encoding="utf-8") as handle:
                handle.write("date_start=x\nstopped_by_signal=TERM\ncleanup_verified=true\n")
            self.rc = 137 if sig == signal.SIGKILL else 143

    def wait(self, timeout):
        return self.rc


class FakeCheckHandle:
    """The CHECK_ONLY pre-pass: instant, then check_only=1 (or a failure)."""

    def __init__(self, clock, env, fail):
        self.clock, self.env, self.fail = clock, env, fail
        self.start, self.rc = clock.now, None

    def poll(self):
        if self.rc is None:
            out = Path(self.env["OUT_DIR"])
            if self.fail:
                text = "failure=model_hash\nfailure_detail=model sha256 mismatch\n"
                self.rc = 5
            else:
                text = "check_only=1 (stopped before launch)\ncleanup_verified=true\n"
                self.rc = 0
            (out / "provenance.txt").write_text(text)
        return self.rc

    def signal(self, sig):
        self.rc = 143

    def wait(self, timeout):
        return self.rc


class FakeWatchdog:
    def __init__(self, alive=True, restartable=True):
        self.is_alive, self.restartable = alive, restartable
        self.ensures = 0

    def alive(self):
        return self.is_alive

    def ensure(self):
        self.ensures += 1
        if not self.is_alive and self.restartable:
            self.is_alive = True
        return self.is_alive


class FakeLauncher:
    """Plays a behaviour per launch: {"seconds", "kind", ...}; default a complete run."""

    def __init__(self, clock, plan=None):
        self.clock = clock
        self.plan = plan or {}
        self.launches = []
        self.checks = []
        self.signals = []
        self.order = []

    def start(self, argv, env, stdout_path, stderr_path):
        arm = Path(env["OUT_DIR"]).parent.name
        stdout_path.write_text("")
        if env.get("CHECK_ONLY") == "1":
            self.checks.append(arm)
            self.order.append(("check", arm))
            return FakeCheckHandle(self.clock, env, self.plan.get(f"{arm}:check_fail"))
        self.order.append(("run", arm))
        n = sum(1 for a, _ in self.launches if a == arm)
        self.launches.append((arm, dict(env)))
        stdout_path.write_text("runner stdout\n")
        stderr_path.write_text("")
        behaviours = self.plan.get(arm, [{}])
        chosen = behaviours[min(n, len(behaviours) - 1)]
        return FakeHandle(self, dict({"seconds": 600, "kind": "complete"}, **chosen), env, argv)


def fake_header(env, key, split, b):
    """The header a correct live run of this env writes (independent of run_queue.plan)."""
    entry = REGISTRY["models"][key]
    ctx = int(b.get("ctx", env.get("CTX", entry["server"]["ctx"])))
    moe = env.get("N_CPU_MOE")
    seeds = [int(s) for s in env["SEEDS"].split(",")]
    if env.get("CASES_FILE"):
        doc = json.loads(Path(env["CASES_FILE"]).read_text())
        cases, subset = list(doc["case_ids"]), {"sha256": doc["sha256"]}
    elif split == "smoke":
        cases, subset = ["dev-001", "dev-005", "dev-008"], None
    else:
        cases, subset = list(DEV_IDS), None
    if b.get("drop_case"):
        cases = cases[1:]
    flags = {name: env.get(var, default) == "1" for var, (name, default) in FLAG_ENV.items()}
    flags.update(b.get("flags", {}))
    header = {
        "event": "run_started",
        "benchmark_version": 3,
        "harness_sha256": b.get("harness_sha256", HARNESS_SHA),
        "dry_run": b.get("dry_run", False),
        "arm": env["ARM"],
        "model_label": entry["label"],
        "model_sha256": entry["sha256"],
        "split": "dev" if split == "smoke" else split,
        "seeds": seeds,
        "ctx": ctx,
        "max_turns": int(env.get("MAX_TURNS", 8)),
        "top_k": 5,
        "flags": flags,
        "sampling": {
            "temperature": float(env.get("TEMPERATURE", 0.1)),
            "top_p": float(env.get("TOP_P", 0.95)),
            "max_tokens": int(env.get("MAX_TOKENS", 4096)),
            "tool_choice": "auto",
            "thinking": env.get("THINKING", "off"),
            "cache_prompt": True,
        },
        "thinking_capability": entry["capabilities"]["thinking"],
        "server": {
            "image": entry["server_image"],
            "args": model_registry.server_args(entry, ctx, None if moe is None else int(moe)),
        },
        "planned_episodes": len(cases) * len(seeds),
        "case_order": cases,
        "case_subset": subset,
    }
    return header, cases, seeds


def write_attempt(out: Path, b: dict, env: dict, argv) -> int:
    """What the runner leaves behind for a given outcome kind."""
    key, split = argv[1], argv[2]
    prov = ["date_start=x", f"runner=fake key={key}"]
    kind = b["kind"]
    tail = [] if b.get("cleanup_unverified") else ["cleanup_verified=true"]
    if kind in ("server_health", "server_start", "infra_vram"):
        prov += [f"failure={kind}", "failure_detail=server not healthy after 900s"]
        lines = prov + ["vram_used_before=100"] + tail
        (out / "provenance.txt").write_text("\n".join(lines) + "\n")
        return 4
    if kind == "oom":
        prov += ["server_oom_killed=true", "failure=server_health", "failure_detail=died"]
        (out / "provenance.txt").write_text("\n".join(prov + tail) + "\n")
        return 4
    if kind == "window_refused":
        prov += ["failure=window", "failure_detail=before server: closed"]
        (out / "provenance.txt").write_text("\n".join(prov + tail) + "\n")
        return 10
    if kind == "no_provenance":
        return 9
    header, cases, seeds = fake_header(env, key, split, b)
    rows = [header]
    not_run, errors = b.get("not_run", 0), b.get("errors", 0)
    total = len(cases) * len(seeds)
    for n, (seed, case) in enumerate((s, c) for s in seeds for c in cases):
        outcome = "answer"
        if n >= total - not_run:
            outcome = "not_run"
        elif n < errors:
            outcome = "error"
        rows.append(
            {
                "event": "episode",
                "model_label": header["model_label"],
                "arm": header["arm"],
                "split": header["split"],
                "case_id": case,
                "seed": seed,
                "outcome": outcome,
            }
        )
    if b.get("row_arm"):
        rows[1]["arm"] = "other"
    if b.get("dup_row"):
        rows.insert(2, dict(rows[1]))
        header["planned_episodes"] += 1
    if kind == "aborted_health":
        reason = "server health check failed after an error episode"
        rows.append({"event": "run_aborted", "reason": reason})
    elif b.get("no_summary"):
        pass
    else:
        rows.append({"event": "run_summary", "summaries": []})
    (out / "ledger-1.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    main_exit = {"complete": 0, "harness_fail": 4, "aborted_health": 4, "main_oom": 137}[kind]
    if b.get("preflight", True):
        prov.append("preflight_exit=0 preflight_oom_killed=false preflight_wall_seconds=30")
    vram = b.get("vram", 6_000_000_000)
    prov += [
        "ledger_file=ledger-1.jsonl",
        f"vram_used_before=500 swap_used_mib_before={b.get('swap_before', 100)}",
        f"server_ready_seconds=4 vram_used_loaded={vram}",
        f"main_exit={main_exit} main_oom_killed={str(kind == 'main_oom').lower()} "
        f"main_wall_seconds={b.get('wall', 100)}",
        f"vram_used_end={vram} swap_used_mib_after={b.get('swap_after', 100)}",
        f"server_memory_peak=1 server_swap_peak={b.get('swap_peak', 0)}",
        "server_oom_killed=false",
    ]
    if b.get("window_budget"):
        prov.append("main_start_epoch=1 main_budget_seconds=100 main_cutoff=1000")
    (out / "provenance.txt").write_text("\n".join(prov + tail) + "\n")
    if "eval_ms" in b:
        (out / "server.log").write_text(
            f"prompt eval time =    999.00 ms /   100 tokens\n"
            f"       eval time = {b['eval_ms']:9.2f} ms /   100 tokens (x ms per token)\n"
        )
    return b.get("runner_exit", main_exit)


def queue_doc(arms, **extra):
    doc = {
        "schema": 1,
        "name": "test",
        "arms": arms,
        "disk_checks": [{"path": "/", "min_free_gb": 1}],
    }
    doc.update(extra)
    return doc


def arm(id_, model="qwen3-4b-2507", **fields):
    return dict({"id": id_, "model": model, "split": "smoke", "arm": "T", "seeds": "17"}, **fields)


class Harness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        # A model root with the registered files present (empty placeholders).
        self.models = self.root / "models"
        for entry in REGISTRY["models"].values():
            path = self.models / entry["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"")
        self.env = {"MIMIK_MODEL_ROOT": str(self.models), "PATH": os.environ.get("PATH", "")}
        self.console = []
        self.cleanups = 0
        self.cleanup_ok = True

    def make(self, doc, start, plan=None, lock_free=True, disk_free=10**12, **kw):
        path = self.root / f"queue-{len(list(self.root.iterdir()))}.json"
        path.write_text(json.dumps(doc))
        self.clock = FakeClock(start)
        self.launcher = FakeLauncher(self.clock, plan)

        def cleanup():
            self.cleanups += 1
            return {"ok": self.cleanup_ok, "remaining": [] if self.cleanup_ok else ["x"]}

        kw.setdefault("vram_reader", lambda: None)
        self.watchdog = kw.setdefault("watchdog", FakeWatchdog())
        return rq.Queue(
            rq.load_queue(path, REGISTRY),
            REGISTRY,
            self.root / f"session-{len(list(self.root.iterdir()))}",
            clock=self.clock,
            launcher=self.launcher,
            disk_free=lambda p: disk_free,
            lock_probe=lambda: lock_free,
            cleanup=cleanup,
            case_ids=lambda split: DEV_IDS,
            console=self.console.append,
            env=self.env,
            **kw,
        )

    def started_at(self, q, arm_id, attempt=1):
        record = next(a for a in q.arms if a["id"] == arm_id)["attempts"][attempt - 1]
        return dt.datetime.fromisoformat(record["started"])


class WindowTests(unittest.TestCase):
    def test_heavy_allowed(self):
        self.assertTrue(rq.heavy_allowed(at(2026, 10, WED, 8, 59)))
        self.assertFalse(rq.heavy_allowed(at(2026, 10, WED, 9, 0)))
        self.assertFalse(rq.heavy_allowed(at(2026, 10, FRI, 17, 59)))
        self.assertTrue(rq.heavy_allowed(at(2026, 10, FRI, 18, 0)))
        self.assertTrue(rq.heavy_allowed(at(2026, 10, SAT, 12, 0)))
        self.assertTrue(rq.heavy_allowed(at(2026, 10, SUN, 9, 30)))
        self.assertFalse(rq.heavy_allowed(at(2026, 10, MON, 9, 0)))

    def test_window_edges(self):
        self.assertEqual(rq.window_end(at(2026, 10, WED, 3, 0)), at(2026, 10, WED, 9, 0))
        self.assertEqual(rq.window_end(at(2026, 10, WED, 20, 0)), at(2026, 10, THU, 9, 0))
        self.assertEqual(rq.window_end(at(2026, 10, FRI, 20, 0)), at(2026, 10, MON, 9, 0))
        self.assertEqual(rq.window_end(at(2026, 10, SAT, 9, 30)), at(2026, 10, MON, 9, 0))
        self.assertEqual(rq.next_window_start(at(2026, 10, THU, 10, 0)), at(2026, 10, THU, 18, 0))
        self.assertEqual(rq.longest_window_seconds(at(2026, 10, WED, 10, 0)), 63 * 3600)


class WindowCheckCliTests(unittest.TestCase):
    """Review fix 7: the clock hook needs two explicit test variables."""

    def run_check(self, env, *argv):
        base = {"PATH": os.environ.get("PATH", "")}
        base.update(env)
        script = [sys.executable, str(TOOLS / "run_queue.py"), "window-check", *argv]
        return subprocess.run(script, env=base, capture_output=True, text=True, check=False)

    def clock_file(self, when):
        handle = tempfile.NamedTemporaryFile("w", delete=False, suffix=".clock")
        handle.write(str(when))
        handle.close()
        self.addCleanup(os.unlink, handle.name)
        return handle.name

    def test_window_check_with_the_test_clock(self):
        for when, code, word in (
            (at(2026, 10, WED, 10, 0), 1, "closed"),
            (at(2026, 10, WED, 20, 0), 0, "open"),
        ):
            env = {"MIMIK_TEST_MODE": "1", "MIMIK_TEST_CLOCK_FILE": self.clock_file(when)}
            proc = self.run_check(env)
            self.assertEqual(proc.returncode, code, proc.stdout)
            self.assertIn(f"schedule_window={word}", proc.stdout)

    def test_cutoff_and_margin(self):
        now = at(2026, 10, WED, 20, 0)
        env = {"MIMIK_TEST_MODE": "1", "MIMIK_TEST_CLOCK_FILE": self.clock_file(now)}
        proc = self.run_check(env)
        self.assertIn(f"seconds_left={13 * 3600}", proc.stdout)
        proc = self.run_check(env, "--cutoff", str(now + 1000), "--margin", "900")
        self.assertEqual(proc.returncode, 0)
        self.assertIn(f"seconds_left=1000 cutoff={int(now + 1000)}", proc.stdout)
        proc = self.run_check(env, "--cutoff", str(now + 800), "--margin", "900")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("schedule_window=closing", proc.stdout)

    def test_one_test_variable_alone_is_ignored(self):
        real = rq.heavy_allowed(time.time())
        # A clock file whose answer is the opposite of the real clock's.
        opposite = at(2026, 10, WED, 10, 0) if real else at(2026, 10, SAT, 12, 0)
        old_hook = {"MIMIK_QUEUE_NOW": str(opposite)}
        clock_only = {"MIMIK_TEST_CLOCK_FILE": self.clock_file(opposite)}
        mode_only = {"MIMIK_TEST_MODE": "1"}
        for env in (old_hook, clock_only, mode_only):
            proc = self.run_check(env)
            self.assertEqual(proc.returncode == 0, real, (env, proc.stdout))


class ScheduleTests(Harness):
    def test_waits_for_the_evening_window(self):
        q = self.make(queue_doc([arm("a")]), at(2026, 10, WED, 10, 0))
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        self.assertEqual(self.started_at(q, "a"), dt.datetime(2026, 10, WED, 18, 0))
        self.assertTrue(manifest["study_complete"])

    def test_arm_that_does_not_fit_tonight_waits_for_a_longer_window(self):
        # 12 h estimate at Wed 22:00: tonight ends Thu 09:00 (11 h); Thu 18:00 gives 15 h.
        doc = queue_doc([arm("long", split="dev", seconds_per_episode=12 * 3600 / 175 / 1.25)])
        doc["startup_seconds"] = 0
        doc["preflight_case"] = None
        q = self.make(doc, at(2026, 10, WED, 22, 0), plan={"long": [{"seconds": 3600}]})
        q.registry = json.loads(json.dumps(REGISTRY))
        q.registry["models"]["qwen3-4b-2507"]["prewarm"] = False
        q.hash_cache.parent.mkdir(parents=True, exist_ok=True)
        q.hash_cache.write_text(REGISTRY["models"]["qwen3-4b-2507"]["sha256"])
        q.run()
        self.assertEqual(self.started_at(q, "long"), dt.datetime(2026, 10, THU, 18, 0))

    def test_weekday_edge_stops_the_arm_and_reruns_it_whole(self):
        plan = {"a": [{"seconds": 20 * 3600}, {"seconds": 600}]}
        doc = queue_doc([arm("a", max_arm_seconds=40 * 3600)])
        q = self.make(doc, at(2026, 10, WED, 20, 0), plan=plan)
        manifest = q.run()
        attempts = manifest["arms"][0]["attempts"]
        self.assertEqual(attempts[0]["status"], "incomplete_by_window")
        self.assertTrue(attempts[0]["excluded"])
        ended = dt.datetime.fromisoformat(attempts[0]["ended"])
        self.assertLessEqual(ended, dt.datetime(2026, 10, THU, 8, 55))
        self.assertGreaterEqual(ended, dt.datetime(2026, 10, THU, 8, 50))
        self.assertIn(signal.SIGTERM, self.launcher.signals)
        self.assertGreaterEqual(self.cleanups, 2)
        self.assertEqual(self.started_at(q, "a", 2), dt.datetime(2026, 10, THU, 18, 0))
        self.assertEqual((attempts[1]["status"], attempts[1]["excluded"]), ("complete", False))
        self.assertEqual(manifest["arms"][0]["state"], "complete")

    def test_window_stop_twice_leaves_the_arm_incomplete(self):
        plan = {"a": [{"seconds": 20 * 3600}]}
        doc = queue_doc([arm("a", max_arm_seconds=40 * 3600)])
        q = self.make(doc, at(2026, 10, WED, 20, 0), plan=plan)
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "incomplete_by_window")
        self.assertEqual(manifest["missing_required"], ["a"])
        self.assertFalse(manifest["study_complete"])

    def test_weekend_runs_through_the_day(self):
        plan = {"a": [{"seconds": 20 * 3600}]}
        doc = queue_doc([arm("a", seconds_per_episode=10, max_arm_seconds=30 * 3600)])
        q = self.make(doc, at(2026, 10, SAT, 9, 30), plan=plan)
        manifest = q.run()
        self.assertEqual(self.started_at(q, "a"), dt.datetime(2026, 10, SAT, 9, 30))
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        self.assertEqual(self.launcher.signals, [])

    def test_estimate_that_never_fits_is_skipped(self):
        doc = queue_doc([arm("huge", split="dev", seconds_per_episode=3600), arm("b")])
        q = self.make(doc, at(2026, 10, SAT, 10, 0))
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "skipped")
        self.assertEqual(manifest["arms"][1]["state"], "complete")
        self.assertEqual([a for a, _ in self.launcher.launches], ["b"])

    def test_session_deadline(self):
        doc = queue_doc([arm("a"), arm("b")], session_deadline="2026-10-17T10:20:00")
        q = self.make(doc, at(2026, 10, SAT, 10, 0), plan={"a": [{"seconds": 600}]})
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        self.assertEqual(manifest["arms"][1]["state"], "skipped")


class FailureTests(Harness):
    START = at(2026, 10, SAT, 10, 0)

    def test_infrastructure_failure_retried_once(self):
        plan = {"a": [{"kind": "server_health"}, {}]}
        q = self.make(queue_doc([arm("a")]), self.START, plan=plan)
        manifest = q.run()
        attempts = manifest["arms"][0]["attempts"]
        self.assertEqual(
            [(x["status"], x["infra"], x["excluded"]) for x in attempts],
            [("failed", "server_health", True), ("complete", None, False)],
        )
        self.assertTrue(manifest["study_complete"])

    def test_oom_retried_once_then_failed(self):
        plan = {"a": [{"kind": "oom"}]}
        q = self.make(queue_doc([arm("a"), arm("b")]), self.START, plan=plan)
        manifest = q.run()
        self.assertEqual(len(manifest["arms"][0]["attempts"]), 2)
        self.assertEqual(manifest["arms"][0]["attempts"][1]["infra"], "oom")
        self.assertEqual(manifest["arms"][0]["state"], "failed")
        self.assertEqual(manifest["arms"][1]["state"], "complete")

    def test_mid_run_health_abort_is_infrastructure(self):
        plan = {"a": [{"kind": "aborted_health"}, {}]}
        q = self.make(queue_doc([arm("a")]), self.START, plan=plan)
        self.assertEqual(q.run()["arms"][0]["attempts"][0]["infra"], "server_health")

    def test_non_retryable_failure_continues_to_next_arm(self):
        plan = {"a": [{"kind": "harness_fail"}]}
        q = self.make(queue_doc([arm("a"), arm("b")]), self.START, plan=plan)
        manifest = q.run()
        self.assertEqual(len(manifest["arms"][0]["attempts"]), 1)
        self.assertEqual(manifest["arms"][0]["state"], "failed")
        self.assertEqual(manifest["arms"][1]["state"], "complete")
        self.assertEqual(manifest["missing_required"], ["a"])
        self.assertEqual(manifest["state"], "done")

    def test_consecutive_failures_stop_the_queue(self):
        plan = {"a": [{"kind": "harness_fail"}], "b": [{"kind": "no_provenance"}]}
        q = self.make(queue_doc([arm("a"), arm("b"), arm("c")]), self.START, plan=plan)
        manifest = q.run()
        self.assertEqual(manifest["state"], "stopped")
        self.assertIn("2 consecutive", manifest["stop_reason"])
        self.assertEqual(manifest["arms"][2]["state"], "pending")
        self.assertEqual(manifest["missing_required"], ["a", "b", "c"])
        self.assertEqual([a for a, _ in self.launcher.launches], ["a", "b"])

    def test_completeness_requires_planned_equal_run(self):
        plan = {"a": [{"not_run": 1}], "b": [{"ctx": 999}]}
        arms = [arm("a"), arm("b"), arm("c", required=False)]
        doc = queue_doc(arms, max_consecutive_failures=3)
        q = self.make(doc, self.START, plan=plan)
        manifest = q.run()
        a, b, c = manifest["arms"]
        self.assertEqual(a["state"], "incomplete")
        self.assertIn("not_run 1", a["reason"])
        self.assertEqual(b["state"], "incomplete")
        self.assertIn("ctx", b["reason"])
        self.assertEqual(c["state"], "complete")
        self.assertEqual(manifest["planned_arms"], 3)
        self.assertEqual(manifest["missing_required"], ["a", "b"])
        self.assertFalse(manifest["study_complete"])
        self.assertTrue((q.dir / "manifest.json").exists())

    def test_window_budget_not_run_is_incomplete_by_window(self):
        plan = {"a": [{"not_run": 1, "window_budget": True}, {}]}
        q = self.make(queue_doc([arm("a")]), at(2026, 10, WED, 20, 0), plan=plan)
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["attempts"][0]["status"], "incomplete_by_window")
        second = self.started_at(q, "a", 2)  # the first window start that holds the estimate
        self.assertEqual(second.date(), dt.date(2026, 10, WED))
        self.assertEqual(manifest["arms"][0]["state"], "complete")

    def test_lock_held_stops_before_launch(self):
        q = self.make(queue_doc([arm("a"), arm("b")]), self.START, lock_free=False)
        manifest = q.run()
        self.assertEqual(manifest["state"], "stopped")
        self.assertIn("lock held", manifest["stop_reason"])
        self.assertEqual(self.launcher.launches, [])

    def test_disk_low_stops(self):
        q = self.make(queue_doc([arm("a")]), self.START, disk_free=5 * 10**8)
        manifest = q.run()
        self.assertIn("disk low", manifest["stop_reason"])
        self.assertEqual(self.launcher.launches, [])

    def test_missing_disk_path_refuses_the_arm(self):
        checks = [{"path": str(self.root / "nope"), "min_free_gb": 1}]
        doc = queue_doc([arm("a")], disk_checks=checks)
        manifest = self.make(doc, self.START).run()
        self.assertEqual(manifest["arms"][0]["state"], "refused")
        self.assertIn("does not exist", manifest["arms"][0]["reason"])
        self.assertEqual(manifest["state"], "done")

    def test_missing_model_root_and_undownloaded_model_refused(self):
        self.env["MIMIK_MODEL_ROOT"] = str(self.root / "absent")
        doc = queue_doc([arm("a"), arm("b", model="granite42-3b")])
        manifest = self.make(doc, self.START).run()
        self.assertIn("model root", manifest["arms"][0]["reason"])
        self.assertIn("not downloaded", manifest["arms"][1]["reason"])
        self.assertEqual(self.launcher.launches, [])


class ReviewFixGateTests(Harness):
    """Review fixes 5, 6 and 8: success needs the runner exit, cleanup and a strict ledger."""

    START = at(2026, 10, SAT, 10, 0)

    def one(self, behaviour, **arm_fields):
        doc = queue_doc([arm("a", **arm_fields)], max_consecutive_failures=5)
        q = self.make(doc, self.START, plan={"a": [behaviour, behaviour]})
        return q.run()["arms"][0]

    def test_runner_exit_gates_success(self):
        for rc in (1, 143):
            with self.subTest(rc=rc):
                result = self.one({"runner_exit": rc})
                self.assertEqual(result["state"], "failed")
                self.assertIn(f"runner exit {rc}", result["reason"])

    def test_runner_cleanup_evidence_required(self):
        result = self.one({"cleanup_unverified": True})
        self.assertEqual(result["state"], "failed")
        self.assertIn("cleanup not verified", result["reason"])

    def test_requested_case_missing_from_a_consistent_ledger(self):
        """Survived the first review's mutation (planned-vs-rows check removed)."""
        result = self.one({"drop_case": True})
        self.assertEqual(result["state"], "incomplete")
        self.assertIn("job set differs", result["reason"])

    def test_duplicate_rows_dry_run_flags_and_harness_refused(self):
        cases = {
            "dup": ({"dup_row": True}, "duplicate"),
            "dry": ({"dry_run": True}, "dry_run"),
            "flags": ({"flags": {"untrusted": True}}, "flags"),
            "harness": ({"harness_sha256": "0" * 64}, "harness_sha256"),
            # Only the harness's own gate (load_ledgers) sees these two:
            "no summary": ({"no_summary": True}, "harness gate"),
            "row arm": ({"row_arm": True}, "harness gate"),
        }
        for name, (behaviour, needle) in cases.items():
            with self.subTest(name=name):
                result = self.one(behaviour)
                self.assertEqual(result["state"], "incomplete")
                self.assertIn(needle, result["reason"])
        result = self.one({}, env={"TEMPERATURE": "0.7"})
        self.assertEqual(result["state"], "complete")  # the fake follows the requested env

    def test_error_episodes_are_reported_distinctly(self):
        result = self.one({"errors": 1})
        self.assertEqual(result["state"], "incomplete_with_errors")
        self.assertIn("error episodes 1", result["reason"])

    def test_main_oom_after_a_good_preflight_is_retried_exactly_once(self):
        doc = queue_doc([arm("a")])
        q = self.make(doc, self.START, plan={"a": [{"kind": "main_oom"}]})
        manifest = q.run()
        attempts = manifest["arms"][0]["attempts"]
        self.assertEqual([x["infra"] for x in attempts], ["oom", "oom"])
        self.assertIn("main_oom_killed", attempts[0]["reason"])
        self.assertEqual(manifest["arms"][0]["state"], "failed")

    def test_infra_vram_retried_once_after_vram_release(self):
        readings = iter([9_000_000_000, 9_000_000_000, 100])
        q = self.make(
            queue_doc([arm("a")]),
            self.START,
            plan={"a": [{"kind": "infra_vram"}, {}]},
            vram_reader=lambda: next(readings, 100),
            cooldown_seconds=47,
        )
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["attempts"][0]["infra"], "infra_vram")
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        self.assertEqual(self.clock.sleeps.count(5), 2)  # waited for the release
        self.assertIn(47, self.clock.sleeps)  # then cooled down


class ReviewFixCrashTests(Harness):
    """Review fix 3: any exit terminates the runner, cleans up and marks the queue crashed."""

    START = at(2026, 10, SAT, 10, 0)

    def manifest(self, q):
        return json.loads((q.dir / "manifest.json").read_text())

    def test_exception_in_classify(self):
        q = self.make(queue_doc([arm("a")]), self.START)

        def boom(*args):
            raise RuntimeError("boom")

        q.classify = boom
        with self.assertRaises(RuntimeError):
            q.run()
        self.assertEqual(self.manifest(q)["state"], "crashed")
        self.assertIn("boom", self.manifest(q)["stop_reason"])
        self.assertGreaterEqual(self.cleanups, 3)  # startup, after the attempt, on the way out

    def test_exception_in_check_ledger(self):
        q = self.make(queue_doc([arm("a")]), self.START)
        original = rq.check_ledger

        def bad(*args):
            raise ValueError("bad ledger")

        rq.check_ledger = bad
        try:
            manifest = q.run()
        finally:
            rq.check_ledger = original
        # Second review round (Sonnet N6): a ledger-parse error fails the arm, not the queue.
        self.assertEqual(manifest["state"], "done")
        self.assertEqual(manifest["arms"][0]["state"], "failed")
        self.assertIn("ledger check error", manifest["arms"][0]["reason"])

    def test_exception_while_the_runner_is_running_terminates_it(self):
        q = self.make(queue_doc([arm("a")]), self.START, plan={"a": [{"seconds": 3600}]})

        def explode(clock):
            if q.handle is not None:
                raise KeyboardInterrupt

        self.clock.on_sleep = explode
        with self.assertRaises(KeyboardInterrupt):
            q.run()
        self.assertIn(signal.SIGTERM, self.launcher.signals)
        self.assertEqual(self.manifest(q)["state"], "crashed")

    def test_exception_in_write_manifest_still_leaves_a_crashed_manifest(self):
        q = self.make(queue_doc([arm("a")]), self.START)
        original = q.write_manifest
        calls = {"n": 0}

        def flaky():
            calls["n"] += 1
            if calls["n"] >= 3:
                raise OSError("disk full")
            original()

        q.write_manifest = flaky
        with self.assertRaises(OSError):
            q.run()
        self.assertEqual(self.manifest(q)["state"], "crashed")
        self.assertGreaterEqual(self.cleanups, 1)

    def test_startup_reconciles_before_waiting_for_a_window(self):
        q = self.make(queue_doc([arm("a")]), at(2026, 10, WED, 10, 0))
        seen = []
        self.clock.on_sleep = lambda clock: seen.append(self.cleanups)
        q.run()
        self.assertGreaterEqual(seen[0], 1)

    def test_unresolved_cleanup_stops_the_queue(self):
        q = self.make(queue_doc([arm("a"), arm("b")]), self.START)
        self.cleanup_ok = False
        manifest = q.run()
        self.assertEqual(manifest["state"], "stopped")
        self.assertIn("cleanup unresolved", manifest["stop_reason"])
        self.assertEqual(self.launcher.launches, [])

    def test_unresolved_cleanup_after_an_attempt_stops_the_queue(self):
        q = self.make(queue_doc([arm("a"), arm("b")]), self.START)

        def cleanup():
            self.cleanups += 1
            ok = self.cleanups == 1  # startup ok, after the first attempt not
            return {"ok": ok, "remaining": [] if ok else ["mimik-toolloop-v3-srv"]}

        q.cleanup = cleanup
        manifest = q.run()
        self.assertEqual(manifest["state"], "stopped")
        self.assertEqual(manifest["arms"][0]["state"], "failed")
        self.assertEqual(manifest["arms"][1]["state"], "pending")

    def test_term_ignored_gets_sigkill(self):
        plan = {"a": [{"seconds": 20 * 3600, "ignore_term": True}]}
        doc = queue_doc([arm("a", max_arm_seconds=40 * 3600)])
        q = self.make(doc, at(2026, 10, WED, 20, 0), plan=plan)
        q.run()
        self.assertEqual(self.launcher.signals[:2], [signal.SIGTERM, signal.SIGKILL])

    def test_stop_margin_below_300_refused(self):
        path = self.root / "margin.json"
        path.write_text(json.dumps(queue_doc([arm("a")], stop_margin_seconds=120)))
        with self.assertRaises(rq.QueueError):
            rq.load_queue(path, REGISTRY)


class ReviewFixScheduleTests(Harness):
    def test_window_refusal_in_a_blocked_period_reruns_tonight(self):
        """F8: a refusal seen at 10:00 must rerun at 18:00 the same day, not the next."""
        plan = {"a": [{"kind": "window_refused", "jump_to": at(2026, 10, WED, 10, 0)}, {}]}
        q = self.make(queue_doc([arm("a")]), at(2026, 10, WED, 8, 0), plan=plan)
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["attempts"][0]["status"], "incomplete_by_window")
        self.assertEqual(self.started_at(q, "a", 2), dt.datetime(2026, 10, WED, 18, 0))

    def test_runner_gets_the_absolute_window_end(self):
        q = self.make(queue_doc([arm("a")]), at(2026, 10, WED, 20, 0))
        q.run()
        _, env = self.launcher.launches[0]
        self.assertEqual(env["WINDOW_END"], str(int(at(2026, 10, THU, 9, 0))))
        self.assertEqual(env["QUEUE_PID"], str(os.getpid()))


class RunnerEnvTests(Harness):
    def test_env_is_an_allowlist(self):
        """Review fix 7: inherited clock hooks, locks, TZ and pass-through values never leak."""
        self.env.update(
            SCHEDULE_OVERRIDE="user-approved",
            CHECK_ONLY="1",
            CTX="1",
            UNTRUSTED="1",
            MIMIK_QUEUE_NOW="1",
            MIMIK_TEST_MODE="1",
            MIMIK_TEST_CLOCK_FILE="/tmp/x",
            LOCK_FILE="/tmp/l",
            TZ="America/New_York",
            HOME="/home/test",
            DOCKER_HOST="tcp://elsewhere",
        )
        arm_env = {"CTX": 24576, "MAX_TURNS": 12}
        doc = queue_doc([arm("a", split="dev", cases_file=str(SUBSET), env=arm_env)])
        manifest = self.make(doc, at(2026, 10, SAT, 10, 0)).run()
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        _, env = self.launcher.launches[0]
        for gone in (
            "SCHEDULE_OVERRIDE",
            "CHECK_ONLY",
            "UNTRUSTED",
            "MIMIK_QUEUE_NOW",
            "MIMIK_TEST_MODE",
            "MIMIK_TEST_CLOCK_FILE",
            "LOCK_FILE",
            "DOCKER_HOST",
        ):
            self.assertNotIn(gone, env)
        self.assertEqual(env["TZ"], ":/etc/localtime")
        self.assertEqual((env["HOME"], env["MIMIK_MODEL_ROOT"]), ("/home/test", str(self.models)))
        self.assertEqual((env["CTX"], env["MAX_TURNS"], env["QUIET"]), ("24576", "12", "1"))
        self.assertEqual(env["PREFLIGHT_CASE"], "dev-001")
        self.assertEqual(env["CASES_FILE"], str(SUBSET))
        self.assertTrue(env["HASH_CACHE"].endswith("hash-cache.txt"))
        recorded = manifest["arms"][0]["attempts"][0]["env"]
        self.assertEqual(recorded, {"CTX": "24576", "MAX_TURNS": "12"})
        for line in self.console:
            self.assertNotIn("dev-0", line)

    def test_queue_file_validation(self):
        bad = {
            "env key": [arm("a", env={"MODEL_PATH": "/x"})],
            "clock key": [arm("a", env={"MIMIK_QUEUE_NOW": "1"})],
            "unsafe value": [arm("a", env={"CTX": "1;rm"})],
            "cases and file": [arm("a", split="dev", cases="dev-001", cases_file=str(SUBSET))],
            "smoke cases": [arm("a", cases="dev-001")],
            "unknown model": [arm("a", model="nope")],
            "duplicate id": [arm("a"), arm("a")],
            "g0 without n_cpu_moe": [arm("a", g0=True)],
        }
        for name, arms in bad.items():
            with self.subTest(name=name):
                path = self.root / f"bad-{name.replace(' ', '-')}.json"
                path.write_text(json.dumps(queue_doc(arms)))
                with self.assertRaises(rq.QueueError):
                    rq.load_queue(path, REGISTRY)

    def test_committed_queue_files_validate(self):
        files = sorted((TOOLS / "queues").glob("*.json"))
        self.assertGreaterEqual(len(files), 2)
        for path in files:
            with self.subTest(path=path.name):
                q = rq.load_queue(path, REGISTRY)
                self.assertGreaterEqual(q["stop_margin_seconds"], 300)
                self.assertEqual(q["g0"]["swap_growth_max_mib"], 0)


class G0Tests(Harness):
    def g0_arms(self, *values):
        return [
            arm(f"g0-{n}", model="qwen36-35b-a3b", g0=True, env={"N_CPU_MOE": n}) for n in values
        ]

    def screen(self, **extra):
        return arm(
            "screen",
            model="qwen36-35b-a3b",
            split="dev",
            cases_file=str(SUBSET),
            n_cpu_moe_from_g0=True,
            seconds_per_episode=60,
            **extra,
        )

    def test_g0_selects_lowest_seconds_per_token_within_limits(self):
        plan = {
            "g0-40": [{"eval_ms": 9000}],
            "g0-36": [{"eval_ms": 7000}],
            "g0-32": [{"eval_ms": 5000, "vram": 7_900_000_000}],
        }
        doc = queue_doc(self.g0_arms(40, 36, 32, 28) + [self.screen()])
        q = self.make(doc, at(2026, 10, SAT, 6, 0), plan=plan)
        manifest = q.run()
        states = {a["id"]: a["state"] for a in manifest["arms"]}
        self.assertEqual(states["g0-28"], "skipped")  # closed after g0-32 exceeded VRAM
        self.assertEqual([r["qualified"] for r in manifest["g0"]], [True, True, False])
        self.assertEqual(manifest["arms"][-1]["selected_n_cpu_moe"], 36)
        self.assertEqual(self.launcher.launches[-1][1]["N_CPU_MOE"], "36")
        self.assertEqual(states["screen"], "complete")
        self.assertTrue(manifest["study_complete"])  # g0 arms are not required

    def test_no_qualified_g0_refuses_the_screen(self):
        doc = queue_doc(self.g0_arms(40) + [self.screen()])
        q = self.make(doc, at(2026, 10, SAT, 6, 0), plan={"g0-40": [{"kind": "oom"}]})
        manifest = q.run()
        self.assertEqual(len(manifest["arms"][0]["attempts"]), 1)  # g0 failures are results
        self.assertEqual(manifest["arms"][1]["state"], "refused")
        self.assertIn("no G0", manifest["arms"][1]["reason"])

    def test_cgroup_swap_with_zero_tolerance(self):
        plan = {"g0-40": [{"eval_ms": 9000}], "g0-36": [{"eval_ms": 7000, "swap_peak": 4096}]}
        doc = queue_doc(self.g0_arms(40, 36, 32) + [self.screen()])
        manifest = self.make(doc, at(2026, 10, SAT, 6, 0), plan=plan).run()
        self.assertEqual([r["qualified"] for r in manifest["g0"]], [True, False])
        self.assertEqual(manifest["g0"][0]["swap_source"], "server cgroup memory.swap.peak")
        self.assertEqual(manifest["arms"][-1]["selected_n_cpu_moe"], 40)

    def test_window_stopped_candidate_is_indeterminate(self):
        plan = {
            "g0-40": [{"eval_ms": 9000}],
            "g0-36": [{"seconds": 60 * 3600}],  # stopped at the Monday 09:00 edge
        }
        g0 = self.g0_arms(40, 36, 32)
        g0[1]["max_arm_seconds"] = 70 * 3600
        doc = queue_doc(g0 + [self.screen()])
        manifest = self.make(doc, at(2026, 10, SAT, 6, 0), plan=plan).run()
        results = {r["arm"]: r for r in manifest["g0"]}
        self.assertEqual(results["g0-36"]["status"], "incomplete_by_window")
        self.assertTrue(results["g0-36"]["indeterminate"])
        self.assertFalse(results["g0-36"]["qualified"])
        self.assertIn("g0-32", results)  # G0 stayed open

    def test_seconds_per_token_fallback_before_qualification(self):
        plan = {"g0-40": [{"wall": 400}]}  # no server.log timings
        doc = queue_doc(self.g0_arms(40) + [self.screen()])
        manifest = self.make(doc, at(2026, 10, SAT, 6, 0), plan=plan).run()
        result = manifest["g0"][0]
        self.assertTrue(result["qualified"])
        self.assertEqual(result["seconds_per_token_source"], "fallback: seconds per episode")

    def test_g0_deadline_bounds_the_active_attempt(self):
        plan = {"g0-40": [{"eval_ms": 9000}], "g0-36": [{"seconds": 6 * 3600}]}
        g0 = self.g0_arms(40, 36)
        g0[1]["max_arm_seconds"] = 10 * 3600
        doc = queue_doc(g0 + [self.screen()], g0={"deadline_seconds": 3600})
        q = self.make(doc, at(2026, 10, SAT, 6, 0), plan=plan)
        manifest = q.run()
        attempt = manifest["arms"][1]["attempts"][0]
        self.assertEqual(attempt["status"], "arm_timeout")
        self.assertIn("g0_deadline", attempt["reason"])


class ReviewTwoQueueTests(Harness):
    """Second review round: watchdog readiness, CHECK_ONLY pre-pass, G0 recheck, swap,
    fit margin, weekend edge, console sanitising."""

    START = at(2026, 10, SAT, 10, 0)

    def test_no_heavy_work_without_a_ready_watchdog(self):
        dog = FakeWatchdog(alive=False, restartable=False)
        q = self.make(queue_doc([arm("a")]), self.START, watchdog=dog)
        manifest = q.run()
        self.assertEqual(manifest["state"], "stopped")
        self.assertIn("watchdog unavailable", manifest["stop_reason"])
        self.assertEqual(self.launcher.launches, [])
        self.assertEqual(self.launcher.checks, [])

    def test_watchdog_lost_mid_attempt_stops_the_arm(self):
        dog = FakeWatchdog()
        q = self.make(
            queue_doc([arm("a"), arm("b")]),
            self.START,
            watchdog=dog,
            plan={"a": [{"seconds": 3600}]},
        )

        def die(clock):
            if q.handle is not None and self.launcher.launches:
                dog.is_alive, dog.restartable = False, False

        self.clock.on_sleep = die
        manifest = q.run()
        self.assertIn(signal.SIGTERM, self.launcher.signals)
        self.assertEqual(manifest["arms"][0]["state"], "failed")
        self.assertEqual(manifest["state"], "stopped")
        self.assertEqual(manifest["arms"][1]["state"], "pending")

    def test_watchdog_restarted_mid_attempt(self):
        dog = FakeWatchdog()
        doc = queue_doc([arm("a", max_arm_seconds=7200)])
        q = self.make(doc, self.START, watchdog=dog, plan={"a": [{"seconds": 1200}]})
        killed = []

        def die(clock):
            if q.handle is not None and self.launcher.launches and not killed:
                dog.is_alive = False  # restartable
                killed.append(1)

        self.clock.on_sleep = die
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        self.assertGreaterEqual(dog.ensures, 2)

    def test_check_only_pre_pass_before_each_arm(self):
        q = self.make(queue_doc([arm("a"), arm("b")]), self.START, plan={"b:check_fail": True})
        manifest = q.run()
        self.assertEqual(self.launcher.order[:3], [("check", "a"), ("run", "a"), ("check", "b")])
        self.assertEqual(manifest["arms"][1]["state"], "refused")
        self.assertIn("CHECK_ONLY pre-pass", manifest["arms"][1]["reason"])
        self.assertEqual([a for a, _ in self.launcher.launches], ["a"])
        check_dir = q.dir / "arms" / "a" / "check-1"
        self.assertIn("check_only=1", (check_dir / "provenance.txt").read_text())

    def test_g0_deadline_rechecked_after_the_window_wait(self):
        g0 = [
            arm(f"g0-{n}", model="qwen36-35b-a3b", g0=True, env={"N_CPU_MOE": n}) for n in (40, 36)
        ]
        # About 13 h: it does not fit the rest of Sunday night (to Monday 09:00), so it waits
        # until Monday 18:00, far past the 4 h G0 deadline counted from g0-40's launch.
        g0[1]["seconds_per_episode"] = (13 * 3600 - 180 - 2 * 20419565568 / 12.6e6) / 5
        doc = queue_doc(g0, g0={"deadline_seconds": 4 * 3600})
        q = self.make(doc, at(2026, 10, SUN, 21, 0), plan={"g0-40": [{"eval_ms": 9000}]})
        manifest = q.run()
        states = {a["id"]: (a["state"], a.get("reason")) for a in manifest["arms"]}
        self.assertEqual(states["g0-40"][0], "complete")
        self.assertEqual(states["g0-36"], ("skipped", "G0 deadline passed"))
        self.assertEqual([a for a, _ in self.launcher.launches], ["g0-40"])

    def g0_with_screen(self, plan):
        g0 = [
            arm(f"g0-{n}", model="qwen36-35b-a3b", g0=True, env={"N_CPU_MOE": n})
            for n in (40, 36, 32)
        ]
        screen = arm(
            "screen",
            model="qwen36-35b-a3b",
            split="dev",
            cases_file=str(SUBSET),
            n_cpu_moe_from_g0=True,
            seconds_per_episode=60,
        )
        q = self.make(queue_doc(g0 + [screen]), at(2026, 10, SAT, 6, 0), plan=plan)
        return q, q.run()

    def test_unreadable_cgroup_swap_is_indeterminate_and_never_closes(self):
        plan = {
            "g0-40": [{"eval_ms": 9000}],
            "g0-36": [{"eval_ms": 7000, "swap_peak": "n/a", "swap_after": 999}],
        }
        q, manifest = self.g0_with_screen(plan)
        results = {r["arm"]: r for r in manifest["g0"]}
        self.assertTrue(results["g0-36"]["indeterminate"])
        self.assertFalse(results["g0-36"]["qualified"])
        self.assertTrue(results["g0-36"]["swap_source"].startswith("host-wide"))
        self.assertIn("g0-32", results)  # G0 stayed open
        self.assertIn("g0_swap_fallback", (q.dir / "queue.log").read_text())

    def test_fit_uses_at_least_900_s(self):
        # 3000 s estimate at Wednesday 08:00: fits a 300 s margin (08:50 <= 08:55) but not
        # the runner's 900 s gate (08:50 > 08:45), so it waits for 18:00.
        doc = queue_doc([arm("a", seconds_per_episode=(3000 - 180 - 2 * 2497280736 / 12.6e6) / 5)])
        q = self.make(doc, at(2026, 10, WED, 8, 0))
        q.run()
        self.assertEqual(self.started_at(q, "a"), dt.datetime(2026, 10, WED, 18, 0))

    def test_saturday_stop_reruns_the_same_weekend(self):
        """Space Bunny N6: a stop on Saturday must not push the rerun past the weekend."""
        plan = {"a": [{"kind": "window_refused"}, {}]}
        q = self.make(queue_doc([arm("a")]), at(2026, 10, SAT, 12, 0), plan=plan)
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["attempts"][0]["status"], "incomplete_by_window")
        self.assertEqual(self.started_at(q, "a", 2).date(), dt.date(2026, 10, SAT))
        self.assertEqual(manifest["arms"][0]["state"], "complete")

    def test_console_never_prints_a_case_id(self):
        q = self.make(queue_doc([arm("a")]), self.START, plan={"a": [{"kind": "harness_fail"}]})
        q.dir.mkdir(parents=True)
        q.transition(q.arms[0], "failed", reason="1-case preflight (dev-001) and ho-129 failed")
        q.stop("stopped near dev-005")
        for line in self.console:
            self.assertNotRegex(line, r"(dev|ho)-\d{3}")
        self.assertIn("<case>", self.console[-2])

    def test_endpoint_override_refuses_the_queue(self):
        proc = subprocess.run(
            [
                sys.executable,
                str(TOOLS / "run_queue.py"),
                "check",
                str(TOOLS / "queues" / "screen-small-s40.json"),
            ],
            env={"PATH": os.environ.get("PATH", ""), "DOCKER_CONTEXT": "remote"},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("DOCKER_CONTEXT", proc.stderr)


class SupervisorTests(unittest.TestCase):
    def test_heartbeat_freshness_and_pid(self):
        with tempfile.TemporaryDirectory() as tmp:
            beat = Path(tmp) / "hb"
            sup = rq.WatchdogSupervisor(beat, Path(tmp) / "w.log", fresh_seconds=90)
            self.assertFalse(sup.alive())
            beat.write_text(json.dumps({"pid": os.getpid(), "time": time.time()}))
            self.assertTrue(sup.alive())
            beat.write_text(json.dumps({"pid": os.getpid(), "time": time.time() - 200}))
            self.assertFalse(sup.alive())
            dead = subprocess.Popen(["true"])
            dead.wait()
            beat.write_text(json.dumps({"pid": dead.pid, "time": time.time()}))
            self.assertFalse(sup.alive())

    def test_default_is_no_watchdog(self):
        self.assertFalse(rq.NoWatchdog().ensure())


class ProcessLauncherTests(unittest.TestCase):
    """The real launcher: own process group, stdin /dev/null, output in files."""

    def test_fake_runner_script(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "tool_loop_benchmark.py").write_text("")
            runner = tmp / "runner.sh"
            runner.write_text(
                textwrap.dedent(
                    """\
                    #!/bin/sh
                    if [ "$CHECK_ONLY" = 1 ]; then
                      echo "check_only=1" > "$OUT_DIR/provenance.txt"; exit 0
                    fi
                    if read -r line; then echo "stdin had input" > "$OUT_DIR/stdin.txt";
                    else echo eof > "$OUT_DIR/stdin.txt"; fi
                    echo "case dev-001 outcome=answer" >&2
                    echo "to stdout"
                    printf 'main_exit=0 main_wall_seconds=1\\nledger_file=l.jsonl\\n' \\
                      > "$OUT_DIR/provenance.txt"
                    echo cleanup_verified=true >> "$OUT_DIR/provenance.txt"
                    exit 0
                    """
                )
            )
            runner.chmod(runner.stat().st_mode | stat.S_IEXEC)
            models = tmp / "models"
            entry = REGISTRY["models"]["qwen3-4b-2507"]
            (models / entry["path"]).parent.mkdir(parents=True)
            (models / entry["path"]).write_bytes(b"")
            path = tmp / "q.json"
            path.write_text(json.dumps(queue_doc([arm("a")])))
            console = []
            q = rq.Queue(
                rq.load_queue(path, REGISTRY),
                REGISTRY,
                tmp / "session",
                clock=FakeWallClock(at(2026, 10, SAT, 10, 0)),
                lock_probe=lambda: True,
                cleanup=lambda: {"ok": True},
                runner=runner,
                poll_seconds=0.05,
                watchdog=FakeWatchdog(),
                console=console.append,
                env={"MIMIK_MODEL_ROOT": str(models), "PATH": os.environ.get("PATH", "")},
            )
            manifest = q.run()
            attempt = Path(manifest["arms"][0]["attempts"][0]["dir"])
            self.assertEqual((attempt / "stdin.txt").read_text().strip(), "eof")
            self.assertIn("outcome=answer", (attempt / "runner.stderr").read_text())
            self.assertEqual((attempt / "runner.stdout").read_text(), "to stdout\n")
            self.assertFalse(any("outcome" in line for line in console))
            # No ledger was written, so the arm is not complete even though the exit was 0.
            self.assertEqual(manifest["arms"][0]["state"], "incomplete")
            self.assertIn("unreadable ledger", manifest["arms"][0]["reason"])


class FakeWallClock(FakeClock):
    """Fake calendar time, real (short) sleeps so a child process can finish."""

    def sleep(self, seconds):
        time.sleep(min(seconds, 0.05))
        self.now += seconds


class EvidenceTests(unittest.TestCase):
    def test_server_seconds_per_token_ignores_prompt_eval(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "server.log"
            log.write_text(
                "prompt eval time =  5000.00 ms /  1000 tokens\n"
                "       eval time =  2000.00 ms /   100 tokens\n"
                "       eval time =  1000.00 ms /    50 tokens\n"
            )
            self.assertAlmostEqual(rq.server_seconds_per_token(log), 0.02)
            self.assertIsNone(rq.server_seconds_per_token(Path(tmp) / "missing.log"))

    def test_parse_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            prov = Path(tmp) / "provenance.txt"
            prov.write_text(
                "main_exit=0 main_wall_seconds=12\n"
                "failure_detail=server not healthy after 900s\n"
                "git_dirty_paths=0 harness_file_git_status='clean'\n"
            )
            values = rq.parse_provenance(prov)
        self.assertEqual(values["main_exit"], "0")
        self.assertEqual(values["failure_detail"], "server not healthy after 900s")
        self.assertEqual(values["harness_file_git_status"], "clean")


if __name__ == "__main__":
    unittest.main()
