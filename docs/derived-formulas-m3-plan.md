# Derived-field formulas: milestone M3 plan (evaluation on view, save and history)

2026-10-16. Status: **plan reviewed: GO-with-amendments (Sonnet 5.5, 2026-10-16); amendments in §10 are binding and supersede earlier sections.**
**Implemented** on branch `derived-formulas` (see [parent plan §16](derived-formulas-plan.md));
independent implementation review pending. Produced by a read-only planning subagent; the main session spot-checked
§1 claims (no `preview` in `src/`, `groups()` renders every group field as an input, the
publication gate at `publication.rs:124-137`). Base: master and `../mimik-wt/formula` at
`1166a1d`. Parent plan: [derived-formulas-plan.md](derived-formulas-plan.md) §7, §9, §10,
§12–§15.

## 1. Current state (verified)

**Engine (M1)**

- `formula::Compiled::new` and `evaluate` both run on `on_formula_stack`, a scoped 16 MiB
  thread (`src/definitions/formula/mod.rs:237-244, 295-306, 390-400`).
- The 500 ms `DEADLINE` (`mod.rs:49`) is checked only *between* formulas (`mod.rs:323,
  343`); it is a safety net. The static `MAX_COST` (`mod.rs:43`) is the deterministic bound.
- `Compiled` is asserted `Send + Sync` at compile time (`mod.rs:222-227`). The
  `cel::Context` is built per evaluation (`mod.rs:311`).
- `Evaluation` holds `derived: Vec<(id, Result<Scalar, Code>)>` and
  `constraints: Vec<(id, Outcome)>` (`mod.rs:157-161`).
- `dependency_failed` carries **no dependency ID** (`mod.rs:325-327`).
- **`preview` does not exist yet**, although parent plan §7 lists it. M3 adds it.
- `Code` is a closed enum with `Code::ALL` (`formula/error.rs:6-58`).
- `evaluate` returns `Err(InputError)` when values do not match the declarations
  (`mod.rs:295`). `convert::inputs` charges 16 bytes per value plus string bytes against
  1 MiB (`convert.rs:22, 104-110`).

**Pin (M2)**

- `ImmutablePin { raw, data: Admitted, validator }` (`src/definitions/private_pin.rs:15-19`).
- `validate` reruns `admission::validate` on every request (`private_pin.rs:33-50`). That
  runs `formula_admission::check`, which compiles the formulas and **discards the
  `Compiled`** (`formula_admission.rs:366-372`, called from `admission.rs:227`). Every pin
  load already compiles once.
- `validate_values` applies the 1 MiB JSON cap to profile pins only, then the schema, then
  the i64 and list checks (`private_pin.rs:81-106`).
- A stored-valid sheet can still fail `input_too_large`: the pin caps JSON bytes while the
  engine charges 16 bytes per value, so many small values pass the first and fail the second.

**Values**

- Derived fields are excluded from the schema (`campaigns/values.rs:55-59`) and the form
  (`values.rs:102-106`). List fields are refused in forms (`values.rs:107-111`).
- Form keys are admitted only on an exact set match (`campaigns/transport.rs:50-58`), and
  derived keys are never admitted (`campaigns/web.rs:709-721`): a submitted
  `value-<derived>` gives **400**.
- Stored or JSON data with a derived key fails the `additionalProperties:false` schema
  (test at `formula_admission_tests.rs:345-346`).

**Latent bug**

- `groups()` renders every group field as an input named `value-<id>` (`web.rs:492-555`,
  `templates/private_sheet.html:6-8`). Admission requires every field, derived ones
  included, to be in exactly one group (`admission.rs:220-224`).
- A published formula sheet would therefore render derived fields as inputs, and every
  browser save would fail with 400. The publication gate keeps this unreachable today; M3
  must fix it before lifting the gate.

**Request paths**

- Routes: `/private-sheets/{id}` (GET/POST) and `/private-sheets/{id}/history/{revision}`
  (`web.rs:59-68`).
- Every sheet route takes the global `definitions::JOBS` permit, `Semaphore(1)`
  (`definitions/mod.rs:22-30`), with `try_acquire`; busy → 503 with `Retry-After: 1`
  (`web.rs:41-54`).
- The permit is carried through `private_read` (DB step, `spawn_blocking` decode/validate at
  `sheets.rs:107-129`, `private_badge` recheck at `sheets.rs:238-284`) and into `rendered`,
  which renders inside `spawn_blocking` while still holding it (`web.rs:557-580`).
