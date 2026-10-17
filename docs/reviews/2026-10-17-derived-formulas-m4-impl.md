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
| N7 | Plan D7 and the contract say "LF-exact" (a browser submits CR LF as LF). | — |
| Sonnet Nit 5 | The old `=1.2.2` pin of `form_urlencoded` had no recorded rationale (it was the locked version at adoption); `"1.2"` stays, with `Cargo.lock` still selecting 1.2.2. | — |

## Status

All Major and Minor findings are fixed or documented; T-F1 was rejected with source evidence
and is pinned by a guard test. The fix commits go to a re-review of the fixes before merge
(main session).
