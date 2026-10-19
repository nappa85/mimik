# Derived formulas and advisory checks

Runtime definitions can declare derived fields and advisory constraints ("checks") as CEL
expressions. Results are advisory: they never write a sheet, never block a save, never
approve evidence and are not a legality ruling. Derived values are never stored; they are a
pure function of the immutable pin and the stored values.

Decisions are listed in `docs/decisions.md` under "Formulas and sheet values". Code comments
in `src/campaigns/` cite the list-editing decisions as "plan D1" … "plan D11"; those are the
entries **D1–D11** there. (Plain "plan Dn" in `src/catalogues/`, `src/main.rs` and
`src/definitions/mod.rs` refers to the catalogue decisions instead.)

## Profile and engine

- Profile ID `cel-mimik-1` (`formula::PROFILE`), selected in a definition by the hashed root
  member `"formula_profile": 1` (`formula_admission::PROFILE`). Any other value is
  `unsupported-formula-profile`. Formulas, lists or constraints without the marker are
  `formula-profile-required`; the marker without them is `formula-profile-without-formulas`.
- Engine: `cel = "=0.15.0"`, `default-features = false`, with its parser runtime
  `antlr4rust` 0.6.0. `pinned_engine_versions_in_lockfile` fails on any version change of
  either. Selection record: `docs/decisions.md` (Dependencies).
- Until the first release, profile 1 may still change without a bump (D-Y). From the first
  release on, any change to admitted semantics or outcomes, including one caused by an engine
  bump, needs `formula_profile: 2`; a pinned definition is never reinterpreted.
- Mimik supplies what cel-rust lacks: static typing, the allowlist, depth, nesting and cost
  bounds, the aggregate functions, JSON→CEL conversion (`cel::to_value` would make
  non-negative integers `uint`) and stable error codes.
- CEL semantics kept as they are: truncating `/` and `%` (`-7 / 2 == -3`, `7 % -2 == 1`);
  commutative error absorption in `&&`/`||` (`err || true` is true, `err && true` is the
  error); lazy `?:`. `!` binds tighter than comparison: write `!(a == b)`.

## Admitted subset

Admission (`profile.rs`) runs on one parse, the macro-expanded AST from `cel::Env::stdlib()`
with optional syntax off. Anything not listed is rejected.

- **Literals:** int, bool, string, and `null` only as an operand of `==`/`!=` against a
  nullable field. Double, uint, bytes, list literals (except as `in` right side), maps and
  structs: `unsupported_construct`.
- **Identifiers:** declared scalar fields, list fields, derived fields (these become
  dependencies), and bound iteration variables. Anything else is `unknown_field`.
- **Selection:** only `<iteration variable>.<property>` and `has(<iteration variable>.<property>)`;
  every item also has `id` (string, at most 64). Indexing, optional syntax, member calls and
  selection on a scalar or list: rejected.
- **Operators and types:** int: `+ - * / %`, unary `-`, `< <= > >=`; bool: `! && ||`; `==`/`!=`
  need equal types (int, bool or string; text and enum are both string). Strings have no
  ordering or `+`. `c ? a : b` needs a bool condition and equal branch types.
- **`in`:** left side int or string; right side a non-empty literal list of literals of that
  type (`x in equipment` is rejected).
- **Macros:** exactly the expansions of `L.filter(x, p)`, two-argument `L.map(x, e)`,
  `L.exists(x, p)` and `L.all(x, p)`, over a list field or a `filter`/`map` chain on one.
  `exists_one`, three-argument `map`, comprehensions over a literal and any macro inside
  another macro's body (`nested_aggregate`) are rejected. `map` must yield int, bool or string.
- **Iteration variable:** an identifier that is not reserved (except `item`), not a declared
  name and not already bound. `item` is reserved as a field name so it can serve here.
- **Functions:** `size(list)` and the catalogue: `sum(list<int>)` (checked add, empty 0),
  `max_of(list<int>, default)`, `min_of(list<int>, default)` (default when empty, evaluated
  eagerly), `clamp(x, lo, hi)` (`lo > hi` → `invalid_argument`), `max`/`min` with 2–256 int
  arguments. Every other stdlib function: rejected. Count is `size(L.filter(...))`.
- **Results:** a derived field's expression must type as its declared type (int, bool, text
  or enum), with literal enum typos rejected (`unknown_enum_option`); a check must be bool.
  Lists cannot be results.
- **Identifiers in declarations:** `[a-z][a-z0-9_]{0,63}`, not in `RESERVED` (CEL keywords,
  type names, macro and catalogue names, `item`, `id`). Constraint IDs are registry slugs in
  their own namespace. Derived-field cycles are `formula_cycle`; evaluation order is Kahn's
  algorithm with declaration-order ties.

Admission failures surface in definition validation as `formula-<code with dashes>` at the
envelope path of the field, constraint or list (`formula_admission::rejection_path`).

## Static limits

Constants in `src/definitions/formula/mod.rs` unless noted:

| Limit | Value |
|---|---|
| `MAX_SOURCE_BYTES` (per expression, UTF-8) | 1,024 → `source_too_long` |
| `MAX_PARSER_DEPTH` (cel parser recursion guard) | 32 → `depth_exceeded` |
| `MAX_AST_DEPTH` (expanded AST, iterative walk) | 64 → `depth_exceeded` (flat chains over about 60 terms) |
| `MAX_FIELDS` / `MAX_LISTS` / `MAX_PROPERTIES` / `MAX_OPTIONS` | 256 / 32 / 32 / 128 |
| `MAX_FORMULAS` (derived + constraints) | 384 |
| `MAX_LIST_ITEMS` (shared with the value schema and form budget) | 256 |
| `MAX_TEXT_CHARS` | 20,000 (text fields 8,000, multiline 20,000: `formula_admission`) |
| `MAX_INPUT_BYTES` (string bytes + 16 per value) | 1 MiB → `input_too_large` |
| `MAX_COST` (static work units per definition) | 2,000,000 → `cost_limit` |
| `DEADLINE` (wall clock, checked before each formula) | 500 ms → `deadline_exceeded` |
| `FORMULA_STACK_BYTES` (dedicated thread) | 16 MiB |

**Work units** (`Checker::charge`): one per AST node; inside a comprehension body every unit is
multiplied by 256, plus 4 per iteration for accumulation; `size`, `sum`, `max_of`, `min_of`
add 256 each; a string `==`/`!=`/`in` adds `min(declared lengths) / 64 + 1`.

**Calibration of `MAX_COST`** (`worst_case_admitted_definition_is_cost_bound`): the largest
admitted worst case is 24 constraints of `pages.exists(item, item.text == note)` over 256
items of 20,000-character declared text, cost 1,966,128. With nearly equal 4,000-character
texts (the most that fits `MAX_INPUT_BYTES`) it evaluated in about 18 ms in a release build
(single shared-host samples, 17.9 and 18.9 ms), far under the 500 ms deadline. The test
asserts the static cost only and prints the time. For scale, the Lantern Reach pack compiles
(admission, pin and compile) in 11–14 ms and evaluates in under 1 ms.

Compile and evaluation run on a scoped 16 MiB thread (2 MiB overflows in debug builds inside
these limits); a panic fails closed as `internal_error`.

## Evaluation

`Compiled::evaluate(values)` first converts every declared input (`convert::inputs`): exact
keys (derived IDs are never inputs), integers through `as_i64` only, enum values in their
options, text within its length, items with unique slug `id`s, at most 256 items. A mismatch
is an `InputError` (`invalid_input`, `input_too_large`, `collection_too_large`) and nothing is
evaluated. Derived fields then run in topological order, each result checked against its
declared type (`invalid_result`). Checks are tri-state (pass, violated, error); an error never
counts as either.

Per formula, before running it: `deadline_exceeded` if the deadline passed; else
`dependency_failed` if a derived field it reads failed; else `null_value` if it reads an
unknown integer (below). `Compiled::failed_dependency` names the first failed dependency in
evaluation order, and the sheet page says "it uses <label>, which could not be computed".

### Failure codes

`src/definitions/formula/error.rs` defines 29 stable snake_case codes; `cel` message text is
never shown or stored. Runtime mapping (`from_execution`): `Overflow` → `overflow`;
division or remainder by zero → `division_by_zero`; `NoSuchKey` → `missing_property` (an
absent optional property read without `has()`); operator, comparison or overload errors with a
`null` operand → `null_value`, otherwise `type_mismatch`; catalogue functions report their own
code; anything else, including a future `cel` variant, → `internal_error`. Admission-only
codes (`parse_error`, `source_too_long`, `depth_exceeded`, `nested_aggregate`,
`unsupported_construct`, `unknown_property`, `unknown_enum_option`, the identifier and
declaration codes, `formula_cycle`, `cost_limit`) are never produced by evaluation.

`src/campaigns/formulas.rs::message` gives every code its human wording through an exhaustive
match; admission-only codes share "unexpected formula state; report this sheet".
`deadline_exceeded` and `internal_error` are the only non-deterministic outcomes; they are
shown as states ("Reload to retry") and logged with sheet ID, pin version and formula ID,
never values.

### Unknown and null values

- `null` is the unknown value of every integer field and every integer item property,
  nullable or not. Text, enum and boolean values accept `null` only when the field is
  nullable. An optional item property may be absent (no key); a present integer property may
  be `null` (unknown) whether or not it is optional. Absent and unknown are different states
  (`has()` is true for a present unknown, L-U3).
- A formula whose static read set contains an unknown integer field, or an integer property
  that is `null` in any item, is not run and reports `null_value` (D-Z). This holds on
  untaken branches (`true || vitality == 0` is `null_value`) because CEL would otherwise give
  `null == 0` = false and `null != 0` = true. Its dependents report `dependency_failed`.
- So `== null` is no fallback for integers: `luck == null` is `null_value` when `luck` is
  unknown. `has(item.p)` tests presence only; an unknown property is present.
- Nullable text, enum and boolean fields keep CEL's null semantics. A `null` result (a copy of
  a null field, or a check returning null) is `null_value`.

