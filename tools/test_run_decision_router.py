"""tools/run_decision_router.sh with a fake `docker` on PATH (no model, no inference).

New runner: red = script absent. The fake docker hashes bind-mounted files for real, runs the
real `decision_router.py prepare` (isolated builder process, real formula CLI) for the states
container, and stands in for the router container by writing a driver summary. The tests
check the container hardening, the probe-summary gate, hash refusals, CHECK_ONLY, the lock,
the schedule gate and the cleanup.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
DEMO = TOOLS.parent / "examples" / "tool-loop-demo"
FORMULA = os.environ.get("MIMIK_FORMULA_EVAL", "")
COPIED = (
    "run_decision_router.sh",
    "decision_models.py",
    "decision_router.py",
    "router_state_builder.py",
    "router_driver.py",
    "local_decision_benchmark.py",
    "tool_loop_benchmark.py",
    "run_queue.py",
    "toolloop_ops.py",
    "run_watchdog.py",
    "model_registry.py",
    "models.json",
)

FAKE_DOCKER = r"""#!PYTHON
import hashlib, json, os, subprocess, sys
from pathlib import Path

log = Path(os.environ["FAKE_DOCKER_LOG"])
args = sys.argv[1:]
with open(log / "calls.jsonl", "a") as h:
    h.write(json.dumps(args) + "\n")
state_file = log / "state.json"
state = json.loads(state_file.read_text()) if state_file.exists() else {"c": {}}
env = os.environ
def save():
    state_file.write_text(json.dumps(state))
VALUE = {"--name", "--network", "--user", "--cap-drop", "--security-opt", "--memory",
         "--memory-swap", "--cpus", "--pids-limit", "--device", "--mount", "--tmpfs", "-e",
         "--pull", "--label", "--entrypoint", "-w"}
if args[0] == "context":
    print("default"); sys.exit(0)
if args[0] == "run":
    opts, i, mounts = {}, 1, {}
    while args[i].startswith("-"):
        if args[i] in VALUE:
            opts.setdefault(args[i], []).append(args[i + 1])
            if args[i] == "--mount":
                kv = dict(p.split("=", 1) for p in args[i + 1].split(",") if "=" in p)
                mounts[kv["dst"]] = kv.get("src")
            i += 2
        else:
            opts.setdefault(args[i], []).append(True)
            i += 1
    image, cmd = args[i], args[i + 1:]
    name = opts["--name"][0]
    labels = dict(l.split("=", 1) for l in opts.get("--label", []))
    if labels.get("mimik.toolloop") != "v3" or "mimik.toolloop.run" not in labels:
        sys.exit("fake docker: run without the ownership labels")
    if "--pull" not in opts or "--init" not in opts or opts.get("--network") != ["none"]:
        sys.exit("fake docker: needs --pull never, --init and --network none")
    if not name.startswith("mimik-toolloop-v3-"):
        sys.exit("fake docker: unowned name " + name)
    def host(path):
        for dst, src in sorted(mounts.items(), key=lambda kv: -len(kv[0])):
            if path == dst or path.startswith(dst + "/"):
                return (src or "") + path[len(dst):]
        return path
    if cmd[:1] == ["sha256sum"]:
        path = env["MIMIK_FORMULA_EVAL"] if cmd[1].startswith("/formula/") else host(cmd[1])
        clock = env.get("MIMIK_TEST_CLOCK_FILE")
        if clock and env.get("FAKE_HASH_SECONDS") and not cmd[1].startswith("/formula/"):
            Path(clock).write_text(str(float(Path(clock).read_text()) + float(env["FAKE_HASH_SECONDS"])))
        digest = hashlib.sha256(Path(path).read_bytes()).hexdigest()
        if env.get("FAKE_BAD_HASH") and not cmd[1].startswith("/formula/"):
            digest = "0" * 64
        print(f"{digest}  {cmd[1]}")
        sys.exit(0)
    if "prepare" in cmd:
        # Enforce the container's filesystem boundary (round 2, R1): only the mounted files
        # exist under a private root; nothing resolves to the host tree.
        import shutil, tempfile
        root = Path(tempfile.mkdtemp(prefix="fake-container-"))
        for dst, src in mounts.items():
            target = root / dst.lstrip("/")
            target.parent.mkdir(parents=True, exist_ok=True)
            if dst == "/formula":
                continue  # the volume: only the pinned CLI, placed below
            if dst == "/prep":
                target.symlink_to(src)  # the one writable mount
            elif Path(src).is_dir():
                shutil.copytree(src, target)
            else:
                shutil.copy2(src, target)
        args_ = []
        for a in cmd[1:]:
            if a.startswith("/formula/"):
                dest = root / a.lstrip("/")
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(env["MIMIK_FORMULA_EVAL"], dest)
            args_.append(str(root / a.lstrip("/")) if a.startswith("/") else a)
        proc = subprocess.run([sys.executable, "-E", "-s"] + args_, capture_output=True, text=True, cwd=root / "tools")
        sys.stdout.write(proc.stdout); sys.stderr.write(proc.stderr)
        shutil.rmtree(root, ignore_errors=True)
        sys.exit(proc.returncode)
    if name.endswith("-router"):
        (log / "router.json").write_text(json.dumps({"image": image, "cmd": cmd, "opts": opts, "mounts": mounts}))
        out = Path(host("/out"))
        unit = json.loads(Path(host(cmd[cmd.index("--plan") + 1])).read_text())["unit"]
        complete = not env.get("FAKE_INCOMPLETE")
        (out / "summary.json").write_text(json.dumps({"unit": unit, "complete": complete}))
        (out / "decisions.jsonl").write_text("")
        state["c"][name] = {"status": "exited", "rc": 0 if complete else 1, "run": labels["mimik.toolloop.run"]}
        save()
        print("feed"); sys.exit(0)
    sys.exit("fake docker: unexpected run " + name)
