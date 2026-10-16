"""In-container driver for one decision-router unit (docs/decision-router-plan.md §5). Prototype.

Runs inside the router container (tools/run_decision_router.sh). It sees opaque state keys
only, never case ids or anything scoring-related. One unit is `probe`, `screen` or
`ablation`; its decisions go to decisions.jsonl (one final record per planned decision), every
attempt to attempts.jsonl, each worker start to its own worker-<n>.log, and summary.json.

Time limits (rev. 3.1): no legacy run budget; the ready and decision timeouts and the absolute
deadline only. A decision timeout kills the worker; the driver restarts it (re-hashing the
artefacts at load) and retries that decision once; a timeout on the retry records the case as
`timeout` and restarts again. Restarts are capped at 3 per unit; when a fourth would be needed,
the rest is `not_run`. No decision starts whose timeout would cross the deadline − 120 s.
"""

import argparse
import json
import math
import statistics
import sys
import time
from pathlib import Path
from types import SimpleNamespace

import decision_router as dr
import local_decision_benchmark as bench


class StartFailure(RuntimeError):
    pass


def quantile_keys(counts: dict, quantiles=("p50", "p90", "max")) -> dict:
    """Nearest-rank quantiles of the prompt token counts; ties to the lower key."""
    ordered = sorted(counts.items(), key=lambda kv: (kv[1], kv[0]))
    n = len(ordered)
    ranks = {q: n if q == "max" else max(1, math.ceil(int(q[1:]) / 100 * n)) for q in quantiles}
    picks, taken = {}, set()
    for q in sorted(quantiles, key=lambda q: (-ranks[q], q != "max")):  # max first, then lower
        rank = ranks[q]
        while rank > 1 and ordered[rank - 1][0] in taken:  # distinct states, nearest below
            rank -= 1
        picks[q] = ordered[rank - 1][0]
        taken.add(picks[q])
    return {q: picks[q] for q in quantiles}


def token_stats(values) -> dict:
    values = sorted(values)
    if not values:
        return {}
    n = len(values)
    return {
        "n": n,
        "min": values[0],
        "median": statistics.median(values),
        "p90": values[max(0, math.ceil(0.9 * n) - 1)],
        "max": values[-1],
    }


