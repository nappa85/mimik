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

## Status

All Major and Minor findings are fixed or documented; T-F1 was rejected with source evidence
and is pinned by a guard test. The fix commits go to a re-review of the fixes before merge
(main session).
