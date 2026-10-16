"""tools/run_tool_loop_benchmark.sh with a fake `docker` on PATH: no GPU, no server.

The fake docker hashes bind-mounted files for real, records the server vector, and runs the
real harness in --dry-run for the harness container (the --endpoint is dropped), so these
tests check that the runner's pass-through reaches the llama.cpp args and the ledger header.
Failing-first: run against the 2566040 runner (hard-coded case statement, no pass-through).
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

import run_queue as rq

TOOLS = Path(__file__).resolve().parent
DEMO = TOOLS.parent / "examples" / "tool-loop-demo"
FORMULA = os.environ.get("MIMIK_FORMULA_EVAL", "")
COPIED = (
    "run_tool_loop_benchmark.sh",
    "model_registry.py",
    "models.json",
    "run_queue.py",
    "tool_loop_benchmark.py",
)
SUBSET = TOOLS / "case-subsets" / "dev-screen-40.json"

FAKE_DOCKER = r'''#!PYTHON
"""Fake docker for the runner tests."""
import hashlib, json, os, subprocess, sys
from pathlib import Path

log = Path(os.environ["FAKE_DOCKER_LOG"])
args = sys.argv[1:]
with open(log / "calls.jsonl", "a") as h:
    h.write(json.dumps(args) + "\n")
VALUE = {"--name", "--network", "--network-alias", "--user", "--cap-drop", "--security-opt",
         "--memory", "--memory-swap", "--cpus", "--pids-limit", "--device", "--group-add",
         "--mount", "--tmpfs", "-e", "--pull"}
if args[0] == "run":
    opts, i = {}, 1
    mounts = {}
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
    if os.environ.get("FAKE_PULL_CHECK") and "--pull" not in opts:
        sys.exit("fake docker: run without --pull never")
    if cmd[:1] == ["sha256sum"]:
        path = cmd[1]
        for dst, src in mounts.items():
            if path.startswith(dst) and src and src.startswith("/"):
                path = src + path[len(dst):]
        if path.startswith("/formula/"):
            path = os.environ["MIMIK_FORMULA_EVAL"]
        digest = hashlib.sha256(Path(path).read_bytes()).hexdigest()
        print(f"{digest}  {cmd[1]}")
        sys.exit(0)
    if "-d" in opts:
        (log / "server.json").write_text(json.dumps({"image": image, "args": cmd, "mounts": mounts, "opts": opts}))
        print("cafe")
        sys.exit(0)
    # harness container: run the real harness as a dry run on the host paths
    mapped = []
    skip = False
    for n, a in enumerate(cmd):
        if skip:
            skip = False
            continue
        if a == "--endpoint":
            skip = True
            continue
        if a.startswith("/formula/"):
            a = os.environ["MIMIK_FORMULA_EVAL"]
        for dst in ("/tools", "/pack", "/out"):
            if a == dst or a.startswith(dst + "/"):
                a = mounts[dst] + a[len(dst):]
        mapped.append(a)
    assert mapped[0] == "python3"
    mapped = [sys.executable] + mapped[1:] + ["--dry-run"]
    (log / "harness-calls.jsonl").open("a").write(json.dumps(cmd) + "\n")
    sys.exit(subprocess.call(mapped))
if args[0] == "exec":
    print("{}")
    sys.exit(0)
if args[0] == "ps":
    print("cafe")
    sys.exit(0)
if args[0] == "inspect":
    fmt = args[2]
    print("false" if "OOMKilled" in fmt else "cafe")
    sys.exit(0)
if args[0] == "logs":
    print("       eval time =   100.00 ms /    10 tokens")
    sys.exit(0)
if args[0] == "image":
    print("image_id=sha256:fake version=b11610 revision=x")
    sys.exit(0)
sys.exit(0)  # network, rm, stop
'''


@unittest.skipUnless(FORMULA and Path(FORMULA).exists(), "needs MIMIK_FORMULA_EVAL (formula CLI)")
@unittest.skipUnless(shutil.which("git") and shutil.which("flock"), "needs git and flock")
class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "repo"
        (self.repo / "tools").mkdir(parents=True)
        for name in COPIED:
            shutil.copy2(TOOLS / name, self.repo / "tools" / name)
        (self.repo / "examples").mkdir()
        (self.repo / "examples" / "tool-loop-demo").symlink_to(DEMO)
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
        real = self.tmp / "store" / "real.gguf"
        real.parent.mkdir()
        real.write_bytes(b"fake gguf")
        self.link = self.tmp / "link.gguf"
        self.link.symlink_to(real)
        self.real, self.sha = real, hashlib.sha256(b"fake gguf").hexdigest()
        self.formula_sha = hashlib.sha256(Path(FORMULA).read_bytes()).hexdigest()

    def run_runner(self, *argv, **env):
        base = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.tmp),
            "FAKE_DOCKER_LOG": str(self.log),
            "FAKE_PULL_CHECK": "1",
            "MIMIK_FORMULA_EVAL": FORMULA,
            "FORMULA_CLI_SHA256": self.formula_sha,
            "LOCK_FILE": str(self.tmp / "lock"),
            "MODEL_PATH": str(self.link),
            "MODEL_SHA256": self.sha,
            "SCHEDULE_OVERRIDE": "user-approved",
            "OUT_DIR": str(self.tmp / f"out-{len(list(self.tmp.iterdir()))}"),
        }
        base.update({k: v for k, v in env.items() if v is not None})
        for k, v in env.items():
            if v is None:
                base.pop(k, None)
        proc = subprocess.run(
            ["bash", str(self.repo / "tools" / "run_tool_loop_benchmark.sh"), *argv],
            env=base,
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            check=False,
            timeout=300,
        )
        out = Path(base["OUT_DIR"])
        return proc, out

    def ledger(self, out):
        prov = rq.parse_provenance(out / "provenance.txt")
        return [json.loads(x) for x in (out / prov["ledger_file"]).read_text().splitlines()]

    def server(self):
        return json.loads((self.log / "server.json").read_text())

    def test_pass_through_reaches_server_and_header(self):
        proc, out = self.run_runner(
            "qwen35-9b",
            "dev",
            CTX="24576",
            ARM="S40-c0",
            SEEDS="17",
            CASES_FILE=str(SUBSET),
            MAX_TURNS="12",
            MAX_TOKENS="2048",
            THINKING="off",
            UNTRUSTED="1",
            SEARCH_NOTES="1",
            NUDGE="0",
            REQUEST_TIMEOUT="420",
            QUIET="1",
            PREFLIGHT_CASE="dev-001",
            BUDGET_SECONDS="0",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr + (out / "harness.stderr").read_text())
        server = self.server()
        args = server["args"]
        self.assertEqual(args[args.index("--ctx-size") + 1], "24576")
        self.assertEqual(server["mounts"]["/models/model.gguf"], str(self.real))  # resolved
        self.assertIn("--pull", server["opts"])
        rows = self.ledger(out)
        header = rows[0]
        self.assertEqual((header["arm"], header["ctx"], header["max_turns"]), ("S40-c0", 24576, 12))
        self.assertEqual(header["sampling"]["max_tokens"], 2048)
        self.assertEqual(
            (
                header["flags"]["untrusted"],
                header["flags"]["search_notes"],
                header["flags"]["nudge"],
            ),
            (True, True, False),
        )
        self.assertEqual(header["server"]["args"], args)
        self.assertEqual(header["thinking_effective"], "off")
        doc = json.loads(SUBSET.read_text())
        self.assertEqual(header["case_subset"]["sha256"], doc["sha256"])
        self.assertEqual(header["planned_episodes"], 40)
        self.assertEqual(header["runner_provenance"]["registry_key"], "qwen35-9b")
        self.assertEqual(rows[-1]["event"], "run_summary")
        calls = [json.loads(x) for x in (self.log / "harness-calls.jsonl").read_text().splitlines()]
        self.assertEqual(len(calls), 2)  # preflight, then the arm
        self.assertIn("--request-timeout", calls[1])
        self.assertEqual(calls[1][calls[1].index("--request-timeout") + 1], "420")
        self.assertIn("--quiet-progress", calls[1])
        stderr = (out / "harness.stderr").read_text()
        self.assertNotIn("outcome=", stderr)
        pre = [json.loads(x) for x in (out / "preflight-ledger.jsonl").read_text().splitlines()]
        self.assertEqual((pre[0]["case_order"], pre[0]["arm"]), (["dev-001"], "S40-c0-preflight"))
        prov = rq.parse_provenance(out / "provenance.txt")
        self.assertEqual((prov["harness_exit"], prov["preflight_exit"]), ("0", "0"))
        self.assertEqual(prov["model_sha256_source"], "hashed")

    def test_defaults_are_c0_and_registry(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", "17")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        header = self.ledger(out)[0]
        self.assertEqual(header["ctx"], 16384)
        self.assertEqual(
            header["flags"],
            {
                "nudge": True,
                "turns_remaining": False,
                "search_notes": False,
                "sanitize_echo": True,
                "pending_current": True,
                "untrusted": False,
            },
        )
        self.assertEqual((header["max_turns"], header["sampling"]["max_tokens"]), (8, 4096))
        self.assertEqual(header["sampling"]["temperature"], 0.1)
        self.assertEqual(header["thinking_capability"], "never")
        self.assertEqual(header["case_order"], ["dev-001", "dev-005", "dev-008"])
        args = self.server()["args"]
        self.assertEqual(args[args.index("--alias") + 1], "qwen3-4b-instruct-2507-q4_k_m")
        self.assertIn("--memory", self.server()["opts"])
        self.assertEqual(self.server()["opts"]["--memory"], ["14g"])

    def test_hash_cache_skips_the_second_hash(self):
        cache = self.tmp / "hash-cache.txt"
        self.run_runner("qwen3-4b-2507", "smoke", "17", HASH_CACHE=str(cache), CHECK_ONLY="1")
        self.assertIn(self.sha, cache.read_text())
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", "17", HASH_CACHE=str(cache), CHECK_ONLY="1"
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        prov = rq.parse_provenance(out / "provenance.txt")
        self.assertEqual(prov["model_sha256_source"], "session_cache")

    def test_check_only_never_starts_a_server(self):
        proc, out = self.run_runner("qwen36-35b-a3b", "smoke", N_CPU_MOE="32", CHECK_ONLY="1")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse((self.log / "server.json").exists())
        text = (out / "provenance.txt").read_text()
        self.assertIn("--n-cpu-moe 32", text)
        self.assertIn("check_only=1", text)

    def test_refusals(self):
        cases = {
            "unknown model": (("nope", "dev"), {}, 2),
            "not downloaded": (("granite42-3b", "dev"), {}, 2),
            "moe required": (("qwen36-35b-a3b", "smoke"), {}, 2),
            "bad flag": (("qwen35-9b", "dev"), {"UNTRUSTED": "yes"}, 2),
            "bad ctx": (("qwen35-9b", "dev"), {"CTX": "12k"}, 2),
            "cases and file": (
                ("qwen35-9b", "dev"),
                {"CASES": "dev-001", "CASES_FILE": str(SUBSET)},
                2,
            ),
            "smoke cases": (("qwen35-9b", "smoke"), {"CASES": "dev-001"}, 2),
            "bad thinking": (("qwen35-9b", "dev"), {"THINKING": "maybe"}, 2),
            "model hash": (("qwen35-9b", "smoke"), {"MODEL_SHA256": "0" * 64}, 5),
            "model missing": (("qwen35-9b", "smoke"), {"MODEL_PATH": "/nonexistent.gguf"}, 5),
        }
        for name, (argv, env, code) in cases.items():
            with self.subTest(name=name):
                proc, _ = self.run_runner(*argv, **env)
                self.assertEqual(proc.returncode, code, proc.stderr)
        self.assertFalse((self.log / "server.json").exists())

    def test_registry_size_check_and_missing_root(self):
        root = self.tmp / "models"
        path = root / "target/local-rag-benchmark-20261015/Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
        path.parent.mkdir(parents=True)
        path.write_bytes(b"short")
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", MODEL_PATH=None, MIMIK_MODEL_ROOT=str(root)
        )
        self.assertEqual(proc.returncode, 5)
        self.assertIn("failure=model_size", (out / "provenance.txt").read_text())
        proc, out = self.run_runner(
            "qwen3-4b-2507", "smoke", MODEL_PATH=None, MIMIK_MODEL_ROOT=str(self.tmp / "absent")
        )
        self.assertEqual(proc.returncode, 5)
        self.assertIn("failure=model_missing", (out / "provenance.txt").read_text())

    def test_schedule_window_refused_on_a_weekday_afternoon(self):
        wednesday = rq.epoch(rq.dt.datetime(2026, 10, 14, 10, 0))
        proc, out = self.run_runner(
            "qwen35-9b", "smoke", SCHEDULE_OVERRIDE=None, MIMIK_QUEUE_NOW=str(wednesday)
        )
        self.assertEqual(proc.returncode, 10, proc.stderr)
        self.assertIn("failure=window", (out / "provenance.txt").read_text())
        self.assertFalse((self.log / "server.json").exists())

    def test_window_sets_the_harness_budget(self):
        wednesday_night = rq.epoch(rq.dt.datetime(2026, 10, 14, 20, 0))
        proc, out = self.run_runner(
            "qwen3-4b-2507",
            "smoke",
            "17",
            SCHEDULE_OVERRIDE=None,
            MIMIK_QUEUE_NOW=str(wednesday_night),
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        prov = rq.parse_provenance(out / "provenance.txt")
        self.assertEqual(prov["budget_from_window"], str(13 * 3600 - 600))

    def test_preflight_failure_stops_before_the_arm(self):
        proc, out = self.run_runner("qwen3-4b-2507", "smoke", PREFLIGHT_CASE="no-such-case")
        self.assertEqual(proc.returncode, 6, proc.stderr)
        text = (out / "provenance.txt").read_text()
        self.assertIn("failure=preflight", text)
        calls = (self.log / "harness-calls.jsonl").read_text().splitlines()
        self.assertEqual(len(calls), 1)

    def test_shellcheck_clean_if_available(self):
        if not shutil.which("shellcheck"):
            self.skipTest("shellcheck not in this image (run koalaman/shellcheck separately)")
        proc = subprocess.run(
            ["shellcheck", str(TOOLS / "run_tool_loop_benchmark.sh")],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stdout)


if __name__ == "__main__":
    unittest.main()
