# Derived-field formulas: implementation plan (revision 2, CEL)

Status: **plan only. Awaiting independent review of this revision.** No
application code, dependency or schema in this worktree has changed.

- **Revision 1** was a custom language with a winnow parser. It was reviewed and
  rated GO-with-amendments.
- **Revision 2** follows the user's later decision to use the
  [Common Expression Language](https://github.com/google/cel-spec) standard
  through the maintained `cel` crate. The user explicitly accepts CEL semantics,
  including truncating integer division and commutative, error-absorbing
  `&&`/`||`. Aggregates are added as custom functions.
- The still-relevant amendments from the revision-1 review are carried over
  (section 9).
- Revision 1 is summarised in Appendix A.

Decision basis: `discovery.md` "Harness assessment decisions (user, 2026-10-15)",
plus the user's CEL decision (to be recorded in `discovery.md` at
implementation). The rules-DSL exception is unchanged: CEL is used for
derived-field and advisory-constraint expressions only, and admission restricts
it to the subset in section 3.

Evidence classes:

- **Spike:** measured in throwaway container builds on 2026-10-15 (section 2).
  The spike sources and the fit copy were deleted afterwards.
- **Registry:** crates.io/PyPI API data fetched the same day. These are claims,
  not audits.

## 1. Verdict

**GO for `cel = { version = "=0.15.0", default-features = false }`.**

- The licence and audit gates pass on the combined tree.
- No `about.toml` change is needed.
- The release binary grows by about 2.0 MiB.
- Every capability we need exists, except three, which Mimik must supply:
  - static type checking (cel-rust has no checker);
  - cost and work limits (cel-rust has none);
  - an allowlist of the CEL subset.

  These are implemented as a bounded AST walk over the public `cel::common::ast`
  types.

One conversion pitfall was found: `cel::to_value(&serde_json::Value)` turns
non-negative JSON integers into **`uint`**. Arithmetic then fails, for example
`UnsupportedBinaryOperator("add", UInt(..), Int(1))`. Mimik must convert
canonical values itself (JSON integer → `Value::Int`, see section 4). A
regression test is required.

## 2. Fit check of `cel` 0.15.0 (evidence)

### Registry facts

| Item | Value |
|---|---|
| `cel` | 0.15.0, released 2026-10-06. The 0.14.x line had monthly releases from 2026-06 to 2026-09. 4.23M downloads (3.71M recent). MIT. Repo `cel-rust/cel-rust`. MSRV 1.86 (we use 1.95). |
| Features | Default is `regex`, `chrono`, `ext_encoders`. We disable all of them: no timestamps/durations, regex or base64. `json` is also not needed, because Mimik owns the JSON→CEL conversion. `parser_pratt` is an alternative parser, but `antlr4rust` remains a **required** dependency, so it saves nothing. Not used. |
| `antlr4rust` | 0.6.0, released 2026-09-27 (0.5.x 2025-10). 4.18M downloads. BSD-3-Clause, which is already in the accept list. Repo `antlr4rust/antlr4`. MSRV 1.85. It is the ANTLR runtime for cel's generated grammar, a community-maintained Rust target. This is the main maintenance risk, since there is one upstream for the parser runtime. Mitigations: an exact `=` pin, cargo-audit in CI, and re-review on every bump. |

### Combined-tree measurements (spike)

These were measured on a copy of this worktree with `cel` added and a minimal
real use in `main.rs`, so that the code was linked.

- **New packages in the Linux build graph (12):** antlr4rust 0.6.0, bit-set
  0.11.1, bit-vec 0.10.1, byteorder 1.5.0 (Unlicense OR MIT), cel 0.15.0,
  lazy_static 1.5.1, minimal-lexical 0.2.1, murmur3 0.4.1, nom 7.1.3, pastey
  0.2.3 (proc-macro), typed-arena 2.0.2 (MIT), uuid 1.28.0 (`std` only, MSRV
  1.89).
- **Lockfile:** it also records toml_edit/winnow/borsh entries, which `cargo tree`
  shows are not in the normal Linux graph.
- **Duplicates introduced:** nom 7.1.3 (cel) next to nom 8.0.0 (lopdf);
  bit-set 0.8/0.11 and bit-vec 0.8/0.10 (jsonschema's fancy-regex vs
  antlr4rust). These are accepted transitive duplicates of maintained crates.
  Reassess when upstreams converge, consistent with the existing html5ever
  precedent in `dependency-decisions.md`.
- **`cargo audit --deny warnings`:** exit 0 (322 lockfile crates, advisory DB
  of 2026-10-15).
- **`cargo about generate --locked --fail`:** exit 0 with the **unchanged**
  `about.toml`.
- **Release binary:** 18,210,072 → 20,288,152 bytes, which is **+2,078,080
  bytes (+11.4%)**. Real integration will add a little more and must be
  re-measured.
- **MSRV:** the highest new MSRV is uuid's 1.89, below the 1.95 toolchain.

### Capability findings (spike, 2026-10-15)

| Need | Finding |
|---|---|
| Compile once, evaluate many | `env.parser().max_recursion_depth(n).parse(src)` returns the macro-expanded `IdedExpr` (`cel::parser::Expression`), which is evaluated with `Value::resolve(&expr, &ctx)`. `Program::parse_with` is `pub(crate)`, so Mimik stores the `IdedExpr`. 100k evaluations of `6 - vitality` took 12.3 ms (release). Still to verify at integration: `IdedExpr: Send + Sync`, needed to keep it in `ImmutablePin` across `spawn_blocking`. |
| Custom functions | `Context::add_function` with typed or variadic (`extractors::Arguments`) closures returning `Result<Value, ExecutionError>`. It refuses names that collide with stdlib overloads (`size`), which protects against silent shadowing. `min`/`max` are not in the CEL stdlib, so ours are registered explicitly. |
| int64 overflow | `9223372036854775807 + 1` → `Overflow("add")`. `i64::MIN / -1` → `Overflow("div")`. `i64::MIN % -1` → `Overflow("rem")`. The literal `-9223372036854775808` parses to `i64::MIN`. Our `sum` uses `checked_add`. |
| Division | Truncating: `-7 / 2 == -3`, `-7 % 2 == -1`, `7 % -2 == 1`. `1/0` → `DivisionByZero`; `1%0` → `RemainderByZero`. |
| Logic | `false && (1/0==1)` → false. `(1/0==1) && false` → false. `(1/0==1) \|\| true` → true. `(1/0==1) && true` → error. This is CEL commutative error absorption. |
| `has()` on optional item properties | `has(item.armor)` is false when the key is absent. Selecting an absent key → `NoSuchKey("armor")`. `has(vitality)` (a bare identifier) is rejected at parse time. |
| List macros and `size()` | `filter`, `map`, `exists` and `all` work; `size(list)` → int. The decision example `equipment.filter(i, i.kind == "outer-armor" && i.equipped).map(i, i.armor)` → `[1]`. `max_of(<that>, 0)` → 1, and `6 - max_of(...)` → 5. With no passing entries → default 0. |
| Types and coercion | `1 + 1.0` and `1 + 1u` → error (no coercion). However `1 == "a"` → **false** and `1 == 1.0` → **true** (CEL heterogeneous equality). Admission static typing rejects both forms (section 3). |
| Undeclared names | `missing + 1` → `UndeclaredReference`. A non-ASCII identifier is a parse error (`token recognition error at 'é'`). `a-b` is subtraction. |
| Feature-gated stdlib | With defaults off, `duration()`, `timestamp()` and `.matches()` → `UndeclaredReference`. `b'..'`, `1.5`, `2u`, `{}`, `type()`, `dyn()`, `int("5")` are still in the stdlib and are rejected by the admission allowlist. |
| Cost or recursion limits | No evaluation cost or iteration limit exists in cel-rust 0.15. The only limit is a configurable **parser** recursion limit (default 96; error-recovery limit 30). |
| Pathological 1024-byte inputs (depth 32) | Nested parens, brackets, calls and ternaries are rejected with a syntax error, without panic or stack overflow, in ≤ 8.4 ms. `!!!…true` and `---…1` parse (the parser collapses repeated unary). A flat `1+1+…` (512 terms) parses into a **binary tree of AST depth 512**. Garbage input gives a syntax error in about 2 ms. |
| Evaluation work | A 1000-item `filter(...).map(...)` takes 1.4 ms. A **nested** comprehension (`equipment.filter(i, equipment.exists(j, ...))`, 1000×1000) takes **442 ms**. Nesting must be rejected statically. |
| Demo pack | All 11 derived formulas and 21 constraints of `examples/tool-loop-demo/definition.json`, translated as in section 8, reproduce the stored derived values of all 4 characters (44/44). All 84 constraint evaluations pass. |

### Deterministic bounds Mimik adds

cel-rust supplies none of these, so Mimik applies them at admission, before any
evaluation.

1. **Source length.** At most 1024 UTF-8 bytes, checked before parsing →
   `source_too_long`.
2. **Parser recursion limit.** `max_recursion_depth(32)` is a stack guard.
   The demo needs at most 4 → `depth_exceeded`.
3. **AST depth.** At most 64 after macro expansion, measured by Mimik's walk
   (the demo maximum is 11). Because CEL builds binary trees, flat chains of
   more than about 60 terms are rejected. This is acceptable for sheet formulas
   and documented → `depth_exceeded`. The walk is recursive but bounded, since
   parsing already succeeded within the 1024-byte input.
4. **No comprehension inside another comprehension's body.** This covers
   `loop_cond`, `loop_step` and `result`, and their user-level equivalent: a
   macro inside another macro's predicate or transform. Chaining through
   `iter_range` (`c.filter(...).map(...)`) stays linear and is allowed →
   `nested_aggregate`.
5. **List size.** Stored lists are capped at 256 items by the value schema and
   sheet decoder. The evaluator checks ≤ 1000 again before binding any
   variable → `collection_too_large`.
6. **Static work budget.** Per formula: (nodes outside comprehensions) + Σ over
   comprehensions (body nodes × 256). String comparisons are weighted by the
   declared maximum length of the compared field: text 8,000 or 20,000 scalars,
   enum ≤ 64, literal = its length. The sum over all formulas and constraints
   of a definition must be ≤ a budget, proposed as 2·10⁶ units. A measurement
   test fixes the final number → `formula-cost-limit`. Evaluation semantics do
   not change.

## 3. Admitted CEL subset ("CEL profile mimik-1")

Validation runs on two parses:

- The **unexpanded** AST, from `Env::default()` with no macros, gives the
  user-written structure: allowed calls, macro forms, and nesting.
- The **expanded** AST, from `Env::stdlib()` with optional syntax disabled, is
  evaluated and depth-checked.

Anything not listed below fails admission with a path/code diagnostic.