- History uses the same `read_private` path with `history = Some(rev)` (`web.rs:610-636`).
  `raw_load` takes the pin from `private_sheets.pin`, and `decode` refuses a history row
  whose pin differs (`snapshots.rs:144-163, 176-181`). A sheet's pin is immutable, so
  history always uses the pin that wrote it.
- Save: `values::form`, `validate_values` and the revision check run in `spawn_blocking`
  (`web.rs:729-751`). Success → 303 to `/private-sheets/{id}` (`web.rs:765-766`);
  Invalid/Conflict → `load_editor` re-renders with 422/409 (`web.rs:752-786`).
- `private_save` hands the view to the DB worker under the DB mutex (`sheets.rs:204-236`,
  `store.rs:107-117`).

**Errors and templates**

- Error→HTTP mapping `web.rs:81-89`: NotFound→403, Conflict→409, Invalid→422,
  AuthStorageBusy→503, else→500.
- `private_sheet.html` does not extend `base.html` and loads **no htmx**; saves are
  full-page PRG. `private_history.html` is a link list; a revision view reuses
  `private_sheet.html`.
- Native precedent for computed display: "Calculations use the persisted snapshot at
  revision N" (`private_sheet.html:10`).

**Publication gate**

- `publication.rs:124-137`, called at `:640`; list gate `:128-133`, formula gate `:134-136`.
- `deterministic_rules` already reports `formulas-advisory` (`publication.rs:116-122`).
- Current test: `publication_tests.rs:188-210`.

**Test support and packaging**

- `campaigns/tests.rs:530` (`router_request`), `:652` (`published_fixture`);
  `formula_admission_tests.rs:277` (`pin_for`) builds a pin without a database.
- `examples/tool-loop-demo/**` and `tests/*.rs` are in Cargo `include`.
- The pack format differs from the Mimik envelope (`fields`/`collections`/`derived`, rule
  IDs like `LR2-5`); 11 derived fields, 21 constraints including `one-outer-armor`.

**Environment**

- Volume `mimik-formula-target` exists (recreated; the benchmark mounts it read-only for
  the `formula_eval` CLI). Its `formula` target dir was cleaned, so the first build is
  mostly cold. Do not build into it while a benchmark run mounts it.

## 2. Design decisions

**D1. The pin holds the compiled formulas; no cache keyed by pin hash.**

- `formula_admission::check` returns `Option<formula::Compiled>`; `admission::domain`
  forwards it; `Admitted` gains `formulas: Option<formula::Compiled>`. `ImmutablePin`
  exposes `evaluate(&self, data) -> Option<Result<Evaluation, InputError>>`, `None` for
  formula-free pins.
- Cost: no extra compile (the pin already compiles once per request). Without this, M3
  would compile twice.
- Send/Sync proven (`mod.rs:222-227`); `SheetView` already moves the pin across
  `spawn_blocking` and DB closures. Admitted ASTs drop safely on 2 MiB threads (§14.1
  item 5 test); tokio blocking threads are 2 MiB.
- Memory: ≤ 384 formulas of ≤ 1 KiB; one alive per request; JOBS = 1 bounds the total.
- Why not a cache: it needs eviction, a global lock and a memory bound, and saves only the
  compile while pin JSON re-admission and the jsonschema build are still paid per request.
- **Measure first:** the pack test prints compile/evaluate time in `--release`. Revisit a
  pin-level cache (pin plus validator) only if measurements justify it.

**D2. Evaluation at render time, inside `rendered`'s existing `spawn_blocking`
(`web.rs:565`), not in `private_read`.**

- After `private_badge`'s final authorization recheck, so no work for denied requests.
- Once per response (the failed-save path calls `load_editor`, which would otherwise
  evaluate twice); one call covers view, history, failed-save re-render and create.
- Never on an async worker or under the DB mutex (§7).
- Concurrency: the JOBS permit is already held, so at most one evaluation process-wide
  plus one 16 MiB scoped thread; busy → existing 503 + Retry-After. No new semaphore.
- Worst-case permit hold ≈ compile + ≤ 500 ms + one formula; static cost calibrates to
  ≈ 18 ms.

**D3. View model: new `src/campaigns/formulas.rs` (`pub(super)`) with a pure
`present(pin, data, &Evaluation) -> FormulaView`.**

- `FormulaView`: `basis` (snapshot revision or "not yet saved");
  `derived: BTreeMap<id, DerivedView>` (`Value(text)` | `Error { code, message }`);
  `checks: Vec<CheckView { id, message, rule_ids, status: pass|violated|error, code,
  explanation }>`; counts; `unavailable: Option<{code, message}>` for `InputError` or a
  failed formula thread.
