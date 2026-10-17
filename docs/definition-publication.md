# Immutable definition publication and wiring approval

Implemented: local operator CLI publication, retained evidence snapshots, explicit
reapproval and withdrawal. Wiring approval records a human comparison of field
mappings with originals. Source-unit review, page completeness, deterministic game
rules, authenticated permissions and generic sheet capability remain separate.

## Finalize source artifacts first

New prepared-source imports verify and finalize the original PDF, extraction,
prepared manifest, page images, raw units and exact ruleset association. Finalized
artifacts and membership are immutable; corrections/review metadata stay editable
through the source-review workflow. An exact duplicate import preserves reviews
and returns the existing source ID. Legacy sources require exact reimport with
their original inputs before publication can use them. Import does not approve
evidence. Changed/corrupt stored artifacts cannot acquire finalization by reimport.

```sh
./mimik --database data/mimik.sqlite3 import-source --bundle /local/prepared --pdf /local/original.pdf
```

Source units have a 512 UTF-8-byte ID admission limit. Publication retains the
strict draft profile in `definition-drafts.md`, requiring at least one mapped
field and finalized source artifacts. Partial wiring is explicit: unresolved
fields remain listed and counted. A current wiring approval can capture unreviewed
units or incomplete pages; it does not turn those into reviewed rule evidence.

## Compare, inspect originals, then approve

Use the positive database draft ID, not its definition slug. Run the same binary
and database as the local server that serves the comparison's image URLs.

```sh
./mimik compare-definition --id 1
./mimik approve-definition --id 1 --comparison-hash HASH_FROM_COMPARISON --actor "Local reviewer" --rationale "Compared field mappings against original page and regions" --originals-compared
./mimik show-definition-version --id VERSION_ID_FROM_APPROVAL
```

The comparison returns a coherent, redacted snapshot and original page/region
URLs. Inspect those images against every used mapping before asserting
`--originals-compared`. URLs are relative to http://127.0.0.1:3000. CLI output does
not print manual text/corrections. Actor/rationale are operator-supplied local
labels, not authenticated attribution. Neither model confidence nor an API
review response can satisfy the human comparison requirement automatically.

The ticket binds target/revision, canonical definition hash, current evidence,
latest published version, latest approval and withdrawal. Approval recomputes it
inside an immediate transaction; any intervening change conflicts without a
partial publication or approval. Identical content to the latest version reuses
that immutable version and appends an approval. Changed content allocates the
next version. Draft changes never rewrite old versions.

## Formula definitions (M3)

Scalar formula definitions (`formula_profile: 1` with derived fields and constraints,
no list fields) publish like any other definition. The approval output reports
`deterministic_rules: "formulas-advisory"` and lists derived fields and constraints
whose rule evidence is still unresolved in `unresolved_formulas` and
`unresolved_constraints` (omitted when empty; formula-free output is unchanged).
Unresolved rule evidence does not block publication; it stays visible. Definitions with
list fields publish too (M4), provided their worst-case sheet form fits the publication
budget: at most 32,768 render units (one control, option or row element with its label;
lists counted full at 256 rows with every optional property present), 65,536 form pairs
and 32,768 stored decoder nodes; otherwise approval refuses `list-form-exceeds-limit`.
Only the render units bind under the admitted maxima; the pair and node counts are
redundant safeguards (plan `derived-formulas-m4-plan.md` D7).

Sheets pinned to a formula definition evaluate it on every view, save and history page,
from the stored values and the immutable pin. A stale or withdrawn approval does not stop
evaluation: the sheet keeps its existing warning and its advisory results. Results never
write a sheet, approve evidence or rule on legality.

## Invalidation, reapproval and withdrawal

`show-definition-version` reports `current`, `stale`, `withdrawn` or
`never_approved`. Captured unit state/correction/revision, page completeness,
printed label/revision and artifact identity are snapshot evidence. A review of
any unit on a captured page advances that page revision and invalidates wiring,
even when that unit was not mapped. Reviews on uncaptured pages do not invalidate
it. Returning text to its old value does not restore the old approval because
revisions advance. Read-time integrity errors are errors, not readiness states.

After comparing originals again, explicitly reapprove an immutable version:

```sh
./mimik compare-definition --id 1 --published
./mimik approve-definition --id 1 --published --comparison-hash NEW_HASH --actor "Local reviewer" --rationale "Recompared after source review" --originals-compared
./mimik withdraw-definition --id 1 --expected-approval CURRENT_APPROVAL_ID --actor "Local reviewer" --rationale "Wiring needs revision"
```

Withdrawal only targets the latest approval; it retains history and never falls
back to an older approval. A fresh comparison and explicit reapproval can append
a new approval after withdrawal. Old withdrawal tokens then conflict.

SQL guards enforce immutable version/approval/withdrawal rows, relational approval
targets, snapshot envelope identity, source artifact membership, review revision
advancement and completion prerequisites. Rust additionally validates canonical
bytes/hashes and retained provenance. This is ordinary-SQL integrity, not
protection against an administrator replacing the database or dropping guards.
Deleting an approval's referenced draft directly makes retained provenance fail;
no supported draft-deletion operation is provided.

## Bounds, concurrency and upgrades

Snapshots are capped at 1 MiB, 32 sources, 1,024 distinct mapped pages/units and
256 unresolved fields. Metadata/hash projections avoid returning manual text in
comparison output. Shared nonqueued admission remains held through cancelled
blocking workers. SQLite serializes cross-process writes; storage/busy failures
are surfaced. On conflict, obtain and inspect a fresh comparison; on busy, retry
after the competing operation finishes, then compare again. Do not blindly reuse
an old ticket. CLI integers must retain exact i64 precision.

Schema7 adds finalization, schema8 retained publication and schema9 retrofits the
review/approval integrity guards into interrupted-session schema7/8 databases.
Migration admission is serialized and reads the version inside an immediate
transaction. Upgrades are atomic and preserve existing artifacts/reviews/history;
they do not automatically finalize legacy sources or approve evidence. Older
binaries reject the newer schema. Back up local data before upgrades.

Verification and supported artifact scope: `release-scope.md` and
`reviews/2026-10-13-definition-publication.md`. Synthetic checks do not establish
real-manual fidelity, generic editing, campaign authorization or provider support.