| Category | Allowed | Rejected |
|---|---|---|
| Literals | signed int, `true`/`false`, double-quoted or single-quoted strings, `null` only as an operand of `==`/`!=` against a nullable field | double, `u` uint, bytes, raw/triple-quoted strings (allowed only if they parse to plain strings; simpler to reject `r`/`b` prefixes) |
| Identifiers | declared non-list fields (scalars), list fields (only as a macro target, `size()` argument or `in` right-hand side), a macro iteration variable | undeclared names, type names (`int`, `bool`, …), CEL reserved words used as field IDs |
| Selection | `<iter>.<declared item property>`, `has(<iter>.<declared item property>)` | any other select, list indexing `_[_]`, optional syntax `.?`/`[?]` |
| Operators | `! && \|\| == != < <= > >= + - * / %`, unary `-`, ternary `?:`, `in` with a list-literal or list-field right-hand side | — |
| Operand types | int: all arithmetic/ordering. bool: `== != ! && \|\|`. string (text/enum): `== !=` and `in` only. | string `+`/ordering, list/map arithmetic |
| Macros (receiver form, 2 args) | `L.filter(x, p)`, `L.map(x, e)`, `L.exists(x, p)`, `L.all(x, p)`, where `L` is a list field or a `filter(...)` chain on one | `map(x, p, e)` 3-arg form, `exists_one`, macros nested in another macro's predicate or transform, comprehension over a literal |
| Functions | `size(L)` (global form), custom catalogue (section 4) | everything else in the stdlib: conversions, `type`, `dyn`, string methods, `contains`/`startsWith`/…, map/struct construction |
| Map/struct literals | — | all |

Further rules:

- **Iteration variable.** Any identifier that is not a field ID, function name or
  reserved word. Documentation and the pack use `item`.
- **Static typing.** Implemented by Mimik over this subset, since cel-rust has no
  checker.
  - Types come from controls: integer → int; boolean → bool; text, multiline
    and enum → string; list → list(item record). A comprehension produces a
    list of its element type.
  - `?:` branches must have equal types.
  - `==`/`!=` operands must have equal types, which removes heterogeneous
    equality.
  - A derived field's type must equal its declared control. A constraint must be
    bool.
  - Enum comparisons against a string literal that is not one of the field's
    options fail (`unknown-enum-option`, typo guard).
- **Identifier policy (D2, amended).** CEL identifiers are
  `[_a-zA-Z][_a-zA-Z0-9]*`, so `-` can never be part of a referenced name.
  Relax **only** field IDs and list item-property IDs to
  `[a-z][a-z0-9_-]{0,63}`. Game, edition, definition, group, choice and
  constraint IDs keep the registry slug. Rejections:
  - an expression that references a hyphenated ID →
    `formula-unreferenceable-field`;
  - a field ID that is a CEL reserved word (`in as break const continue else
    for function if import let loop package namespace return var void while`,
    plus `true false null`) and is referenced →
    `formula-unreferenceable-field`.

## 4. Custom function catalogue

All functions are registered on a root `Context`, built once per
`ImmutablePin`, or per evaluation if the root is not `Sync`; to be verified.
Every function validates its own arguments and returns `ExecutionError::function_error(name, <code>)`
with a Mimik code as the message, which section 5 maps exactly.

| Signature | Semantics | Errors |
|---|---|---|
| `sum(list<int>) -> int` | Σ elements, `checked_add`; empty → 0 | `overflow`; non-int element → `type_mismatch`; non-list → `type_mismatch` |
| `max_of(list<int>, int default) -> int` | max, or `default` if empty | as above. Note: `default` is evaluated eagerly; CEL functions receive evaluated arguments. |
| `min_of(list<int>, int default) -> int` | min, or `default` if empty | as above |
| `clamp(int x, int lo, int hi) -> int` | `max(lo, min(x, hi))` | `lo > hi` → `invalid_argument`; non-int → `type_mismatch` |
| `max(int, int, ...) -> int` / `min(int, int, ...)` | variadic, ≥ 2 arguments | < 2 → `arity_mismatch` (also static); non-int → `type_mismatch` |

There are no `count` or `if` functions:

- **count:** `size(L.filter(item, p))`. Prefer `L.exists(item, p)` and
  `L.all(item, !p)` when only zero or non-zero matters.
- **conditional:** `c ? a : b`, which CEL evaluates lazily (spike: the
  `vitality == 0 ? 0 : 1/0` case returned 0).

Arity and argument types are also checked statically.

### Value conversion (Mimik-owned)

`serde_json::Value` maps to `cel::Value` as follows:

- integer → `Int` (never `UInt`);
- bool → `Bool`;
- string → `String(Arc)`;
- `null` → `Null`, only for nullable scalars;
- list → `List` of `Map` items.

Item maps contain the host-managed `id` key plus the declared properties.
Absent optional properties are absent keys, never `null`. Conversion happens
**once per sheet evaluation**: one child scope holds all variables, and every
derived field and constraint borrows it. Strings are shared `Arc`s rather than
copied per formula. Derived results are added to the same scope in topological
order.

## 5. Error-code vocabulary (mapped from CEL)

| Code | Source |
|---|---|
| `parse_error` | `ParseErrors` other than the recursion limit |
| `source_too_long`, `depth_exceeded`, `nested_aggregate` | Mimik pre/post-parse checks (recursion-limit parse error → `depth_exceeded`) |
| `unknown_field` | `UndeclaredReference`; static at admission, defensive at runtime |
| `missing_property` | `NoSuchKey` |
| `null_value` | `UnsupportedBinaryOperator`/`NoSuchOverload`/`ValuesNotComparable` where any operand is `Null` |
| `type_mismatch` | the same errors without a `Null` operand; `UnexpectedType`, `UnsupportedTargetType`; our functions' `type_mismatch` |
| `overflow` | `Overflow(..)`; our `sum` overflow |
| `division_by_zero` | `DivisionByZero`, `RemainderByZero` |
| `invalid_argument` | `clamp` bounds |
| `arity_mismatch` | `InvalidArgumentCount`; our variadic check |
| `collection_too_large` | Mimik pre-binding check |
| `dependency_failed` | **Static**: a derived field whose formula references a derived field that failed is not evaluated, and reports `dependency_failed(<id>)` |
| `internal_error` | any other `ExecutionError`. The enum is `#[non_exhaustive]`, so the wildcard arm is required. Fail closed: the value is shown as an error and logged without values. |

Mapping is one function with a test per row. CEL's message text is never shown
to users. Diagnostics carry code, field or constraint ID and, for parse errors,
a byte offset only, all escaped through the existing `safe_text`.

**Constraint result** is a tri-state, `pass | violated | error(code)`, reported
with `{id, rule_ids, message}`. `error` is never counted as pass or violated.
A non-bool result is impossible after admission; at runtime it would map to
`error(type_mismatch)`.

## 6. Definition schema integration

### D1: additive members in contract 1 (recommended, unchanged in substance)

`definition_drafts`/`definition_versions` pin `contract_version IS 1` in
`CHECK` constraints on immutable, foreign-keyed tables, so a contract 2 would
need a high-risk table rebuild. The plan therefore extends contract 1
additively.

**Profile marker (review amendment).** A definition that uses formulas, lists or
constraints must carry a root `"formula_profile": 1`.

- The marker is part of the hashed canonical bytes. It names the CEL profile
  mimik-1 of section 3, including the custom catalogue and limits. Any later
  change to admitted semantics needs `formula_profile: 2`; a pinned definition
  is never reinterpreted.
- Definitions without formulas omit all new members, so existing canonical bytes
  and hashes are byte-identical. The frozen vector `89861020…ad20` must stay
  green.
- `formula`, `constraints`, `items` and `formula_profile` must be present or
  absent together. For example, formulas without the marker → `formula-profile-required`.

**Re-admission regression (amendment).** Re-admitting the stored canonical
bytes of both a formula-free definition and a formula definition must yield
identical bytes and hash. This exercises the `private_pin::validate` path.

**Members:**

- **`Field.formula`:** `{"expr": "<CEL>", "rule_ids": [...], "evidence": <Mapping>}`.
  - `control` is the result type (integer, boolean, text or enum); a formula
    field cannot be `list`.
  - `nullable` must be false, because failures are explicit error states.
  - `evidence` reuses `Mapping`. Mapped regions must belong to selected
    `core`/`supplement` sources.
- **Root `constraints`:** `[{id, expr, rule_ids, message, evidence}]`, at most
  128, in their own ID namespace.
  - `message` is 1–500 scalar values.
  - `rule_ids` are 0–16 slug labels, sorted and deduplicated.
- **`control: "list"`:** `items: {"properties": [{id, label, control, optional, options?}]}`.
  - At most 32 properties, with controls text, multiline, integer, boolean or
    enum.
  - The list field itself has `nullable: false`.
  - **Every item carries a host-managed `id`** (amendment), unique within the
    list, using the registry slug grammar. The host generates it on creation and
    it is immutable afterwards. It is not a declared property, but formulas may
    read it as `item.id` (string). Change addressing (`{list, entry_id, field}`)
    uses it.
  - The value schema is an array of at most 256 objects with
    `additionalProperties: false`. `id` plus non-optional properties are
    required, and `null` never appears inside items.

**Canonical form:**

- Expression text is hashed verbatim (no normalization).
- `rule_ids` and evidence regions are sorted.
- Constraint and property order is preserved.
- A new frozen vector covers `schemas/definition-formula-example.json`, a
  synthetic lantern-reach-style definition.

**Touch points:**

- `schemas/definition-v1.json`: `$defs` for formula, constraint, items and
  property.
- `decode.rs` limits:
  - `expr` 1024 scalars as the decoder bound (bytes are checked in the formula
    layer);
  - `message` 500;
  - `rule_ids` 16;
  - `constraints` 128;
  - `properties` 32.
- Sheet decoding of lists of objects must stay within the depth 12 / 256-item
  caps (test).

### Migration

This branch is rebased onto master, which has 016 (admin) and 017 (retrieval). M1 adds no
migration. In M2, `018_formula_profile.sql` only bumps the schema version: there is a
comment and no table changes, so that older binaries refuse a database that may contain
formula definitions ("schema is newer").

- **Mechanism (master):** `src/store.rs` has `MIGRATIONS: [&str; 17]` and
  `SCHEMA_VERSION = 17`, applied in order (`(1..).zip(MIGRATIONS)`). An existing test
  asserts that the array length equals `SCHEMA_VERSION` and that the migration files are
  contiguous and in order.
  - M2 appends `include_str!("../migrations/018_formula_profile.sql")`.
  - M2 sets `MIGRATIONS: [&str; 18]` and `SCHEMA_VERSION = 18`.
  - The contiguity test covers the review's ladder amendment (item 15), so no new
    mechanism is needed.
- **Regression (amendment):** a database at version 19 is refused. Databases at 15 and 17
  migrate to 18 with all definitions intact.

### Publication

The snapshot walks formula and constraint `evidence` regions as captured
dependencies, which gives page-scoped invalidation.

Unresolved evidence is recorded separately (amendment):

- `unresolved` remains field mappings only;
- new `unresolved_formulas` holds field IDs;
- new `unresolved_constraints` holds constraint IDs.

Because `Snapshot` is `deny_unknown_fields`, snapshots of formula definitions
use snapshot `encoding: 2`. Existing encoding-1 snapshots stay readable and
unchanged.

`PublishedDefinition.deterministic_rules` changes from `"unsupported"` to
`"formulas-advisory"` with an evidence state of current, partial or stale.
Stale evidence never stops evaluation for existing pins.

## 7. Modules, evaluation points, milestones

### `src/definitions/formula/` (`pub(crate)`)

