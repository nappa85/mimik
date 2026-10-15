# First runtime-definition contract — revision 2

Status: specification independently approved as plan-ready on2026-10-12;
registry/source identity, strict draft admission, immutable publication and private
generic campaign editing are implemented. See `runtime-registry.md`,
`definition-drafts.md`, `definition-publication.md`, `campaign-workspace.md` and
TODO for executed gates and scope. Reviewer `ses_ed079ec05ffetAvueggSB2b6Ip` resolved
R1–R4 at specification level. Container fit results are in `runtime-validator-fit.md`.
This provider-free contract does not establish model wiring, uploaded template
fidelity or semantic edit review; those are the next experience experiment in
`experience-validation-plan.md`. Unsupported controls below remain explicit.
Resolves the R1–R4 design findings in `reviews/2026-10-12-creation-layout-closure.md`.
Library integration and implementation gates remain separate from plan approval.

## 1. Bounded acceptance story

Without rebuilding Mimik, a local operator registers an unknown game/edition,
imports its selected source set, imports/edits a declarative definition draft,
explicitly approves its sheet wiring against original pages, creates a generic
character, and edits/reopens its values. No provider is required. Model-generated
drafts can later enter the same admission path; no live model is included here.

Campaign clarification: every new generic sheet must select a campaign whose
exact registered game/edition pair matches the approved definition. Campaign
membership is explicit, and Master grants apply only to that campaign. Implement
a compatible campaign binding before the generic creation proof; authentication
later supplies actual current-user grants. Existing prototype records remain
unaffiliated until explicit assignment, never autoassigned by their game labels.

Prototype reset policy: the user authorizes wiping disposable prototype records
if they obstruct solid constraints. Preservation above describes existing
compatibility, not a requirement to weaken new generic/campaign invariants or
build elaborate migration paths for test data. New draft admission has no such
exception; its additive table does not require a reset.

First controls: text, multiline, signed integer, nullable boolean, enum, flat
ordered groups. Lists, nested/repeating groups, decimals, formulas, scripts,
generated templates and deterministic game-rule enforcement are unsupported.
Current native MÖRK BORG characters/checklist remain compatibility fixtures.

## 2. Registry, identity and legacy migration (R3)

- Stable `game_id`, `edition_id`, `definition_id`, field/group/enum-option IDs use
  `[a-z][a-z0-9-]{0,63}`. Edition IDs are unique **within a game**; definition IDs
  within that game/edition. IDs are immutable data, never paths, templates or
  trusted CSS selectors. Labels are independent escaped Unicode text.
- Source printing/language/PDF identity remain separate metadata. Registration
  does not guess equivalence between books, languages or printings.
- Exact legacy alias table maps `(old system_id, old edition string)` to stable
  `(game_id, edition_id)`. One alias has one target; collisions/ambiguous labels
  fail explicitly. Comparison is exact after existing stored normalization,
  without case-folding, filename matching or approximate edition inference.
- Seed `mork-borg` / `it-2025` with display label `MÖRK BORG — IT 2025` and its exact
  legacy alias. Reserve that pair/native adapter identity. Runtime registration,
  aliases or definition publication cannot shadow native behavior.
- Transactional migration adds nullable registry associations to existing
  character/source records while retaining old strings and saved data. Backfill
  only explicit aliases. Unmapped legacy records remain readable/editable through
  their legacy path and visibly unresolved, never relabelled or silently generic.
  Owner/operator can later register an explicit mapping; native aliases cannot
  be reassigned. Migrations do not reconstruct nonexistent old edit history.
- New source imports require registered exact identity. During transition accept
  the old importer identity only through an exact alias; new explicit stable
  game/edition selection must agree with bundle metadata/registered aliases.
  Add a reviewed versioned source-bundle identity extension rather than treating
  every legacy edition label as a slug. Unknown identity fails before insertion;
  importing evidence still does not approve it. Transitional v1 compatibility
  imports for legacy `dnd`, `pathfinder`, `vampire`, `mork-borg` may remain
  unresolved; this is an explicit exception to new v2 registered-identity imports,
  not permission to activate an adapter from raw labels. Unknown-family v1 imports
  require an explicit same-game alias. v2 always requires a registered stable pair.

## 3. Closed definition profile and admission (R2)

Contract version 1 defines a Mimik-owned envelope, **not uploaded JSON Schema**:
game/edition/definition IDs; title; selected source references and roles;
fields (ID, label, control, `nullable`, optional bounded help text and enum
options); groups (ID, label, ordered field IDs, column hint); per-field mappings.
Column hints are only `auto`, `main`, `sidebar`; they are advisory reflow hints.
Unknown properties fail at every object level. Every field appears in one group
exactly once. References must resolve within the admitted envelope/source set.

