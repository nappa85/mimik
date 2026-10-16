"""Shared stdlib-only admission for host orchestration and container workers."""

import math
import time


def admit_execution(cutoff, remaining, free_bytes):
    if cutoff <= time.time() or not 1 <= remaining <= 384 or free_bytes < 12 * 1024**3:
        raise ValueError("Inference cutoff, aggregate allowance or host RAM admission failed")
    return True


def setup_remaining(global_cutoff, candidate_started, now=None, cap_seconds=900):
    """A transport change must keep the original candidate start, not restart it."""
    now = time.time() if now is None else now
    if (
        any(
            isinstance(value, bool) or not math.isfinite(value)
            for value in (global_cutoff, candidate_started, now, cap_seconds)
        )
        or candidate_started > now
        or cap_seconds <= 0
    ):
        raise ValueError("Invalid setup clock")
    remaining = min(global_cutoff, candidate_started + cap_seconds) - now
    if remaining <= 0:
        raise ValueError("Shared candidate/global elapsed setup deadline exhausted")
    return remaining
