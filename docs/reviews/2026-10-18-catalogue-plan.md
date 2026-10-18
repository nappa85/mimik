# Review: read-only catalogue plan, revision 1 (2026-10-18)

**Scope.**
- `docs/catalogue-plan.md` rev 1 (`3b63274`), checked against `src/definitions/*`,
  `src/campaigns/*`, `src/sources/*`, `src/auth/*`, the migrations, the sheet templates and the
  contracts it cites.
- Prompt: the session scratchpad `catplan-prompt.md`.

**Reviewers.**
- **Sol** (`--chain main`): **NO-GO** as written; three Blocking findings.
- **Fresh Sonnet subagent:** **GO WITH AMENDMENTS**; no Blocking findings, many Major ones.
- **Space Bunny** (`--chain third`): **GO WITH AMENDMENTS**; three Blocking findings, and the
  amended plan needs re-review.
- All three reviews were static. No code, builds or inference.

**Overall.** All three accept the core: a separate content-addressed catalogue artifact,
optional definition linkage that leaves the frozen hash vectors untouched, string-valued
references with the formula profile unchanged, structural key validation (the same class as
enum options), and escaped plain-text rendering. The plan stays blocked until revision 2 is
re-reviewed.

## Blocking findings (adjudicated against the code: all confirmed)

| # | Source | Finding | Planned disposition (rev 2) |
|---|---|---|---|
| B1 | Sol 1, Sonnet F4, Space Bunny M2 | Showing the latest approval's materialised text to older pins doesn't reproduce what the player saw; withdrawn and never-approved behaviour is undefined | Drop materialisation (Sonnet F4): descriptions are a **live reference aid** read from the current effective unit text, labelled as such, with live badges. Rejected or withdrawn text is never shown. Document the divergence in the contract |
| B2 | Sol 2, Sonnet F9, Space Bunny M11 | Materialised rows not bound into the approval hash or transaction; ticket recomputation unspecified | Removed by B1's disposition. The remaining approval (references only) recomputes its ticket inside the IMMEDIATE transaction, with unit (id, revision, state, effective hash) in the comparison hash |
| B3 | Sol 3, Sonnet F7 | Sheet authorization alone does not confine `{catalogue}`; a guessed slug reaches another catalogue | Resolve catalogues only through the sheet's validated pin (exact version, hash and pair); uniform 404; cross-pin tests |
| B4 | Space Bunny B1, Sonnet F8 | D3 limits are unreachable with `decode.rs` (array 256, nodes 32,768, string 5,000, 256 KiB); raising `MAX_NODES` would silently raise the list budget (`values.rs:196`) | A separate catalogue decode profile with its own constants; `MAX_NODES` unchanged; regression `MAX_NODES == FORM_LIMITS.nodes`; reconcile per-entry and total caps |
| B5 | Space Bunny B2, Sol 5, Sonnet F2 | No aggregate bound on materialisation (16,384 units × 256 KiB) while holding the single `JOBS` permit; search work unbounded | Materialisation removed (B1). Bound per-request reads (first N bytes of effective text per entry page), reuse `Budget`/`interrupted`/`guarded` from `sources/search.rs`, and calibrate before the schema |
| B6 | Space Bunny B3, Sonnet F6, F19 | First player-visible manual-derived text, under an approval weaker than `--originals-compared`; no recorded user decision | **User decision required** (Q5 below): sharing of catalogue text with players, a per-catalogue shareable switch, and a "not compared with the original" badge. Amend `definition-publication.md` |

## Major findings (to apply in rev 2)

- **Relational storage.** Add entry and entry-unit tables filled at publication, with indexes,
  composite keys, replacement and append guards, and named triggers each with a regression.
  Sources: Sonnet F1, Sol 7, Space Bunny m11/m12.
- **Citation freshness.** Page label and completion changes need handling; "Checked" should
  require `complete = 1`. Sources: Sol 4, Sonnet F14, Space Bunny M1.
- **Lifecycle.** A status matrix and published-target reapproval commands. Sources: Sol 6,
  Sonnet F3/F17.
- **Editing.** Active sheets are always in edit mode, so the click-to-description path is
  needed there too, along with a no-JS inspect flow that preserves the draft. Sources: Sol 8,
  Space Bunny M7.
- **Picker.** Use `<select>` below a threshold, and a browse pick flow with an `op=` action
  above it. Sources: Sonnet F11/F21, Space Bunny M7.