- `FieldView` gains `derived: Option<DerivedView>`, `expr`, `rule_ids`. `SheetView` is
  unchanged (storage/authorization type).

**D4. Rendering of each state.** All output Askama-escaped; never `|safe`.

| State | Rendering |
|---|---|
| Value | Inline in its group: `<span id="label-x">Label</span> <output aria-labelledby="label-x">5</output>` with a "Computed" badge. No `name`, never submitted. Ints as decimals, bools True/False (matching the boolean select), enums by option label. |
| Error | "Not computed: <human message>" plus `<code>division_by_zero</code>`. Never a default. |
| `dependency_failed` | "Not computed: a derived value it uses could not be computed." The failing dependency shows its own error on the page. |
| `deadline_exceeded` | **Shown**: "Not computed: the time limit was reached. Reload to retry." Documented as non-deterministic, with `internal_error`. |
| Constraint | "Advisory checks" section opening with "Advisory only: checks never block saving and are not a legality ruling." Violated and error first and expanded; passes in a collapsed `<details>`. Status words in text, never colour alone. |
| Unavailable | "Derived values unavailable: <message>" with the code. Page still **200**. |

- Messages come from one exhaustive `match` over `Code`, no wildcard, so a new `Code` needs
  reviewed wording. Admission-only codes share one "unexpected formula state" message.
- Collapsed "How this is computed" shows the expression in `<pre class="evidence-text">`
  and the rule IDs.
- A summary line near the top ("Advisory checks: 1 violated, 0 not evaluated", linking to
  `#advisory-checks`) makes violations visible right after a save.

**D5. Save keeps PRG.**

- Success → 303; the GET evaluates the stored data and shows advisory checks. Derived
  values are a pure function of pin and data, so this equals evaluating the proposal.
- A violating save is never blocked: the POST does no evaluation.
- Failed save (422/409) re-renders the draft; derived values are labelled "Computed from
  saved revision N" (native precedent).
- No htmx partial (the page has none); live recompute while typing is cut from M3.

**D6. Other decisions**

- **History** recomputes against the historical pin through the same path; identical except
  `deadline_exceeded`/`internal_error`.
- **Stale or withdrawn approval** does not stop evaluation (parent §6); the existing
  warning stays.
- **Create page:** "Derived values are computed after the first save." (`{}` is not valid
  input, nothing is evaluated.)
- **Unsupported pin and native sheets:** no `ImmutablePin` (`sheets.rs:102`), so no formula
  call and no section.
- **No schema change, no migration.** `SCHEMA_VERSION` stays 18.
- **`preview`:** library-only
  `pub fn preview(&Compiled, before: &Json, after: &Json) -> Result<Preview, InputError>`
  returning changed derived fields (before/after `Result`s) and constraints that changed
  outcome, with `new_violations()`/`resolved_violations()`. Error→error is never a
  violation. No route or CLI op until the tool loop.
- **Model output never alters sheets:** no model integration exists; evaluate/preview take
  `&` inputs and return values only; viewing never writes (tested).
- **Gate:** remove `publication.rs:134-136` and update the doc comment at `:124-125`. The
  list gate stays until M4.

## 3. Questions for the user (recommended default first)

**Answered 2026-10-16:** Q1 PRG (default). Q3 **browser scenario in M3** (not the default:
step 6 is now mandatory). Q4 expressions and rule IDs in a collapsed `<details>` (default).
Q5 **name the failed dependency** (not the default: add an engine accessor, see below). Q2
was not asked; the default (unit test `formula_pack_tests.rs`) applies, with the rename
recorded. Superseded option text was removed after the plan review.

**Consequences.** Q5: `dependency_failed` must identify which dependency failed. Either
`Code::DependencyFailed` stays a unit code and `Compiled` exposes the declared
dependencies of each derived field (`dependencies(id) -> &[id]`) so `present` names the
first dependency in topological order whose result is an error, or the engine records it in
the `Evaluation`; the reviewer and implementer pick the smaller change that keeps the M1
golden fixture and its digest unchanged. Failing-first: a chain `a -> b -> c` where `a`
fails shows "uses Lucky, which could not be computed" on `b`, and `c` names `b`. Q3: step 6
adds a scalar formula fixture to `source_fixture`/`smoke_release.py`/`browser_smoke.py`
with desktop and mobile `no_overflow`, a visible violation after save, derived values as
non-input `<output>`, and the expression disclosure; `release-scope.md` then lists formula
sheets as browser-verified for scalar fields.


## 4. Failing-first test list (in order)