| File | Content |
|---|---|
| `mod.rs` | API: `Compiled::new(&Definition) -> Result<Compiled, Diagnostics>`; `Compiled::evaluate(&serde_json::Value) -> SheetEvaluation`, with derived `id → Ok(Value) / Err(Code)` and constraints `id → Pass / Violated / Error(Code)`; `preview(&Compiled, before, after) -> Preview`. |
| `profile.rs` | Two-parse admission: allowlist walk, nesting and depth checks, static types, cost budget, references, cycle detection. Cycles use Kahn's algorithm, about 30 lines with field-order tie-break; `petgraph` is not justified. |
| `functions.rs` | Catalogue (section 4). |
| `convert.rs` | JSON → `cel::Value` (section 4). |
| `error.rs` | `Code` enum, the CEL mapping and stable snake_case `as_str()`. |
| `tests.rs` | Unit tests. |
| `conformance.rs` | Fixture runner. |

### Touch points elsewhere

These are unchanged from revision 1:

- `definitions/mod.rs`, `admission.rs` (call `formula::profile` after
  `domain()`), `schemas/`, `decode.rs`, `store.rs:69`;
- `publication.rs`;
- `private_pin.rs`: `ImmutablePin` holds `Compiled`;
- `campaigns/values.rs`: derived fields are excluded from the schema and the
  form; the list schema is added;
- `sheets.rs`/`web.rs`/`snapshots.rs`;
- `templates/private_sheet.html`;
- `store.rs`: migration 18.

The native MÖRK BORG adapter is untouched.

### Where evaluation happens

- Derived values are **never stored**. They are a pure function of the immutable
  pin and the canonical values.
- **View, reopen and history:** computed inside the existing `spawn_blocking`
  after `validate_values`. Errors are explicit states, never defaults.
- **Save:** proposed data is evaluated and violations are returned as advisory
  output. They do **not** block the structural save.
- **Preview:** library-only for the future tool loop. A submitted derived key is
  rejected by the existing exact form/schema checks.

### Milestones

Each milestone gets failing-first tests and an independent review.

| Milestone | Scope |
|---|---|
| **M1** | Dependency plus the `formula` module, profile, functions, conversion, error map and conformance fixture (no schema change). Update `dependency-decisions.md`. |
| **M2** | Envelope, admission, hash, marker, list value schema, publication evidence and migration 018. List definitions are rejected at creation until M4 (`list-editing-unsupported`). |
| **M3** | Evaluation on view, save and history, plus preview. |
| **M4** | Server-rendered list editing (add/remove rows with submit buttons) plus browser and package gates. |

## 8. Demo pack translation (`examples/tool-loop-demo/definition.json`)

Rewrite rules (the custom form maps to CEL):

| Custom form | CEL |
|---|---|
| `and` / `or` / `not X` | `&&` / `\|\|` / `!X` |
| `if(c,a,b)` | `c ? a : b` |
| `count(C, p)` | `size(C.filter(item, p))` |
| `sum(C, v[, w])` | `sum(C[.filter(item, w)].map(item, v))` |
| `max_of(C, v, d[, w])` | `max_of(C[.filter(item, w)].map(item, v), d)` |
| `has(item.p)` | unchanged |

**Derived (11):**

| Field | CEL |
|---|---|
| `down` | `vitality == 0` |
| `armor` | `max_of(equipment.filter(item, item.kind == "outer-armor" && item.equipped).map(item, item.armor), 0) + max_of(equipment.filter(item, item.kind == "shield" && item.equipped).map(item, item.armor), 0)` |
| `armor_pace_penalty` | `max_of(equipment.filter(item, item.kind == "outer-armor" && item.equipped && !(talent_mail_trained && item.category == "mail")).map(item, item.armor), 0)` |
| `burden` | `sum(equipment.map(item, item.weight * item.quantity))` |
| `burden_limit` | `6 + 2 * might` |
| `encumbered` | `burden > burden_limit` |
| `bleeding_stacks` | `sum(conditions.filter(item, item.kind == "bleeding").map(item, item.stacks))` |
| `weary_stacks` | same pattern with `"weary"` |
| `chilled` | `sum(conditions.filter(item, item.kind == "chilled").map(item, item.stacks)) > 0` |
| `shaken` | same pattern with `"shaken"` |
| `pace` | `down ? 0 : max(1, 6 - armor_pace_penalty - weary_stacks - (encumbered ? 1 : 0) - (chilled ? 1 : 0))` |

**Constraints (21):**

- The eight range checks become `x >= lo && x <= hi`.
- `lit-needs-oil` becomes `!lamp_lit || lamp_oil > 0`.
- `no-lamp-no-oil` becomes `size(equipment.filter(item, item.kind == "lamp")) > 0 || (lamp_oil == 0 && !lamp_lit)`.
- The other eleven become `size(L.filter(item, <predicate with && || !>)) <op> n`
  with the predicate translated literally. Examples:
  - `outer-armor-fields`:
    `size(equipment.filter(item, item.kind == "outer-armor" && !(has(item.armor) && has(item.category)))) == 0`;
  - `condition-entries`: four `size(conditions.filter(...)) == 1` joined with `&&`.

The exact strings used in the spike reproduced all stored derived values
(44/44) and passed all constraints (84/84).

**Meaning changes reviewed (none change a successful result in the pack):**

1. **Division and modulo.** The pack uses neither, so truncation has no effect.
2. **Short-circuit vs error absorption.** CEL can only turn a result that would
   have been an *error* in the custom spec into a value. For example,
   `(error) && false` is false; it never changes a defined result. Every pack
   guard (`has(item.armor) && …`,
   `talent_mail_trained && item.category == "mail"`) behaves identically. An
   outer armor lacking `category` with the talent set still errors in both.
3. **`not` precedence.** Custom `not` binds looser than comparison; CEL `!`
   binds tighter. Every pack use is `not <ident>` or `not (<expr>)`, so the
   result is identical. This is a translation hazard for future packs, and the
   authoring docs must say "parenthesize after `!`".
4. **`max_of` default is now eager.** All pack defaults are the literal `0`, so
   there is no effect.
5. **Heterogeneous `==`** returns false in CEL instead of an error. Admission
   typing rejects it, and the pack has none.
6. **The pack format is not the Mimik envelope.** The pack's `definition.json`
   stays a benchmark format. Only its expression strings change, with a
   `"formula_language": "cel-mimik-1"` marker and an updated README.
   A **differential check** replays every dev and held-out case's proposed state
   through both `formula_ref` and the CEL engine. Any difference blocks
   retiring `formula_ref`. This is a one-time Python-side check, run before
   deleting `tools/formula_ref.py`.

## 9. Carried-over review amendments

- **Error vocabulary** is mapped from CEL (section 5).
- **`dependency_failed`** is static (section 5).
- **Constraint tri-state** (section 5).
- **`formula_profile: 1`** is a hashed marker, with the re-admission regression
  (section 6).
- **List items carry a host-managed `id`** (section 6).
- **Shared strings and comparison budget.** One conversion per sheet with
  shared `Arc` strings; string-comparison weight is part of the cost budget
  (sections 2 and 4).
- **D2** is scoped to field and item-property IDs (section 3).
- **Constraint IDs are kept separate** from the field `unresolved` list in
  publication (section 6).
- **Further tests:**
  - newer-schema refusal (section 6);
  - HTML escaping of messages, labels, expressions and diagnostics: a
    `<script>`-bearing message renders escaped;
  - pack integration (section 10);
  - MÖRK BORG regression: the existing native tests stay green, and a native
    sheet view/save never calls the formula layer.

## 10. Failing-first test list

**Conversion and engine:**

- JSON `5` becomes `Int(5)`, not `UInt`. This is a regression test for the spike
  pitfall: `n + 1` with JSON `n = 5` evaluates to 6.
- `i64::MAX + 1`, `i64::MIN / -1` and `i64::MIN % -1` → `overflow`.
- `-7 / 2 == -3`; `1 / 0` and `1 % 0` → `division_by_zero`.
- Absorption truth table (four cases).
- Ternary laziness.
- `has()` on present and absent properties; reading an absent property →
  `missing_property`.
- `null` operand → `null_value`.

**Catalogue:** each function's success, empty-list, error and arity cases;
`clamp` with `lo > hi`.

**Allowlist (one rejection each):**

- double, uint and bytes literals;
- map literal; indexing; `.?`;
- `type()`, `int()`, `"a".contains()`;
- `exists_one`, 3-arg `map`;
- nested macro; macro over a literal;
- undeclared identifier;
- string `+` and string `<`;
- mixed-type `==`; enum literal typo;
- hyphenated reference; reserved-word field reference.

**Limits:**

- 1024 vs 1025 bytes, including a multibyte boundary;
- parser recursion 32 vs 33;
- AST depth 64 vs 65; a flat 70-term sum is rejected and documented;
- the 1024-byte pathological set from the spike (parens, brackets, calls,
  ternaries, `!`/`-` runs, garbage): an error or a bounded success, no panic,
  timing assertion only as a smoke check;
- list of 1000 vs 1001 items;
- cost budget over and under the limit.

**Admission and hash:**

- derived cycle and self-cycle;
- derived type vs control mismatch; non-bool constraint;
- `nullable` derived field;
- evidence on a sheet-only source;
- formula members without the marker;
- list definition before M4;
- v1 frozen vector unchanged; new formula vector; re-admission byte identity;
- `rule_ids` order-insensitive; whitespace in the expression changes the hash;
- formula-free definitions emit no new keys.

**Migration:** 15 → 18 (and 17 → 18 after rebase) keeps data; 19 is refused.

**Evaluation:**

- a derived key is rejected in the form and in JSON;
- view shows derived values; a failure shows an explicit error state;
- a violation is advisory, the save succeeds and the revision increments;
- history recomputes identically;
- `dependency_failed` propagation;
- `preview` reports diffs and new or resolved violations;
- HTML escaping;
- MÖRK BORG regression.

**Pack integration:**

- `tests/formula_pack.rs` builds a Mimik envelope from the pack definition with
  synthetic sources;
- all four characters reproduce the stored derived values and pass all
  constraints;
- a mutated character (a second outer armor equipped) gives
  `one-outer-armor = violated`.

**Conformance fixture `schemas/formula-conformance-v1.json`.** Shared, in the
existing package allowlist, and loaded with `include_str!` into
`deny_unknown_fields` structs. Example:

```json
{"version": 1, "profile": "cel-mimik-1",
 "expressions": [
  {"id": "trunc-div", "expr": "-7 / 2", "env": {}, "expect": {"value": -3}},
  {"id": "rem-zero", "expr": "1 % 0", "env": {}, "expect": {"error": "division_by_zero"}},
  {"id": "armor", "expr": "max_of(equipment.filter(item, item.kind == \"outer-armor\" && item.equipped).map(item, item.armor), 0)",
   "env": {"equipment": [{"id": "coat-1", "kind": "outer-armor", "equipped": true, "armor": 1}]},
   "types": {"equipment": {"list": {"kind": "string", "equipped": "bool", "armor": "int?"}}},
   "expect": {"value": 1}}],
 "sheets": [{"id": "pace-chain", "derived": [{"id": "armor", "type": "int", "expr": "..."}],
             "constraints": [{"id": "one-outer", "expr": "..."}],
             "values": {}, "expect": {"derived": {"armor": {"value": 1}},
             "constraints": {"one-outer": "pass"}}}],
 "rejections": [{"id": "nested", "expr": "equipment.filter(item, equipment.exists(j, true))",
                 "types": {}, "expect": {"error": "nested_aggregate"}}]}
```

