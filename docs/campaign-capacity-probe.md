# Stage-C campaign capacity: preliminary freeze probe

Date: 2026-10-13. **Research evidence only. Recommend carrying 32 sheets per
custodian/campaign into the concrete Stage-C plan as a provisional cap.** This
is not a final capacity decision, Rust performance gate, security certification,
or authorization/history implementation. The 64-sheet comparison increases
transaction time and history growth substantially; it does not establish that
64 is unsafe. No latency objective was supplied or certified.

## Candidate numeric limits

| Resource | Proposed cap | Evidence/status |
| --- | ---: | --- |
| Retained sheets per custodian/campaign | 32 | Measured synthetic departure workload; provisional |
| Members per campaign | 64 | Planning proposal; member admission/list scaling not measured |
| Campaigns per instance | 128 | Planning proposal; instance saturation not measured |
| Campaigns created per user | 32 | Planning proposal; creator admission not measured |
| List page | 50 summaries | Planning proposal; list queries/rendering not measured |
| History page | 50 summaries | Planning proposal; summary queries/rendering not measured |
| Full snapshot response | At most 1, ≤1,048,576 canonical UTF-8 bytes | Probe enforces byte cap; HTTP reads not measured |
| Canonical fields | Flat object, ≤256 fields; multiline string ≤20,000 Unicode scalars | Fixture exercises 256 fields and 20,000-scalar values |

Counts must have explicit semantics in the actual plan: proposed sheet cap
counts retained campaign-linked sheets, including frozen sheets, so departure
cannot accumulate an unbounded set behind an editable-only count. Transfers,
deletion/restoration and campaign/member retirement must define reservation and
release semantics before they ship. This fixture's retained-sheet and
created-campaign counts stay unchanged; the active-member count correctly drops
from 2 to 1. Membership deactivation does not delete earlier history.

These caps **do not bound instance storage**. Even the simple product
128 campaigns × 64 custodians × 32 sheets × 1 MiB is **256 GiB** of current
payload, before creation history, indices, journals, or later revisions. History
growth is unbounded without an explicit storage/retention/admission policy;
50 summaries is a page cap, not a retention cap. This probe measures one
custodian leaving one campaign, not that product.

## Isolation and reproducibility

- Fresh local ignored directory: `target/campaign-capacity-20261013/`.
  `ls target docs` verified parents before creation; `git check-ignore` confirmed
  target exclusion. Only the assigned directory and this report were written.
- Existing local `mimik-ci` image ID:
  `sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f`.
  Python **3.12.12**, SQLite **3.50.4**. No dependency/tool installation.
- Container network disabled, read-only root, all capabilities dropped,
  no-new-privileges; only this fresh directory bind-mounted at `/probe`.
  No application, owner data, manuals, `.env`, credentials or other workspace
  directories mounted. Child environment allowlists PATH and bytecode-disable.
- Non-root Python startup was unavailable: the existing venv resolves under
  `/root/.local/share/uv`. Used image-default root with dropped capabilities,
  host supplementary group, probe-directory mode **2770**, umask **002**.
  Outputs are root-owned/host-group-readable and writable. The host UID owns
  the directory. No host tool installation or image modification was made.
- Bind backing filesystem: `/home`, `/dev/sda3`, **ext4**, mount options
  `rw,nosuid,nodev,relatime`. Timings depend on this host/filesystem, cache state
  and concurrent load; they are not portable fsync guarantees.
- Preflight free bytes: **21,589,053,440** (required ≥3 GiB). Retained database
  bytes for all 14 workloads: **1,268,842,496**. No raw databases cleaned up.
- SQLite: page size **4096**, cache **2048 KiB** (`cache_size=-2048`),
  `mmap_size=0`, `temp_store=MEMORY`, `journal_mode=DELETE`,
  `synchronous=FULL` (2), foreign keys ON. This is a prototype schema;
  shipping WAL/schema/connection/cache choices require fresh measurement.

Exact run command (from the project root; runner refuses to overwrite a completed
matrix, and each child refuses an existing database):

```sh
docker run --rm --network=none --read-only --cap-drop=ALL \
  --security-opt=no-new-privileges --group-add "$(id -g)" --workdir /probe \
  --mount "type=bind,src=$PWD/target/campaign-capacity-20261013,dst=/probe" \
  sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f \
  bash -c 'umask 002 && python -B /probe/probe.py'
```

For a rerun, allocate a new owned directory/copy and fresh filenames; do not
remove existing evidence. The recorded matrix used fixed seed **20261013** to
shuffle three fresh-process runs each of 8/16/32/64 sheets, followed by two
32-sheet rollback controls. Samples ran sequentially, without explicit CPU or
memory cgroup limits. Wrapper subprocess timeout is 180 seconds per workload.

## Fixture and transaction