if args[0] == "inspect":
    fmt, name = args[2], args[3]
    c = state["c"].get(name)
    if c is None:
        sys.exit(1)
    if "mimik.toolloop.run" in fmt:
        print(c["run"])
    elif "State.Status" in fmt:
        print(f"{c['status']} {c['rc']} false")
    elif "OOMKilled" in fmt:
        print("false")
    sys.exit(0)
if args[0] == "ps":
    print("\n".join(sorted(state["c"]))); sys.exit(0)
if args[0] == "logs":
    sys.exit(0)
if args[0] == "stop":
    sys.exit(0)
if args[0] == "rm":
    if env.get("FAKE_RM_FAIL"):
        sys.exit(1)
    for n in args[2:]:
        state["c"].pop(n, None)
    save(); sys.exit(0)
sys.exit(f"fake docker: unhandled {args}")
"""


@unittest.skipUnless(FORMULA and Path(FORMULA).exists(), "needs MIMIK_FORMULA_EVAL (formula CLI)")
@unittest.skipUnless(shutil.which("git") and shutil.which("flock"), "needs git and flock")
class RouterRunnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "repo"
        (self.repo / "tools" / "case-subsets").mkdir(parents=True)
        for name in COPIED:
            shutil.copy2(TOOLS / name, self.repo / "tools" / name)
        shutil.copy2(
            TOOLS / "case-subsets" / "dev-screen-40.json", self.repo / "tools" / "case-subsets"
        )
        (self.repo / "examples").mkdir()
        (self.repo / "examples" / "tool-loop-demo").symlink_to(DEMO)
        self.models = self.tmp / "models"
        gguf = self.models / "g" / "m.gguf"
        gguf.parent.mkdir(parents=True)
        gguf.write_bytes(b"fake gguf" * 20)
        cache = self.models / "hf"
        ckpt = cache / "models--k" / "snapshots" / "abc"
        ckpt.mkdir(parents=True)
        kev_files = []
        for role in ("adapter", "head", "base_weights", "base_tokenizer", "base_config"):
            path = ckpt / f"{role}.bin"
            path.write_bytes(role.encode())
            kev_files.append(self.entry(role, path))
        registry = json.loads((TOOLS / "decision_models.json").read_text())
        registry["routers"]["r1-qwen3-4b"]["files"] = [self.entry("gguf", gguf)]
        kev = registry["routers"]["r4-kev-0.8b"]
        kev.update(hf_cache="hf", kev_checkpoint="hf/models--k/snapshots/abc", files=kev_files)
        (self.repo / "tools" / "decision_models.json").write_text(json.dumps(registry))
        git = [
            "git",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-C",
            str(self.repo),
        ]
        subprocess.run(git + ["init", "-q"], check=True)
        subprocess.run(git + ["add", "-A"], check=True)
        subprocess.run(git + ["commit", "-q", "-m", "t"], check=True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text(FAKE_DOCKER.replace("#!PYTHON", "#!" + sys.executable))
        docker.chmod(0o755)
        python3 = self.bin / "python3"
        python3.write_text(f'#!/bin/sh\nexec "{sys.executable}" "$@"\n')
        python3.chmod(0o755)
        self.log = self.tmp / "log"
        self.log.mkdir()

    def entry(self, role, path):
        data = Path(path).read_bytes()
        return {
            "role": role,
            "path": str(Path(path).relative_to(self.models)),
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }

    def run_router(self, key, unit, **env):
        out = self.tmp / f"out-{len(list(self.tmp.iterdir()))}"
        base = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.tmp),
            "FAKE_DOCKER_LOG": str(self.log),
            "MIMIK_FORMULA_EVAL": FORMULA,
            "FORMULA_CLI_SHA256": hashlib.sha256(Path(FORMULA).read_bytes()).hexdigest(),
            "LOCK_FILE": str(self.tmp / "lock"),
            "MIMIK_MODEL_ROOT": str(self.models),
            "SCHEDULE_OVERRIDE": "user-approved",
            "RUNNER_POLL_SECONDS": "0",
            "MIMIK_TEST_MODE": "1",
            "MIMIK_TEST_HARD_DEADLINE": str(int(__import__("time").time()) + 10 * 86400),
            "CHECK_REGISTRY": str(self.tmp / "checks.txt"),
            "OUT_DIR": str(out),
        }
        base.update({k: v for k, v in env.items() if v is not None})
        proc = subprocess.run(
            ["bash", str(self.repo / "tools" / "run_decision_router.sh"), key, unit],
            env=base,
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            timeout=600,
            check=False,
        )
        prov = {}
        if (out / "provenance.txt").exists():
            for line in (out / "provenance.txt").read_text().splitlines():
                for token in line.split():
                    if "=" in token:
                        k, v = token.split("=", 1)
                        prov.setdefault(k, v)
        return proc, out, prov

    def router(self):
        path = self.log / "router.json"
        return json.loads(path.read_text()) if path.exists() else None

    def test_probe_unit_hardened_container_and_provenance(self):
        proc, out, prov = self.run_router("r1-qwen3-4b", "probe")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(prov["cleanup_verified"], "true")
        self.assertEqual(prov["main_exit"], "0")
        self.assertEqual(prov["ledger_file"], "unit/decisions.jsonl")
        r = self.router()
        opts = r["opts"]
        self.assertEqual(opts["--network"], ["none"])
        self.assertNotIn("--device", opts)  # no /dev/dri while the GPU is out of service
        self.assertIn("--read-only", opts)
        self.assertEqual(opts["--cpus"], ["6"])
        self.assertEqual(opts["--cap-drop"], ["ALL"])
        model = next(m for m in opts["--mount"] if "dst=/models/model.gguf" in m)
        self.assertIn("readonly", model)
        self.assertIn("readonly", next(m for m in opts["--mount"] if "dst=/prep/prepared" in m))
        for mount in opts["--mount"]:  # the host-only mapping (prep/run/host) is never mounted
            self.assertFalse(mount.split(",")[1].endswith(("/prep", "/run", "/host")), mount)
        self.assertTrue((out / "prep" / "run" / "host" / "mapping.json").exists())
        cmd = r["cmd"]
        self.assertEqual(cmd[cmd.index("--device") + 1], "cpu")
        self.assertEqual(cmd[cmd.index("--max-restarts") + 1], "1")
        self.assertEqual(cmd[cmd.index("--decision-timeout") + 1], "1800")
        deadline = int(prov["deadline_epoch"])
        self.assertEqual(int(cmd[cmd.index("--deadline-epoch") + 1]), deadline - 900)
        self.assertTrue((out / "prep" / "run" / "prepared" / "states.jsonl").exists())
        self.assertFalse((out / "prep" / "run" / "prepared" / "mapping.json").exists())
        extra = json.loads((out / "prep" / "summary-extra.json").read_text())
        self.assertEqual(extra["router_key"], "r1-qwen3-4b")

    def probe_summary(self, **change):
        doc = {
            "unit": "probe",
            "complete": True,
            "planned": 3,
            "statuses": {"completed": 3},
            "decision_seconds": {"max": 100.0},
        }
        registry = json.loads((self.repo / "tools" / "decision_models.json").read_text())
        e = registry["routers"]["r1-qwen3-4b"]
        doc.update(
            router_key="r1-qwen3-4b",
            disk_rate=20000000,
            image=e["image"],
            artefacts=[{k: f[k] for k in ("role", "size", "sha256")} for f in e["files"]],
        )
        doc.update(change)
        path = self.tmp / f"probe-{len(list(self.tmp.iterdir()))}.json"
        path.write_text(json.dumps(doc))
        return str(path)

    def test_screen_needs_a_complete_probe_of_the_same_router(self):
        proc, _, prov = self.run_router("r1-qwen3-4b", "screen")
        self.assertEqual((proc.returncode, prov.get("failure")), (3, "ineligible"))
        for bad in (
            self.probe_summary(complete=False),
            self.probe_summary(router_key="r4-kev-0.8b"),
            self.probe_summary(statuses={"completed": 2}),
            self.probe_summary(image="sha256:" + "9" * 64),
        ):
            proc, _, prov = self.run_router("r1-qwen3-4b", "screen", PROBE_SUMMARY=bad)
            self.assertEqual((proc.returncode, prov.get("failure")), (3, "ineligible"))
        self.assertIsNone(self.router())
        proc, _, prov = self.run_router("r1-qwen3-4b", "screen", PROBE_SUMMARY=self.probe_summary())
        self.assertEqual(proc.returncode, 0, proc.stderr)
        cmd = self.router()["cmd"]
        self.assertEqual(cmd[cmd.index("--decision-timeout") + 1], "300")
        self.assertEqual(cmd[cmd.index("--max-restarts") + 1], "3")

    def test_artefact_hash_mismatch_refuses_before_any_router_container(self):
        proc, _, prov = self.run_router("r1-qwen3-4b", "probe", FAKE_BAD_HASH="1")
        self.assertEqual((proc.returncode, prov.get("failure")), (5, "model_hash"))
        self.assertIsNone(self.router())
        self.assertEqual(prov["cleanup_verified"], "true")

    def test_check_only_stops_before_the_router(self):
        proc, _, prov = self.run_router("r4-kev-0.8b", "probe", CHECK_ONLY="1")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(prov.get("check_only"), "1")
        self.assertIsNone(self.router())

    def test_kev_mounts_its_cache_offline(self):
        proc, _, _ = self.run_router("r4-kev-0.8b", "probe")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        r = self.router()
        self.assertIn("readonly", next(m for m in r["opts"]["--mount"] if "dst=/hf" in m))
        self.assertIn("HF_HUB_OFFLINE=1", r["opts"]["-e"])
        cmd = r["cmd"]
        self.assertEqual(cmd[cmd.index("--kev-checkpoint") + 1], "/hf/models--k/snapshots/abc")
        self.assertNotIn("--device", cmd)

    def test_lock_held_exits_9(self):
        with open(self.tmp / "lock", "w") as handle:
            holder = subprocess.Popen(
                ["flock", "-x", str(self.tmp / "lock"), "sleep", "30"], stdout=handle
            )
            try:
                __import__("time").sleep(0.5)
                proc, _, _ = self.run_router("r1-qwen3-4b", "probe")
            finally:
                holder.kill()
                holder.wait()
        self.assertEqual(proc.returncode, 9)

    def test_closed_window_refuses_heavy_steps(self):
        clock = self.tmp / "clock"
        import datetime as dt

        day = dt.datetime.now().replace(hour=10, minute=0, second=0, microsecond=0)
        while day.weekday() != 2:
            day += dt.timedelta(days=1)
        clock.write_text(str(day.timestamp()))
        proc, _, prov = self.run_router(
            "r1-qwen3-4b", "probe", SCHEDULE_OVERRIDE="", MIMIK_TEST_CLOCK_FILE=str(clock)
        )
        self.assertEqual((proc.returncode, prov.get("failure")), (10, "window"))
        self.assertIsNone(self.router())

    def test_incomplete_unit_exits_1_with_cleanup(self):
        proc, _, prov = self.run_router("r1-qwen3-4b", "probe", FAKE_INCOMPLETE="1")
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(prov["main_exit"], "1")
        self.assertEqual(prov["cleanup_verified"], "true")

    def clock(self, offset=0):
        import time

        path = self.tmp / "clock"
        now = int(time.time())
        path.write_text(str(now))
        return str(path), now

    def test_a_cutoff_crossed_mid_hashing_stops_before_the_next_artefact(self):
        """95cc971: setup was bounded by fixed timeouts only, one gate before all hashes (Sol 3)."""
        clock, now = self.clock()
        proc, _, prov = self.run_router(
            "r4-kev-0.8b",
            "probe",
            MIMIK_TEST_CLOCK_FILE=clock,
            MIMIK_TEST_HARD_DEADLINE=str(now + 3000),
            FAKE_HASH_SECONDS="5000",
        )
        self.assertEqual((proc.returncode, prov.get("failure")), (10, "window"), proc.stderr)
        self.assertIsNone(self.router())
        self.assertEqual(prov["cleanup_verified"], "true")
        hashed = [
            line
            for line in (self.tmp / "log" / "calls.jsonl").read_text().splitlines()
            if "/artefact" in line
        ]
        self.assertEqual(len(hashed), 1)  # the second artefact never started

    def test_setup_steps_are_bounded_by_the_time_left(self):
        clock, now = self.clock()
        proc, out, prov = self.run_router(
            "r1-qwen3-4b",
            "probe",
            MIMIK_TEST_CLOCK_FILE=clock,
            MIMIK_TEST_HARD_DEADLINE=str(now + 800),
        )
        text = (out / "provenance.txt").read_text()
        self.assertIn("hash_gguf_limit_seconds=440 (bounded by the cutoff)", text)

    def test_check_only_keeps_the_absolute_deadline(self):
        clock, now = self.clock()
        proc, _, prov = self.run_router(
            "r1-qwen3-4b",
            "probe",
            CHECK_ONLY="1",
            MIMIK_TEST_CLOCK_FILE=clock,
            MIMIK_TEST_HARD_DEADLINE=str(now - 10),
        )
        self.assertEqual((proc.returncode, prov.get("failure")), (10, "window"))

    def test_a_queued_run_refuses_a_dirty_tree(self):
        """Review m8: a registered run must not run uncommitted code."""
        (self.repo / "tools" / "decision_router.py").write_text(
            (self.repo / "tools" / "decision_router.py").read_text() + "\n# edit\n"
        )
        proc, _, prov = self.run_router(
            "r1-qwen3-4b", "probe", QUEUE_PID=str(__import__("os").getpid())
        )
        self.assertEqual((proc.returncode, prov.get("failure")), (2, "dirty_tree"))

    def test_router_container_mounts_an_allowlist_of_tools(self):
        """Review m8: the whole tools/ directory was mounted."""
        proc, _, _ = self.run_router("r1-qwen3-4b", "probe")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        dsts = [
            dict(kv.split("=", 1) for kv in m.split(",") if "=" in kv)["dst"]
            for m in self.router()["opts"]["--mount"]
        ]
        self.assertNotIn("/tools", dsts)
        self.assertIn("/tools/router_driver.py", dsts)
        self.assertNotIn("/tools/decision_models.json", dsts)

    def test_summary_extra_carries_the_run_identity(self):
        proc, out, _ = self.run_router("r1-qwen3-4b", "probe")
        extra = json.loads((out / "prep" / "summary-extra.json").read_text())
        registry = json.loads((self.repo / "tools" / "decision_models.json").read_text())[
            "routers"
        ]["r1-qwen3-4b"]
        self.assertEqual(extra["image"], registry["image"])
        self.assertEqual(extra["artefacts"][0]["sha256"], registry["files"][0]["sha256"])

    def test_an_artefact_resolving_outside_the_model_root_is_refused(self):
        """95cc971: only lexical checks; a symlinked directory escaped the root (Sol 6)."""
        outside = self.tmp / "outside"
        (self.models / "g").rename(outside)
        (self.models / "g").symlink_to(outside)
        proc, _, prov = self.run_router("r1-qwen3-4b", "probe")
        self.assertEqual((proc.returncode, prov.get("failure")), (5, "model_path"))

    def test_unresolved_cleanup_exits_11(self):
        proc, _, prov = self.run_router("r1-qwen3-4b", "probe", FAKE_RM_FAIL="1")
        self.assertEqual(proc.returncode, 11)
        self.assertEqual(prov["cleanup_verified"], "false")


if __name__ == "__main__":
    unittest.main()