- **Typed env:** each env key has a declared type in `types`: `int`, `bool`,
  `string`, nullable `T?`, or `list` with item property types where a `?`
  suffix marks the property optional. JSON integers are always int.
- **`expect`:** exactly one of `value` (the JSON type must match the declared
  result type) or `error` (a code only).
- **Sections:** `rejections` covers admission-profile codes; `sheets` covers
  topological order, `dependency_failed` and the constraint tri-state.
- **Size:** at least 150 cases, at least one per code and per allowlist row.

## 11. Python side recommendation

**Recommendation: option (a).** A Rust CLI example in this repository is the
single source of truth: `examples/formula_eval.rs`, run with
`cargo run --example formula_eval`. It exposes the same `formula` module through
a JSON-lines stdin/stdout protocol:

- requests are `{"op":"load","definition":…}`, `{"op":"evaluate","values":…}`
  and `{"op":"check_value",…}`;
- responses carry `{"derived":…, "constraints":…}` or `{"error":{code,path}}`.

Reasons:

- **One implementation.** Python is not a second semantics to keep in sync, so
  the duplicate-logic rule is satisfied. The benchmark measures exactly what the
  product will run, including Mimik's allowlist, typing, limits and custom
  functions, none of which a generic Python CEL library has.
- **Option (b) costs more.** `common-expression-language` 0.10.0 on PyPI
  (Apache-2.0, 2026-09-15) wraps cel-rust through PyO3. It would still
  re-implement the catalogue, conversion, allowlist and error mapping in Python,
  and its cel-rust version may drift from our `=0.15.0` pin. `cel-python` 0.5.0
  is an independent pure-Python implementation with its own semantics and no
  declared licence metadata.
- **Latency.** The JSON-lines server is long-lived, so there is no process
  spawn per call. The benchmark's `FormulaEngine` seam in
  `tools/tool_loop_benchmark.py` already isolates the evaluator; a
  `CelCliEngine` implements `fields`, `collections`, `derived_order`,
  `is_derived`, `check_value`, `derive` and `violations` over the subprocess.
- **Containers.** The binary is built in `mimik-ci` (Ubuntu 22.04, glibc 2.35)
  and mounted read-only into the Python 3.12-slim benchmark container (Debian
  bookworm, glibc 2.36, so compatible). No Rust toolchain is needed in the
  benchmark image, and no host install.

Conformance:

- the Rust test runs the fixture in-process;
- `tools/test_formula_cli.py` runs the same fixture through the CLI, which
  tests the protocol;
- after the differential check (section 8), `formula_ref.py` and its tests are
  removed so that the custom spec is not maintained in parallel.

Until M1 lands, the benchmark keeps `formula_ref`. Results from the two engines
are labelled by `formula_language`.

## 12. Clippy pedantic, security, process

**Clippy pedantic:**

- `cast_*`: use `i64::try_from` for sizes.
- `needless_pass_by_value`: closures take `Value` by the cel extractor API.
  Keep it local, and use a narrow `#[allow]` with a comment only if the lint
  fires on extractor signatures, subject to reviewer approval.
- `missing_errors_doc`: keep the API `pub(crate)`.
- `match_same_arms` in the error map: group the arms explicitly.
- `too_many_lines`: split `profile.rs` per node kind.
- No module-level or crate-level allows.

**Security:**

- The source length is checked before parsing; parser recursion is limited;
  the AST depth walk and the static cost budget run before any evaluation.
- No regex, network, clock or float access.
- Expressions are never interpolated into HTML, SQL or JS; Askama escapes all
  output.
- Model output never creates or approves formulas outside the existing human
  draft/publication path.
- Violations are advisory and never mutate sheets.

**Process:**

- This revision gets an independent review, recorded in
  `docs/reviews/2026-10-15-derived-formulas-plan.md` (review of the review is
  not recursive).
- `TODO.md` gets an ACTIVE entry when M1 starts.
- `dependency-decisions.md` records `cel` and `antlr4rust`, the rejected
  alternatives (Appendix A plus evalexpr/fasteval/rhai) and the duplicates.
- `runtime-definition-contract.md` and `definition-drafts.md` get the formula
  profile.

## 13. Container commands

Builds use the worktree mount and the `mimik-formula-target` volume, never host
`/tmp`.

```sh
WT=/home/marco/Progetti/mimik-wt/formula
RUN="docker run --rm --cpus 4 --mount type=bind,src=$WT,dst=/workspace \
  --mount type=volume,src=mimik-cargo-cache,dst=/opt/cargo/registry \
  --mount type=volume,src=mimik-formula-target,dst=/target \
  -e CARGO_TARGET_DIR=/target/formula mimik-ci:latest"
$RUN cargo add cel@=0.15.0 --no-default-features      # only networked step; review Cargo.lock diff
$RUN cargo tree -e normal,features -i cel             # expect no optional cel features
$RUN cargo fmt --check
$RUN cargo clippy --locked --all-targets --all-features -- -D warnings
$RUN cargo test --locked --all-targets --all-features
$RUN bash -c 'ruff check tools && ruff format --check tools && python -m unittest discover -s tools -p "test_*.py"'
$RUN cargo audit --deny warnings
$RUN cargo about generate --locked --fail about.hbs -o /target/formula/THIRD-PARTY-NOTICES.md
$RUN cargo build --release --locked                   # re-measure size vs 18,210,072 bytes
```

After the fetch, add `--network none` to the remaining steps. Use one command
per `docker run`, or a script file, rather than long `bash -c` chains. Package,
vendor, unpacked-artifact and browser gates follow `.github/workflows/ci.yml`
at M3/M4.

**Leftovers from the spike:** the volume `mimik-formula-target` still holds the
spike target directories `/target/cel-spike`, `/target/fit-base` and
`/target/fit-cel` (build output only). They can be removed with
`docker run --rm --mount type=volume,src=mimik-formula-target,dst=/target mimik-ci:latest rm -r /target/cel-spike /target/fit-base /target/fit-cel`.

## 14. Rev 2 review amendments (independent review, GO-with-amendments)

Reviewer: a fresh-context Sonnet 5.5 subagent (the external reviewer was out of quota). Each
item is resolved below. **M1** items are implemented in this worktree and verified as
described in 14.2. Items marked **M2**/**M3**/**M4** have their exact change and failing-first
regression fixed here, but no code yet. Where M1 differs from sections 1–13, this section
supersedes them.

### 14.1 Dispositions