Python stdlib `sqlite3`, `hashlib`, `json`, `resource` and `subprocess` are used
for this extraction/evaluation prototype. No shipping infrastructure is copied
or replaced. Every complete synthetic snapshot includes identity, ruleset,
immutable definition version/hash pin, sheet/campaign/custodian IDs, state,
revision and flat fields. This is a **synthetic canonical format**, not a claim
to match the eventual Rust sheet schema.

Snapshots before and after freeze are **exactly 1,048,576 bytes**. One ASCII
field has 20,000 scalars; 17 fields each contain 19,999 CJK scalars and a newline;
remaining fields are short multiline strings and one dedicated ASCII padding
field. Total field count is 256. Padding is recomputed after state/revision
changes; all other domain fields are preserved. Strings use strict UTF-8,
reject surrogates, and are not normalized. Scalar counts use Python `len(str)`
after strict encoding; newline escaping counts toward canonical bytes.

Canonical bytes use sorted keys, compact separators, `ensure_ascii=False` and
`allow_nan=False`. Proposed versioned-domain hash:

```text
SHA256(UTF8("mimik:campaign-sheet-snapshot:v1") || 0x00 || canonical_UTF8)
prefix length: 33 bytes
prefix hex: 6d696d696b3a63616d706169676e2d73686565742d736e617073686f743a763100
golden canonical bytes: {"a":"x","n":1}
golden SHA256: e6c18c4356a467441e884698a6bfc1142b22b462e148caf2e944f3f21bff6935
```

The 1 MiB cap excludes the prefix. This is a probe hash proposal, not approval
of a final domain protocol. Each child verifies the fixed golden vector.

Setup creates live revision-1 rows and one complete creation-history snapshot
per sheet. A single `BEGIN IMMEDIATE` transaction deactivates membership,
decrements active-member count, freezes every selected sheet, advances revision
1→2, hashes the full canonical post-state and inserts departure history with
synthetic actor 2. Integer IDs alone are materialized after BEGIN; each payload
is read by primary key, cursor closed, then updated. No list of full payloads
is accumulated. The timed interval includes Python parse/validation/padding/
serialization/hash, SQLite updates/inserts and COMMIT; it excludes setup and
post-commit verification. Process time includes setup, verification and reopen.

All RSS readings use **the fresh child's self metric**, Linux `ru_maxrss` in
KiB, never wrapper `RUSAGE_CHILDREN` or container memory. Linux exec may inherit
the pre-exec high-water mark, so this metric can include a small launcher floor;
that floor was not separately measured. Whole-child peaks include setup,
transaction, logical hashing and
verification. Before/after readings are peak-so-far high-water marks, not
current RSS, incremental allocation, or transaction-only memory. No RSS
scaling inference is drawn from their near-flat values. Main DB bytes are final
file size, not peak storage or journal size.

Raw result constants (`field_count`, scalar/byte caps, retained counts, verification
flags) are **asserted invariants**, not independent instrumentation readings.
Timings, resource high-water marks and file sizes are measured values. Fixture
Unicode covers ASCII/CJK/newlines, not 4-byte astral characters; include emoji
and Rust/JavaScript scalar-count differences in the final cross-language gate.

## Measured results

Three success samples per size; min/median/max, rounded for presentation:

| Sheets | Transaction wall ms min / median / max | CPU ms median | Whole-process wall ms median | Child peak RSS KiB min / median / max |
| ---: | --- | ---: | ---: | --- |
| 8 | 251.692 / 295.220 / 306.688 | 174.422 | 1,650.263 | 40,740 / 41,032 / 41,056 |
| 16 | 551.253 / 579.032 / 639.401 | 362.591 | 2,863.770 | 40,056 / 40,116 / 41,220 |
| 32 | 961.262 / 1,174.740 / 1,276.201 | 743.613 | 5,366.832 | 40,852 / 40,868 / 41,260 |
| 64 | 1,997.519 / 2,022.382 / 2,263.202 | 1,405.248 | 10,016.115 | 41,052 / 41,260 / 41,504 |

| Sheets | DB bytes before | DB bytes after | DB growth bytes | Post-freeze history rows |
| ---: | ---: | ---: | ---: | ---: |
| 8 | 16,834,560 | 25,235,456 | 8,400,896 | 16 |
| 16 | 33,636,352 | 50,434,048 | 16,797,696 | 32 |
| 32 | 67,231,744 | 100,831,232 | 33,599,488 | 64 |
| 64 | 134,430,720 | 201,625,600 | 67,194,880 | 128 |

All success DBs have zero freelist pages. Page counts are 6,161 / 12,313 /
24,617 / 49,225 respectively. At 32, post-DB is about **96.16 MiB**, departure
adds about **32.04 MiB**, and measured transaction holds the write transaction
for about **0.96–1.28 seconds**. At 64, median wall is **1.722×** the 32 median,
and DB growth approximately doubles. These are 3-sample observations, not bounds,
percentiles or a sub-second responsiveness promise.

