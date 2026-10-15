"""Sample the real release-mode 32-sheet freeze/rollback test in one fresh child."""

import argparse
import json
import re
import resource
import subprocess
import tempfile
from pathlib import Path

from smoke_release import smoke_environment


def durations(output):
    samples = re.findall(r"StageC actual Rust (.*?): ([0-9.]+)(ms|s)", output)
    expected = [
        "maximum last-row failure/rollback",
        "32 × nearly 1 MiB freeze, revision9→10",
    ]
    if [label for label, _, _ in samples] != expected:
        raise AssertionError("Expected both actual Rust freeze and rollback samples")
    return [float(value) * (1000 if unit == "s" else 1) for _, value, unit in samples]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("test_binary", type=Path)
    args = parser.parse_args()
    if resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss:
        raise AssertionError("Profile requires fresh child RSS accounting")
    with tempfile.TemporaryDirectory(prefix="mimik-campaign-profile-") as temporary:
        result = subprocess.run(
            [
                str(args.test_binary.resolve()),
                "--exact",
                "campaigns::tests::maximum_32_retained_sheets_digit_growth_headroom_and_atomic_freeze",
                "--nocapture",
            ],
            capture_output=True,
            text=True,
            timeout=120,
            env=smoke_environment(Path(temporary)),
        )
    if result.returncode != 0 or "test result: ok. 1 passed; 0 failed;" not in result.stdout:
        raise AssertionError("Actual release workload failed (diagnostics redacted)")
    samples = durations(result.stdout)
    rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    print(json.dumps({"transaction_samples_ms": samples, "whole_child_peak_rss_kib": rss}))
    if max(samples) > 5000 or rss > 131072:
        raise AssertionError("Reviewed sampled workload budget failed")


if __name__ == "__main__":
    main()
