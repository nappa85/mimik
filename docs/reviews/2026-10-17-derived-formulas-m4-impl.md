# Review record: derived formulas M4 implementation

2026-10-17. Scope: branch `derived-formulas`, commits `381f416`…`18c6a07` (rebased on master
`f9d34e0`), against [derived-formulas-m4-plan.md](../derived-formulas-m4-plan.md) revision 3.1.
Implementation summary and evidence: [parent plan §17](../derived-formulas-plan.md).

## Independence

- Implementer: an Opus 5.5 subagent. It also applied the fixes below; it is not a reviewer.
- Reviewers, run in parallel by the main session with fresh contexts, read-only:
  - Sonnet subagent: **MERGE**, with minor findings.
  - OpenAI Sol (`openai/gpt-6.1-sol`, `--chain main`): **MERGE AFTER FIXES**, two Major findings.
  - Third chain (`--chain third`): **Space Bunny** (`opencode/space-bunny-free`) was available
    this time: **MERGE AFTER FIXES**, F1 (claimed Blocking) and F2–F9.
- All three **accepted the D5 deviation** (no `#list-f` fragment on op buttons), on the strength
  of the §17 browser negative control.
- The main session adjudicated the findings, including the F1 rejection below. The full
  reviewer texts are kept by the main session.

## Findings and dispositions