### Where it runs

- **View and history** (`campaigns/web.rs`, the `rendered` blocking step under the JOBS
  permit): once per response, of the saved snapshot only, through
  `ImmutablePin::evaluate` (`definitions/private_pin.rs`). History recomputes from the sheet's
  immutable pin and the historical data. Stale or withdrawn approval does not stop it.
- **Declared-input projection:** `ImmutablePin::evaluate` first drops stored values of input
  fields the formula layer does not declare (non-identifier IDs such as `hit-points`, which
  stay valid field IDs but cannot be referenced: `formula_admission::undeclared_inputs`). Any
  other undeclared key still reaches the engine and is refused.
- **Save** uses post/redirect/get: the POST evaluates nothing, so a violating save is never
  blocked; the GET that follows shows the results. A rejected save (422/409) and draft
  operations re-render the draft beside "Computed from saved revision N — not from your
  unsaved changes"; the draft is never evaluated. The create page shows "Derived values are
  computed after the first save."
- **Unavailable:** an `InputError` on stored-valid values renders "Derived values
  unavailable: <message>" with its code; the page stays 200.
- **Preview:** `formula::preview(compiled, before, after)` returns changed derived values and
  changed checks; `NewViolation` and `Resolved` only for pass↔violated, any error side is
  `Unknown`. Library-only (no route or CLI op); each side is subject to the deadline.
- **Write-only pins** (`private_pin::validate_for_write`, used when freezing up to 32 sheets)
  drop the compiled formulas; evaluating one fails closed as `internal_error`.
- **Native MÖRK BORG sheets** and unsupported pins never call the formula layer.
- `examples/formula_eval.rs` is a JSON-lines development CLI over the same engine (used by the
  Python tool-loop benchmark).

## Lists and plain list editing

A `control: "list"` field declares `items.properties` (at most 32; text, multiline, integer,
boolean or enum; optional or required; ID a CEL identifier, not `id`). The list field is not
nullable, cannot carry a formula and must have an identifier ID. Stored values are at most 256
objects with exactly `id` plus the declared properties. Every list property is editable; there
is no per-property read-only flag and no item catalogue in the definition.

As built, with the details in decisions D1–D11 (`campaigns/values.rs`, `transport.rs`,
`web.rs`, `snapshots.rs`, `history_diff.rs`):

- Full-form list transport: exact key set `id-f-r`, `row-f-r-p`, `set-f-r-p` plus the clicked
  `op`, at most 65,536 pairs (`SHEET_FORM_PAIRS`); host IDs `r<N>-<k>` enforced by the writer
  (`snapshots::host_item_ids`). Body and echo limits: `docs/campaigns.md`; per-unit routes:
  `docs/assisted-entry.md`.
- On the create page and the interim list forms (P1) until 1.5b, Add, Remove, Move up and Move
  down re-render an unsaved draft (200, never written, never evaluated); Save persists (D5).
- Publication refuses a definition whose worst-case form exceeds 32,768 render units
  (`values::RENDER_UNITS`) → `list-form-exceeds-limit` (D7; headroom in
  `docs/assisted-entry.md`).
- History shows ID-labelled read-only rows and a "Changes in revision N" summary (names only,
  at most 200 lines).

## Conformance fixture

`schemas/formula-conformance-v1.json` is the golden behaviour of the profile: 152 expression
cases and 35 sheet cases over two declaration sets (`basic` and the Lantern Reach `lantern`,
whose four characters and the two-outer-armor mutation reproduce the demo pack's stored
values). Each case pins an outcome: a value, an evaluation error, a rejection code or an input
error. Tests in `src/definitions/formula/tests.rs`:

- `conformance_fixture_is_hash_locked`: SHA-256 of the file must equal `FIXTURE_SHA256`
  (`5af7ce8f…cb6b40`, content revision 2). `tools/test_formula_cli.py` pins the same digest
  and checks the one `formula_eval` reports.
- `conformance_expressions`, `conformance_sheets`: every case must match.
- `fixture_covers_every_code_not_tested_below`: every code except the limit and internal ones
  appears in the fixture; at least 150 cases.
- `expanded_comprehensions_match_unexpanded_macro_calls`: the recognised expansion shapes match
  user-written macro calls one-to-one.

Changing the file (any byte, including its `note`) changes the digest. A change is accepted
only through a reviewed profile decision that updates the case expectations, the note's
content revision and both pinned digests together. Before the first release this stays under
profile 1 (D-Y); after it, an outcome change means profile 2.

## Code map

- `src/definitions/formula/`: `mod.rs` (API, limits, evaluation, `preview`), `profile.rs`
  (allowlist, types, cost), `convert.rs` (inputs, unknowns, result check), `functions.rs`,
  `error.rs`, `tests.rs`.
- `src/definitions/formula_admission.rs` (envelope rules, `formula_spec`, `undeclared_inputs`),
  `private_pin.rs` (pin holds `Compiled`), `src/campaigns/formulas.rs` (view model, wording),
  `templates/private_sheet.html`.