- **Page budget.** Add a byte term for datalists and excerpts, and extend the three markup
  assertions. Sources: Sol 9, Sonnet F11, Space Bunny M5.
- **Validation path.** A bounded immutable key/type lookup for every validation path (reads,
  history, freeze), not a JSON-Schema enum, with a loader inside the read transaction.
  Sources: Sol 11, Space Bunny M3/M4.
- **Definition linkage checks.** Run them in `store::references` (draft save), publication and
  readiness, in the same transaction, and document the authoring order. Sources: Sonnet F10,
  Space Bunny M3.
- **History diff.** Keep the values-free invariant: show the key or name only via a link, or
  amend the contract explicitly. Sources: Sol 13, Sonnet F12, Space Bunny M13.
- **Search.** Reuse `fts_query`/`Budget`, cap `q`, use escaped matching, and order by
  `(name, key)`. Sources: Sonnet F13, Space Bunny M6.
- **Entry checks.** Validate entry `type` and the exact property key set. Source: Space Bunny M8.
- **Redacted output.** The CLI never prints names, owner text or descriptions, with sentinel
  tests. Sources: Sol 10, Sonnet F18.
- **Homebrew.** Hard rejection of unknown keys blocks homebrew items. **User decision** Q4.
  Source: Sonnet F5.
- **Calibration first.** Calibrate before the schema; slice the order so the catalogue,
  linkage and routes come before `catalogue_ref`, with publication gated until rendering is
  ready. Sources: Space Bunny M10, Sonnet F22, Sol order.

## Minor findings

These go into rev 2:
- §1 citation fixes: the cascade is in 007, not 009; "private sheet pages use no JS"; decoder
  allocation; envelope closure is not the reason.
- Canonical ordering and the unit-order definition; `(source, page, unit)` consistency.
- One key grammar (`registry::valid_id`).
- Option and source naming aligned with the definition envelope.
- Owner text cannot be corrected without a new version.
- Route path validation and bound parameters.
- Rate limits.
- The migration upgrade test.

**Verification.** Static only. Blocking findings B3–B5 were checked against
`snapshots.rs:271-286`, `decode.rs:9,21-37,175`, `values.rs:155,196` and `publication.rs:221-251`.

## Round 2: re-review of revision 2 (`2179a60`)

| Reviewer | Verdict |
|---|---|
| Sol | **NO-GO**. Blocking: the seal must bind the relational projections to the hashed content, because D9 skips membership checks on reads, history and freeze. Majors: FK/FTS deployability, the picker op grammar, picker form state, bounded prefix reads, cross-process contention, and the validated-pin boundary |
| Sonnet | **GO WITH AMENDMENTS**: op grammar, `q` admission, the Enter key, the entry-type table, per-request re-verification cost, the DB step in render/Pick, and minors |
| Third chain | **Unavailable**. Space Bunny, DeepSeek and Muse all failed preflight (timeouts). Proceeded with the two main reviews, as `AGENTS.md` allows |

All findings were applied as R1–R16 in revision 2.1 (`673fef6`, plan §7).

## Round 3: final check of revision 2.1 (`673fef6`)

| Reviewer | Verdict |
|---|---|
| Sol | **GO WITH AMENDMENTS**. R1 closes the Blocking finding, and the two-way equality is implementable in a SQLite trigger. No new Blocking issue. A1–A4 (submitters need `formnovalidate`, exact picker state and closing it on row ops, Pick sets presence, the second boundary extends `private_badge`), A5 (FTS integrity), and an R14 correction. "No further plan-review round is necessary" |
| Sonnet | **GO WITH AMENDMENTS**, no Blocking findings. The projections must carry source/page/kind/owner text/properties inside the seal equality; the seal SQL must use linear `EXCEPT`; seal ordering; an FTS surrogate id and rowid-range deletes; prefix-read details; R14 is wrong (wildcard arms); hidden button technique and form-pairs bump; readiness definition; calibration covers pin validation. It recommends a short check of the concrete trigger SQL |
| Third chain | Retried; see below |

All findings were applied as R17–R28 in revision 2.2 (`8d14f65`, plan §8).

**Status: the plan is cleared for step 0** (calibration spike, prototype tier, not merged).
- The concrete migration and seal-trigger SQL get their own independent review in step 2,
  which also serves as Sonnet's requested SQL check.
- The implementation of every step is reviewed before it is marked complete.