Each test is run red first, for the stated reason. Route tests must fail on rendering, not
on the publication gate (see step 4).

| # | File | Test |
|---|---|---|
| 1 | `src/definitions/formula/tests.rs` | `preview_reports_only_changed_derived_values` |
| 2 | same | `preview_reports_new_and_resolved_violations` (error transitions are neither; unchanged inputs) |
| 3 | same | `preview_rejects_bad_input_on_either_side` |
| 4 | `src/definitions/formula_admission_tests.rs` | `pin_evaluates_formula_definitions_and_not_formula_free_ones` (`Some` equals direct `Compiled::new(formula_spec)`; `None` formula-free) |
| 5 | `src/campaigns/formulas.rs` (`#[cfg(test)]`) | `every_code_has_a_human_message` (iterates `Code::ALL`) |
| 6 | same | `derived_states_present_value_error_and_dependency_failed` (int, bool, enum label) |
| 7 | same | `constraint_tri_state_counts_never_count_errors` |
| 8 | same | `deadline_and_internal_error_are_states_not_values` (hand-built `Evaluation`) |
| 9 | same | `input_error_presents_unavailable` (`input_too_large`) |
| 10 | `src/campaigns/tests.rs` (new `published_formula_fixture`) | `formula_sheet_view_shows_derived_values_and_advisory_checks` |
| 11 | same | `derived_fields_render_without_form_names` (rendered `name=` set equals admitted keys; regression for the latent bug) |
| 12 | same | `submitted_derived_key_is_rejected_and_not_saved` (400; revision unchanged) |
| 13 | same | `violating_save_succeeds_and_view_lists_violation` (303; revision +1; stored data has no derived keys) |
| 14 | same | `failed_save_rerender_labels_the_saved_revision` (422) |
| 15 | same | `history_recomputes_identically` (rev 1 advisory/derived HTML captured before the edit equals `/history/1` after) |
| 16 | same | `withdrawn_approval_still_evaluates_with_warning` |
| 17 | same | `formula_text_is_escaped` (`<script>` in label, constraint message, expression string literal and enum option label → `&lt;script&gt;`) |
| 18 | same | `viewing_never_writes` (revision and history count unchanged after GETs) |
| 19 | same | `native_mork_borg_view_and_save_never_evaluate` (no `#advisory-checks`; no `FormulaView` built; existing native tests green) |
| 20 | `src/definitions/publication_tests.rs:188` | renamed `list_definitions_stay_drafts_until_editing_ships`: scalar formula definition publishes with `deterministic_rules == "formulas-advisory"`; list definition still `list-editing-unsupported` |
| 21 | `src/definitions/formula_pack_tests.rs` | `lantern_reach_characters_reproduce_stored_derived_values` (4 × 11 derived; 4 × 21 passes) |
| 22 | same | `second_outer_armor_gives_one_outer_armor_violated` (equip `cuirass-1` on `bram-holt`) |

**Fixture for 10–19** (scalar only; lists gated):

- inputs: `label` (text, label `<script>Label`), `vitality` (int), `luck` (nullable int),
  `kind` (enum);
- derived: `pace = 6 - vitality`; `lucky = luck + 1` (`null_value` when unknown);
  `lucky_twice = lucky * 2` (`dependency_failed`); `tier` (enum)
  `= vitality > 3 ? "high" : "low"` with option label `<b>High</b>`; `marked` (bool)
  `= label == "<script>"`;
- constraints: `vitality-cap: vitality <= 10` with message `<script>alert(1)</script>`;
  `lucky-positive: lucky > 0` (error);
- evidence unresolved. Verify first that approval accepts unresolved formula evidence
  (parent §15); otherwise give the fixture a `core` source.

**Pack translation (test only):** `type` → `control`; `collections` → list fields with
`items`; rule IDs lowercased (`LR2-5` → `lr2-5`); all fields in one group; character
`values` split into inputs and expected derived values (expectations come from the stored
characters, not engine output). The test prints compile/evaluate times for D1.

## 5. Milestone steps (each one commit plus review)

1. **Thread `Compiled` into `Admitted`/`ImmutablePin` (`ImmutablePin::evaluate`).** Tests 4,
   21–22 (pack tests need only the pin). Files: `formula_admission.rs`, `admission.rs`,
   `private_pin.rs`, new `formula_pack_tests.rs`, `definitions/mod.rs`.
2. **`formula::preview`.** Tests 1–3. File: `formula/mod.rs`.
3. **View model `campaigns/formulas.rs`.** Tests 5–9; exhaustive code→message mapping; no
   template change.
