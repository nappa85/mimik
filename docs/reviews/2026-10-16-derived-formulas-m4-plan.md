# Review record: derived formulas M4 plan

2026-10-16. Scope: [derived-formulas-m4-plan.md](../derived-formulas-m4-plan.md). Round 1
reviewed revision 1 (draft `9c79436`) against master `1ddbf17`; the plan was rewritten as
revision 2 (`1b8baa8`). Round 2 re-reviewed revision 2; revision 3 also took in two changed user decisions.
Round 3 re-reviewed revision 3's new scope; the plan is now revision 3.1. Documentation only: no code, builds, tests or containers
were run for this record. Test numbers below are revision 3's.

## Independence

- Planner: a read-only Opus 5.5 subagent. Reviser and adjudicator (both rounds): a separate
  Opus 5.5 subagent, which checked every finding against the code read-only.
- **Round 1** (revision 1), in parallel with fresh contexts:
  - OpenAI Sol (`openai/gpt-6.1-sol`, `--chain main`): **NO-GO**. Six blocking, five medium and
    one low finding.
  - Sonnet 5.5 subagent: **GO-with-amendments**, R1–R15 plus mutation gaps.
- **Round 2** (revision 2), in parallel with fresh contexts:
  - Sol: **GO-with-amendments**, RR1–RR5 (four medium, one low). It confirmed every round-1
    finding resolved in plan text, with M4 partly resolved (RR4).
  - Sonnet: **GO-with-amendments**, N1–N10.
- Third reviewer (`--chain third`): **unavailable in both rounds.** Space Bunny, DeepSeek v4.1
  Flash and Muse Spark failed preflight. Per `AGENTS.md`, the review proceeds with the two main
  reviews; retry at implementation review.
- User decisions:
  - round 1: M4 ships plain list editing with every list property editable; a later milestone
    adds a per-property `editable: false` flag and an item catalogue (revision 1's D11, Q7 and
    test 29 removed; plan §8 added);
  - round 2: Q1 draft ops with Save persisting, as planned; **Q2 changed**, Move up and down
    in M4 with tests and accessibility (edge buttons, focus after an op, 390 px layout); Q3
    5 MiB, as planned; **Q4 changed**, a read-only per-revision change summary in history
    (rows added, removed, edited with properties, moved, plus scalar field changes), host-side,
    `<list>/<entry_id>/<property>` addressing, a defined display for reused slots, escaped,
    bounded, render-budgeted and tested, with no revert action.

## Round 1 findings (revision 1)

Verdicts: valid; valid-partial (the core is right, and a part is wrong or moot); wrong.