Implemented draft profile freezes optional help/non-enum options omission/null
as canonical explicit null; enum options must be a nonempty supported array.
Mapping references are positive database source/page/unit IDs, not coordinates.
See `definition-drafts.md` for the exact bounded parser and canonical encoding.

Each field has one mapping state: `mapped` with source/page/unit references, or
`unresolved` with a reason. Source roles are `sheet`, `core`, `supplement`; a source
can have several explicit roles. At least one selected sheet source is required
to approve any mapped wiring. At least one field is mapped for partial approval.
Unresolved fields and counts remain visible after partial wiring approval.
No supplied minima/maxima, prerequisites or model-generated gameplay restrictions
are enforced. `nullable` is a reviewed storage/form choice, not proof that a
game requires or forbids an unknown value.

### Input and work limits

| Resource | First-slice limit |
| --- | --- |
| Draft definition body and retained raw invalid draft | 256 KiB UTF-8 each |
| Canonical character body / URL-encoded submission | 1 MiB each; independently bound decoded payload |
| JSON nesting | 12 container levels, checked during decoding; retain Serde recursion limit |
| Fields / groups | 256 / 32; no nested groups |
| Sources / evidence dependencies | 32 / 1,024 distinct unit/page references |
| Enum choices | 128 per enum, 2,048 total |
| Labels / help / rationale | 100 / 2,000 / 5,000 Unicode scalar values, within body cap |
| Text / multiline values | 8,000 / 20,000 Unicode scalar values, within body cap |
| Character name / general notes | 100 / 20,000 Unicode scalar values |
| Structured diagnostics | 32 items, 256 characters each, then an explicit truncation marker |
| Derived value-validation schema | 256 KiB; compile only admitted definitions |

Reject duplicate keys **before** conversion to a `serde_json::Value` map or typed
envelope, including escaped-equivalent keys. Reject duplicate IDs, oversized
strings/collections and excess depth during decoding rather than after allocating
arbitrary trees. Use Serde's deserializer/visitor machinery; the narrow custom
admission wrapper covers its demonstrated last-key-wins `Value` gap, not a new
JSON tokenizer. Repeated URL form fields also fail, not last-value-wins.

Use proposed maintained `jsonschema` 0.58.6, `default-features = false`, explicitly
Draft 2020-12 and `.offline()`. Compile the trusted embedded envelope schema and
host-generated value schemas only. Host keywords: type/properties/required,
additionalProperties:false, bounded array/string size, enum and nullable unions.
Only local embedded `$defs` references; no uploaded `$schema`, `$ref`, regex,
formats, remote/file retrieval or arbitrary expression/content hooks. Generate
schemas from typed fields, never splice uploaded schema fragments. IDs use
host-owned simple checks; no user-controlled regex compilation. Domain checks
own identity/reference uniqueness, i64 range and lifecycle, avoiding duplicate
generic validation logic. See `dependency-decisions.md` for selection evidence
and the containerized fit gate still required before adding the dependency.

First slice has no unbounded compiled-validator cache: compile per admitted
operation, release afterward; embedded envelope validator is initialized once.
One nonqueued runtime admission/compile job per process; acquire before body
allocation/worker enqueue, fail busy with 503/Retry-After, and hold capacity until
bounded synchronous work ends even if the request is cancelled. Do not pretend
an async timeout stops an already-running synchronous validator. Cache and wider
concurrency are later measured changes. Apply server body limits independently;
raw invalid drafts return to the submitting client, not a global shared cache.

## 4. Publication and evidence validity (R1)

Drafts are mutable and revision-checked; each successful edit invalidates any
pending approval comparison. A published definition version/content is immutable.
Its approval is an immutable historical event, with actor label, rationale,
expected draft revision, explicit unresolved field IDs and exact dependencies.
Until authentication ships, actor is a local operator label, not authenticated
Owner/Master identity.

Approval snapshot binds:

- definition version and host-computed content hash;
- selected source-set membership and roles;
- source DB identity plus source ID, PDF hash, prepared/extraction/render hashes,
  exact language/printing identity and original page/image identity;
- page IDs/PDF numbers, page review revision/state, mapped unit/block IDs,
  polygon identity, unit revision/state and effective correction/text hash;
- what was actually compared: original image regions and the approved field
  mapping/layout, with unresolved regions listed rather than hidden.