| ID | Source | Sev. | Finding | Adjudication and disposition | Regression |
|---|---|---|---|---|---|
| S1 | Sol | Major | The submitted `revision` was echoed in full twice (the intro paragraph and the hidden input), breaking RE3-1's single-full-echo bound: about 1 MiB of `"` gave about 10 MiB of escaped text. | **Valid.** The visible "submitted/form revision" now shows a 64-character excerpt; the hidden control keeps the full value. Audit: the template's only other use is the hidden input; draft-op warnings use the parsed integer. Plan D7 amended. | `malformed_near_cap_revision_is_echoed_once` (executed red: 2,000,014 quotes, want 1,000,078) |
| S2 | Sol | Major | A textarea drops the newline right after its opening tag, so values starting with `\n` lost one newline per round trip. | **Valid.** One sacrificial newline after every textarea opening tag in the private sheet template (multiline fields, general notes), the list template and the native fields used by private native sheets. The legacy and source-review templates are unchanged (outside M4). | `leading_newlines_survive_ops_rejected_saves_and_reloads` (executed red: "saved reload: notes" `"notes"` vs `"\nnotes"`; a mutation dropping the list textarea's newline also fails); browser list stage (executed red on a build without the newline: "Leading newlines lost after draft operations") |
| S2-follow-up | implementer | — | The first gate rerun failed `smoke_release.py`: its `html.parser` form reader keeps the newline that browsers drop after `<textarea>`, so the sacrificial newline appeared in the legacy native editor's persisted-field check. | The smoke's `FormFields` now drops one leading newline at `</textarea>`, as HTML parsing does. | `TextareaNewlineTests` (executed red: `'\nplain'` vs `'plain'`) |
| T-F1 | Space Bunny | Claimed Blocking | `DefaultBodyLimit::max` only checks `Content-Length`, so a streamed body is unbounded. | **Rejected, not a defect.** axum-core 0.5.6 (`Cargo.lock`): `DefaultBodyLimit::max` sets `DefaultBodyLimitKind::Limit(limit)` (`src/extract/default_body_limit.rs:150-152`); the `Bytes` extractor calls `req.into_limited_body()` (`src/extract/request_parts.rs:76`), which wraps the body in `http_body_util::Limited` for `Limit(n)` (`src/ext_traits/request.rs:321-330`). There is no `OnlyValidateHeaders` kind. Kept as a guard. | `streamed_body_over_5mib_without_length_is_413` (guard; mutation "limit disabled on the sheet POSTs" fails it) |
| T-F2 | Space Bunny | Minor | `FIXED_PAGE_BYTES` is not demonstrated for formula definitions. | **Valid.** Stated precisely in D7 and the contract: the bound covers the editor chrome and controls, not the formula sections, which are bounded by the 256 KiB definition content cap and the 20,000-character text limit of each derived value. | — (scope statement) |
| T-F3 | Space Bunny | Minor | `form_budget`'s unit terms are not a literal element count for optional properties. | **Valid.** A comment in `form_budget` records the approximation and its calibration; a second calibration measures the opposite row shape (20 optional booleans, labels of 100 `"`): 7,810,422 bytes for 31,748 units, 246 bytes per unit against 2,048. | `all_optional_worst_label_page_is_within_the_unit_bound` (guard) |
| T-F4 | Space Bunny | Minor | The optional-first-property autofocus branch was untested. | **Valid.** | `add_focuses_the_set_select_of_an_optional_first_property` (guard; mutation "Set/Not set select never autofocused" fails it) |
| T-F5 | Space Bunny | Minor | The plan text still required `formaction="#list-f"`. | **Valid.** Plan revision 3.2 amends D5, D11, test 25 and step 4 in place, citing the §17 negative control and this ruling; the plan review record notes it. | — |
| T-F6 | Space Bunny | Minor | `outside_increasing` absorbed an impossible search failure with `break`. | **Valid.** The invariant is stated and enforced with `expect`, so a bug cannot silently over-report moves. | existing test 30 |
| T-F7 | Space Bunny | Minor | `rejected_editor` silently returns 403 when edit rights were lost between the rejected save and the re-render. | **Valid, documented.** The code comment and the contract name this second loss path (a plain 403, nothing saved). | — |
| SN1 | Sonnet | Minor | A 5 MiB body is buffered under the runtime permit. | **Valid as a risk, already the design.** The single runtime permit is taken by the `runtime` middleware before the handler reads the body, so at most one sheet POST buffers at a time and other runtime requests answer 503 meanwhile. Recorded in the contract next to the slow-upload hold. | — |
| N-1 | Sonnet, Space Bunny F8 | Nit | `role="status"` on a freshly loaded page is not reliably announced. | Contract wording added; the focused control and its name carry the result. | — |
| N-2 | Space Bunny F9 | Nit | "1,334 advisories" read as findings. | §17 reworded: the advisory database has 1,334 entries; no findings. | — |
| N-3 | coordinator | Nit | `form_urlencoded = "=1.2.2"`. | Relaxed to `"1.2"`; `Cargo.lock` still pins 1.2.2. | — |
| N-4 | coordinator | Nit | The stale draft-op warning should say Save will be refused with 409. | The warning now reads "…saving this draft will be refused with a conflict (409). Copy your changes, reload and re-apply them." | `stale_op_warns` (executed red on the old wording) |

## Verification

Fix commit `3225a5c` (tree `2cbd51b`); the full gate set ran on that tree before the commit,
every container at `--cpus 2`:

- fmt; strict Clippy; `cargo test --locked --all-targets --all-features`: 447 passed, 0
  failed, 1 ignored (300 library tests); release formula tests 82 passed; release
  measurements 7 passed; `formula_eval` build; `source_fixture`; Ruff check and format;
  tools unittest 452 run (39 skipped); actionlint; `cargo about --fail`; `cargo audit --deny
  warnings` (333 crates scanned, no findings).
- Release build 21,049,464 bytes (+246,944 against the master `f9d34e0` baseline of
  20,802,520, +1.19%); `cargo package --offline` and `verify_source_package.py`; `cargo vendor`
  and `package_release.py`. SHA-256: runtime archive
  `a34583e8b0e00a00de8b49b9dd5c7ebb15a75fe90c5577805538dc43caaca6db`, source crate
  `924a0c6b2399639ceadc35cedfcd2eaf29d28153ab13c894cdaf7559d1f303b0`, dependency sources
  `58be801102dab843036058155f60e676f2168376506c4c4eee04f0fc93ac0244`; checksums verified,
  unpacked binary equal to the release build.
- `smoke_release.py --auth` on the unpacked binary; offline Chromium with the CI flags at both
  viewports (`mimik-browser:m4`, rebuilt with the current tool files); fresh offline rebuild
  of the delivered source with an empty `CARGO_HOME` (5 min 34 s); `lan_smoke.py` (with the
  `--cpus=2` docker shim): all passed.
- An earlier pass on tree `c90257e` failed `smoke_release.py` (the S2 follow-up above); it was
  fixed and the whole set rerun.

Review findings are not proof of correctness.

## Re-review (fix commits `3225a5c`, `68e9697`)

Run in parallel by the main session with fresh contexts, read-only:
- Sonnet subagent: **MERGE** (Minor 1: no native-sheet newline test; Nit 5: the old
  `=1.2.2` pin had no recorded rationale).
- Sol (`openai/gpt-6.1-sol`): **MERGE AFTER FIXES**. Every disposition verified by source
  inspection; one Minor: the smoke's textarea reader did not normalise literal CRLF and lone CR
  to LF before the initial-newline rule, as browsers do. It confirmed that the runtime permit
  is held through body extraction (`Extension<RuntimeAdmission>` is extracted before `Bytes`).
- Space Bunny (third chain): **MERGE AFTER FIXES**. Per-finding table (summary):

| ID | Space Bunny's verdict |
|---|---|
| S1 | Verified (excerpt on a char boundary; test reaches the single-echo count) |
| S1 audit | Verified for `private_sheet.html` (see N6 for other templates) |
| S2 | Verified for the three templates; native half untested (N1); other templates lossy (N2) |
| S2 follow-up | Verified; cannot hide a persistence bug |
| T-F1 | Verified; the guard discriminates (see N4 on which layer answers) |
| T-F2, T-F3, T-F4, T-F5, T-F6 | Verified |
| T-F7 | Verified as documented; no test (N3) |
| SN1 | Verified (permit before body; one sheet POST buffers at a time) |
| N-1 … N-4 | Verified (contract quotation truncated, N5) |

New findings, all Minor or Nit, nothing Blocking or Major: N1 (native newline test missing),
N2 (five other templates still lose a leading newline), N3 (T-F7 untested), N4 (state what the
streamed guard proves), N5 (truncated quotation), N6 (legacy editor and material page show the
revision twice), N7 ("LF-exact").

## Final fixes

| Finding | Disposition | Regression |
|---|---|---|
| Sol Minor (CRLF/CR) | `FormFields.feed` normalises literal CRLF and CR to LF before tokenisation, so character references such as `&#13;` stay distinct; then the initial-LF rule applies. | `TextareaNewlineTests.test_literal_crlf_and_cr_are_normalised_before_the_initial_newline_rule` (`\r\nplain`, `\rplain`, a CRLF-heavy value, `&#13;`); executed red: `'\r\nplain'` vs `'plain'` |
| Sonnet Minor 1, Space Bunny N1 (native) | Test only; the native template already had the newline. | `native_sheet_textareas_keep_leading_newlines` (description, weapon_one, equipment through a rejected save, a reload and an unchanged save; native sheets have no draft ops). Guard; mutation "description textarea without the newline" fails it |
| N2 (decided: fix) | Sacrificial newline added to every textarea with a value in `material_page.html`, `source_page.html`, `editor.html`, `index.html` and `mork_context.html` (empty textareas unchanged). Helper audit: only `smoke_release.py`'s `FormFields` reads textareas from HTML, and it now models browsers; `browser_smoke.py`, `material_browser.py` and `admin_ruleset_browser.py` use Chromium. | `material_textareas_keep_leading_newlines` (stored correction and a rejected draft's rationale; executed red: `"corrected text"` vs `"\ncorrected text"`); `tests/foundation.rs::legacy_editor_notes_keep_leading_newlines` (reload, unchanged save, stale re-render; executed red: `"\nlegacy notes"` vs `"\n\nlegacy notes"`) |
| N6 (decided: fix) | **Adjudicated not reachable**: both revisions are `i64` (`CharacterInput::revision`, the materials form structs), so a malformed revision is refused before any page renders and the double echo shows at most 20 digits. The visible copies use Askama's `truncate(64)` anyway, defensively. | `material_malformed_revision_is_not_echoed` and the malformed-revision part of `legacy_editor_notes_keep_leading_newlines` (guards: 4xx, no draft re-render, fewer than 64 `&#34;`) |
| N3 (T-F7 403 path) | **Not tested; recorded why.** The window lies between two awaited steps of one handler, after a database-rejected save; the only seam there (`AFTER_FAILED_SAVE`) is a synchronous hook that cannot run the async store calls a departure needs, so a test would need a new async test seam in shipped code for a documented, nothing-saved 403. | — |
| N4 | The streamed guard proves that a body without `Content-Length` is bounded (413, whichever layer answers; the router-wide 1 MiB layer also covers these routes). That the 5 MiB limit, not 1 MiB, is the one that applies to sheet POSTs is pinned by `sheet_post_accepts_5mib_and_413s_5mib_plus_1` (exactly 5 MiB is not 413). | — |
| N5 | The contract quotes the full stale warning. | — |
| N7 | Plan D7 and the contract said "LF-exact" (a browser submits CR LF as LF). **Superseded by the final review:** that description of browser submission was wrong; see "Final review". | — |
| Sonnet Nit 5 | The old `=1.2.2` pin of `form_urlencoded` had no recorded rationale (it was the locked version at adoption); `"1.2"` stays, with `Cargo.lock` still selecting 1.2.2. | — |