class Unit:
    def __init__(
        self,
        plan,
        states,
        out_dir,
        worker_factory,
        ready_timeout,
        decision_timeout,
        deadline_epoch,
        method="raw",
        now=time.time,
        monotonic=time.monotonic,
        max_restarts=dr.MAX_RESTARTS,
        extra_summary=None,
        states_path=None,
        plan_path=None,
    ):
        self.plan = plan
        self.states = states
        self.out = Path(out_dir)
        self.factory = worker_factory
        self.ready_timeout = float(ready_timeout)
        self.decision_timeout = float(decision_timeout)
        self.deadline = float(deadline_epoch)
        self.method = method
        self.now = now
        self.monotonic = monotonic
        self.max_restarts = max_restarts
        self.restarts = 0
        self.starts = []
        self.worker = None
        self.stopped = None
        self.extra = dict(extra_summary or {})
        # Bindings (review Sol B1, Sonnet M2): the exact states and plan files this unit ran.
        self.extra["states_sha256"] = dr.sha_file(states_path) if states_path else None
        self.extra["input_plan_sha256"] = dr.sha_file(plan_path) if plan_path else None
        self.decisions = self.out / "decisions.jsonl"
        self.attempts = self.out / "attempts.jsonl"

    # -- io
    def emit(self, path, row):
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(row, ensure_ascii=False) + "\n")

    def left(self) -> float:
        return self.deadline - dr.DEADLINE_MARGIN - self.now()

    def stop(self, reason):
        if self.stopped is None:
            self.stopped = reason

    # -- worker lifecycle
    def start_worker(self):
        n = len(self.starts) + 1
        if self.ready_timeout > self.left():
            raise StartFailure("deadline: the ready timeout would cross the deadline")
        began = self.monotonic()
        worker = self.factory(n)
        try:
            ready = worker.receive(min(self.ready_timeout, self.left()))
            if "ready" not in ready:
                raise StartFailure("no readiness/provenance from the worker")
        except (TimeoutError, RuntimeError, ValueError, OSError) as error:
            worker.close()
            self.starts.append(
                {"start": n, "ok": False, "error": f"{type(error).__name__}: {error}"}
            )
            raise StartFailure(str(error)) from None
        self.starts.append(
            {
                "start": n,
                "ok": True,
                "ready_seconds": self.monotonic() - began,
                "ready": ready["ready"],
            }
        )
        self.worker = worker

    def restart(self) -> bool:
        """After a kill: one more start, unless the per-unit cap is reached."""
        if self.worker is not None:
            self.worker.close()
            self.worker = None
        if self.restarts >= self.max_restarts:
            self.stop(f"restart cap {self.max_restarts} reached")
            return False
        self.restarts += 1
        try:
            self.start_worker()
        except StartFailure as error:
            self.stop(f"restart failed: {error}")
            return False
        return True

    # -- decisions
    def payload(self, entry) -> dict:
        return {
            "state": dr.state_text(self.states[entry["key"]], entry["variant"]),
            "method": self.method,
            "order": dr.order_ids(entry["order"]),
            "question": dr.QUESTION,
            "choices": dr.CHOICES,
            "tolerance": dr.SUM_TOLERANCE,
            "clear_cache": False,
        }

    def not_run(self, entry, why) -> dict:
        return {**entry, "status": "not_run", "error": why, "probabilities": None}

    def write_plan(self, decisions):
        (self.out / "plan.json").write_text(
            json.dumps({"unit": self.plan["unit"], "decisions": decisions}, indent=2) + "\n",
            encoding="utf-8",
        )

    def one_attempt(self, entry, attempt):
        """-> (record, dead). dead: the worker was killed or exited (retry path)."""
        payload = self.payload(entry)
        began = self.monotonic()
        dead = False
        try:
            raw = self.worker.predict(payload, self.decision_timeout)
            record = dr.normalise_worker_result(raw, payload["order"])
            for key in (
                "seconds",
                "prompt_hash",
                "prompt_tokens",
                "question_sha256",
                "worker_peak_rss_kib",
            ):
                if key in raw:
                    record[key] = raw[key]
            if record["status"] == "completed" and raw.get("question_sha256") != dr.OPTION_HASH:
                record = {
                    "status": "failed",
                    "error_class": "question_hash_mismatch",
                    "error": f"worker question/option hash {raw.get('question_sha256')}",
                    "probabilities": None,
                }
                self.stop("question/option hash mismatch: configuration error")
        except TimeoutError as error:
            record = {
                "status": "timeout",
                "error_class": "timeout",
                "error": str(error),
                "probabilities": None,
            }
            dead = True
        except (RuntimeError, OSError, ValueError) as error:
            record = {
                "status": "failed",
                "error_class": "worker_exit",
                "error": f"{type(error).__name__}: {error}",
                "probabilities": None,
            }
            dead = True
        except Exception as error:  # noqa: BLE001 (any other fault fails this decision only)
            record = {
                "status": "failed",
                "error_class": "driver_error",
                "error": f"{type(error).__name__}: {error}",
                "probabilities": None,
            }
            dead = True  # the worker's state is unknown: never reuse it
        record = {
            **entry,
            **record,
            "state_hash": dr.text_hash(payload["state"]),
            "attempt": attempt,
            "worker_start": len(self.starts),
            "request_seconds": self.monotonic() - began,
        }
        self.emit(self.attempts, {"event": "attempt", **record})
        return record, dead

    def decide(self, entry) -> dict:
        record = None
        for attempt in (1, 2):
            if self.stopped:
                return self.not_run(entry, self.stopped)
            if self.decision_timeout > self.left():
                self.stop("deadline: the decision timeout would cross the deadline")
                return self.not_run(entry, self.stopped)
            if self.worker is None or self.worker.process.poll() is not None:
                if not self.restart():
                    return self.not_run(entry, self.stopped)
            record, dead = self.one_attempt(entry, attempt)
            if not dead:
                return record
            if self.worker is not None:
                self.worker.close()
                self.worker = None
            if attempt == 2:
                # The retry's own failure is final. No eager restart: the next decision, if
                # any, restarts the worker (review m6: nothing left must not burn the cap).
                return record
            if not self.restart():
                return self.not_run(entry, f"no retry: {self.stopped}")
        return record

    def finish(self, rows) -> dict:
        if self.worker is not None:
            self.worker.close()
            self.worker = None
        completed = [r for r in rows if r["status"] == "completed"]
        seconds = [r["seconds"] for r in completed if isinstance(r.get("seconds"), (int, float))]
        summary = {
            "unit": self.plan["unit"],
            "planned": len(rows),
            "complete": all(r["status"] != "not_run" for r in rows) and bool(rows),
            "not_run": sum(r["status"] == "not_run" for r in rows),
            "statuses": {
                s: sum(r["status"] == s for r in rows) for s in sorted({r["status"] for r in rows})
            },
            "restarts": self.restarts,
            "max_restarts": self.max_restarts,
            "starts": self.starts,
            "stopped": self.stopped,
            "ready_timeout": self.ready_timeout,
            "decision_timeout": self.decision_timeout,
            "deadline_epoch": self.deadline,
            "decision_seconds": {
                "max": max(seconds) if seconds else None,
                "median": statistics.median(seconds) if seconds else None,
            },
            "ready_seconds": [s.get("ready_seconds") for s in self.starts if s.get("ok")],
            "option_hash": dr.OPTION_HASH,
            "runtime": next((s.get("ready") for s in self.starts if s.get("ok")), None),
            "worker_peak_rss_kib_max": max(
                (
                    r["worker_peak_rss_kib"]
                    for r in rows
                    if isinstance(r.get("worker_peak_rss_kib"), int)
                ),
                default=None,
            ),
            "worker_peak_rss_scope": "worker process (ru_maxrss), not the whole container",
            **self.extra,
        }
        (self.out / "summary.json").write_text(
            json.dumps(summary, indent=2) + "\n", encoding="utf-8"
        )
        return summary

    def run(self) -> dict:
        self.out.mkdir(parents=True, exist_ok=True)
        for path in (self.decisions, self.attempts):
            if path.exists():
                raise FileExistsError(f"preserve existing evidence: {path}")
            path.touch()
        try:
            self.start_worker()
        except StartFailure as error:
            self.stop(f"first start failed: {error}")
        if self.plan.get("select") != "token_quantiles":
            self.write_plan([dict(d) for d in self.plan["decisions"]])  # before any decision
        decisions = self.plan_decisions()
        self.write_plan(decisions)
        rows = []
        for entry in decisions:
            record = self.decide(entry)
            row = {"event": "decision", **record}
            rows.append(row)
            self.emit(self.decisions, row)
        return self.finish(rows)

    def plan_decisions(self) -> list:
        if self.plan.get("select") != "token_quantiles":
            return [dict(d) for d in self.plan["decisions"]]
        # Probe: count every candidate's prompt with the loaded tokenizer, then decide on the
        # p50, p90 and max states (no decision is needed for the counts).
        counts = {}
        if self.worker is not None:
            for key in self.plan["candidates"]:
                payload = dict(
                    self.payload({"key": key, "variant": "full", "order": "canonical"}),
                    count_only=True,
                )
                try:
                    reply = self.worker.predict(
                        payload, min(self.decision_timeout, max(1.0, self.left()))
                    )
                except (TimeoutError, RuntimeError, OSError, ValueError) as error:
                    self.stop(f"probe token count failed: {type(error).__name__}: {error}")
                    break
                if reply.get("status") != "counted":
                    self.stop(f"probe token count failed: {reply.get('error')}")
                    break
                counts[key] = reply["prompt_tokens"]
        if len(counts) != len(self.plan["candidates"]):
            self.stop(self.stopped or "probe token counts incomplete")
            picks = {q: self.plan["candidates"][0] for q in self.plan["quantiles"]}
        else:
            picks = quantile_keys(counts, self.plan["quantiles"])
        self.extra["prompt_tokens"] = token_stats(counts.values())
        self.extra["prompt_tokens_over_n_ctx"] = None
        self.extra["probe_picks"] = picks
        self.extra["probe_pick_tokens"] = {q: counts.get(k) for q, k in picks.items()}
        return [
            {"key": k, "variant": "full", "order": "canonical", "probe": q}
            for q, k in picks.items()
        ]


