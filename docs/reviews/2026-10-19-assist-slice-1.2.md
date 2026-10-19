# Review: assisted sheet slice 1.2, notes and scribbles (2026-10-19)

**Scope.** Commit `a985be5` on branch `assist-1.2` (parent master `cccd9ff`). Specification:
`docs/assisted-sheet-plan.md` rev 3.1 §0 (D-E, D-M, D-T, D-U), the slice 1.2 section and §8;
implementation notes, deviations N1–N9, red→green and gates in §10. Contents: migration 020
(`private_sheet_notes`, insert-only triggers bound to the live revision and its actor, 256-note
cap), `src/campaigns/notes.rs`, note recording in `values::form`/`reconcile`, scribbles over known
integers, no-op saves (`Store::private_unchanged`), notes in the save's IMMEDIATE transaction,
display under cells and on history pages, the slice-1.1 follow-ups.

**Reviewers.**

| Reviewer | Verdict |
|---|---|
| Sol (`--chain main`) | MERGE-READY |
| Sonnet (fresh subagent; static plus a scratch-SQLite probe of the triggers) | MERGE-READY |
| Third chain | unavailable |

Both confirmed: notes cannot be inserted for a non-live revision or another actor, replaced,
updated or deleted; a failed save leaves no notes; the no-op path rechecks auth, edit rights,
state and revision; raw text is escaped and isolated in `<bdi>`; L1 (unsealed notes) is
accurately scoped. N1–N9 accepted; the N8 test adaptations are not weakened.

## Findings and disposition

| # | Source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | Sol | Minor | The browser no-op check does not compare the revision before and after | Follow-up (slice 1.3); router test `notes_tests.rs:435-504` already pins it |
| 2 | Sol | Minor | The no-op race test covers loss of edit rights only, not submitting-session invalidation or an intervening revision | Follow-up (slice 1.3): two hook-driven cases |
| 3 | Sonnet | Minor | `address` length CHECK ≥ 7 bytes refuses 6-byte `meta:` addresses (probe-verified) | Follow-up in the slice that first writes `meta:` addresses (1.5a), via a new migration |
| 4 | Sonnet | Minor | Adapted CRLF/leading-newline tests no longer pin LF normalisation on a changed multiline save | Follow-up (slice 1.3): one edit-plus-CRLF test |
| 5 | Sonnet | Note | The no-op path skips `admitted.unchanged(db)` (pin freshness); `authorize_edit` still refuses unsupported pins | Recorded |
| 6 | Sonnet | Note | L1 is wider than N7 says: any process that can write the database can forge notes, and a malformed latest note hides a real scribble | Recorded under L1 (sealing before release) |
| 7 | Sonnet | Note | The upgrade test now uses `SCHEMA_VERSION` instead of the pinned 19 | Accepted |

**Verification.** Author gates in plan §10 (Monday 2026-10-19, `--cpus 2`): fmt, pedantic Clippy,
611 Rust tests passed, ruff, tools unittest 762 OK, release build, smoke and browser smoke (with a
negative control), actionlint, `cargo audit --no-fetch`, `cargo about`, source package, vendor
archives, unpacked smoke and Chromium, `lan_smoke.py` (now self-capping), demo setup. Not run:
offline corresponding-source rebuild; a hand-driven demo session. Reviews were static.
