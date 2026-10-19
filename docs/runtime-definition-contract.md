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
ordered groups. Nested/repeating groups, decimals, scripts, generated templates and
deterministic game-rule enforcement are unsupported. Current native MÖRK BORG
characters/checklist remain compatibility fixtures.

Derived fields and constraints are supported under `formula_profile: 1` ("CEL profile
cel-mimik-1", `docs/derived-formulas-plan.md`). Their results are **advisory**: derived
values are computed from the stored values and the immutable pin on every view, save
and history page, are never stored or submitted, and checks never block a save or rule
on legality. List fields (`control: list`) are editable on private sheets (M4, §5 "List
fields") and publishable within the render-unit budget (`list-form-exceeds-limit`,
`docs/definition-publication.md`).

How each result is shown (`src/campaigns/formulas.rs`, one exhaustive match): a value is
shown as text (integers as decimals, booleans as True/False, enums by option label); an
error as "Not computed: <message>." with its code; a check as Pass, Violated or Not
evaluated. Evaluation-reachable codes:

| Code | Message |
|---|---|
| `invalid_input` | the saved values do not match the formula declarations |
| `input_too_large` | the saved values exceed the formula input size limit |
| `collection_too_large` | a list has more items than formulas accept |
| `missing_property` | an optional item property it reads is absent |
| `null_value` | a value it uses is unknown (null) |
| `type_mismatch` | a value has a type the formula cannot use |
| `overflow` | the integer result is out of range |
| `division_by_zero` | it divides by zero |
| `invalid_argument` | a function received an argument outside its accepted range |
| `invalid_result` | the result is not a valid value for this field |
| `unknown_field` | it reads a field that is not available |
| `arity_mismatch` | a function was called with the wrong number of arguments |
| `dependency_failed` | it uses <Label>, which could not be computed (the first failed dependency in evaluation order) |
| `deadline_exceeded` | the time limit was reached. Reload to retry |
| `internal_error` | an internal error occurred. Reload to retry |

Admission-only codes (parse, depth, construct, identifier, declaration, cycle and cost
codes) share "unexpected formula state; report this sheet", which evaluation never
reaches. `deadline_exceeded` and `internal_error` are not deterministic (load, a failed
formula thread) and are logged with sheet ID, pin version and formula ID, never values.
A stored-valid sheet whose values the engine refuses as a whole (`invalid_input`,
`input_too_large`, `collection_too_large`) still renders with "Derived values
unavailable"; such saves are allowed.

## 2. Registry, identity and legacy migration (R3)

- Stable `game_id`, `edition_id`, `definition_id`, field/group/enum-option IDs use
  `[a-z][a-z0-9-]{0,63}`. Field and list item-property IDs also allow `_` (M2), so
  formulas can reference multi-word fields; list fields and item properties must be CEL
  identifiers (`[a-z][a-z0-9_]*`), and hyphenated scalar fields stay valid but cannot be
  referenced by formulas. Edition IDs are unique **within a game**; definition IDs
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

### Linked catalogues

Catalogue plan §3 step 3 (D3, D8, R10, R27). An optional top-level `catalogues` member links
read-only catalogues of the definition's own game and edition: 1–8 objects `{"id": …}`, each
id a registry slug, no duplicates, closed objects. Author order is kept and is part of the
identity (it is the sheet's display order). The member is the last one and is omitted when
absent, so definitions without it keep their canonical bytes and frozen hash vectors.

A link names no version. Readers use the catalogue's latest sealed version; append-only
publication keeps every key and entry type of earlier versions, so a link that was valid
stays valid. The link is checked at three sites: draft save, the publication comparison and
its approval transaction, and the readiness check (a linked catalogue without a sealed
version makes the wiring stale, so new sheets cannot be created; existing sheets stay
readable). The checks verify existence only: catalogue state never enters the wiring
snapshot, its hash or the approval envelope, and publishing a catalogue version never makes
a definition approval stale.

**Reproducibility contract.** Stored values, definition pins and formula results stay exactly
reproducible. Catalogue entry names, properties, descriptions, badges and citations are
current reference aids, shown from the latest catalogue version and the current source
review and labelled "current text"; a historical revision shows today's catalogue text.
Sheets cannot store catalogue keys until the `catalogue_ref` control lands (plan step 4).
Reading routes and badges: [catalogue-publication.md](catalogue-publication.md#reading-through-a-sheet).

## 5. Canonical values and writes (R4)

Characters pin game/edition/definition/version/hash and immutable schema. Generic
saves are **full replacements** of name, notes and values, plus expected revision
and exact pin. Every declared field key is required on every create/save; there
is no generic PATCH behavior in this slice. Native legacy-post omission behavior
remains confined to its existing compatibility editor.

| Submitted canonical value | Behavior |
| --- | --- |
| Missing declared key or unknown key | Reject; never keep an omitted old value implicitly |
| `null` | Unknown. Valid for every integer field and integer list property (assisted sheet slice 1.4, D-M: a blank or unreadable number is stored as unknown); for every other control only if the approved field is nullable |
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
the UI describes this as structural completion, not game legality. Since assisted
sheet slice 1.4 a blank or unreadable number is that value: it is stored as unknown
(`null`, with the scribble as a change note), never as a default. Enum and boolean
controls and the Name still need a value. Name is
trimmed and must contain 1–100 characters; general notes retain exact text.

Revisions are positive i64, transmitted as exact decimal strings in HTML/envelopes.
Creation starts at1; a successful save (including a valid no-op) increments once.
Expected revision and pin must match; overflow rejects without mutation. Identity,
values and revision update in one transaction from a coherent snapshot. Structural
invalidity gives422 with raw draft; stale writes give409 with the submitted token,
not a refreshed token. Failed writes preserve all persisted state. Definition
updates never reinterpret saved values; explicit cross-version migration is a
separately reviewed future operation.

### List fields (M4)

Plan: `docs/derived-formulas-m4-plan.md` (D1–D11). Every declared list property is
editable; a per-property `editable: false` flag and an item catalogue are a later milestone.

- **Values.** A list is an array of at most 256 items. Each item has a host-managed `id`
  plus its declared properties; a required property is always present, an optional one is
  present or absent. Item integers are exact i64 or `null` (unknown, slice 1.4); no other
  property holds `null`.
- **Host IDs (slot labels).** A new row's ID is `r<N>-<k>`: N is the revision being written
  and k = 1, 2, … numbers that list's newly allocated rows in row order. The sole writer
  enforces it for runtime `create` and `edit` (freezing keeps the data unchanged): every ID
  absent from the previous revision's list must have that form, checked after the revision
  conflict check, so a stale write answers 409 whatever its IDs. Clients never choose IDs, a
  removed ID never returns, and IDs label **row slots, not items**: overwriting every
  property of a row, or swapping hidden IDs between rows, is accepted. A move carries a
  row's ID with it. Future import and tool-loop writers must re-ID through the host.
- **Address.** A row property is addressed as `<list id>/<entry id>/<property id>` (a row as
  `<list id>/<entry id>`), scoped to one sheet and its immutable pin; form indices never
  appear in addresses.
- **Form encoding.** For list field index f (its position in the pinned definition), row r
  and property index p: hidden `id-f-r` (the saved ID, or empty for a new row), `row-f-r-p`
  (the value) and, for optional properties, `set-f-r-p` = `present` / `absent` (an explicit
  Set/Not set control). `op` comes only from the clicked submitter: `save`, `add-f`,
  `remove-f-r`, `up-f-r` or `down-f-r`. Indices use the canonical grammar
  `0|[1-9][0-9]{0,2}`; rows are contiguous from 0 and stored in submitted order. Sheet POSTs
  allow 65,536 form pairs (other forms 516), keys of at most 70 bytes and no duplicates.
- **Status precedence** (private runtime and private native saves; the legacy
  `/characters` editor is unchanged): 413 body limit (after a pre-body principal check, 403
  without reading the body); 400 parse; 403 CSRF, authorization or a read-only sheet; 400
  key admission, op grammar or range (including an edge move); a draft operation (400 for a
  non-integer revision) re-renders with 200, or 422 for Add at 256 rows; for Save a
  malformed revision is 422 and **a stale revision is 409 before any value validation**, so
  a stale draft with invalid values or unknown IDs answers 409 (a deliberate change for
  scalar runtime and private native sheets too); then 422 for values, an unknown or
  duplicate row ID at the current revision, or the pinned validation; then the writer (409
  for a concurrent writer; 422 for the ID invariant or an undecodable snapshot). Every
  re-render keeps the submitted hidden revision; nothing rebases a draft.
- **Draft operations.** Add, Remove, Move up and Move down re-render the unsaved draft with
  200 and a `role="status"` message ("Row added to Equipment. Not saved yet."). They never
  write, validate, evaluate or reveal saved IDs the draft lacked, and still need CSRF,
  authorization, edit rights and the runtime permit. A move swaps a row with its
  neighbour: values, Set/Not set states and the hidden ID together. Row 1's Move up and the
  last row's Move down are disabled; a submitted edge move is forged (400). Op buttons carry
  `formnovalidate` and post to the form's own action with no URL fragment (a fragment that
  names an element blocks `autofocus` in browsers); exactly one control gets `autofocus`, which
  also scrolls it into view
  (the new row's first control, the list's Add button after Remove, the moved row's move
  button at its new position). A draft from an older revision shows "The saved sheet
  changed (revision N → M); saving this draft will be refused with a conflict (409). Copy your changes, reload and
  re-apply them."
  The `role="status"` message is part of a freshly loaded page, which screen readers do not
  reliably announce; the focused control and its name carry the result. List sheets also render a
  Save button before the groups, so Enter saves.
- **Change summary** (history page of revision N of a runtime sheet with a supported pin):
  stored values only, names and addresses but never values. It lists name and notes
  changes, changed scalar fields, and per list the added, removed and edited rows (with the
  changed property labels, set ↔ not set included) and the moved rows: the rows outside one
  longest increasing subsequence of their previous positions, ties broken so that the rows
  that ended up earlier are reported. This is a deterministic minimal explanation of the net
  order change, not a record of clicks. A retained row of a list with two or more
  properties whose every property changed reads as a possibly reused slot. At most 200
  lines of at most 8 property labels, then one line with exact per-list totals. Revision
  N − 1 is read in the same transaction and validated like N; if it fails, or the sheet is
  native or its pin unsupported, the section reads "Change summary unavailable …". There is
  no revert or restore.
- **Limits and accepted risks.** Stored snapshots stay ≤ 1 MiB and must reopen under the
  sheet decoder's limits (32,768 nodes, 256 keys and elements per container); the writer
  refuses anything else with 422, atomically. The create and edit POSTs accept bodies up to
  5 MiB; a logged-in principal (editor or not) can make the server buffer up to 5 MiB
  without a valid CSRF token for at most the 5 s request timeout, holding a request permit
  (and, on create, the runtime permit). The whole body must arrive within that 5 s, so a
  5 MiB upload needs about 8.4 Mbit/s. A re-render echoes the draft only when its decoded values total ≤
  1 MiB (a larger draft could never be stored); otherwise it answers 422 with an alert:
  "Nothing was saved; copy large text before submitting." Each submitted string is echoed
  in full at most once (the submitted revision included); second visible contexts show a
  64-character excerpt.
- **Line breaks.** Two separate guarantees apply:
  - *Displayed and edited content is preserved.* Every textarea and every stored-text
    `<pre class="evidence-text">` in the application emits a sacrificial newline after its
    opening tag, because HTML parsing drops the first newline there. This covers private
    sheets, list cells, native fields, the legacy editor, character creation, the MÖRK BORG
    context page, source review, material review, review history and evidence search, so
    leading newlines show and resubmit intact.
  - *New writes of textarea-backed fields and campaign-transport values are stored with
    LF.* A native form submission serialises textarea line breaks as CR LF. These decoders
    normalise CR LF and lone CR to LF before validation, through the shared
    `form_text::newlines_to_lf`:
    - `campaigns::transport`, for every value of private runtime and native sheets and of the
      other campaign routes;
    - the textarea fields of the legacy `/characters` editor, character creation, the MÖRK
      BORG sheet and context forms, source review (unit and page) and material review
      (correction, rationale and completeness rationale).
    Single-line inputs of the legacy forms (for example `printed_label`, actor names, and
    credentials on the account forms) are not normalised, and must not be. The campaign
    transport normalises **every** value it decodes, because no campaign field's CR is
    meaningful. A future byte-exact field routed through it would need an exemption. The serde
    wrappers also apply if those structs are ever deserialised from a non-form source.
    Length limits therefore count what the user typed, matching each control's `maxlength`.
    For values already stored with LF, an unchanged browser Save is byte-identical. This
    applies to new writes only; existing data is not migrated. A private sheet or record
    saved before this change may hold CR LF, and its next save rewrites it to LF as a new
    revision, while history keeps the earlier revision unchanged.
- **Large bodies and permits.** The 5 MiB limit is enforced on the body itself, with or
  without a `Content-Length`. On **create** the single runtime permit is taken before the
  body is read, so buffering happens under it: at most one create POST (≤ 5 MiB) buffers at a
  time, and any other runtime request answers 503 meanwhile; this is an accepted risk,
  alongside the 5 s slow-upload hold. On **edit** (the full form and, since assisted sheet
  slice 1.5a, the list forms) the body is buffered and parsed outside the permit; then CSRF
  and the session are checked and the save waits up to 2 s for the permit (D-X3). So up to 8
  edit bodies (the auth layer's request slots) of 5 MiB each, about 40 MiB raw, can be
  buffered at once; with each one's decoded pairs (up to about 5 MiB more, 65,536 pairs at
  most) and a busy page of at most 5 MiB plus its chrome, the worst case is a few times that,
  still bounded, and a slow edit upload never holds the permit. Unit saves (`meta/…`,
  `fields/…`) take at most 256 KiB, buffered the same way.
- **Lost re-render.** If edit rights are lost between a rejected save and its re-render, the
  answer is a plain 403 and the draft is not re-rendered; nothing is saved.
- **Page-size bound.** It covers the editor chrome and controls, not the formula sections of
  a formula definition (see the M4 plan D7).

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