### Verification of the final fixes

Commit `f80a245` (tree `a5168e3`); the full gate set ran on that tree before the commit, every
container at `--cpus 2`: fmt; strict Clippy; 451 tests passed, 0 failed, 1 ignored (303 library
tests); release formula tests 82; release measurements 7; `formula_eval`; `source_fixture`;
Ruff; tools unittest 454 run (39 skipped); actionlint; `cargo about --fail`; `cargo audit`
(333 crates, no findings); release build 21,054,776 bytes (+252,256 against the master
`f9d34e0` baseline of 20,802,520, +1.21%); package, verify, vendor and release archives
(SHA-256 runtime `95eee0e870993f473ecbd732c45a1de0b7a3d394ff5b3a64689ab31480cd22c2`, source
`3d4aa4be0564b517a387b714ff06a16b133214a21c70bf537ef692bf7d7b0e5a`, dependencies
`94605ae7c2818665349a5ea7e068e16c3edac4eaba3ddb9f78809136378205f8`), checksums and binary
equality; `smoke_release.py --auth`; Chromium at both viewports; the fresh offline rebuild
(6 min 6 s); `lan_smoke.py`. All passed.

## Final review (commit `1745e36`)

- Sonnet: **MERGE**, with a nit: `tests/foundation.rs` accepted any success or redirect.
- Sol: **MERGE AFTER FIXES**, Minor. N7 described browser form serialisation incorrectly. HTML
  parsing turns CR LF into LF, but a native form submission serialises textarea line breaks as
  CR LF (`%0D%0A`), and `campaigns::transport` stored them raw. An unchanged browser Save of
  `"\nnotes"` therefore stored `"\r\nnotes"`. The tests did not catch it, because the
  `submitted` helper posts the DOM text (LF) directly. Confirmed by the main session.
