"""Queue arm kind `router` (decision-router plan §5, rev. 3.1): fake runner, fake clock.

Failing-first: before this change load_queue refused `kind`/`router`/`unit` (unknown keys) and
the watchdog's fallback names did not include the router container.
"""

import json
import subprocess
import unittest
from pathlib import Path

import decision_models
import decision_router as dr
import run_queue as rq
import toolloop_ops as ops
from test_run_queue import (
    REGISTRY,
    SAT,
    FakeCheckHandle,
    FakeClock,
    FakeWatchdog,
    Harness,
    at,
    cool_gpu,
    manual_fan,
    queue_doc,
)

TOOLS = Path(__file__).resolve().parent
DECISION = decision_models.load()


def router_arm(id_, router="r4-kev-0.8b", unit="probe", **fields):
    return dict(
        {"id": id_, "kind": "router", "router": router, "unit": unit, "arm": f"SYS-{id_}"}, **fields
    )


def probs():
    p = {k: 0.0 for k in dr.CANONICAL}
    p["ask_rule"] = 1.0
    return p


def write_unit(out: Path, unit: str, behaviour: dict, key: str):
    u = out / "unit"
    u.mkdir(parents=True, exist_ok=True)
    n = {"probe": 3, **dr.UNIT_DECISIONS}[unit]
    decisions = [{"key": f"k{i:03d}", "variant": "full", "order": "canonical"} for i in range(n)]
    rows = []
    for i, d in enumerate(decisions):
        status = "completed"
        if i < behaviour.get("not_run", 0):
            status = "not_run"
        elif i < behaviour.get("failed", 0):
            status = "timeout"
        rows.append(
            {
                "event": "decision",
                **d,
                "status": status,
                "probabilities": probs() if status == "completed" else None,
                "error_class": None if status == "completed" else status,
                "seconds": behaviour.get("seconds_per_decision", 50.0),
            }
        )
    (u / "plan.json").write_text(json.dumps({"unit": unit, "decisions": decisions}))
    (u / "decisions.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    statuses = {}
    for r in rows:
        statuses[r["status"]] = statuses.get(r["status"], 0) + 1
    summary = {
        "unit": unit,
        "planned": n,
        "complete": not behaviour.get("not_run"),
        "not_run": behaviour.get("not_run", 0),
        "statuses": statuses,
        "decision_seconds": {"max": behaviour.get("seconds_per_decision", 50.0)},
        "disk_rate": 20_000_000,
        "router_key": key,
        "stopped": behaviour.get("stopped"),
    }
    (u / "summary.json").write_text(json.dumps(summary))
    main_exit = 0 if summary["complete"] else 1
    prov = [
        "date_start=x",
        f"runner=tools/run_decision_router.sh key={key} unit={unit}",
        f"main_exit={main_exit} main_oom_killed=false main_wall_seconds=100",
        "ledger_file=unit/decisions.jsonl summary_file=unit/summary.json",
        "router_oom_killed=false",
        "cleanup_verified=true",
    ]
    if behaviour.get("failure"):
        prov = prov[:2] + [
            f"failure={behaviour['failure']}",
            "failure_detail=x",
            "cleanup_verified=true",
        ]
        main_exit = {"window": 10, "ineligible": 3}.get(behaviour["failure"], 12)
    (out / "provenance.txt").write_text("\n".join(prov) + "\n")
    return main_exit


class RouterHandle:
    def __init__(self, launcher, behaviour, env, argv):
        self.launcher, self.behaviour, self.env, self.argv = launcher, behaviour, env, argv
        self.start, self.rc = launcher.clock.now, None
        self.out = Path(env["OUT_DIR"])

    def poll(self):
        if self.rc is None and self.launcher.clock.now >= self.start + self.behaviour.get(
            "seconds", 600
        ):
            self.rc = write_unit(self.out, self.argv[2], self.behaviour, self.argv[1])
        return self.rc

    def signal(self, sig):
        if self.rc is None:
            (self.out / "provenance.txt").write_text(
                "stopped_by_signal=TERM\ncleanup_verified=true\n"
            )
            self.rc = 143

    def wait(self, timeout):
        return self.rc


class RouterLauncher:
    def __init__(self, clock, plan=None):
        self.clock, self.plan = clock, plan or {}
        self.launches, self.checks = [], []

    def start(self, argv, env, stdout_path, stderr_path):
        arm = Path(env["OUT_DIR"]).parent.name
        stdout_path.write_text("")
        if env.get("CHECK_ONLY") == "1":
            self.checks.append(arm)
            return FakeCheckHandle(self.clock, env, self.plan.get(f"{arm}:check_fail"))
        n = sum(1 for a, _, _ in self.launches if a == arm)
        self.launches.append((arm, list(argv), dict(env)))
        behaviours = self.plan.get(arm, [{}])
        return RouterHandle(self, dict(behaviours[min(n, len(behaviours) - 1)]), env, argv)


class RouterQueueTests(Harness):
    START = at(2026, 10, SAT, 4, 30)

    def setUp(self):
        super().setUp()
        for entry in DECISION["routers"].values():
            for item in entry["files"]:
                path = self.models / item["path"]
                path.parent.mkdir(parents=True, exist_ok=True)
                with open(path, "wb") as handle:
                    handle.truncate(item["size"])

    def make_router(self, arms, plan=None, **extra):
        path = self.root / f"queue-{len(list(self.root.iterdir()))}.json"
        path.write_text(json.dumps(queue_doc(arms, **extra)))
        self.clock = FakeClock(self.START)
        self.launcher = RouterLauncher(self.clock, plan)
        return rq.Queue(
            rq.load_queue(path, REGISTRY),
            REGISTRY,
            self.root / f"session-{len(list(self.root.iterdir()))}",
            clock=self.clock,
            launcher=self.launcher,
            disk_free=lambda p: 10**12,
            lock_probe=lambda: True,
            cleanup=lambda: {"ok": True, "remaining": []},
            console=self.console.append,
            env=self.env,
            vram_reader=lambda: None,
            watchdog=FakeWatchdog(),
            thermal_reader=lambda: cool_gpu(),
            fan_reader=lambda: manual_fan(enable=2, pwm=56),
        )

    def test_router_arm_validation(self):
        path = self.root / "q.json"
        for bad in (
            router_arm("a", router="nope"),
            router_arm("a", unit="all"),
            router_arm("a", device="gpu"),
            router_arm("a", cases_file="x"),
            router_arm("a", model="qwen3-4b-2507"),
            router_arm("a", env={"CTX": "1"}),
            router_arm("a", g0=True),
            router_arm("a", arm="R4-probe"),
        ):
            path.write_text(json.dumps(queue_doc([bad])))
            with self.assertRaises(rq.QueueError, msg=bad):
                rq.load_queue(path, REGISTRY)
        path.write_text(json.dumps(queue_doc([router_arm("a")])))
        q = rq.load_queue(path, REGISTRY)
        self.assertEqual((q["arms"][0]["kind"], q["arms"][0]["device"]), ("router", "cpu"))

    def test_probe_then_screen_with_the_probe_summary(self):
        q = self.make_router([router_arm("p"), router_arm("s", unit="screen")])
        manifest = q.run()
        self.assertEqual(
            [a["state"] for a in manifest["arms"]], ["complete", "complete"], manifest["arms"]
        )
        (_, argv_p, env_p), (_, argv_s, env_s) = self.launcher.launches
        self.assertEqual(Path(argv_p[0]).name, "run_decision_router.sh")
        self.assertEqual(argv_p[1:], ["r4-kev-0.8b", "probe"])
        self.assertNotIn("PROBE_SUMMARY", env_p)
        self.assertEqual(
            env_s["PROBE_SUMMARY"], str(Path(env_p["OUT_DIR"]) / "unit" / "summary.json")
        )
        for env in (env_p, env_s):
            self.assertNotIn("ARM", env)
            self.assertIn("WINDOW_END", env)
            self.assertIn("QUEUE_PID", env)
        self.assertEqual(self.launcher.checks, ["p", "s"])

    def test_screen_without_a_probe_is_refused_as_ineligible(self):
        q = self.make_router([router_arm("s", unit="screen")])
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "refused")
        self.assertIn("ineligible", manifest["arms"][0]["reason"])
        self.assertEqual(self.launcher.launches, [])

    def test_a_probe_with_a_failed_decision_makes_the_router_ineligible(self):
        q = self.make_router(
            [router_arm("p"), router_arm("s", unit="screen")], plan={"p": [{"failed": 1}]}
        )
        manifest = q.run()
        self.assertEqual(manifest["arms"][0]["state"], "complete")
        self.assertEqual(manifest["arms"][1]["state"], "refused")
        self.assertEqual(len(self.launcher.launches), 1)

    def test_estimate_is_the_projection_from_the_probe(self):
        q = self.make_router(
            [router_arm("p"), router_arm("s", unit="screen")],
            plan={"p": [{"seconds_per_decision": 40.0}]},
        )
        q.run()
        screen = q.arms[1]
        total = sum(f["size"] for f in DECISION["routers"]["r4-kev-0.8b"]["files"])
        expected = dr.unit_timeouts(
            "screen", total, json.loads(Path(q.router_probes["r4-kev-0.8b"]).read_text())
        )
        self.assertAlmostEqual(q.estimate(screen), expected["projection_seconds"])
        probe_estimate = dr.unit_timeouts("probe", total)["projection_seconds"]
        self.assertAlmostEqual(q.estimate(q.arms[0]), probe_estimate)

    def test_incomplete_unit_is_never_complete(self):
        q = self.make_router([router_arm("p")], plan={"p": [{"not_run": 2}]})
        manifest = q.run()
        self.assertNotEqual(manifest["arms"][0]["state"], "complete")
        attempt = manifest["arms"][0]["attempts"][0]
        self.assertFalse(attempt["unit_check"]["complete"])

    def test_deadline_stop_is_incomplete_by_window(self):
        q = self.make_router(
            [router_arm("p")],
            plan={
                "p": [
                    {
                        "not_run": 2,
                        "stopped": "deadline: the decision timeout would cross the deadline",
                    }
                ]
            },
        )
        q.run()
        self.assertEqual(q.arms[0]["attempts"][0]["status"], "incomplete_by_window")

    def test_projection_cap_skips_a_router_whose_two_units_exceed_it(self):
        arms = [
            router_arm("p", router="r3-qwen35-9b"),
            router_arm(
                "s",
                router="r3-qwen35-9b",
                unit="screen",
                max_projection_seconds=12 * 3600,
                end_margin_seconds=3600,
            ),
        ]
        q = self.make_router(arms, plan={"p": [{"seconds_per_decision": 300.0}]})
        manifest = q.run()
        self.assertEqual(manifest["arms"][1]["state"], "skipped")
        self.assertIn("projection", manifest["arms"][1]["reason"])
        q2 = self.make_router(arms, plan={"p": [{"seconds_per_decision": 20.0}]})
        manifest = q2.run()
        self.assertEqual(manifest["arms"][1]["state"], "complete", manifest["arms"][1])

    def test_cpu_temperatures_recorded_for_router_attempts(self):
        q = self.make_router([router_arm("p")])
        manifest = q.run()
        attempt = manifest["arms"][0]["attempts"][0]
        self.assertIn("amdgpu:junction", attempt["temperatures"])
        self.assertIsNone(manifest["arms"][0]["vram_budget_bytes"])

    def test_tool_loop_arms_unchanged(self):
        from test_run_queue import arm

        path = self.root / "t.json"
        path.write_text(json.dumps(queue_doc([arm("a")])))
        q = rq.load_queue(path, REGISTRY)
        self.assertEqual(q["arms"][0]["kind"], "toolloop")

    def test_committed_router_queue(self):
        q = rq.load_queue(TOOLS / "queues" / "router-cpu-dev.json", REGISTRY)
        self.assertEqual(
            [(a["router"], a["unit"]) for a in q["arms"]],
            [
                ("r4-kev-0.8b", "probe"),
                ("r1-qwen3-4b", "probe"),
                ("r3-qwen35-9b", "probe"),
                ("r4-kev-0.8b", "screen"),
                ("r4-kev-0.8b", "ablation"),
                ("r1-qwen3-4b", "screen"),
                ("r1-qwen3-4b", "ablation"),
                ("r3-qwen35-9b", "screen"),
                ("r3-qwen35-9b", "ablation"),
            ],
        )
        self.assertEqual({a["device"] for a in q["arms"]}, {"cpu"})
        r3 = [a for a in q["arms"] if a["router"] == "r3-qwen35-9b" and a["unit"] != "probe"]
        self.assertTrue(all(a["max_projection_seconds"] == 43200 for a in r3))


class RouterWatchdogTests(unittest.TestCase):
    def test_router_container_in_the_fallback_names(self):
        self.assertIn(ops.OWNED_PREFIX + "router", ops.KNOWN_NAMES)
        labels = {ops.OWNED_PREFIX + "router": "v3"}
        removed = []

        def run(argv, **kw):
            args = argv[1:]
            if args[0] in ("ps", "network"):
                return subprocess.CompletedProcess(argv, 1, "", "")
            if args[0] == "inspect":
                name = args[-1]
                return subprocess.CompletedProcess(
                    argv, 0 if name in labels else 1, labels.get(name, "") + "\n", ""
                )
            if args[:2] == ["rm", "-f"]:
                removed.append(args[2])
                return subprocess.CompletedProcess(argv, 0, "", "")
            return subprocess.CompletedProcess(argv, 1, "", "")

        ops.Docker(run=run).cleanup()
        self.assertEqual(removed, [ops.OWNED_PREFIX + "router"])


if __name__ == "__main__":
    unittest.main()