## Atomicity and retained evidence

All 12 successes verified every current and history row, canonical byte
round-trip, exact fixture semantics/pins, revisions, hashes and attribution,
preserved original creation rows and retained-sheet/creator counts, and expected
membership/live-member changes. Checks repeated after closing/reopening SQLite;
integrity/FK checks passed and no journal remained.

Both rollback controls inject **only at sheet 32**, after 31 sheets have already
been updated and 31 departure history rows inserted. Python counters and SQL
queries confirm those preceding mutations and membership deactivation before
rollback:

| Fault | Distinct rejection | Transaction through rollback ms | Child peak KiB | Final history rows | DB bytes/growth |
| --- | --- | ---: | ---: | ---: | --- |
| Exactly 1,048,577 canonical bytes, fields still valid | `snapshot_bytes` | 887.025 | 41,224 | 32 | 67,231,744 / 0 |
| One 20,001-scalar field, repadded to exactly 1 MiB | `field_scalars` | 888.850 | 40,596 | 32 | 67,231,744 / 0 |

Each hook fired exactly once. Both restored ordered logical database SHA256
`2c3157e665ef53ab18857f702e788da024e2da906ca363c326ec317070d350ba` exactly,
including schema, user_version and all tables, before/after rollback and after
reopen. Membership is active again, every revision is 1/unfrozen, creation
history and all counts remain. Page count 16,414; freelist 0. This verifies
exception rollback, not power-loss/crash durability or interprocess races.

Raw paths (all local, ignored, synthetic):

- `target/campaign-capacity-20261013/probe.py`: fixture, workload and retained assertions.
- `target/campaign-capacity-20261013/results.json`: all 14 raw samples in run order.
- `target/campaign-capacity-20261013/summary.json`: derived medians/ranges; `summarize.py` reproduces them.
- `target/campaign-capacity-20261013/n{8,16,32,64}-success-r{1,2,3}.json` and matching `.sqlite3`: individual evidence.
- `target/campaign-capacity-20261013/n32-{overflow,badrow}-r1.json` and matching `.sqlite3`: rollback evidence.
- `target/campaign-capacity-20261013/plan.md`, `review-plan.md`, `review-implementation.md`: reviewed scope/dispositions.
- `target/campaign-capacity-20261013/test_probe.py`: retained failed-child diagnostic regression.

## Review, verification and handoff

- [x] Fresh independent pre-code plan review GO after two rounds of blocking
  clarifications: closed scans, exact NUL/hash vector, isolated late rejection
  cases, exact post-padding and correct peak labels. CLI review records are
  explicit summaries, not verbatim transcripts.
- [x] 12 success workloads and two rollback controls passed in the existing CI image.
- [x] One local diagnostics regression failed first (captured child stderr was
  absent from the displayed exception), then passed after wrapper diagnostics
  were surfaced. Ruff default checks passed for the probe files. These are local
  prototype checks, not application strict-lint/CI integration.
- [x] Fresh independent material implementation/results/report review GO;
  nonblocking caveats/dispositions recorded in `review-implementation.md`.
- [ ] Final Stage-C Rust capacity choice and actual implementation gates.

Initial startup failures produced no database: host-UID Python was inaccessible,
and dropped-capability root lacked permission on the initial 0755 directory.
The group-scoped permission setup above resolved startup. Measurement code was
unchanged after samples; the only subsequent runner fix surfaces failing child
stdout/stderr and does not change successful child measurement behavior.

**Recommendation/handoff:** carry the 32/64/128/32/50/50/1-MiB proposal into the
independently reviewed concrete plan, labeling all unmeasured caps accordingly.
Choose 32 finally only after the real Rust schema/serializer/hash domain,
SQLite mode/indexes and authenticated departure path verify admission limits,
all-or-nothing freeze/history/count updates, malformed-row rejection, stale
revision/race/access cases, bounded summary queries and one-snapshot reads.
Define the numeric transaction-latency/memory/disk acceptance budget before that
gate; this probe cannot establish one. Preserving 32 rather than doubling to 64
is a conservative workload choice given the observed 1.17s versus 2.02s medians
and 32 versus 64 MiB history growth, not proof of acceptable service latency.

Campaign-wide retirement is a separate workload: 64 × 32 could affect **2,048
sheets**, or **2 GiB** of full snapshot payload, before journal/history overhead.
No campaign-wide operation may inherit this one-custodian evidence as approval.
The local digest is an equality check for this fixed fixture schema, not a
general typed database commitment (table/type framing is not universal).

User-owned-path scope excludes `TODO.md` and `docs/reviews/`; checklist, blockers
and handoff are recorded here and under the assigned target path instead.
No application/provider/browser/manual tests, security certification or shipping
release checks are claimed by this research-only task.