- Space Bunny: **MERGE AFTER FIXES**, Minor and Nit only. It verified the textarea newline
  claims (18 valued textareas, each with exactly one sacrificial LF, nothing stripped twice)
  and N6 (every revision field is `i64`).
  - F1: `FormFields.feed` was not chunk-safe; a CR LF split across feeds became two newlines.
  - F2: the smoke's newline strip made it blind to a missing sacrificial newline on
    `index.html`, `mork_context.html`, `source_page.html` and the material completion branch.
  - F3: the empty completion textarea had no sacrificial newline.
  - F4: the contract documented the guarantee only for M4 sheets.
  - F5: the N6 guards are weak discriminators; their discriminating assertions are the 4xx
    status and "no draft re-render".

| Finding | Disposition | Regression |
|---|---|---|
| Sol (CR LF storage) | *(Scope extended in the last review: every form decoder, not only campaigns.)* `campaigns::transport` normalises CR LF and lone CR to LF in every submitted value while decoding. This covers private runtime and native sheets and every campaign route. It runs before validation, so the 20,000-character limits count the normalised text. The legacy `/characters`, source-review and material-review forms use their own extractors with no shared decode point; they keep CR LF, recorded as a known limitation in the contract. Plan D7, the contract and this record now separate "textarea content is preserved" from "canonical storage is LF". | `submitted_values_have_crlf_and_cr_normalised_to_lf` (unit test); `browser_crlf_submission_is_stored_as_lf` covers an unchanged browser Save with CR LF (stored bytes identical), a raw `a\r\nb\rc` (stored `a\nb\nc`) and 20,000 CR LF pairs accepted as 20,000 characters. It failed first: `"\r\n\r\nscalar…"` was stored. |
| Space Bunny F1 | `FormFields.feed` holds back a trailing CR until the next feed or `close()`. | `test_a_crlf_split_across_two_feeds_is_one_newline`; failed first: `'\nplain'` vs `'plain'` |
| Space Bunny F2 | The smoke's source-review round trip stores the correction `"\nVerified fixture correction"` and reads it back. The positive-advice context posts the goal `"\n\nreach the gate"` and reads it back. A scraper test covers the material completion-rationale branch. | Smoke mutation: a release build of `mork_context.html` without the newline failed `smoke_release.py --auth` with "Context goal lost its leading newlines". `material_completion_rationale_keeps_leading_newlines` is a guard; a mutation removing that branch's newline fails it. |
| F2 follow-up | The first gate pass on the final-review tree failed `smoke_release.py` ("Reviewed evidence persistence failed"): the post-restart search check still expected the correction without its new leading newline. The check was relaxed to accept an optional newline, and the whole set was rerun. *Correction (last review):* that relaxation did not match the first check. The only exact check was the pre-restart textarea assertion, so the post-restart check no longer verified the leading newline; it is strict again (see "Last review"). | the smoke itself |
| Space Bunny F3 | The empty completion textarea also emits the sacrificial newline, for consistency. | — |
| Space Bunny F4 | The contract's line-break paragraph lists every template that carries the newline. | — |
| Space Bunny F5 | Noted; no change. | — |
| Sonnet nit | `legacy_editor_notes_keep_leading_newlines` asserts 303 exactly. | — |