4. **Wiring plus gate lift, one commit.** `render_private` calls `pin.evaluate` and
   `present` inside `rendered`'s `spawn_blocking`; `FieldView` derived/read-only branch;
   `private_sheet.html` inline `<output>`, summary line and `#advisory-checks`; CSS only if
   needed (reuse `.panel`, `.badge`, `.evidence-text`; `body` already has
   `overflow-wrap:anywhere`); remove `publication.rs:134-136`. Tests 10–20. Failing-first:
   write 10–19 and the fixture, apply the gate removal locally, confirm they are red on
   rendering (e.g. test 11 on `name="value-pace"`), then implement. Combined so no commit
   allows publishing a definition whose view cannot save.
5. **Docs** (§7), TODO and review record.
6. **Browser fixture and scenario** (Q3 answered "M3"). Q5 dependency naming (engine accessor) is in step 2, see §10.5.

Mutation checks for the reviewer: render evaluation skipped; derived fields rendered as
inputs; gate restored; `present` maps errors to values; history evaluates live data;
escaping changed to `|safe`; native branch evaluating.

## 6. Accessibility and mobile

- Derived values: `<output aria-labelledby>` pointing at the visible label; not focusable
  controls; "Computed" badge.
- Advisory section: `<section aria-labelledby>` with heading and `<ul>`; statuses as words
  (Pass, Violated, Not evaluated). No live region (static full page; existing
  `role="status"` stays the save-message channel).
- Native `<details>` for passes and expressions.
- Reflow: no fixed widths; expressions `pre-wrap`/`overflow-wrap:anywhere` so space-free
  1,024-byte expressions wrap; codes inline `<code>`.
- Mobile overflow is verified in the browser gate (Q3); unverified until then, and the docs
  say so.

## 7. Docs to update

| Doc | Change |
|---|---|
| `docs/runtime-definition-contract.md:37` | Formulas/constraints supported and advisory under `formula_profile: 1`; lists unsupported until M4 |
| `docs/definition-drafts.md:32` | "No … formulas" line points to the profile |
| `docs/definition-publication.md` | Formula definitions publishable; `formulas-advisory`; `unresolved_formulas`/`unresolved_constraints`; list gate remains; stale evidence does not stop evaluation |
| `docs/release-scope.md` | Advisory derived values and checks on runtime sheets; not legality; browser-scope note (Q3) |
| `docs/sheet-presentation.md:14` | Derived values update on save/reload; no live recompute |
| `docs/campaign-workspace.md` | History recomputes from the immutable pin |
| `docs/architecture.md:66` | `preview` exists as a library function; no tool loop yet |
| `docs/derived-formulas-plan.md` | New §16 "M3 implementation" incl. pack-test rename and `preview` correction |
| Review record, `TODO.md` | `docs/reviews/2026-10-1x-derived-formulas-m3.md`; TODO ACTIVE entry and handoff |

## 8. Container gates (parent §13, adapted)

```sh
git -C /home/marco/Progetti/mimik-wt/formula merge --ff-only master   # expect no-op at 1166a1d
df -h /var/lib/docker                                                 # stop below 10G
WT=/home/marco/Progetti/mimik-wt/formula
RUN="docker run --rm --network none --cpus 4 --mount type=bind,src=$WT,dst=/workspace \
  --mount type=volume,src=mimik-cargo-cache,dst=/opt/cargo/registry \
  --mount type=volume,src=mimik-formula-target,dst=/target \
  -e CARGO_TARGET_DIR=/target/formula mimik-ci:latest"
$RUN cargo fmt --check
$RUN cargo clippy --locked --all-targets --all-features -- -D warnings
$RUN cargo test --locked --all-targets --all-features
$RUN cargo test --release --locked --lib formula        # pack timings printed, report-only
$RUN cargo build --locked --example formula_eval
$RUN ruff check tools
$RUN ruff format --check tools
$RUN python -m unittest discover -s tools -p "test_*.py"
$RUN actionlint .github/workflows/ci.yml
$RUN cargo about generate --locked --fail about.hbs -o /target/formula/THIRD-PARTY-NOTICES.md
$RUN cargo build --release --locked                     # re-measure vs 18,210,072 bytes
```

- `cargo audit --deny warnings` is the one networked run (advisory DB fetch).
- One command per `docker run`; no long `bash -c` chains.
- Lifting the gate changes shipped behaviour, so the CI package, unpacked and browser gates
  run in M3 as regression: `cargo package` + `verify_source_package.py`; `cargo vendor` +
  `package_release.py`; `smoke_release.py --auth`; the `mimik-browser` container run.