| ID | Reviewer | Sev. | Verdict and evidence | Disposition (as carried into revision 3) |
|---|---|---|---|---|
| B1 | Sol | Blocking | **Valid.** `ItemProperty` is `deny_unknown_fields` with no `editable` (`definitions/mod.rs:173-182`); the schema's `item_property` has `additionalProperties:false` (`definition-v1.json:72-79`); the translator drops `editable` (`formula_pack_tests.rs:54-69`), although the pack marks `conditions.kind` non-editable. | Removed from M4 (user decision); §8 lists the full flag contract for the later milestone. |
| B2 | Sol | Blocking | **Valid.** New-row provenance of immutable stats has no source; the adjudication needs a rule or `ask_rule` (`tool-loop-pack-adjudication-2026-10-16.md:112-121`). | Deferred with the catalogue (§8). M4 claims neither immutability nor an acquisition workflow. |
| B3 | Sol | Blocking | **Valid.** `encode` checks only the byte count (`snapshots.rs:50-56`, reserve `:326-334`); `decode` uses `parse_sheet` with a 32,768-node cap (`decode.rs:38`). Four lists × 256 rows × 32 integers = 34,820 nodes at about 250 KB. | D7: `encode` re-parses its own output and the reserve (→ 422, atomic); the node budget shares `decode::MAX_NODES`. Tests 5, 6, 44–46. |
| B4 | Sol | Blocking | **Valid.** `admit` runs `keys.contains` per pair (`transport.rs:54`), O(n·m), on the async worker (`web.rs:815`). | D2: `HashSet` admission in its own `spawn_blocking` step; indices bounded first; canonical grammar. Tests 7–9. |
| B5 | Sol | Blocking | **Valid.** The admitted maxima allow ≈ 2.1 M `<option>`s; the old pair cap did not count options. | D7 render-unit budget, the only binding count (RR2). Tests 44–45; §6 page measurements. |
| B6 | Sol | Blocking | **Valid.** Name is `required` (`private_sheet.html:6`), so an op button without `formnovalidate` never POSTs on an empty create page. | D5 `formnovalidate` on every op button; test 25; browser stage (step 8). |
| M1 | Sol | Medium | **Valid.** Values are validated before the revision check (`web.rs:821-831`). | D4 precedence; re-renders keep the submitted revision. Tests 10, 18, 24. |
| M2 | Sol | Medium | **Valid.** `k` and the scope were undefined; freeze reuses the writer. | D3 allocation, scope and invariant placement. Tests 1–4, 47. |
| M3 | Sol | Medium | **Valid.** The lift was combined with other work. | §5: the lift is step 8 alone, committed only after the full gate set passes on the uncommitted candidate; all bypass parts deleted. |
| M4 | Sol | Medium | **Valid.** `submitted` ignores buttons and disabled controls (`formula_tests.rs:794-827`); many negatives were red only because lists were unsupported. | §4 preamble: paired positive baselines, guards named with their killing mutation, helper rebuilt in step 3 (test 16). Tests 7, 21, 22, 45. Completed by RR4 and N5. |
| M5 | Sol | Medium | **Valid-partial.** Contextual witnesses and the two-Save locator clash are right. The `formaction` part is moot: it is built from numeric indices only. | D11 contextual witnesses with DOM assertions; `formaction` asserted equal; D6 top Save on list sheets only. |
| L1 | Sol | Low | **Valid.** Five factual corrections confirmed. | §1 rewritten; D8 address uses list, entry and property IDs. |
| R1 | Sonnet | Blocking | **Valid** (same as B1). | §8, user decision. |
| R2 | Sonnet | Medium | **Valid.** IDs label slots. | D3 slot semantics; test 3 pins the accepted swap; D8 shows a reused slot. |
| R3 | Sonnet | Medium | **Valid.** | D3: runtime writer, `create` and `edit` only. Tests 1 and 4. |
| R4 | Sonnet | Medium | **Valid.** | D1 grammar and D2 step 1. Test 7. |
| R5 | Sonnet | Medium | **Valid.** The 1 MiB snapshot cap is `snapshots.rs:52`. | D7 derivation of 5 MiB on the create and edit POST methods; tests 39 and 41. The round-1 "GET unchanged" assertion is withdrawn (RR3). |
| R6 | Sonnet | Medium | **Valid-partial.** Scroll and focus needed definition; `no-store` is already global (`auth/mod.rs:426-429`). | D5 focus rules; test 25; test 19 guards `no-store`. |
| R7 | Sonnet | Medium | **Valid.** | D4 step 4 and D5. Tests 23 and 24. |
| R8 | Sonnet | Medium | **Valid.** | Test 17. |
| R9 | Sonnet | Medium | **Valid.** | D11 attribute contexts on every page. Test 36. |
| R10 | Sonnet | Medium | **Valid.** Strict clippy denies `dead_code`. | §5 names each item's production caller; `form_budget` lands with `publishable` in step 8. |
| R11 | Sonnet | Low | **Valid.** | §1; D8 address grammar; test 28. |
| R12 | Sonnet | Low | **Valid.** Edge buttons were moot under revision 2; with reorder now in M4 they are in scope. | D5 edges, focus and 390 px layout; tests 21, 25; browser stage. |
| R13 | Sonnet | Low | **Valid.** | Step 6; test 38. |
| R14 | Sonnet | Low | **Valid** recommendation, **superseded by the user**: reorder is in M4. | D5 moves; tests 20–21, 25–26. |
| R15 | Sonnet | Low | **Valid-partial.** | §4 names each failing assertion (RR4); the step 9 table records each executed red. |
| Mut. | Sonnet | — | **Valid.** | §4 mutation table, each with its killing test. |
| A1 | Adjudicator | Medium | **New, valid.** `auth` replaces 4xx documents unless the response carries `PrivateTrustedEditor` (`auth/mod.rs:249-266`). | D5: the Add-at-256 422 goes through `rendered`; test 22. |

