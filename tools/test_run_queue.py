"""Unattended queue (tools/run_queue.py): fake runner, fake clock (plan §11 item 26).

Failing-first: run before tools/run_queue.py existed (ModuleNotFoundError); the mutation
checks are in docs/tool-loop-benchmark.md ("Run infrastructure").
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
import unittest
from pathlib import Path

import model_registry
import run_queue as rq

TOOLS = Path(__file__).resolve().parent
REGISTRY = model_registry.load()
SUBSET = TOOLS / "case-subsets" / "dev-screen-40.json"


def at(*args) -> float:
    """Epoch of a naive local time."""
    return dt.datetime(*args).timestamp()


# 2026-10-14 is a Wednesday; 2026-10-16 a Friday; 2026-10-17 a Saturday.
WED, THU, FRI, SAT, SUN, MON = 14, 15, 16, 17, 18, 19


class FakeClock:
    def __init__(self, start):
        self.now = start
        self.sleeps = []

    def time(self):
        return self.now

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds


class FakeHandle:
    def __init__(self, launcher, behaviour, env, argv):
        self.launcher, self.behaviour, self.env, self.argv = launcher, behaviour, env, argv
        self.start = launcher.clock.now
        self.rc = None
        self.out = Path(env["OUT_DIR"])

    def poll(self):
        if self.rc is None and self.launcher.clock.now >= self.start + self.behaviour["seconds"]:
            self.rc = write_attempt(self.out, self.behaviour, self.env, self.argv)
        return self.rc

    def signal(self, sig):
        self.launcher.signals.append(sig)
        if self.rc is None:
            prov = self.out / "provenance.txt"
            with open(prov, "a", encoding="utf-8") as handle:
                handle.write("date_start=x\nstopped_by_signal=TERM\n")
            self.rc = 143

    def wait(self, timeout):
        return self.rc


class FakeLauncher:
    """Plays a behaviour per launch: {"seconds", "kind", ...}; default a complete run."""

    def __init__(self, clock, plan=None):
        self.clock = clock
        self.plan = plan or {}
        self.launches = []
        self.signals = []

    def start(self, argv, env, stdout_path, stderr_path):
        arm = Path(env["OUT_DIR"]).parent.name
        n = sum(1 for a, _ in self.launches if a == arm)
        self.launches.append((arm, dict(env)))
        stdout_path.write_text("runner stdout\n")
        stderr_path.write_text("")
        behaviours = self.plan.get(arm, [{}])
        behaviour = dict(
            {"seconds": 600, "kind": "complete"}, **behaviours[min(n, len(behaviours) - 1)]
        )
        return FakeHandle(self, behaviour, env, argv)


def write_attempt(out: Path, b: dict, env: dict, argv) -> int:
    """What the runner leaves behind for a given outcome kind."""
    key, split = argv[1], argv[2]
    entry = REGISTRY["models"][key]
    prov = ["date_start=x", f"runner=fake key={key}"]
    kind = b["kind"]
    if kind in ("server_health", "server_start"):
        prov += [f"failure={kind}", "failure_detail=server not healthy after 900s"]
        (out / "provenance.txt").write_text("\n".join(prov) + "\n")
        return 4
    if kind == "oom":
        prov += ["server_oom_killed=true", "failure=server_health", "failure_detail=died"]
        (out / "provenance.txt").write_text("\n".join(prov) + "\n")
        return 4
    if kind == "no_provenance":
        return 9
    ctx = int(b.get("ctx", env.get("CTX", entry["server"]["ctx"])))
    seeds = [int(s) for s in env["SEEDS"].split(",")]
    if env.get("CASES_FILE"):
        doc = json.loads(Path(env["CASES_FILE"]).read_text())
        cases, subset = doc["case_ids"], {"sha256": doc["sha256"]}
    else:
        cases, subset = (
            (["dev-001", "dev-005", "dev-008"] if split == "smoke" else ["c1", "c2"]),
            None,
        )
    header = {
        "event": "run_started",
        "arm": env["ARM"],
        "model_label": entry["label"],
        "model_sha256": entry["sha256"],
        "ctx": ctx,
        "split": "dev" if split == "smoke" else split,
        "seeds": seeds,
        "planned_episodes": len(cases) * len(seeds),
        "case_order": cases,
        "case_subset": subset,
    }
    rows = [header]
    not_run = b.get("not_run", 0)
    for n, (seed, case) in enumerate((s, c) for s in seeds for c in cases):
        outcome = "not_run" if n >= len(cases) * len(seeds) - not_run else "answer"
        rows.append({"event": "episode", "case_id": case, "seed": seed, "outcome": outcome})
    if kind == "aborted_health":
        rows.append(
            {"event": "run_aborted", "reason": "server health check failed after an error episode"}
        )
    else:
        rows.append({"event": "run_summary", "summaries": []})
    (out / "ledger-1.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    harness_exit = {"complete": 0, "harness_fail": 4, "aborted_health": 4}[kind]
    prov += [
        "ledger_file=ledger-1.jsonl",
        f"vram_used_before=500 swap_used_mib_before={b.get('swap_before', 100)}",
        f"server_ready_seconds=4 vram_used_loaded={b.get('vram', 6_000_000_000)}",
        f"harness_exit={harness_exit} harness_wall_seconds={b.get('wall', 100)}",
        f"server_oom_killed=false server_memory_peak=1 vram_used_end={b.get('vram', 6_000_000_000)} "
        f"swap_used_mib_after={b.get('swap_after', 100)}",
    ]
    if b.get("window_budget"):
        prov.append("budget_from_window=1000")
    (out / "provenance.txt").write_text("\n".join(prov) + "\n")
    if "eval_ms" in b:
        (out / "server.log").write_text(
            f"prompt eval time =    999.00 ms /   100 tokens\n"
            f"       eval time = {b['eval_ms']:9.2f} ms /   100 tokens (x ms per token)\n"
        )
    return harness_exit


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

    def make(self, doc, start, plan=None, lock_free=True, disk_free=10**12, **kw):
        path = self.root / f"queue-{len(list(self.root.iterdir()))}.json"
        path.write_text(json.dumps(doc))
        self.clock = FakeClock(start)
        self.launcher = FakeLauncher(self.clock, plan)

        def cleanup():
            self.cleanups += 1

        return rq.Queue(
            rq.load_queue(path, REGISTRY),
            REGISTRY,
            self.root / f"session-{len(list(self.root.iterdir()))}",
            clock=self.clock,
            launcher=self.launcher,
            disk_free=lambda p: disk_free,
            lock_probe=lambda: lock_free,
            cleanup=cleanup,
            case_count=lambda split: 174,
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

    def test_window_check_cli(self):
        script = [sys.executable, str(TOOLS / "run_queue.py"), "window-check"]
        for when, code, word in (
            (at(2026, 10, WED, 10, 0), 1, "closed"),
            (at(2026, 10, WED, 20, 0), 0, "open"),
        ):
            env = dict(os.environ, **{rq.NOW_ENV: str(when)})
            proc = subprocess.run(script, env=env, capture_output=True, text=True, check=False)
            self.assertEqual(proc.returncode, code)
            self.assertIn(f"schedule_window={word}", proc.stdout)
        env = dict(os.environ, **{rq.NOW_ENV: str(at(2026, 10, WED, 20, 0))})
        proc = subprocess.run(script, env=env, capture_output=True, text=True, check=False)
        self.assertIn(f"seconds_left={13 * 3600}", proc.stdout)


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
        self.assertLessEqual(ended, dt.datetime(2026, 10, THU, 9, 0))
        self.assertGreaterEqual(ended, dt.datetime(2026, 10, THU, 8, 50))
        self.assertIn(signal.SIGTERM, self.launcher.signals)
        self.assertGreaterEqual(self.cleanups, 1)
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
        doc = queue_doc([arm("a"), arm("b"), arm("c", required=False)], max_consecutive_failures=3)
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
        self.assertEqual(self.started_at(q, "a", 2), dt.datetime(2026, 10, THU, 18, 0))
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
        doc = queue_doc(
            [arm("a")], disk_checks=[{"path": str(self.root / "nope"), "min_free_gb": 1}]
        )
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


class RunnerEnvTests(Harness):
    def test_env_is_controlled_and_quiet(self):
        self.env.update(SCHEDULE_OVERRIDE="user-approved", CHECK_ONLY="1", CTX="1", UNTRUSTED="1")
        doc = queue_doc(
            [arm("a", split="dev", cases_file=str(SUBSET), env={"CTX": 24576, "MAX_TURNS": 12})]
        )
        manifest = self.make(doc, at(2026, 10, SAT, 10, 0)).run()
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        _, env = self.launcher.launches[0]
        for gone in ("SCHEDULE_OVERRIDE", "CHECK_ONLY", "UNTRUSTED"):
            self.assertNotIn(gone, env)
        self.assertEqual((env["CTX"], env["MAX_TURNS"], env["QUIET"]), ("24576", "12", "1"))
        self.assertEqual(env["PREFLIGHT_CASE"], "dev-001")
        self.assertEqual(env["CASES_FILE"], str(SUBSET))
        self.assertTrue(env["HASH_CACHE"].endswith("hash-cache.txt"))
        for line in self.console:
            self.assertNotIn("dev-0", line)

    def test_queue_file_validation(self):
        bad = {
            "env key": [arm("a", env={"MODEL_PATH": "/x"})],
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
                rq.load_queue(path, REGISTRY)


class G0Tests(Harness):
    def test_g0_selects_lowest_seconds_per_token_within_limits(self):
        g0 = [
            arm(f"g0-{n}", model="qwen36-35b-a3b", g0=True, env={"N_CPU_MOE": n})
            for n in (40, 36, 32, 28)
        ]
        screen = arm(
            "screen",
            model="qwen36-35b-a3b",
            split="dev",
            cases_file=str(SUBSET),
            n_cpu_moe_from_g0=True,
            seconds_per_episode=60,
        )
        plan = {
            "g0-40": [{"eval_ms": 9000}],
            "g0-36": [{"eval_ms": 7000}],
            "g0-32": [{"eval_ms": 5000, "vram": 7_900_000_000}],
        }
        q = self.make(queue_doc(g0 + [screen]), at(2026, 10, SAT, 6, 0), plan=plan)
        manifest = q.run()
        states = {a["id"]: a["state"] for a in manifest["arms"]}
        self.assertEqual(states["g0-28"], "skipped")  # closed after g0-32 exceeded VRAM
        self.assertEqual([r["qualified"] for r in manifest["g0"]], [True, True, False])
        self.assertEqual(manifest["arms"][-1]["selected_n_cpu_moe"], 36)
        self.assertEqual(self.launcher.launches[-1][1]["N_CPU_MOE"], "36")
        self.assertEqual(states["screen"], "complete")
        self.assertTrue(manifest["study_complete"])  # g0 arms are not required

    def test_no_qualified_g0_refuses_the_screen(self):
        g0 = [arm("g0-40", model="qwen36-35b-a3b", g0=True, env={"N_CPU_MOE": 40})]
        screen = arm("screen", model="qwen36-35b-a3b", n_cpu_moe_from_g0=True)
        q = self.make(
            queue_doc(g0 + [screen]), at(2026, 10, SAT, 6, 0), plan={"g0-40": [{"kind": "oom"}]}
        )
        manifest = q.run()
        self.assertEqual(len(manifest["arms"][0]["attempts"]), 1)  # g0 failures are results
        self.assertEqual(manifest["arms"][1]["state"], "refused")
        self.assertIn("no G0", manifest["arms"][1]["reason"])


class ProcessLauncherTests(unittest.TestCase):
    """The real launcher: own process group, stdin /dev/null, output in files."""

    def test_fake_runner_script(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            runner = tmp / "runner.sh"
            runner.write_text(
                textwrap.dedent(
                    """\
                    #!/bin/sh
                    if read -r line; then echo "stdin had input" > "$OUT_DIR/stdin.txt";
                    else echo eof > "$OUT_DIR/stdin.txt"; fi
                    echo "case dev-001 outcome=answer" >&2
                    echo "to stdout"
                    printf 'harness_exit=0 harness_wall_seconds=1\\nledger_file=l.jsonl\\n' > "$OUT_DIR/provenance.txt"
                    exit 0
                    """
                )
            )
            runner.chmod(runner.stat().st_mode | stat.S_IEXEC)
            models = tmp / "models"
            entry = REGISTRY["models"]["qwen3-4b-2507"]
            (models / entry["path"]).parent.mkdir(parents=True)
            (models / entry["path"]).write_bytes(b"")
            doc = queue_doc([arm("a")])
            path = tmp / "q.json"
            path.write_text(json.dumps(doc))
            console = []
            q = rq.Queue(
                rq.load_queue(path, REGISTRY),
                REGISTRY,
                tmp / "session",
                clock=FakeWallClock(at(2026, 10, SAT, 10, 0)),
                lock_probe=lambda: True,
                cleanup=lambda: None,
                runner=runner,
                poll_seconds=0.05,
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
        import time

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
                "harness_exit=0 harness_wall_seconds=12\n"
                "failure_detail=server not healthy after 900s\n"
                "git_dirty_paths=0 harness_file_git_status='clean'\n"
            )
            values = rq.parse_provenance(prov)
        self.assertEqual(values["harness_exit"], "0")
        self.assertEqual(values["failure_detail"], "server not healthy after 900s")
        self.assertEqual(values["harness_file_git_status"], "clean")


if __name__ == "__main__":
    unittest.main()