### Verification of the final-review fixes

Commit `24a08ca` (tree `ac6b775`); the full gate set ran on that tree before the commit, every
container at `--cpus 2`: fmt; strict Clippy; 454 tests passed, 0 failed, 1 ignored (306 library
tests); release formula tests 82; release measurements 7; `formula_eval`; `source_fixture`;
Ruff; tools unittest 455 run (39 skipped); actionlint; `cargo about --fail`; `cargo audit`
(333 crates, no findings); release build 21,045,592 bytes (+243,072 against the master
`f9d34e0` baseline of 20,802,520, +1.17%); package, verify, vendor and release archives
(SHA-256 runtime `a3404ff123abdeb9fdb798fd0f76036c36bd6fe099723bcbc3e88e6e7897d76a`, source
`ed2c0f55068c85058da4563a88ee5049eda11501d08db308d275b7911d7e3ec7`, dependencies
`dcae9cc95078b60625bb701ffab465534f14da995a5a7b1dad9fc8c79467847e`), checksums and binary
equality; `smoke_release.py --auth`; Chromium at both viewports; the fresh offline rebuild
(5 min 11 s); `lan_smoke.py`. All passed.

## Last review (commit `e0deed0`)

- Sonnet: **MERGE**, with nits: `FormFields.close()` is never called; the material
  else-branch change is untested.
- Sol: **MERGE AFTER FIXES**, two Minors:
  1. The relaxed post-restart search check can hide the loss of the leading newline.
  2. The LF-storage guarantee was stated more broadly than implemented: existing snapshots
     are not migrated.
- Space Bunny: **MERGE AFTER FIXES**:
  - F1, **Major (accepted)**: the legacy routes count CR LF as two characters, so text with
    many line breaks near a limit is refused although `maxlength` allowed it; three routes
    were missing from the limitation.
  - F2: existing CR LF data is rewritten on the next save.
  - F3: the relaxed post-restart check, and the record's sentence about it.
  - F4: `<pre>` elements drop a leading newline.
  - F5: `close()` is not called.
  - F6: the 5 MiB sentence had been absorbed into the limitation bullet.
  - F7: no §17 row for the gate failure.
  - F8: no draft-op normalisation test.
  - F9: an extra allocation (not changed).