- Afterwards `cargo clean --release` in the volume. Report every unrun gate as not run.
- Do not build while a benchmark run mounts `mimik-formula-target`, or the user is using
  the machine (CPU load skews benchmark timeouts).

## 9. Risks

1. **JOBS = 1 is shared with draft and publication admission.** Sheet requests hold it
   longer (compile + evaluation), so more 503s under concurrent use. Mitigation: measure in
   the pack test; a pin-level cache is a follow-up, not M3.
2. **`input_too_large` on stored-valid sheets** (§1). Mitigation: explicit "unavailable"
   state, test 9.
3. **History not identical under overload** (`deadline_exceeded`, `internal_error`).
   Documented; shown as a state, never a value.
4. **Latent bug** (derived fields rendered as inputs). If the gate lift landed without the
   wiring, every formula sheet save would 400. Mitigation: one commit (step 4), test 11.
5. **Unresolved-evidence publication assumption** for the step-4 fixture (§4).
6. **Pack translation in the test duplicates the benchmark's.** Keep it minimal and
   reviewed; expectations from stored characters, not engine output.
7. **Escaping relies on Askama defaults.** Test 17 and the mutation check.
8. **No cross-family independent reviewer so far** (M1/M2 used Sonnet). Record it if still
   true.

Critical files: `src/definitions/private_pin.rs`, `src/definitions/formula_admission.rs`,
`src/campaigns/web.rs`, `templates/private_sheet.html`, `src/definitions/publication.rs`.

## 10. Plan review amendments (binding)

Review record: [2026-10-16-derived-formulas-m3-plan.md](reviews/2026-10-16-derived-formulas-m3-plan.md).
Where this section differs from §1–§9, it wins.

1. **(B1) Commit shape.** `ImmutablePin::evaluate` (`pub(crate)`) and `campaigns/formulas.rs`
   (`pub(super)`) would be dead code until wiring, failing `clippy -D warnings`. Steps 1, 3
   and 4 merge into **one commit** (pin threading + view model + wiring + gate lift). Step 2
   (`formula::preview`, reachable via `pub mod formula`) may stay separate, and so may the
   pack tests if they compile without new dead code. No `#[allow]`.
2. **(B2) Create page.** Evaluation is skipped when `sheet_id == 0`. Route tests: GET create
   for a formula pin has no `value-<derived>` and shows "Derived values are computed after
   the first save"; POST create → 303; failed create (422) re-renders without evaluating.
3. **(M1) Test 4** compares `evaluate` outputs on the same values, not `Compiled` equality;
   no `PartialEq`/`Debug` added to `Compiled`.
4. **(M2) Drop/carry.** The only new drop case is a pin dropped on async workers or the DB
   worker; add a test building a max-depth `Compiled` on one thread and dropping it on a
   fresh 2 MiB thread. `domain` returns `Option<Compiled>` explicitly; draft, store and
   publication callers (`store.rs:30`, `store.rs:180`, `publication.rs:348`) drop it at once
   (or use a `check`/`check_and_compile` split), so only the pin carries it.
5. **(M3) Q5 design.** `Compiled::failed_dependency(&Evaluation, kind, id) -> Option<&str>`
   filters declared deps by `derived_order()` (not `BTreeSet` order), covers derived fields
   and constraints (separate namespaces unless proven disjoint), and leaves `Evaluation`,
   `Code` and the golden digest unchanged. Tests: chain `a→b→c` (`b` names `a`, `c` names
   `b`); a constraint naming its dependency; a deadline cascade reports `deadline_exceeded`,
   not `dependency_failed`. These go with step 2.
6. **(M4) Permit and deadline.** Document that the JOBS permit stays held after a client
   disconnect (`spawn_blocking` is not cancellable), bounded by compile + 500 ms + one
   formula. Route tests must not flake under CPU load: add a `#[cfg(test)]` deadline seam on
   `ImmutablePin::evaluate` or make assertions tolerate `deadline_exceeded`. Record pack
   compile/evaluate timings in the M3 review record before accepting JOBS = 1.
7. **(M5) History wording.** "Recomputes from the sheet's immutable pin and the historical
   data." Test 15 edits a value that changes a derived result and compares extracted
   derived/advisory fragments (not full pages, which carry CSRF and revision).
8. **(M6) Test 12** also covers a same-count substitution (one valid key swapped for
   `value-<derived>`) and a `null-<derived>` key.
9. **(M7) Test 11** also asserts no `for="value-<derived>"`; test 19 asserts the native page
   has no `advisory`/`Computed` markers; existing native save tests stay green.