Round 1 rejected two parts, no finding: Sol M5's hostile-text-in-`formaction` item and Sonnet
R6's `no-store` requirement (already met, kept as a guard).

## Round 2 findings (revision 2)

| ID | Reviewer | Sev. | Verdict and evidence | Disposition (revision 3) |
|---|---|---|---|---|
| RR1 | Sol | Medium | **Valid.** Tests 12 and 20 of revision 2 (now 13 and 17) needed the submitter-aware helper in step 3, but `scraper` landed in step 5. | D11 and step 3: the parser, the fetch, the licence entry and the helper land first in step 3 (test 16). §4 states that tests 1–11 use direct fixtures. |
| RR2 | Sol | Medium | **Valid.** Recomputed: a list row costs 4 + p + 2·opt units, 1 + p + opt pairs and 2 + p nodes, so pairs ≤ units + 5 and units > nodes for any list definition while `ENVELOPE_NODES` < 514. Only render units can bind. | D7: `within(counts, limits)` tested per term with test limits (test 44); real publication at the render boundary (test 45); a dominance test; pairs and nodes documented as redundant safeguards; `ENVELOPE_NODES` pinned by a counted measurement, not a boundary. |
| RR3 | Sol | Medium | **Valid.** `DefaultBodyLimit` acts through extractors; `new_sheet`, `read_sheet` and `read_history` extract no body (`web.rs:672-725`). | The GET assertion and the "5 MiB on GETs" mutation are removed. D7 specifies per-method layering; test 39 checks the POST limit and the 8 KiB non-sheet routes. |
| RR4 | Sol | Medium | **Valid.** The current single Save is already the first submitter (`private_sheet.html:9`), and test 9 could fail at the 516 parser cap. | Test 17 asserts `op=save` and its position before the groups; test 9 builds `Pairs` with a test constructor that bypasses the parser. Generic "no ops" and "same" entries replaced by exact assertions. |
| RR5 | Sol | Low | **Valid.** Steps 1 and 2 change shared paths. | §5 claims only that list publication stays disabled until step 8, and lists the shared-path changes of steps 1, 2 and 7. |
| N1 | Sonnet | Medium | **Valid.** The 516 cap is a literal in the visitor (`transport.rs:25`); `serde_urlencoded::from_bytes` takes no seed, but `serde_urlencoded::Deserializer::new(form_urlencoded::parse(..))` is public in 0.7.1. | D2: `PairsSeed` (`DeserializeSeed`) through that deserializer; `form_urlencoded` direct at the locked 1.2.2; test 8. |
| N2 | Sonnet | Medium | **Valid.** Admission returning a `StatusCode` cannot share the validation closure, whose errors go to `rejected_editor`. CSRF already precedes admission today. | D2: its own blocking step after `load_editor` and `editable`; test 11 (403 for bad CSRF or no rights with a bad key set), a guard killed by the mutation "admission moved before CSRF". |
| N3 | Sonnet | Medium | **Valid.** No principal check precedes body buffering; CSRF needs the body; the JOBS permit is taken before the body (`web.rs:42-55`); the 5 s `boundary` timeout bounds the hold (`auth/mod.rs:217`). Keeping a 1 MiB limit until CSRF passes would need a streaming parser. | D7: a pre-body principal check on the 5 MiB POST methods (test 40), and the residual risk (a logged-in principal, ≤ 5 MiB, ≤ 5 s, JOBS held) recorded as accepted and measured in §6. |
| N4 | Sonnet | Medium | **Valid.** Revision 2's test 15 used a hand-built fixture, but the exact-budget save/reopen/freeze needs `form_budget`. | Test 46, its own row in step 8. |
| N5 | Sonnet | Low | **Valid.** Revision 2's test 7 "baseline 400" contradicted the preamble. | Test 7: the positive case is the failing-first test; the negatives are guards before step 2, run against the mutation "admit any well-formed list key". |
| N6 | Sonnet | Medium | **Valid** as a required audit; **its premise is not borne out**: no existing test asserts 422 for a stale invalid `save_private` save. Checked: `formula_tests.rs:394, 479, 545, 684, 759` and `campaigns/tests.rs:1408` post the current revision; `tests.rs:2930` is the join route; `tests/sheet_layout.rs`, `foundation.rs` and `creation_flow.rs` use the legacy `/characters` editor, which D4 does not change. | §1 lists the audit; test 10 is new; step 2 re-runs the grep and records it. D4 now names its scope (`save_private` only). |
| N7 | Sonnet | Medium | **Valid.** `write` loads the old row and checks the revision (`snapshots.rs:290-293`) before `checked_add` (`:310`). | D3: the invariant runs after the conflict check and uses `previous + 1`; test 4 `stale_write_with_bad_ids_is_409_not_422`; two mutations. |
| N8 | Sonnet | Medium | **Valid.** At 5 MiB a body of quotes would be echoed as ≈ 30 MiB. | D7 echo cap at 1 MiB decoded (such drafts cannot be stored), alert, page bound; test 42. User confirmation asked (plan Q5). |
| N9 | Sonnet | Medium | **Valid-partial.** `selectors 0.33` is MPL-2.0 (local registry); **`servo_arc 0.4.3` is MIT OR Apache-2.0**, so it needs no exception. The one-time fetch point is right. | D11: per-crate `selectors` acceptance (as `cssparser`), a version choice that reuses a locked `html5ever` if possible, a recorded one-time fetch in step 3, the networked-run rule reworded (§6), and a locked-`html5ever` fallback. |
| N10 | Sonnet | Low | **Valid.** One router-wide layer covers GET and POST (`web.rs:70-78`). | D7 per-method layering note; the 5 MiB layer moves to its own step 7 before the lift, acknowledged as a shipped change. |