def worker_factory(args, out: Path):
    def make(n):
        ns = SimpleNamespace(
            backend=args.backend,
            model=args.model,
            model_sha256=args.model_sha256,
            kev_checkpoint=args.kev_checkpoint,
            output=out / "decisions.jsonl",
            worker_log=out / f"worker-{n}.log",
            device=args.device if args.backend == "anyjev" else None,
            threads=args.threads,
            n_ctx=args.n_ctx,
            raw_only=args.backend == "anyjev",
        )
        return bench.Worker(ns)

    return make


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--backend", choices=("anyjev", "kev"), required=True)
    p.add_argument("--model")
    p.add_argument("--model-sha256")
    p.add_argument("--kev-checkpoint")
    p.add_argument("--device", choices=("cpu",), default="cpu")
    p.add_argument("--threads", type=int, default=6)
    p.add_argument("--n-ctx", type=int, default=8192)
    p.add_argument("--states", type=Path, required=True)
    p.add_argument("--plan", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--ready-timeout", type=float, required=True)
    p.add_argument("--decision-timeout", type=float, required=True)
    p.add_argument("--deadline-epoch", type=float, required=True)
    p.add_argument("--max-restarts", type=int, default=dr.MAX_RESTARTS)
    p.add_argument("--summary-extra", type=Path, help="JSON merged into summary.json")
    args = p.parse_args(argv)
    plan = json.loads(args.plan.read_text(encoding="utf-8"))
    states = dr.read_states(args.states)
    extra = json.loads(args.summary_extra.read_text(encoding="utf-8")) if args.summary_extra else {}
    unit = Unit(
        plan,
        states,
        args.out,
        worker_factory(args, args.out),
        args.ready_timeout,
        args.decision_timeout,
        args.deadline_epoch,
        method="raw" if args.backend == "anyjev" else "kev",
        max_restarts=args.max_restarts,
        extra_summary=extra,
        states_path=args.states,
        plan_path=args.plan,
    )
    summary = unit.run()
    print(
        json.dumps(
            {
                k: summary[k]
                for k in ("unit", "planned", "complete", "not_run", "restarts", "stopped")
            }
        ),
        flush=True,
    )
    return 0 if summary["complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
