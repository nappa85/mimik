"""Sample Linux definition admission costs with a fresh peak-RSS wrapper per case."""

import argparse
import copy
import json
import re
import resource
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def measured(binary, database, source, expected_success):
    command = [str(binary), "--database", str(database)]
    command += (
        ["import-definition-draft", "--file", str(source), "--expected-revision", "0"]
        if source
        else ["list-rulesets"]
    )
    started = time.perf_counter()
    result = subprocess.run(command, capture_output=True, timeout=30)
    if (result.returncode == 0) != expected_success:
        raise AssertionError("Admission outcome differs from profile expectation")
    if not expected_success:
        # Deliberately coupled to this CLI's current Debug error representation.
        # A format change must fail the probe rather than mislabel another failure.
        report = re.fullmatch(
            rb"Error: Invalid\(ValidationReport \{ diagnostics: \[(.*?)\], truncated: (?:true|false) \}\)\s*",
            result.stderr,
            flags=re.DOTALL,
        )
        diagnostic = b'Diagnostic { path: "/fields/0/nullable", code: "type" }'
        if result.returncode != 1 or report is None or diagnostic not in report.group(1):
            raise AssertionError(
                "Expected nullable/type validation report, not an unrelated failure"
            )
    print(
        json.dumps(
            {
                "process_ms": round((time.perf_counter() - started) * 1000, 3),
                "peak_child_rss_kib": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,
                "accepted": result.returncode == 0,
                "input_bytes": source.stat().st_size if source else 0,
            }
        )
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--example", type=Path)
    parser.add_argument("--measure", action="store_true")
    parser.add_argument("--database", type=Path)
    parser.add_argument("--file", type=Path)
    parser.add_argument("--reject", action="store_true")
    args = parser.parse_args()
    if args.measure:
        measured(args.binary, args.database, args.file, not args.reject)
        return
    example = json.loads(args.example.read_text(encoding="utf-8"))
    try:
        fixture_valid = (
            example["contract_version"] == 1
            and example["sources"] == []
            and len(example["fields"]) == 1
            and len(example["groups"]) == 1
            and example["groups"][0]["fields"] == [example["fields"][0]["id"]]
            and example["fields"][0]["mapping"]["state"] == "unresolved"
        )
    except (KeyError, TypeError, IndexError):
        fixture_valid = False
    if not fixture_valid:
        raise AssertionError(
            "Profile fixture requires contract1, empty sources and one unresolved field/group"
        )
    with tempfile.TemporaryDirectory(prefix="mimik-definition-profile-") as directory:
        database = Path(directory) / "profile.sqlite3"
        subprocess.run(
            [
                str(args.binary),
                "--database",
                str(database),
                "register-ruleset",
                "--game-id",
                example["game_id"],
                "--game-label",
                "Synthetic game",
                "--edition-id",
                example["edition_id"],
                "--edition-label",
                "First edition",
            ],
            check=True,
            capture_output=True,
            timeout=30,
        )
        cardinality = copy.deepcopy(example)
        cardinality["definition_id"] = "cardinality"
        cardinality["fields"] = []
        cardinality["groups"][0]["fields"] = []
        for index in range(256):
            field = copy.deepcopy(example["fields"][0])
            field.update(id=f"field-{index}", help=None, options=None)
            if index < 16:
                field.update(
                    control="enum",
                    options=[
                        {"id": f"option-{choice}", "label": f"Choice {choice}"}
                        for choice in range(128)
                    ],
                )
            cardinality["fields"].append(field)
            cardinality["groups"][0]["fields"].append(field["id"])
        byte_bound = copy.deepcopy(cardinality)
        byte_bound["definition_id"] = "byte-bound"
        byte_bound["fields"] = byte_bound["fields"][:128]
        byte_bound["groups"][0]["fields"] = byte_bound["groups"][0]["fields"][:128]
        for length in range(2000, -1, -1):
            for field in byte_bound["fields"]:
                field.update(control="text", options=None, help="x" * length)
            if len(json.dumps(byte_bound, separators=(",", ":")).encode()) <= 261_000:
                break
        else:
            raise AssertionError("Profile template cannot fit the bounded canonical sample")
        invalid = copy.deepcopy(cardinality)
        invalid["definition_id"] = "invalid"
        for field in invalid["fields"]:
            field["nullable"] = "not-a-boolean"
        cases = [
            ("baseline", None, False),
            ("cardinality", cardinality, False),
            ("byte-bound", byte_bound, False),
            ("invalid", invalid, True),
        ]
        for label, value, reject in cases:
            command = [
                sys.executable,
                str(Path(__file__).resolve()),
                str(args.binary),
                "--measure",
                "--database",
                str(database),
            ]
            if value is not None:
                source = Path(directory) / f"{label}.json"
                source.write_text(json.dumps(value, separators=(",", ":")), encoding="utf-8")
                command += ["--file", str(source)]
            if reject:
                command.append("--reject")
            result = subprocess.run(command, capture_output=True, text=True, check=True, timeout=40)
            print(label, result.stdout.strip())
        print("release_binary_bytes", args.binary.stat().st_size)


if __name__ == "__main__":
    main()