No round-2 finding was rejected. One premise was not borne out (N6) and one licence fact was
wrong (N9, `servo_arc`).

## User-decision changes (round 2)

- **Reorder (Q2).** D1 `up-f-r`/`down-f-r`; D5 edges (disabled buttons, forged edges → 400),
  one `autofocus` per page with move-specific targets, status messages, 24 px targets and the
  390 px button row; tests 20–21, 25–26; the browser keyboard step; risk 6.
- **Change summary (Q4).** D8: host-side `history_diff::diff` of revisions N and N − 1 on stored
  data; added, removed, edited (with properties, presence included) and moved (a deterministic
  longest increasing subsequence) per list, plus scalar, name and notes changes; the `<list>/
  <entry id>/<property>` address; a reused slot shown as "all properties changed"; create and
  freeze lines; 200 lines, 8 labels per line, an overflow line with exact counts, a ≈ 1.25 MB (corrected to 1,675,264 bytes in round 3, N-E)
  worst-case section; read-only with no revert. Tests 29–34, 36; risk 7.
- Open: Q5 (oversized drafts not echoed) and Q6 (names only, no values, in the summary), each
  with a default applied.

## Round 3 findings (revision 3)

Round 3 re-reviewed revision 3's new scope (D5 moves, D8 change summary, the D7 pre-body check
and echo cap) in parallel with fresh contexts:
- Sol (`--chain main`): **GO-with-amendments**, RE3-1 and RE3-2 (both medium). It confirmed
  RR1–RR5 and N1–N10 incorporated.
- Sonnet subagent: **GO-with-amendments**, N-A to N-H. The main session relayed these as
  amendments without severities, so the table shows none.