10. **(M8) Unavailable at route level.** Add a route test for a stored-valid sheet whose
    evaluation returns `InputError` (page 200, "unavailable"); document that such saves are
    allowed.
11. **(M9) Fixture.** Before tests 10–19, a throwaway admission check confirms the fixture
    admits (nullable `luck + 1`, unresolved evidence publishable); adjust if not. Test 17
    asserts the escaped string is present **and** the raw string absent, including attribute
    contexts.
12. **(M10) Browser scope (step 6)** adds: how the formula definition is published/approved
    in the browser harness fixture; the saved form POST carries no derived name; a
    1,024-character unbroken expression and a long text value at mobile width; keyboard
    operation of the `<details>`; native MÖRK BORG layout unchanged. `release-scope.md`
    lists native / formula scalar / list (M4) separately.
13. **(L1) Markup.** Use a `<dl>`/`<dd>` (or `<p><strong>`) with a visible "Computed" label,
    not `<output>` (implicit live region). Advisory section and `<details>` sit outside the
    `<fieldset disabled>` so they work in read-only history.
14. **(L2)** Derived text values use `white-space: pre-wrap` (reuse `.evidence-text` or one
    new rule), checked at mobile width.
15. **(L3)** Record in parent plan §16 that evaluation happens at render time with PRG (user
    decision), a deviation from parent §7 "save evaluates proposed data".
16. **(L4/L5)** Keep `preview` minimal; correct `architecture.md`. The pack translation is
    the only duplicated code; expectations come from the stored characters.


### 10b. Further amendments from the reviewer comparison (binding)

Source: [reviewer comparison](reviewer-benchmark.md) (Sonnet B,
Muse Spark, Space Bunny on the same pre-amendment plan; each finding adjudicated against
the code).

17. **Gate lift goes last.** The `publication.rs:134-136` removal and test 20 land in the
    final commit, after the docs (step 5) and the browser scenario (step 6) pass. Until then
    route tests publish through a test-only fixture path. No commit may allow publishing
    while the docs or browser gate still describe formulas as unsupported.
18. **Round-trip regression.** New test `rendered_form_round_trips`: parse every `name`/value
    from the GET HTML, POST them back, expect 303 and revision +1. This is the failing-first
    proof of the latent bug. Test 11 builds its expected key set from spec literals and also
    asserts that each derived value **is rendered** (so omitting derived fields fails it).
    Tests 12 and 18 are permanent guard tests, not red-first; 13, 14 and 16 stay red-first.
19. **Memory bound.** `depart` (`src/campaigns/domain.rs:218-233`) validates up to 32 pins at
    once. The bound is therefore 32 compiled sets, not one; `Compiled` drops can also happen
    inside the DB mutex. Measure the AST size of the largest admitted definition in the
    pack-timing test and record it.
20. **"Unavailable" is `InputError` only.** A failed formula thread yields `failed_all`
    (`formula/mod.rs:304-305, 363-376`) and cannot be told apart, so it renders as
    per-value `internal_error` states, not as "unavailable".
21. **Failed-save re-render.** Derived values and checks on a 422/409 re-render come from
    the saved revision and are shown in a visually separate block headed "Computed from
    saved revision N — not from your unsaved changes". The draft is not evaluated (it is
    invalid by definition on 422). Test 14 asserts the heading and the separation.
22. **Text values.** Test 6 adds `Scalar::Text`. Test 17 adds a derived text field echoing a
    `<script>` input, checked on the view, the 422 re-render and `/history/N`, including
    attribute contexts. A source check asserts that the new template code contains no
    `|safe`.
23. **Messages.** Test 5 asserts distinct, non-placeholder wording for every
    evaluation-reachable code. No reachable code may use the "unexpected formula state"
    text. The code→message table is documented in `runtime-definition-contract.md`.
24. **503 after save.** Add a route test: with the permit held, the GET that follows a
    successful save returns 503 with `Retry-After`. Set `Retry-After` from the measured
    worst-case hold (compile + conversion + 500 ms + one formula), and document that an
    abandoned request keeps the permit until it finishes.
25. **Single-formula bound.** The per-formula wall time is bounded statically by
    `MAX_COST`, not by the deadline. Cite the M1 calibration (≈18 ms at the cap) in D2.
26. **Logging.** `internal_error` and `deadline_exceeded` are logged with sheet ID, pin
    version and formula ID, never with values.
27. **Pack test scope.** The M1 conformance fixture already evaluates all four characters
    and the two-outer-armor mutation at engine level
    (`schemas/formula-conformance-v1.json:1594…3502`). `formula_pack_tests.rs` therefore
    only covers what the fixture cannot reach: envelope → admission → pin → `validate_values`
    → `ImmutablePin::evaluate`. Use one character plus the mutation, and keep the translation
    helper minimal.