Wiring can be compared using unreviewed sheet renders; approving wiring never
approves extracted rule units or page completeness. Rules readiness separately
requires accepted/corrected eligible units on complete exact-edition pages.

Perform expensive admission outside DB locks against an immutable candidate
snapshot. In one publication transaction re-read and match expected draft,
source set, artifact identities and **all captured review revisions/states**;
any mismatch rejects stale approval without partial publication. Allocate the
next positive i64 version and store definition/snapshot/approval atomically.
Database constraints prohibit published-content edits/deletion by ordinary code.

Changes to a dependency's correction, review state/revision, page completeness,
artifact, removal or selected-set membership make its current claims stale.
Use conservative **page-scoped** invalidation for captured pages: an unrelated
unit edit on the same captured page also changes its page revision and invalidates
that snapshot. Unrelated pages outside dependencies do not invalidate it. Replacing
a source artifact cannot overwrite the old provenance; register a new identity.
Deleting/removing a dependency produces explicit missing/stale state.

Never reactivate a stale approval merely because content later looks identical.
Explicit comparison/reapproval creates a new immutable approval event bound to
current revisions; identical published definition content can receive a new
approval event without rewriting its historical version. A changed definition
requires a new version. Capability use/creation rechecks snapshot validity in
the same DB snapshot/transaction as the operation, not against a cached badge.

Stale wiring blocks **new character creation** from that definition. Existing
characters remain readable and structurally editable using their immutable pin;
show stale wiring/evidence clearly and suppress affected rule/model readiness.
Do not delete their values or silently switch them to another definition.

### Hash contract

Use existing SHA-256/hex over domain-separated UTF-8 bytes from host serialization
of the admitted typed contract: fixed struct/property order, ID-keyed maps sorted
by ID, ordered fields/groups/enum choices retained, no floating-point values or
unknown members. Incoming key order/whitespace is not identity. Store the exact
hashed bytes, contract version and hash algorithm; maintain regression vectors.
This is a Mimik content encoding, not an invented claim of general JSON/JCS
canonicalization. Approval-event identity is separate from definition content.

## 5. Canonical values and writes (R4)

Characters pin game/edition/definition/version/hash and immutable schema. Generic
saves are **full replacements** of name, notes and values, plus expected revision
and exact pin. Every declared field key is required on every create/save; there
is no generic PATCH behavior in this slice. Native legacy-post omission behavior
remains confined to its existing compatibility editor.

| Submitted canonical value | Behavior |
| --- | --- |
| Missing declared key or unknown key | Reject; never keep an omitted old value implicitly |
| `null` | Unknown, valid only if the approved field is nullable |
| Text `""` / whitespace | Known empty / exact text retained; not silently null or trimmed |
| Boolean `false` / `true` | Exact JSON boolean, distinct from null; no string/number coercion |
| Integer `0` / negative | Exact signed i64, distinct from null; no gameplay bounds inferred |
| Fraction/exponent/numeric string | Reject in canonical JSON; integer token grammar `-?(0|[1-9][0-9]*)`, range checked without f64 |
| Enum value | Exact option ID, not display label; null only if nullable |

Canonical storage uses i64 JSON integer tokens; first-slice browser editing uses
server-rendered HTML form strings, never JS JSON.parse/Number for integer values
or revisions. Integer controls are text inputs with numeric input hints; explicit
unknown controls send null semantics, and server parsing checks the integer
grammar/range. Boolean controls are `unknown`/`true`/`false` selects, not ambiguous
omitted checkboxes. Text has a separate unknown control; blank text stays `""`.
Form decoding is the documented transport conversion into canonical types, not
permission for canonical JSON to coerce strings into integers/booleans. Reject
two contradictory values/duplicate form keys; preserve malformed raw strings.

Creation requires explicit valid full values: do not manufacture zero/false or
other defaults. The create form may offer null for nullable fields but the player
submits it explicitly. Required nonnullable fields need a value before saving;
the UI describes this as structural completion, not game legality. Name is
trimmed and must contain 1–100 characters; general notes retain exact text.

Revisions are positive i64, transmitted as exact decimal strings in HTML/envelopes.
Creation starts at1; a successful save (including a valid no-op) increments once.
Expected revision and pin must match; overflow rejects without mutation. Identity,
values and revision update in one transaction from a coherent snapshot. Structural
invalidity gives422 with raw draft; stale writes give409 with the submitted token,
not a refreshed token. Failed writes preserve all persisted state. Definition
updates never reinterpret saved values; explicit cross-version migration is a
separately reviewed future operation.