- Third reviewer (`--chain third`): **unavailable** again; proceed with the two main reviews
  and retry at implementation review.

Revision 3.1 applies every amendment in plan text. Each was checked against the code read-only
at master `8174be6`.

| ID | Reviewer | Sev. | Verdict and evidence | Disposition (revision 3.1) |
|---|---|---|---|---|
| RE3-1 | Sol | Medium | **Valid.** The scalar template already echoes an invalid enum, boolean or state value twice, as the option `value` and in its visible text (`private_sheet.html:7-8`); the planned row legend would repeat the hidden ID. `6 × ECHO_BYTES` counted each byte once (Askama's widest escape is the 5-byte `&#34;`, `askama 0.14` `filters/escape.rs`). | D7 "Echo multiplicity": each string in full at most once; repeated contexts show a 64-character excerpt; page ≤ `FIXED_PAGE_BYTES` + units × `UNIT_MARKUP_BYTES` + 5 × `ECHO_BYTES`. Test 42 fills the maximum-amplification contexts; a mutation "a repeated context echoes the full value". |
| RE3-2 | Sol | Medium | **Valid.** `[A,B,C]` → `[B,A,C]` gives N − 1 positions `[1,0,2]`; the lexicographically smallest N positions keep B and C and report A, contradicting "moving one row to the top shows that row". | D8: a deterministic minimal explanation; the tie-break keeps the lexicographically **greatest** N positions, so the rows that ended up earlier are reported. Pinned cases in test 30, a mutation "the tie-break reversed". The adjacent-swap-at-the-bottom case reports the row that moved up, as stated. |
| N-A | Sonnet | — | **Valid.** In a one-property list every edit changes "all properties". | D8: the reused-slot wording only for lists with ≥ 2 properties; neutral edit text otherwise; test 31 one-property case. |
| N-B | Sonnet | — | **Valid.** `auth::normalize` sets `no-store` on every response (`auth/mod.rs:426-429`), so Back-button recovery cannot be promised. | D7 and Q5: "Nothing was saved; copy large text before submitting"; test 42 asserts the wording. |
| N-C | Sonnet | — | **Valid.** Revision N − 1 had no stated validation. | D8: N − 1 goes through the same decode, identity, `match_pin` and `validate_values`; any failure shows "Change summary unavailable for this revision."; test 33 extended; a mutation. |
| N-D | Sonnet | — | **Valid.** Test 20 did not cover new rows moved around retained rows, or invalid text through a move. | Test 20 extended: new IDs follow the final row order; invalid text kept. |
| N-E | Sonnet | — | **Valid.** The ≈ 1.25 MB figure assumed 10 labels at 100 bytes, not 8 labels escaped at 5 bytes per character, and left out the overflow line. | D8 derives `SUMMARY_LINE_BYTES`, `SUMMARY_OVERFLOW_BYTES` and `SUMMARY_FIXED_BYTES`; `SUMMARY_BYTES` = 1,675,264, asserted by test 34. |
| N-F | Sonnet | — | **Valid.** D8 named `history::diff`. | `history_diff::diff` everywhere. |
| N-G | Sonnet | — | **Valid.** "Still announced" is not something the plan can guarantee for disabled buttons. | D5: the claim dropped; the focus target's accessible name carries the new position; D8 summary markup (`section[aria-labelledby]`, `h2`, `ul`, `code`) with a DOM assertion in test 32. |
| N-H | Sonnet | — | **Valid.** The 5 s `boundary` timeout (`auth/mod.rs:217`) covers the body too. | D7: recorded as an accepted limit (about 8.4 Mbit/s for 5 MiB). |

No round-3 finding was rejected. The user's open questions Q5 and Q6 keep their defaults (Q5
amended by N-B).

## Status

Revision 3.1 resolves every round-1, round-2 and round-3 finding in plan text and is ready for
execution. Review of the review record does not recurse. Review findings are not proof of
correctness; the implementation review (plan step 9) is still required.