28. **Browser fixture touch points.** Step 6 edits `tests/support/source_fixture.rs` (a new
    scalar, list-free formula definition with `formula_profile: 1`),
    `tools/smoke_release.py` (`prepare_workspace_materials` publication of a second
    version) and `tools/browser_smoke.py`. §8 adds CI's
    `cargo run --locked --example source_fixture -- target/smoke-source`.
29. **Docs.**
    - `sheet-presentation.md` scopes the "no live recompute" note to runtime definition
      sheets; the native htmx editor is unchanged.
    - Admin Phase B coordinates `definition-publication.md` and `published()` edits with M3.
    - `TODO.md` marks M2 done with its review record.
    - The parent plan records that the `pub mod formula` decision (§14.1 item 18) is
      unchanged: `preview` stays `pub` in `pub mod formula`, because `pub(crate)` without a
      caller would be `dead_code`.
30. **`preview` stays minimal** and is documented as deadline-subject. An error on either
    side is reported as "unknown", never as a change or a resolved violation.

### 10c. Amendments from the free-model reviews (binding)

Source: [reviewer benchmark](reviewer-benchmark.md), answer-key items K34–K39. The main
session checked K35, K38 and K39 against the code.

31. **(K34) `pub mod formula`:** record the M1/M2 open item as decided for M3. It stays
    `pub` (the CLI example and `preview` need it), and the parent plan records why.
32. **(K35) Pack translation:** drop or map the pack's top-level `encoding`,
    `rules_revision` and `formula_language`. The `deny_unknown_fields` envelope rejects
    them (`examples/tool-loop-demo/definition.json:2,5,7`).
33. **(K36) Release-size baseline:** re-measure the current release build first and use it
    as the baseline. 18,210,072 B predates `cel`; the spike measured 20,288,152 B.
34. **(K37) "Unavailable" signature:** `present` takes `Result<&Evaluation, &InputError>`,
    or the caller branches into `FormulaView::unavailable`. Pick one in step 3 and test it.
35. **(K38) Permit scope:** only `/campaigns/{id}/sheets/new`, `/private-sheets/{id}` and
    `/private-sheets/{id}/history/{revision}` sit behind the `runtime` permit layer.
    `/private-sheets` and `/private-sheets/{id}/history` do not
    (`src/campaigns/web.rs:59-79`). Correct §1 accordingly.
36. **(K39) Save conflict when busy:** the 409/422 re-render re-acquires the permit with
    `admit()` and, when it is busy, returns a bare 503 with no `Retry-After` and loses the
    draft (`src/campaigns/web.rs:768-769`). Longer permit holds in M3 make this more likely.
    Either keep the permit from the failed save for the re-render, or return 503 with
    `Retry-After` and say the draft was not saved. Add a failing-first route test.

### 10d. Amendments from the OpenAI Sol review (binding)

Source: [reviewer benchmark](reviewer-benchmark.md), K40–K44. These were found only by Sol.
The main session checked K40 and K43.

37. **(K40, weight 2) Project the values before evaluating.** Hyphenated scalar field IDs
    pass admission but are not declared to the engine (`formula_admission.rs:258-263`,
    guarded by `cel_identifier`). `convert::inputs` rejects every undeclared key
    (`formula/convert.rs:151-153`). Passing the stored sheet data unchanged would therefore
    make **every** evaluation of such a definition return `InputError` ("unavailable").
    - `ImmutablePin::evaluate` passes only the declared input fields.
    - Failing-first test: a formula definition with a hyphenated, non-referenced scalar
      field evaluates normally.
    - Undeclared keys that are not definition fields must still be rejected.
38. **(K41) Checks:** `CheckView` carries the constraint expression too, so the collapsed
    "How this is computed" works for checks, not only for derived values.
39. **(K42) `preview` transitions:** violated→error and error→violated are reported as
    "unknown", never as resolved or new violations. Test both.
40. **(K43) Dependency record:** add the `cel`/`antlr4rust` decision to
    `docs/dependency-decisions.md`, as M1 required (it is missing). Cover the version pin,
    the license, the alternatives rejected (evalexpr, fasteval, rhai, a custom winnow
    parser), the lockfile gate and the exit path. This is done in M3 step 5, or earlier as
    a docs-only fix.
41. **(K44) Field ID grammar:** `runtime-definition-contract.md` documents the `_` that M2
    allows in field IDs.
