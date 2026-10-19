# Review: assisted sheet MVP plan (2026-10-19)

**Scope.** `docs/assisted-sheet-plan.md`, the plan for the assisted paper sheet:
- lenient number input with sums;
- change notes, the "Did you mean" switch and Undo;
- unknown values;
- per-cell save on Enter and quick-add;
- a CPU model that interprets free text into confirmable proposals, bundles and reminders;
- catalogue name suggestions and "Which Sword?".

The record covers revisions 1 (`71a7841`), 2 (`cfd3475`) and 3 (`0921f6c`), and the rev 3.1
amendments. Plan review only. Each slice and step gets its own implementation review.

**Reviewers.**

| Revision | Sol (`--chain main`) | Sonnet (fresh subagent) | Third chain |
|---|---|---|---|
| rev 1 | NO-GO | GO WITH AMENDMENTS | unavailable |
| rev 2 | blocking R1–R3 | GO WITH AMENDMENTS | unavailable |
| rev 3 | GO WITH AMENDMENTS | GO WITH AMENDMENTS | unavailable |

The third chain was unavailable in every round. The work proceeded with the two main reviews,
as AGENTS.md allows. Blocking findings were adjudicated against the code before they were acted
on.

Rev 3.1 changes are amendments requested by the rev-3 reviews; slices touching them get
implementation review.

## Findings and disposition

The full map, from finding to resolution, section and test, is in plan §8. A summary by
revision:

| Rev | Blocking | Majors and minors | Disposition |
|---|---|---|---|
| 1 | Sol B1 and Sonnet B1: relative signs subtract again when negatives are re-posted. Sol B2: a stale proposal becomes a literal. Sol B3: the notes table lacks integrity | Sol M1–M14 (grammar, unresolved new rows, note lifecycle, job bounds, auth on poll, draft loss, Undo semantics, suggestion confinement and budget, duplicate names, CSP overwrite, i64 in JS, injection wording, no usable demo); Sonnet M1–M10 and minors | Rev 2: pool-only relative signs and an unchanged-cell rule; a dedicated apply route with no literal fallback; sealed note sets. Every Major got a contract and tests, or a deferral |
| 2 | Sol R1: an unrelated save rebases dirty drafts. Sol R2: freshness binds the target only. Sol R3: migration window on existing revisions | Sol M1–M7 (address collisions, the Enter contract, pool choices in lists, note representation, the null path, forced Undo, stale derived values), m1–m2; Sonnet M-A to M-H, m1–m8 | Rev 3: per-unit `base`; read/write-set and effect binding with single-use jobs. R3 moot under D-T (simple notes, limitation L1). D-Q (sign-led input is literal) removed the pool machinery. Step 1 sliced |
| 3 | none | Sol M1–M6 (same-unit draft generations, projection ordering, conflict semantics for Remove and Move, typed note values and clearing a scribble, nullable schema and `Null` results, contextual freshness), minors 1–2, slice-1.1 constraints; Sonnet M1–M5, m1–m13 | Rev 3.1 folds every amendment into its slice as a contract plus failing-first tests. Slice 1.1 is execution-ready. 1.5 is split into 1.5a and 1.5b. Sonnet M5 is recorded as release risk R7 (D-X2) |

**Accepted POC limitations** (plan §6):
- **L1:** notes are unsealed, so a process with DB write access could add notes to the current
  revision. Sealing is required before release.
- **L2:** without JavaScript, text typed but not entered is lost on reload.
- **L3:** value-based conflicts do not detect ABA changes.
- **R7:** history has no cap. A write-rate limit or storage guard is required before release.

## User decisions

Binding user decisions, recorded in plan §0:
- **D-A to D-P** (after rev 1). D-A and D-L were later superseded by D-Q.
- **D-Q to D-U** (after rev 2):
  - sign-led input is literal, with a "Did you mean" offer;
  - the "Which Sword?" choice is remembered by key;
  - each cell saves on Enter, with a quick-add line;
  - simple notes, with sealing later;
  - no history cap, and no-op saves write nothing.
- **D-V and D-W** (after rev 3): Remove happens at once with Undo; any decrease of a top-level
  number may trigger a reminder, with the model judging relevance.

**Main-session defaults**, recorded as such; the user may revise them:
- **D-X1:** an `erased` value is Undo-eligible.
- **D-X2:** no history cap in the POC; a rate limit or storage guard before release.
- **D-X3:** a bounded permit wait, then a styled 503 that keeps the draft.
- **D-X4:** the "Did you mean" offer stays on every number field, non-blocking and
  dismissible.

## Verification

Static reviews only. The reviewers read the plan and the source tree. No implementation,
migration, build, test, browser run or model inference was performed for this review. Slice
1.1 may start; its gate and implementation review are recorded with the slice.