| Finding | Disposition | Regression |
|---|---|---|
| Space Bunny F1 (Major) | Fixed rather than documented. `form_text::newlines_to_lf` is the shared helper, and `campaigns::transport` uses it. Serde `deserialize_with = form_text::lf` (or `lf_option`) applies it to every textarea-bearing value of the legacy extractors before validation: `CharacterInput.notes` (`/characters/{id}`), `CreateRequest.notes` (`/` creation), the six `SheetForm` textareas (`/characters/{id}/mork-borg`), `Context.goal` (`/characters/{id}/context`), `UnitReview` correction and rationale, `PageReview` rationale (source review), and material `Draft` correction and rationale plus `Completion` rationale. These are all eight textarea-bearing templates. The "known limitation" is removed; nothing remains. | `form_text::tests::crlf_and_cr_become_lf`. `material_crlf_at_the_limit_is_stored_as_lf_and_shown_intact`, `tests/source_review.rs::source_review_crlf_at_the_limit_is_accepted_and_stored_as_lf` and `tests/foundation.rs::legacy_editor_crlf_notes_at_the_limit_are_stored_as_lf` send `%0D%0A` bodies of exactly 20,000 typed characters (and 5,000 for rationales). All three failed first with 422 (want 303). They now assert LF storage. Mutation: dropping `lf` from the material correction fails the material test. |
| Space Bunny F4 | A sacrificial LF after every stored-text `<pre class="evidence-text">`: material and source history, evidence search, retained canonical data, and formula expressions. The first gate pass then failed one M3 test (`formula_sheet_view_shows_derived_values_and_advisory_checks`), which asserted the raw `<pre>` markup; it now expects the sacrificial LF, and the whole set was rerun. | The same material and source tests read the history `<pre>` with scraper. Mutations removing the material and the source `<pre>` newline each fail their test. |
| Sol 1, Space Bunny F3 | The smoke's search checks are strict: `corrected_evidence` requires the raw markup `<pre class="evidence-text">` + sacrificial LF + the stored `\nVerified fixture correction`, before and after the restart. The record sentence is corrected above. | `CorrectedEvidenceTests` (the newline-less shapes are refused); failed first with ImportError, as the helper is new. The smoke run on the final tree passed. |
| Sol 2, Space Bunny F2 | The guarantee is scoped to new writes. Existing CR LF data is not migrated; its next save rewrites it to LF as a new revision, and history keeps the old revision. Stated in the contract, plan D7 and a release-scope migration note. | — |
| F5 (Space Bunny, Sonnet) | `FormFields.reset()` discards a held-back CR, and `assert_fields` calls `close()`. | `FormFieldsLifecycleTests` (red without the override) |
| F6 | The 5 MiB, lost re-render and page-bound sentences are their own bullets again. | — |
| F7 | §17 has a row for the gate failure. | — |
| F8 | — | `draft_op_rerender_has_crlf_normalised`: no CR in the re-render, echo is LF. A guard, since the transport already normalised. |
| Sonnet nit (material else branch) | — | The empty completion textarea is asserted empty in `material_crlf_at_the_limit_is_stored_as_lf_and_shown_intact`. A guard: an empty textarea is empty with or without the sacrificial LF, so no mutation can distinguish it; the newline is kept for consistency (F3 of the final review). |
| F9 | Not changed: bounded and correct. | — |

### Verification of the last-review fixes

Commit `6a2d21b` (tree `9dbf78f`); the full gate set ran on that tree before the commit, every
container at `--cpus 2`:

- fmt; strict Clippy.
- 459 tests passed, 0 failed, 1 ignored (309 library tests); release formula tests 82; release
  measurements 7.
- `formula_eval`; `source_fixture`; Ruff; tools unittest 457 run (39 skipped); actionlint;
  `cargo about --fail`; `cargo audit` (333 crates, no findings).
- Release build 21,037,656 bytes (+235,136 against the master `f9d34e0` baseline of
  20,802,520, +1.13%).
- Package, verify, vendor and release archives. SHA-256: runtime
  `5682f99a50c1f62ef09113547d0686c52ca1ca121b91676b5efa16729feda0f6`, source
  `ec1fa3d0dad4a969e1c75f7770c9e700dcd538a93dd466d4db0440fe1a8be621`, dependencies
  `9dc6cb0a5073da0c53538b817bd23e849e168c753a4ad492b795573091ab0c07`. Checksums verified and
  the unpacked binary is identical to the release build.
- `smoke_release.py --auth`; Chromium at both viewports; the fresh offline rebuild (5 min
  24 s); `lan_smoke.py`.

All passed.

Two incidents during the rerun, neither on the gated tree:

- Tree `dcf4a8d` failed one M3 raw-markup test, fixed as recorded above.
- An overlapping second gate run, started while the first was still going, made
  `source_fixture` collide. Both runs were stopped, and the set was rerun once, alone, on
  tree `9dbf78f`.

## Status

All Major and Minor findings are fixed or documented; T-F1 was rejected with source evidence
and is pinned by a guard test. The fix commits go to a re-review of the fixes before merge
(main session).