## 6. Capability states and future task/history seams

Report independent dimensions with reasons/dependency revision:

- **Wiring:** draft / approved-current (complete or partial) / approved-stale.
- **Sheet:** new creation available only for current approval; existing pinned
  editing available even when evidence is stale.
  New creation also requires an explicitly selected campaign with the exact
  same registered game/edition pair and, once accounts ship, a current campaign
  permission. Campaign changes cannot silently reinterpret a saved sheet.
- **Evidence prerequisites:** insufficient / reviewed eligible / stale or missing.
- **Advisory tasks:** unsupported in this provider-free slice even if evidence
  prerequisites are satisfied; later configured/ready/stale-result states cannot
  imply a model call or accuracy test has occurred.
- **Deterministic rules:** unsupported for generic games. Native fixture coverage
  remains separate; source review/schema validation is not game-rule coverage.

Later SheetWiring/ValidateSheet/RulesInquiry envelopes bind task/prompt version,
request/provider/model, definition/source/approval identities, character snapshot
revision, declared question/facts and bounded eligible evidence. Citations must
resolve into that snapshot but do not by themselves prove entailment. Recheck
bindings after network completion before labelling a result current. Models
cannot approve or mutate records; explicit authorized human saves remain separate.
Use established Rust provider/config abstractions, selected outbound content,
timeouts/token/concurrency/retry bounds and redacted logging; no DB lock over
network awaits, background uploads or silent fallback. Provider execution needs
its own reviewed contract and live verification before shipping.

Future collaboration/history mutation boundary carries operation, expected/new
revision, exact schema pin, before/after reconstruction data and authenticated
actor once available. Rollback will be a new revision with target-event provenance,
not history rewrites. Full history, permissions, soft deletion and Owner-only
purge remain deferred requirements in `collaboration-and-history.md`; this
contract does not claim revisions alone provide rollback today.

## 7. Implementation sequence and acceptance matrix

1. Containerized minimal-feature validator fit gate: resolved feature graph/MSRV,
   strict lint/audit/licenses, no external retrieval, exact integer behavior and
   measured release-size/startup/compile/validation cost. Initial isolated sample
   completed and independently approved for candidate selection
   (`runtime-validator-fit.md`); verify the real host-generated profile/combined
   dependency graph at integration.
2. Registry/legacy migration and bounded admission/definition validation; preserve
   native fixtures and add unknown-game source identity extension.
3. Transactional draft/publication/evidence snapshots and current capability gates.
4. Generic full-value create/edit/reopen with raw-draft/stale-write browser behavior.
5. Extend current process/browser/package/offline source gates for that complete
   synthetic unknown-game flow. No native game code/provider required.

| Scenario | Required result / verification |
| --- | --- |
| Unknown game register/import/draft/approve/create/edit/reopen | Same pin/values persist; integration/process/browser; synthetic not real sheet approval |
| Publication racing draft/evidence/review changes | Stale conflict, no partial event/version; multi-connection transaction regression |
| Correction, withdrawal, page completeness, removed dependency | Approval history intact; new creation/rules claims stale; existing pin editable |
| Explicit reapproval / unrelated page changes | New approval event; no silent revival; unrelated pages do not invalidate |
| Missing/ambiguous alias, alias collision/native shadow | Fail or remain unresolved; legacy/native data preserved |
| Campaign game/edition mismatch or unrelated campaign membership | Creation rejected; Master in another campaign grants no access; integration/browser once campaigns/accounts ship |
| Duplicate escaped JSON keys/IDs, excessive depth/bytes/counts | Rejected before unsafe expansion/compilation; parser/admission tests |
| Unknown properties, unsupported control, external schema references | Fail closed; offline/container network-denial checks |
| Missing/null/empty/false/zero, enum ID vs label | Exact table semantics; unit/integration/browser reopen |
| i64 extrema and ±(2^53±1), overflow, decimals/exponents/coercions | Exact integer tokens/text round trips; reject bad grammar/range |
| Full replacement, simultaneous/no-op writes, revision overflow | One coherent atomic snapshot; no omission retention or failed-write increment |
| Escaped labels/values/IDs and raw invalid/stale drafts | No executable markup; draft/token retained; browser |
| All new runtime/source/HTTP gates | Existing strict CI/deployability gates remain fail-closed; unpacked source rebuild |

Every reproducible defect needs a permanent failing-before-fix regression.
Independent reviews precede each material implementation step and completion.
Documentation approval, helper tests, browser synthetic proof, real source fidelity
and live model execution are distinct verification layers.