| # | Sev. | Resolution | Status |
|---|---|---|---|
| 1 | Block | `convert.rs` takes a JSON integer only through `as_i64()`. No `as` casts, and `uint` is never produced; a u64 above `i64::MAX` or any float is `invalid_input`. Item properties use the same scalar path, with no nulls. **M2:** `private_pin::validate_values` gets an explicit i64 walk over list items, mirroring the current top-level check. | M1 done: fixture `item-int-above-i64` (`armor: 9223372036854775808`), `field-int-above-i64`, and unit test `json_integers_become_signed_cel_ints`. M2 regression: the same value in a stored sheet is rejected by `validate_values`. |
| 2 | Block | Read from the pinned source: cel 0.15.0 `int.rs` uses `checked_add/sub/mul/div/rem/neg` for every int operator. Evaluation is also wrapped in `catch_unwind`, and any panic maps to `internal_error`. | M1 done. Fixture covers `*`, binary `-`, unary `-` on `i64::MIN`, `/`, `%` at the limits. A deterministic xorshift differential test (3,000 pairs, edge-biased) runs against a checked Rust reference, **passing in debug and in `cargo test --release`** (release has no overflow checks). |
| 3 | Block | **Composition:** total formula text is bounded by the existing 256 KiB canonical definition cap, which equals the 262,144-byte pin load cap, so 384 × 1 KiB is never reachable. M2 adds a test that a definition at the canonical cap with formulas still loads through `private_pin`. **One list constant:** `formula::MAX_LIST_ITEMS = 256` is used for the input check, the cost budget and, at M2, the value schema `maxItems` (the decoder's generic array cap is also 256). **Step guard:** a deterministic static budget (`MAX_COST` = 2,000,000 units), plus a wall-clock `DEADLINE` (500 ms) checked before every formula. When it fires, the remaining formulas report `deadline_exceeded`; this is documented as a non-deterministic safety net only. **20,000-character strings:** `==`/`in` are charged `min(declared lengths)/64 + 1` units, and text is up to `MAX_TEXT_CHARS` = 20,000. **Calibration:** see 14.2. | M1 done. |
| 4 | Med | The allowlist and typing now run on the **expanded** AST. The four recognized expansion shapes (`filter`, two-arg `map`, `exists`, `all`) are matched exactly against the pinned macro output. Any other comprehension (`exists_one`, three-arg `map`) is `unsupported_construct`. A single parse is used, so the earlier two-parse design is dropped. | M1 done: `expanded_comprehensions_match_unexpanded_macro_calls` compares macro calls in the unexpanded AST (`Env::default()` parser) with recognized comprehensions in the expanded AST, for every admitted fixture expression. |
| 5 | Med | The depth walk uses an explicit stack and stops at 65. **New finding:** in debug builds the ANTLR parser overflowed a 2 MiB stack at 12 nested parentheses, and cel's recursive evaluator overflowed it on a 61-term chain, both inside the limits. Fix: `Compiled::new` and `evaluate` run on a scoped thread with `FORMULA_STACK_BYTES` = 16 MiB (`on_formula_stack`). A failed spawn or a panic gives `internal_error`. Rejected deep parses are dropped on that thread. Admitted ASTs (depth ≤ 64) are dropped by their owner; the small-stack test drops them on a 2 MiB thread. | M1 done: `pathological_inputs_are_bounded_on_a_small_stack` and `admitted_limits_compile_and_evaluate_from_a_small_stack` call from 2 MiB threads in debug. |
| 6 | Med | `error::from_execution` matches variants. The only string match is the parser recursion message, plus our own function codes. There is one test row per mapping (20 rows) and per parse class. The hash-locked golden fixture is the digest gate. | M1 done. |
| 7 | Med | Golden fixture `schemas/formula-conformance-v1.json` is SHA-256-locked in both the Rust test and the Python CLI test, and the CLI reports its embedded digest. `pinned_engine_versions_in_lockfile` fails on any `cel`/`antlr4rust` version change, which gates `cargo update`. An outcome diff after a bump means `formula_profile: 2` plus a dual-engine decision, never a silent fixture edit. | M1 done. |
| 8 | Med | **M2:** new `Definition`/`Field` members go **after** the existing ones (admission.rs serializes in struct order), with `#[serde(default, skip_serializing_if = ...)]`. The JSON Schema, structs and decoder limits change in one commit. A byte-compare test asserts that formula-free canonical bytes equal the pre-change bytes. The frozen vector stays. | M2 |
| 9 | Med | **M2:** the underscore relaxation applies only to field and item-property IDs, through a new `valid_field_id` used only at those two call sites. Every other `valid_id` caller (game, edition, definition, group, choice, constraint, form names `value-<id>`) is unchanged; `_` is URL- and form-safe and will be tested. A test keeps formula-free definitions admitted under the old grammar admissible. In M1, field IDs matching custom or stdlib names (`sum max min size has clamp max_of min_of item in int filter`, …) are `reserved_identifier`; item property `id` is reserved. | M1 part done (fixture `reserved-field-*`); M2 rest. |
| 10 | Med | `in` accepts only a non-empty **literal list of scalars** of the left side's type, and the left side must be int or string. `x in equipment` is `unsupported_construct`. | M1 done (`in-record-list`, `in-mixed`). |
| 11 | Med | Derived results are checked at runtime against the declared type, enum options and text length (`invalid_result`), and statically for literals (`unknown_enum_option`). Item-property enum values are checked on input (`invalid_input`). | M1 done. |
| 12 | Med | Compile-time `assert_send_sync` on `Compiled`, `cel::IdedExpr` and `cel::Env` passes. The `Context` is built per evaluation, so it need not be `Sync`; the cross-thread reuse test passes. | M1 done. The re-parse fallback is not needed. |
| 13 | Med | Constraint tri-state with CEL absorption is documented and tested: `err \|\| true` gives pass, `err && false` gives violated, a bare `err` gives error. | M1 done (sheet `constraint-tri-state`). |
| 14 | Low | `examples/formula_eval.rs` is a JSON-lines CLI on stdin/stdout only, with no file, database or network access. CI builds it with `cargo build --locked --example formula_eval` (default product features). It is excluded from the corresponding-source package: Cargo `include` now lists examples explicitly, and a Python test asserts this. The Python test pins the fixture digest and checks the digest the CLI reports. It skips only when the example source is absent (unpacked source release). | M1 done. |
| 15 | Low | Master's `MIGRATIONS` array/`SCHEMA_VERSION` contiguity test (admin, 1c848f1) covers it. M2 appends 018 and bumps both. | Resolved by rebase; M2 entry |
| 16 | Low | **M2:** every `encoding != 1` site (publication.rs:363, :608 at the time of review) is listed in the M2 change. A test rejects an encoding-2 snapshot of a formula-free definition. | M2 |
| 17 | Low | The lockfile gate (item 7) fails on any antlr4rust change, including a new major. **Exit path:** the engine is isolated behind `formula::{Compiled, Evaluation, Code}`. Replacing `cel` means a new profile ID, a new engine implementation passing the golden fixture (or a reviewed diff with profile 2), and dual evaluation of existing pins until migration. Pinned definitions keep their profile. | M1 done |
| 18 | Low | M4 (list editing) stays separate. `list-editing-unsupported` remains until M4. The history addressing change is deferred to M4. | Plan |
| 19 | Low | No `#[allow]` was added in M1 code. Any future allow must be function-scoped, justified and flagged for review. | M1 done |
| 20 | Low | Failing-first tests exist for every M1 item above. **M2:** a test where dropping `formula_profile` while formulas remain fails with `formula-profile-required` via `private_pin::validate`. | M1 done / M2 |

Other changes from section 4 made during M1:
- `count` is still `size(L.filter(...))`.
- `max_of`/`min_of` take a list plus a default.
- `min`/`max` take 2–256 arguments.
- Iteration variables may not shadow declared names. `item` is reserved as a field name so
  that it can serve as the iteration variable.
- Error codes added during implementation: `unsupported_construct`, `unknown_property`,
  `invalid_identifier`, `reserved_identifier`, `duplicate_identifier`,
  `invalid_iteration_variable`, `invalid_declaration`, `formula_cycle`, `cost_limit`,
  `invalid_input`, `input_too_large` (§14.1a), `invalid_result` and `deadline_exceeded`.

### 14.1a M1 implementation review fixes (Sonnet 5.5 review, APPROVE-with-fixes)

The record is in `reviews/2026-10-15-derived-formulas-m1.md`.

- **Panic boundaries.** The per-formula boundary is now `guarded(closure)`, and `run` uses
  it. A test-only `fault_point!` (compiled out of non-test builds) injects panics at
  `run`, `compile` and `evaluate`. Tests:
  - a panicking formula fails alone with `internal_error` while the others still
    evaluate;
  - a compile panic gives `Rejection{internal_error}`;
  - an evaluate panic gives `failed_all(internal_error)`;
  - `guarded` and `on_formula_stack` are checked directly.

  Removing `catch_unwind`, or bypassing the formula thread, fails these tests (mutation
  confirmed).
- **Aggregate input bound.** `convert::inputs` charges every converted value 16 bytes plus
  its string bytes against `MAX_INPUT_BYTES` (1 MiB, equal to the stored-sheet body cap)
  *before* copying, and reports `input_too_large`.
  - **M2 gate:** the `evaluate` boundary in the sheet path must also reject bodies over
    the stored-sheet cap before conversion (`parse_sheet` already enforces 1 MiB). It
    must be tested with a sheet whose per-value caps are all respected but whose total
    exceeds the cap.
  - Because of this bound, the worst-case data is now 256 items × 4,000 characters,
    which fits under the cap.
- **List function cost.** `size`, `sum`, `max_of` and `min_of` are charged
  `MAX_LIST_ITEMS` units each. Exact costs are pinned by a test.
- **Filter-shape checks.** A unit test hand-builds near-miss comprehensions (wrong
  else-branch accumulator, wrong appended element) and asserts they are rejected. Each
  check was confirmed by mutation.
- **`pub mod formula`.** It is public only so the development CLI example can link; there
  is a code comment saying so. **Revisit at M2:** narrow it to `pub(crate)` with a
  dedicated thin public facade, or move the CLI behind a non-default feature.
- **Calibration test.** It now asserts static cost (at most `MAX_COST`, and within one
  formula of it); wall-clock time is only printed. The release report is 18.9 ms for
  24 formulas at cost 1,966,128, a single shared-host sample.

### 14.2 M1 implementation and verification (2026-10-15)

**Files:**
- `src/definitions/formula/{mod,profile,convert,functions,error,tests}.rs`;
- `schemas/formula-conformance-v1.json` (132 expression cases and 34 sheet cases,
  including all four Lantern Reach characters and a mutated one);
- `examples/formula_eval.rs`, `tools/test_formula_cli.py`;
- `Cargo.toml`: `cel = "=0.15.0"` with default features off, and the explicit example
  include list;
- `.github/workflows/ci.yml`: the CLI build step.

There is no definition, schema, migration or UI change; that is M2/M3.

**Failing-first record.** Tests were written against the drafted module; the first test
run was red. Findings from that run:
- Real defects: debug stack overflows (amendment 5) and an unhelpful code for `item.x`
  outside an aggregate.
- Test or fixture authoring errors: an expectation for `luck != null`, a 1,024-byte case
  meant to be 1,025 bytes, and one oversized pathological input.

The test and fixture errors were corrected to match the documented semantics. The conformance
expectations were hand-specified before the first run. The Lantern Reach expectations come
from the pack's stored character values, not from engine output.

**Calibration.** The largest admitted worst-case definition is 24 constraints of
`pages.exists(item, item.text == note)`: 256 items, nearly equal 20,000-character texts, cost
1,966,128 of 2,000,000. It evaluated in **48 ms** in release (superseded by §14.1a: 18.9 ms on 4,000-character data within the input cap) (one sample, shared host, not a
hard bound), so `MAX_COST` stays at 2·10⁶ with about 10× headroom under the 500 ms deadline.

**Gates run in `mimik-ci`.** Volume `mimik-formula-target` was used; disk had 47 GiB free.
- `cargo fmt --check`: pass.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: pass.
- `cargo test --locked --all-targets --all-features`: all suites pass, 142 lib tests
  including 21 formula tests at first; 27 after the §14.1a fixes, rerun green.
- `cargo test --release --locked --lib formula`: 21 pass (27 after §14.1a).
- `ruff check` and `ruff format --check`: pass.
- `python -m unittest discover -s tools`: 128 tests pass, including 5 CLI conformance tests.
- `actionlint`: pass.
- `cargo audit --deny warnings`: pass.
- `cargo about generate --locked --fail`: pass, with no `about.toml` change.

Not run: release build size, package/vendor/unpacked/browser gates (M3/M4 scope). These
results are unit and CLI verification only, not app integration.

## 15. M2 implementation (2026-10-15, independently reviewed, committed as `1166a1d`)

Scope as approved: the definition envelope, admission, canonical hash, list value schema,
pin validation, publication evidence and migration 018. Evaluation on view/save is still M3
and list editing is M4.

**What changed**

- **Envelope** (`definitions/mod.rs`, `schemas/definition-v1.json`, `decode.rs`):
  - `Definition.formula_profile` and `constraints` follow the existing members.
  - `Field.items` and `Field.formula` follow `mapping`.
  - Every new member is `skip_serializing_if` when absent.
  - `Control::List`, plus `Items`, `ItemProperty`, `Formula` and `Constraint`.
  - Schema `$defs` and decoder limits: `expr` 1,024, `message` 500, `rule_ids` 16,
    `constraints` 128, `properties` 32.
- **Admission** (`formula_admission.rs`, called from `admission::domain`/`validate`):
  - The marker is required both ways: `formula-profile-required` and
    `formula-profile-without-formulas`.
  - List and derived shape rules.
  - `rule_ids` use the slug grammar and are canonicalized as sorted sets.
  - Evidence must cite selected `core`/`supplement` sources, and is counted in the
    1,024-region dependency limit.
  - The envelope is projected to `formula::SheetSpec` and compiled with the M1 engine. A
    `Rejection` is mapped to `/fields/<i>/formula`, `/fields/<i>`, `/constraints/<i>` or
    `/fields/<i>/items` with code `formula-<code>`.
- **D2:** `admission::valid_field_id` adds `_` for field IDs only. No other `valid_id`
  caller changed.
  - Item-property and list-field IDs must be CEL identifiers (`[a-z][a-z0-9_]*`); this is
    a new control with no legacy data.
  - Hyphenated scalar field IDs remain valid but are not declared to formulas, so a
    reference gives `formula-unknown-field` (this replaces the planned
    `formula-unreferenceable-field`).
  - Reserved names (`size`, `sum`, `max`, …) are rejected only in formula definitions.
- **Values** (`campaigns/values.rs`, `private_pin.rs`):
  - Derived fields are excluded from the value schema, the form and the web save keys.
  - A list is an array of at most 256 objects with `id` plus declared properties, no
    `additionalProperties` and no null.
  - `validate_values` adds item-ID slug/uniqueness checks, exact i64 item integers and a
    1 MiB aggregate size cap. The cap is the sheet-boundary gate from the M1 review.
  - `values::form` rejects list fields (`List editing is unsupported until milestone M4`).
- **Store:** formula/constraint evidence references are verified transactionally
  (`evidence-unit-page-source-mismatch`).
- **Publication:**
  - Evidence pages and units are captured in the wiring snapshot, which gives page-scoped
    invalidation.
  - `PublishedDefinition` gains `unresolved_formulas` and `unresolved_constraints`,
    separate from the field-only `unresolved`, omitted when empty so formula-free output
    is unchanged. `deterministic_rules` reports `formulas-advisory` for profile
    definitions.
  - Approval is refused with `list-editing-unsupported` (until M4) and
    `formula-evaluation-unsupported` (until M3), so no sheet can pin a definition whose
    derived values would not be shown.
- **Migration:** `018_formula_profile.sql` only bumps the schema version. It is appended
  to `MIGRATIONS` and `SCHEMA_VERSION = 18`. The existing schema fixture test now refuses
  version 19.

**Deviations from §6 (need reviewer confirmation)**

1. **No snapshot encoding 2.** `definition_approvals.encoding CHECK(encoding=1)` (008) and
   the provenance trigger's fixed eight-key set (009) would require rebuilding an
   immutable, foreign-keyed table. That is the risk D1 avoided. The separate unresolved
   lists are derived from the immutable definition content instead, and the snapshot stays
   encoding 1.
   - Encoding sites: the 008 CHECK, the 009 trigger, the approval INSERT (`encoding` 1),
     and `verify_approval` (`encoding != 1 || snapshot.encoding != 1`).
   - A test tampers an approval to encoding 2 with a consistent snapshot hash and asserts
     it is rejected (mutation-confirmed).
2. **Formula publication is gated until M3.** This is stricter than the plan, which gated
   only lists.
3. **The migration contiguity test skips `--` comment lines** before checking that the
   last statement sets the version. A version-only migration has no earlier `;`. The
   test's check is unchanged otherwise.
4. **Still to do:** the pack-integration test (`tests/formula_pack.rs`) and the
   HTML-escaping test are M3, since nothing renders formulas yet. The M1 fixture already
   evaluates all four Lantern Reach characters.

**Review fixes (APPROVE-with-fixes, see the review record):**
- the evidence-capture test;
- empty evidence region lists rejected;
- the 1 MiB cap applied to formula definitions only (no retroactive cap on formula-free
  pins);
- one shared dependency walk with exact evidence error paths;
- the list-cap test made to isolate the cap.

**Failing-first and verification.**

- 13 envelope/pin tests in `definitions/formula_admission_tests.rs`. With a stub
  projection, 12 were red; the 13th is the pre-M2 byte/hash regression vector, captured
  from the pre-change code before any edit, which must stay green.
- The 3 publication tests were written after the code. They were confirmed by mutation:
  publish gate removed, evidence check skipped, encoding gate opened, and the item-i64
  check disabled. All 4 mutants were killed.
- Gates passed: `cargo fmt --check`; strict clippy; `cargo test --all-targets
  --all-features` (329 passed, 182 lib); release formula tests (43, calibration
  17.0 ms); the `formula_eval` build; ruff check/format; unittest (93, including 5
  CLI); actionlint; `cargo audit`; `cargo about`.
- No browser, package, unpacked or live-model checks were run.

## 16. M3 implementation (2026-10-16, branch `derived-formulas`; reviewed, fixes applied, re-review pending)

Plan: [derived-formulas-m3-plan.md](derived-formulas-m3-plan.md), with its binding §10–§10d
amendments. Commits are local only.

**What changed**

- **Pin carries the compiled formulas** (D1). `formula_admission::check` and
  `admission::domain` return `Option<formula::Compiled>`; `admission::validate` drops it at
  once for draft, store and publication callers; `admission::validate_compiled` keeps it
  for `private_pin::validate` only. `ImmutablePin::evaluate` (`None` for formula-free
  pins) runs with a test-only 60 s deadline seam; production uses `formula::DEADLINE`.
- **Evaluation at render time with PRG** (user decision Q1). This deviates from §7 "save
  evaluates proposed data": the POST never evaluates; the redirected GET evaluates the
  stored data, which is the same pure function of pin and data. Evaluation runs once per
  response inside `rendered`'s `spawn_blocking`, after the final authorization recheck,
  under the existing `JOBS` permit, never on the create page and never of a rejected draft.
- **View model** `campaigns/formulas.rs`: `present(pin, Result<&Evaluation, &InputError>)`
  (K37), an exhaustive code→message match (table in `runtime-definition-contract.md`),
  enum values by option label, tri-state checks with violations first, and "unavailable"
  for an `InputError` only (§10b 20). `dependency_failed` names the first failed
  dependency in evaluation order via `Compiled::failed_dependency` (Q5); `Evaluation`,
  `Code` and the golden fixture digest are unchanged.
- **Template**: derived fields are `<dl class="derived">` read-only text with a
  "Computed" label and a collapsed "How this is computed" disclosure (expression and rule
  IDs); they are never controls (the M2 latent bug). Advisory checks are a section after
  the form. Runtime sheets disable each control instead of the whole fieldset so the
  disclosures work in read-only history (L1); the native sheet keeps its disabled
  fieldset. A failed save shows saved-revision results in a separate "Computed from saved
  revision N — not from your unsaved changes" block.
- **K40**: `ImmutablePin::evaluate` drops only the values of input fields that the formula
  layer does not declare (hyphenated IDs); other unknown keys still reach the engine.
- **K39**: a save rejected by the database step keeps its permit for the re-render
  (`private_save_keeping`), so a concurrent request cannot turn it into a 503 that loses
  the draft.
- **`preview`** (§7 listed it, but it did not exist): library-only
  `formula::preview(&Compiled, before, after)`; changes in evaluation order; an error on
  either side is `Unknown`, never a new or resolved violation (K42). No route or CLI.
- **`pub mod formula`** (§14.1 item 18, K34): stays `pub`, for `examples/formula_eval.rs`
  and because `preview` has no in-crate caller (`pub(crate)` would be dead code).
- **Pack test**: `formula_pack_tests.rs` covers envelope → admission → pin →
  `validate_values` → `ImmutablePin::evaluate` for one character (`bram-holt`) and the
  two-outer-armor mutation (§10b 27); the M1 fixture keeps the engine-level coverage of
  all four characters. The test-only translation drops the pack's `encoding`,
  `rules_revision` and `formula_language` (K35).
- **Freezing uses write-only pins** (§10b 19): `depart` validates up to 32 pins at once and
  drops them in the database worker; carrying 32 compiled sets is avoided with
  `private_pin::validate_for_write`, whose evaluation fails closed.
- **Gate**: `publication.rs` no longer refuses formula definitions; list definitions stay
  `list-editing-unsupported` until M4. Route tests of list sheets publish through a
  test-thread-only path.

**Measurements (report-only)**

- Pack, `--release`: admission + pin + compile 15.7–19.2 ms; evaluate 1.06–1.09 ms;
  cost 79,065; 656 expression nodes (debug: 67–78 ms and 3.0–8.3 ms).
- Densest admitted definition (240 formulas of 1,021 bytes, balanced sums): 122,640
  expression nodes, `size_of::<IdedExpr>()` 64 bytes, i.e. several MB per pin plus box
  overhead. The largest cost-budget definition has 312 nodes at cost 1,966,128 (≈18 ms
  evaluation at the cap, M1 calibration).
- Worst-case permit hold per sheet request ≈ pin admission and compile (tens of ms) +
  input conversion + the 500 ms deadline + one formula (bounded by `MAX_COST`, not the
  deadline). `Retry-After: 1` therefore stays. `spawn_blocking` is not cancellable: an
  abandoned request keeps the permit until its evaluation ends, within that bound.
- Release binary: 20,802,360 bytes vs 20,447,416 for master `60e88a6` built in the same
  container (+354,944 bytes, +1.7%). The 18,210,072 figure predates `cel`.

**Verification after the review fixes (2026-10-16, revision `3708836`, rebased on master
`2566040`, every container at `--cpus 2`)**

- `cargo fmt --check`; strict Clippy (all + pedantic, `-D warnings`); `cargo test --locked
  --all-targets --all-features`: 369 passed, 0 failed, 1 ignored (the existing performance
  measurement), of which 222 are library tests.
- `cargo test --release --locked --lib formula`: 82 passed (timings above).
- The `formula_eval` example build; `source_fixture` generation; Ruff check and format;
  the tools unittest suite, 259 run (3 skipped); actionlint; `cargo audit --deny warnings`
  (322 crates); `cargo about generate --locked --fail`.
- Release build of 20,802,520 bytes (+355,104 against master `60e88a6`, +1.7%).
- `cargo package --offline` (the gate containers have no network), then
  `verify_source_package.py`, `cargo vendor` and `package_release.py`.
- Artifact `BUILD.txt` revision `3708836a7484e5b0620a4f97730147a5168b0651`. SHA-256:
  - `mimik-0.1.0-linux-x86_64.tar.gz`
    `f2edc84928c346be665ccfaa253c89f3aa1329cb5e789db4a2d924cf87618609`
  - `mimik-0.1.0-source.crate`
    `cc36718e9e64f54e44bc135f8e42757282dc077056b2a7543d65aead3fe86a44`
  - `mimik-0.1.0-dependency-sources.tar.gz`
    `89942d18cfb867e1ab1300abc238aef575e6ea3927bf7d03e0dfbb59953b145c`
- The checksums verified, the archive unpacked, and the unpacked binary equals the release
  build. `smoke_release.py --auth` on the unpacked binary passed.
- The offline Chromium gate (CI flags, 1280×720 and 390×844) with the formula stage
  passed. Its image tools match the reviewed sources by hash.
- The delivered source and vendored dependencies rebuilt offline with an empty `CARGO_HOME`
  and a fresh target directory (`/target/formula-fresh`, removed afterwards): passed in
  6 min 19 s. The rebuilt binary is not byte-identical to the shipped one; CI does not
  require identity, and the release build is not reproducible across paths.
- `tools/lan_smoke.py` passed on the same unpacked runtime and the matching unpacked
  source/vendor: separate-client LAN HTTP, HTTPS proxy, a denied peer, and QR decoding at
  both viewports. A local `docker` shim added `--cpus=2` to the one container the script
  starts without a CPU limit (the QR decoder), for the machine schedule.
- Not certified: physical phone/router setups, as the gate itself states.

**Red→green record (review-ready)**

Commands ran in `mimik-ci` with `--cpus 2`: `cargo test --locked --lib -- <filter>`.
"Executed red" means the test ran and failed for the stated reason before the fix.
"Compile red" means the API did not exist yet, so the only red state was a compile failure.
"Guard" means the test passed when it was introduced and protects existing behaviour.
Mutations were applied to the fixed code, run, and then reverted.

| Test | Kind | Base or mutation | Actual failure | Fixed in |
|---|---|---|---|---|
| `formula_sheet_view_shows_derived_values_and_advisory_checks` | executed red | C1 tree with `web.rs` and the template stashed | "derived pace not rendered" | C1 (`b1c896b`) |
| `derived_fields_render_without_form_names` | executed red | same | rendered names included `value-pace`…`value-echo` | C1 |
| `failed_save_rerender_labels_the_saved_revision` | executed red | same | "section formula-results missing" | C1 |
| `formula_text_is_escaped` | executed red | same | "view: escaped <script>alert(1)</script> absent" | C1 |
| `history_recomputes_identically` | executed red | same | "section advisory-checks missing" | C1 |
| `stored_valid_sheet_with_refused_input_shows_unavailable` | executed red | same | "section formula-results missing" | C1 |
| `violating_save_succeeds_and_view_lists_violation` | executed red | same | summary line "Advisory checks: 1 violated…" absent | C1 |
| `withdrawn_approval_still_evaluates_with_warning` | executed red | same | "derived pace not rendered" | C1 |
| `submitted_derived_key_is_rejected_and_not_saved`, `viewing_never_writes`, 503-after-save | guard | same base | passed | C1 |
| view-model tests (`campaigns::formulas`) | compile red, then mutation | `present` maps errors to values | 3 tests failed (tri-state, deadline, derived states) | C1 |
| `pin_evaluates_formula_definitions_and_not_formula_free_ones`, pack tests | compile red, then mutation | `pin.evaluate` always `None` | all failed on `unwrap` of `None` | C1 |
| `preview_*` (3), `failed_dependency_names_…` | executed red | stubs (`preview` empty, `failed_dependency` `None`) | assertion failures | C2 (`bd792f8`) |
| `failed_dependency_names_…` | mutation | dependency-set order instead of `derived_order()` | `Some("b")` vs `Some("m")` | C2 |
| `dependency_failed_names_the_dependency_by_label` | executed red | C1 view model | "a derived value it uses could not be computed" | C2 |
| `hyphenated_unreferenced_fields_do_not_block_evaluation` (K40) | executed red | C2 | `pin.evaluate` returned `InputError` at `/hit-points` | C3 (`8f71120`) |
| `create_page_never_evaluates_and_create_saves` | guard, then mutation | evaluate on the create page | "create: `{}` was evaluated" | create skip landed in C1 |
| `failed_database_save_keeps_its_permit_for_the_rerender` (K39) | executed red | C3 | a concurrent request took the permit, so 503 and the draft was lost | C4 (`ad44dd0`) |
| same, review fix F2 | mutation | kept permit dropped, `None` passed to `rejected_editor` | `[false]` vs `[true]`: "must reuse the rejected save's own permit" | `3708836` |
| `rendered_form_round_trips` | mutation red | template renders derived fields as controls (the M2 bug) | POST of the parsed form gave 400, want 303 | fixed template since C1; test in `a19ac6f` |
| `write_only_pins_carry_no_compiled_formulas_and_fail_closed` | compile red, then mutation | compiled formulas kept | assertion failed | `6b53743` |
| `departure_and_removal_freeze_formula_sheets_with_write_only_pins` (F1) | mutation | `depart` uses `private_pin::validate` | `[true, true]` vs `[false, false]`: "departure pins carry compiled formulas" | `6b53743`; test in `3708836` |
| `formula_free_pins_never_evaluate_even_when_write_only` (F3) | executed red | `6b53743` (write-only check first) | `evaluate` returned `Some` | `3708836` |
| `undeclared_inputs_are_the_fields_missing_…` (F4) | guard, then mutation | predicate also excludes booleans | `["hit-points"]` vs `["hit-points", "x-y"]` | `3708836` |
| `quote_payloads_stay_one_attribute_value` (R2) | guard, then mutation | input `value` rendered with `|safe` | "view: attribute injected" | `3708836` |
| `formula_text_is_escaped`, saved-revision block (R2) | mutation | re-render block value with `|safe` | "re-render: raw script tag" | `3708836` |
| `list_definitions_stay_drafts_until_editing_ships` (test 20) | executed red | gate on | `formula-evaluation-unsupported` | `81c4d01` |
| browser formula stage | negative control | expected pace changed to `9` | "Derived value missing after save" | `81c4d01` |

## 17. M4 implementation (2026-10-17, branch `derived-formulas`; independent review pending)

Plan: [derived-formulas-m4-plan.md](derived-formulas-m4-plan.md) revision 3.1 (review record
[reviews/2026-10-16-derived-formulas-m4-plan.md](reviews/2026-10-16-derived-formulas-m4-plan.md),
round 3). Commits are local only; the branch was rebased on master `f9d34e0` before the
step-8 gate pass (master gained tool-loop pack revision 3 and tools; no `src/` change).

**Commits per step**

| Step | Commit | Content |
|---|---|---|
| 3.1 | `381f416` | Plan revision 3.1 and the round-3 review record |
| 1 | `b5b003c` | Writer host-ID invariant; decoder-bound `encode` |
| 2 | `6e475dd` | `PairsSeed`/`parse_limited`, set-based admission, `values::sheet_keys`, D4 precedence |
| 3 | `61074d8` | `scraper` adoption, rebuilt `submitted` helper, `ListDraft`/`reconcile`, list template, top Save |
| 4 | `0456bbb` | Draft operations Add, Remove, Move up, Move down |
| 5 | `b30c002` | History rows; `history_diff` change summary |
| 6 | `4dbfe99` | Contract and workspace docs, smoke version selection, `list-definition.json` fixture |
| 7 | `76f057e` | 5 MiB sheet POSTs, pre-body principal check, echo cap and excerpts |
| 8 | `92cd03f` | Publication budget and gate lift, bypass deleted, smoke and browser list stages, scope docs |

Steps 1–7 were gated individually (fmt, strict Clippy, full tests) before the rebase; the
rebased commits were not re-gated one by one. The step-8 candidate (tree `aac0b3a`, which is the tree of `92cd03f`) passed the
full set below before it was committed.

**What changed (beyond the plan text)**

- **Deviation, demonstrated: no `#list-f` fragment on op buttons.** D5 asked for
  `formaction="<action>#list-f"` and exactly one `autofocus`. Chromium does not run
  autofocus when the document URL has a fragment that indicates an element, so the focus
  rules could not hold together with the fragment. The browser stage failed with the
  fragment (`document.activeElement` stayed the body after Add, "Add did not focus the new
  row's first control ('')") and passed without it, on otherwise identical builds. Op
  buttons now post to the form's own action; the focused control scrolls into view. Tests
  25 and 36 assert that no op button carries `formaction` and that the form action is the
  generated same-origin path. The contract says so.
- **Admission limits the budget arithmetic.** Admission allows 2,048 enum options per
  definition in all, so a scalar-only definition reaches at most a few thousand render
  units and no M3-publishable definition is refused by the new budget. The exact-budget
  fixture reaches 32,768 units through three lists of 32 properties plus 244 texts.
- **Test 5 reaches the writer directly.** Its 4 lists × 256 × 32 integers definition is now
  refused at publication (render and node budgets), so the test stores the admitted
  definition as an unpublished version row and calls the writer with that pin.
- **Echo excerpts (RE3-1)** cover the scalar invalid-value and invalid-state options too.
- **Test placement:** tests 7, 10 and 11 live in `campaigns/list_tests.rs`; test 12 in
  `campaigns/list_values_tests.rs`; tests 44 and 45 (dominance) in
  `definitions/publication_tests.rs`. The shared source fixture is re-exported test-only as
  `definitions::source_fixture` for the `list-definition.json` route test.
- **Heavy tests slimmed for the shared CI box.** An auth test once timed out (408) while the
  heavy page tests ran in parallel in a debug build; test 34 now uses 10 properties and 296
  rows, and the worst-label markup test one list of 256 × 32.

**Measurements (report-only, release build, `--cpus 2`, a CPU benchmark running on the box)**

- Exact-budget sheet (32,768 render units, 27,640 form pairs, 1,156,401-byte body, 959,233-byte
  stored snapshot, 18 envelope nodes against `ENVELOPE_NODES` = 32): save 196 ms (303); view
  5,025,717 bytes in 136 ms; op re-render of the full draft 5,037,418 bytes in 151 ms; 422
  re-render 5,037,504 bytes in 134 ms; change summary of every row edited 60,577 bytes (≤
  `SUMMARY_BYTES` = 1,675,264) in a 4,591,229-byte history page. Each request holds the runtime
  permit for its own duration, so these times are the permit holds. No legitimate save comes
  near the 2.5 s stop threshold.
- Worst-label page (labels of 100 `"`, 256 rows × 32 properties, 9,220 units): 5,733,829 bytes,
  622 bytes per unit against the 2,048-byte bound.
- A slow 5 MiB upload (64 chunks every 100 ms) by a signed-in principal holds the runtime permit
  for 5.006 s and gets 408 from the 5 s request timeout (the accepted N3/N-H risk). A 5 MiB
  request that completes was not timed separately; the release measurement run, including the
  5 MiB oversized-draft test, took 8.7 s for 6 tests.
- Pack (release): admission + pin + compile 10.9–13.9 ms; evaluate 0.64–0.89 ms.
- Release binary 21,048,408 bytes against 20,802,520 for master `f9d34e0` built in the same
  container (+245,888 bytes, +1.18%). `scraper` is a dev-dependency and not in the binary.

**Verification on the step-8 candidate (tree `aac0b3a`, every container `--cpus 2`)**

- `cargo fmt --check`; strict Clippy (all + pedantic, `-D warnings`); `cargo test --locked
  --all-targets --all-features`: 442 passed, 0 failed, 1 ignored (the existing performance
  measurement), of which 295 are library tests.
- `cargo test --release --locked --lib formula`: 82 passed; the release measurement run: 6
  passed.
- The `formula_eval` example build; `source_fixture` generation (now with
  `list-definition.json`); Ruff check and format; the tools unittest suite, 452 run (39
  skipped); actionlint; `cargo about generate --locked --fail`; `cargo audit --deny warnings`
  (333 crates scanned against an advisory database of 1,334 entries, no findings; the one
  networked run besides the step-3 fetch).
- Release build; `cargo package --locked --allow-dirty --offline` (including its verify
  build), `verify_source_package.py`, `cargo vendor --offline` and `package_release.py`
  (revision recorded as `tree-aac0b3a…`).
- SHA-256: `mimik-0.1.0-linux-x86_64.tar.gz`
  `51fee670e0f7a3d24c33ceb8a3e08052041bcdc7827c543f34827d8a6c137d7c`;
  `mimik-0.1.0-source.crate`
  `12156e54a472d1a59e6a9ba45786e537bf8884fe8cba6f7cdf27e086c13ee57a`;
  `mimik-0.1.0-dependency-sources.tar.gz`
  `427e0aabf938bb7d797f3e2943648c8e64beb307f7c758ba200de81d50996f52`.
  The checksums verified, the archive unpacked, and the unpacked binary equals the release
  build.
- `smoke_release.py --auth` on the unpacked binary passed, including the new list stage
  (third published version, chosen by ID and logged).
- The offline Chromium gate with the CI flags (1280×720 and 390×844) passed, including the
  list stage. The image `mimik-browser:m4` is `mimik-browser:formula` (same pinned base and
  pip layer) plus the four current tool files, whose SHA-256 match the worktree; the
  Dockerfile's pip layer could not be rebuilt offline.
- The delivered source and vendored dependencies rebuilt offline with an empty `CARGO_HOME`
  and a fresh target directory (removed afterwards): passed in 5 min 49 s.
- `tools/lan_smoke.py` on the unpacked runtime and the matching unpacked source/vendor passed
  (separate-client LAN HTTP, HTTPS proxy, a denied peer, QR decoding at both viewports). A
  local `docker` shim added `--cpus=2` to the containers the script starts without a CPU
  limit.
- Not certified: physical phone/router setups, item catalogues, per-property `editable:
  false`, value diffs and revert.

**Red→green record**

"Executed red" ran the test against the previous step's code (the step's source files
stashed) and saw it fail for the stated reason. "Compile red" means the API did not exist.
"Guard" passed when written and is paired with a killing mutation where one is named.
Mutations were applied to the fixed code, run, and reverted.

| Test (plan #) | Kind | Base or mutation | Failure seen |
|---|---|---|---|
| `writer_refuses_client_chosen_new_ids` (1) | executed red | step-1 base | "create: want Invalid, got Ok(1)" |
| `writer_refuses_removed_id_reintroduced_and_wrong_or_noncontiguous_k` (2) | executed red | step-1 base | "removed ID reintroduced: want Invalid, got Ok(3)" |
| `writer_accepts_mixed_retained_new_rows_per_list_reuse_and_swaps` (3), `departure_and_removal_freeze_keep_arbitrary_valid_ids`, `stale_write_with_bad_ids_is_409_not_422` (4), `decoder_bound_snapshot_writes_reopens_and_freezes` (6) | guard | mutation: invariant uses the pre-increment revision | 6 of 7 list tests failed |
| `writer_refuses_undecodable_snapshot_atomically` (5) | executed red; mutation | step-1 base; decode check removed (step 8) | "want Invalid, got Ok(1)"; failed |
| `list_row_keys_admit_and_bad_key_sets_are_400` (7) | executed red; guard negatives | step-2 base; mutation "admit any well-formed list key" | positive answered 400; negatives failed under the mutation |
| `parse_limited_takes_its_limit_from_the_seed` (8) | compile red; mutation | seed ignored, 516 hard-coded | failed |
| `admission_of_65536_pairs_is_linear` (9) | mutation red | `contains` over a key slice | 78.08 s against the 5 s limit |
| `stale_invalid_draft_conflicts` (10) | executed red | step-2 base | "scalar runtime left: 422 right: 409" |
| `stale_invalid_list_draft_conflicts` (10, list case) | guard | — | already 409 after step 2 |
| `bad_keys_after_csrf_and_authorization` (11) | guard | admission before CSRF | failed |
| `list_values_tests` (12, five tests) | compile red; mutations | unknown ID accepted, duplicates accepted, `set-` ignored | each failed |
| `list_sheet_rendered_form_round_trips` (13) | executed red; mutation | step-3 base; sheet cap reverted to 516 | pair-count assertion failed; failed |
| `list_rows_render_as_labelled_fieldsets_unique_ids` (14) | executed red | step-3 base | "equipment and tags left: 0 right: 2" |
| `failed_save_rerender_keeps_draft_and_submitted_revision` (15) | executed red | step-3 base | "row-1-0-0 missing" |
| `submitted_helper_sends_only_the_clicked_submitter` (16) | compile red | — | — |
| `first_submitter_in_tree_order_is_op_save_before_groups` (17) | executed red; mutation | step-3 base; top Save with the wrong op | "section:=" first; failed |
| `rendered_form_round_trips` (M3, now expects `op=save`) | executed red | step-3 base | 9 pairs, want 10 |
| op tests 18–27 (19 tests) | executed red | step-4 base | buttons absent; with forged op pairs every op answered 400 |
| 19, 20 | mutations | op does nothing; wrong neighbour; `set-` state lost; hidden ID lost | each failed |
| 21 | mutation | edge move accepted | failed |
| 23 | guard; mutation | op skips CSRF | failed |
| 25 | mutations | `formnovalidate` dropped; a second `autofocus` | each failed |
| `history_diff` unit tests (29–31, seven tests) | compile red; mutations | moved by index; tie-break reversed; optional presence ignored | each failed |
| `history_page_shows_change_summary_for_revision_n_against_n_minus_1` (32) | executed red; mutation | step-5 base; N against N − 2 | no lines; failed |
| `change_summary_unavailable_*` (33, two tests) | executed red; mutation | step-5 base; N − 1 used without validation | no section; failed |
| `change_summary_large_diff_is_bounded` (34) | executed red; mutation | step-5 base; summary cap removed | 0 lines; failed |
| `decoder_bound_history_renders_read_only` (28) | executed red | step-5 base | no summary lines |
| `list_history_rows_have_stable_ids_and_no_ops` (28), `derived_values_over_lists_update_after_save` (35) | guard | — | — |
| `list_payloads_escaped_in_context` (36) | executed red (summary part); mutation | step-5 base; `|safe` on a summary label | summary assertion; failed |
| `VersionSelectionTests` (38) | executed red | step-6 base | ImportError `version_link` |
| `sheet_post_accepts_5mib_and_413s_5mib_plus_1`, `non_sheet_posts_keep_8kib` (39) | mutation red | no 5 MiB layer; 5 MiB on every route | 413; failed |
| `unauthenticated_large_post_is_403_without_reading_the_body` (40) | mutation red | pre-body check removed | body polled |
| `stored_over_1mib_is_422_with_draft` (41) | mutation red | no 5 MiB layer | 413 |
| `oversized_draft_is_not_echoed`, `one_mib_of_quotes_is_echoed_within_bound` (42) | mutation red | no 5 MiB layer; echo cap removed; excerpts disabled | each failed |
| `list_definition_publishes` (43), `lantern_reach_list_sheet_end_to_end` (47), `render_budget_boundary_publishes_and_plus_one_refuses` (45), `exact_budget_sheet_saves_reopens_and_freezes` (46) | mutation red | the gate restored | `list-editing-unsupported` |
| `budget::*` (44, 45 dominance) | compile red; mutations | `within` ignores units, pairs or nodes; the optional +2 dropped | each failed |
| browser list stage (48) | negative control | the `#list-f` fragment restored | "Add did not focus the new row's first control ('')" |

Not mutation-run: "the op path writes" and "the draft is evaluated" (covered by the
nothing-written assertions of 19/20 and the saved-revision assertions of 23).

**Open for the review**

- The independent implementation review (plan step 9) with Sol, Sonnet and the third chain.
- The `#list-f` deviation above, and the slimmed heavy tests.

**Implementation-review fixes (2026-10-17)**

Review: Sonnet MERGE, Sol MERGE AFTER FIXES, Space Bunny (third chain) MERGE AFTER FIXES; all
three accepted the D5 deviation. Record:
[reviews/2026-10-17-derived-formulas-m4-impl.md](reviews/2026-10-17-derived-formulas-m4-impl.md).
Fixes: the visible submitted revision is excerpted (Sol Major 1); every private-sheet
textarea emits a sacrificial newline (Sol Major 2); the streamed-body 413 guard (Space Bunny
F1, adjudicated not a defect); D7 and the contract state what the page bound covers (F2); the
`form_budget` approximation comment and a second calibration (F3); the optional-first-
property autofocus test (F4); plan revision 3.2 (F5); the `outside_increasing` invariant
(F6); the documented lost-draft 403 path (F7); the buffering risk (Sonnet); wording nits.

| Test | Kind | Base or mutation | Failure seen |
|---|---|---|---|
| `malformed_near_cap_revision_is_echoed_once` | executed red | `18c6a07` templates and `web.rs` | 2,000,014 `&#34;`, want 1,000,078 |
| `leading_newlines_survive_ops_rejected_saves_and_reloads` | executed red; mutation | `18c6a07`; list textarea without the newline | "saved reload: notes" `"notes"` vs `"\nnotes"`; failed |
| `stale_op_warns` (new wording) | executed red | `18c6a07` | old wording |
| browser list stage, leading newlines | executed red | release build without the newline | "Leading newlines lost after draft operations" |
| `streamed_body_over_5mib_without_length_is_413` | guard; mutation | `DefaultBodyLimit::disable()` on the sheet POSTs | failed |
| `add_focuses_the_set_select_of_an_optional_first_property` | guard; mutation | Set/Not set select never autofocused | failed |
| `all_optional_worst_label_page_is_within_the_unit_bound` | guard (measurement) | — | 246 bytes per unit |
| `TextareaNewlineTests` CRLF/CR case (re-review, Sol) | executed red | reader before `feed` normalisation | `'\r\nplain'` vs `'plain'` |
| `native_sheet_textareas_keep_leading_newlines` (re-review) | guard; mutation | native description textarea without the newline | failed |
| `material_textareas_keep_leading_newlines` (re-review N2) | executed red | `material_page.html` before the newline | `"corrected text"` vs `"\ncorrected text"` |
| `legacy_editor_notes_keep_leading_newlines` (re-review N2, N6) | executed red | `editor.html` before the newline | `"\nlegacy notes"` vs `"\n\nlegacy notes"` |
| `material_malformed_revision_is_not_echoed` (re-review N6) | guard | — | revision is `i64`: refused before rendering |
| `browser_crlf_submission_is_stored_as_lf`, `submitted_values_have_crlf_and_cr_normalised_to_lf` (final review, Sol) | executed red | transport without CR normalisation | CR LF stored |
| `test_a_crlf_split_across_two_feeds_is_one_newline` (final review F1) | executed red | reader without the held-back CR | `'\nplain'` vs `'plain'` |
| smoke context goal and source correction newlines (final review F2) | mutation red | `mork_context.html` without the newline | "Context goal lost its leading newlines" |
| `material_completion_rationale_keeps_leading_newlines` (final review F2) | guard; mutation | completion branch without the newline | failed |
| final-review gate pass (tree `217cf06`) | gate failure | `smoke_release.py` post-restart search check expected no leading newline | "Reviewed evidence persistence failed"; first relaxed, then made strict in the last review |
| `material_crlf_at_the_limit_…`, `source_review_crlf_at_the_limit_…`, `legacy_editor_crlf_notes_at_the_limit_…` (last review F1) | executed red | legacy extractors without `form_text::lf` | 422, want 303 |
| history and search `<pre>` sacrificial LF (last review F4) | mutation | material or source `<pre>` without the newline | failed |
| `CorrectedEvidenceTests`, `FormFieldsLifecycleTests` (last review) | executed red | smoke helper absent; no `reset()` override | ImportError; AttributeError |
| `draft_op_rerender_has_crlf_normalised` (last review F8) | guard | — | — |
| `TextareaNewlineTests` (smoke form reader) | executed red | `smoke_release.py` before the fix | `'\nplain'` vs `'plain'`; found by the first gate rerun (`smoke_auth` failed "Persisted field differs: equipment") |

## Appendix A: superseded custom-spec design (revision 1)

Revision 1 proposed a Mimik-specific expression language:

- `and`/`or`/`not` keywords and `if()`;
- floor division and modulo;
- strict short-circuit logic;
- aggregates as `sum/max_of/min_of/count(coll, value, [default], where)` with
  an implicit `item`.

It was parsed with `winnow` 1.0.4 and backed by a Python reference
(`tools/formula_ref.py`) and a shared fixture. CEL crates had been rejected for
truncating division and absorbing logic. The user has since accepted exactly
those CEL semantics, which removes the reason for a custom language and the
maintenance of two implementations.

Carried forward from revision 1:

- the explicit error vocabulary;
- static admission checks (types, cycles, enum-typo guard, cost budget);
- derived values that are never stored;
- advisory constraints;
- the evidence/publication binding;
- the additive contract-1 decision (D1);
- the milestone structure.

Dropped: the winnow parser, gap list G1–G19 (resolved by CEL semantics), and
the custom-spec fixture.
