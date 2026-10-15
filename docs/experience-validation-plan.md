# Experience-first experiment: model–sheet harness

Status: product direction independently reviewed; protocol/provider/template
format and bounded technical implementation plan remain undecided. No live model
integration or usage trial has been implemented/tested. This revision supersedes
the earlier vision-model wiring assumption, explicitly rejected by the user.

## Confirmed direction

Mimik is a game-agnostic harness enabling a model to dialogue with a character
sheet. The Owner supplies the sheet template and relevant game material. The model
receives sheet context, can ask questions and consult rules, and semantically
reviews/proposes edits. No compiled game-specific logic, fields or templates are
required for a new game. Existing native MÖRK BORG functionality is an evaluation
fixture, not the intended expansion mechanism.

The application requires a **local text model with RAG over extracted material**;
local operation is a core viability condition, not a fallback to hosted dialogue.
The API key is reserved for optional OCR/extraction double-checking, not ordinary
Mimik use. No hosted/work account is used in this experiment.

The first experiment uses a **text-only model**, not vision or image-based field
discovery. The user permits JavaScript for the web interaction and does not want
another scripting language. No new rules DSL, Python-based application workflow
or model-generated executable code is required. Existing optional extraction
prototypes are separate tooling, not the model–sheet interaction architecture.

## Three separate artifacts

1. **Owner-uploaded template:** a web-native presentation/reference with explicit
   field bindings. HTML/CSS with stable field annotations is a proposal, not yet a
   confirmed upload format. Do not substitute PDF/image uploads as the default.
2. **Adopted field/presentation definition:** bounded data declaring stable IDs,
   types, grouping, repeated structures and template version. A model can propose
   mappings from textual structure when useful; it need not visually infer them.
3. **Canonical character state:** actual current typed values and stable repeated-
   entry identities, independent of layout and pinned to an adopted definition.

Build model context from the field catalog plus canonical state. Maintained HTML
parsing/DOM inspection may supply labels/groups/bindings, but raw HTML serialization
is not authoritative current state: live form properties differ from defaults.
Distinguish missing from empty values. Include template/sheet revisions and the
exact proposed diff. New template adoption never silently migrates characters;
field identities stay stable when meaning is unchanged.

## Harness responsibilities and conceptual operations

The conversation is more than a one-shot template generator or validator. Expose
a small, bounded domain interface; names/schemas/protocol below are illustrative:

The user explicitly allows changing the harness to improve accuracy and discarding
unsuccessful experiments. "Sheet as context" does not prescribe raw HTML or a
single prompt. Proposed revisioned state/situation/evidence context, bounded
generic arithmetic and staged evaluations are in
[accuracy experiments](accuracy-experiments.md). They extend this conceptual
interface; they are not implemented operations or game-specific rules code.

| Operation | Purpose |
|---|---|
| `read_template` | Field catalog, groups/bindings and immutable template identity. |
| `read_sheet` | Authorized current canonical values, entry IDs and sheet revision. |
| `lookup_rules` | Authorized exact-edition text evidence, citations and review/coverage status. |
| `ask_user` | Ask for missing facts instead of inventing them. |
| `propose_sheet_patch` | Typed proposed changes tied to base revision/template pin, with separate findings. |

The harness checks the real caller's permission on every operation, filters private
context before disclosure and enforces input/output shapes, revisions and resource
limits. Model-selected IDs never confer access. Tools do not expose arbitrary JS,
`eval`, SQL, filesystem paths or autonomous writes. Questions do not spend resources
or implicitly mutate sheets. Findings remain associated with the proposal/evidence,
separate from committed revisions; they never certify legality.

Generic trusted application JavaScript supplies browser interaction, rendering and
field bindings. Rust owns structural validation, authorization, version checks,
storage/history, context/tool orchestration and explicit application of changes.
The owner template is presentation/data by default: do not execute its scripts,
event handlers, executable URLs, embedded documents or external-resource behavior
with session authority. Select maintained sanitization/rendering controls in the
technical plan. Uploading a template does not authorize active code. This uses
JavaScript and ordinary structured data, not a second custom scripting language.

## Proposed real usage loop

1. Owner supplies a web sheet template and exact-edition manual evidence. Inspect
   actual fields/repeated structures; bind them to canonical state. Do not assume
   the current flat renderer fits, omit fields, flatten required structured entries
   into notes or pre-author a developer fixture to manufacture success.
2. Preview the live sheet and correct bindings, optionally through text-model
   dialogue. Keep recognizable field grouping and long-text/mobile reflow.
3. Player creates/opens a character and talks to the model with that sheet as
   structured context. The model can read authorized state, retrieve rules and ask
   for missing facts, then explain or propose edits.
4. A user edit also enters this loop: capture a draft/diff, review semantically
   through the model, show findings and optional alternatives. Run on an explicit
   action rather than on each keystroke; do not rely on an unbounded chat transcript.
5. Display the exact final diff for explicit user application. Recheck authorization,
   base sheet revision and template pin at commit. Stale proposals require renewed
   review/confirmation; never apply to a silently changed sheet. A suggested change
   is a new displayed proposal, not an automatic calculation or state change.

Proposed first-trial save policy: completed semantic review before commit, not
model agreement as a legality gate. An authorized user can explicitly accept the
displayed edit despite advisory uncertainty/conflicts. Provider failure retains
a recoverable draft and delays commit, which must be measured as a usability cost.
Confirm whether an explicitly unreviewed-save path is desired before implementation;
it must never be labelled validated. Types, permission/revision/storage limits and
supported source-backed deterministic checks remain enforced by generic code.

## Viability experiment and execution prerequisites

User testing prerequisite: no usable test materials are currently available/loaded.
The first harness milestone must include a small self-authored synthetic demo pack:
web sheet template, matching text rules, example character and concrete dialogue/
edit tasks, with a working load/open path and concise instructions. This is explicit
demo data, not compiled game logic or borrowed manual content. Do not require the
user to find/author a manual/template before trying the milestone. Synthetic runs
establish interaction mechanics only; real-model and real-game viability remain
separate evidence. No fixture/import path is claimed implemented by this note.

Use one real owner-selected template and an explicitly enabled, version-pinned
source set (possibly player/GM/bestiary/expansion books), not a developer-written schema.
Agree a small task/effort/latency/cost scorecard before the trial: character creation,
resource edit, long text, repeated equipment entry, source question, valid/conflicting/
ambiguous edit, missing evidence, stale response and model failure. Count Owner
binding/evidence-review effort and all retries/failures. Check citation support
against original source evidence and compare table-task completion with the user's
existing sheet. Success is scoped to this package/model/workflow, not all games.

Mocks can test interaction mechanics, not viability. Exercise real text-model
context/tool dialogue and semantic edit review through the intended application
path. Use local execution and evaluate a maintained Rust provider
abstraction's current license, maintenance, structured-output/tool support,
context/usage reporting, portability and dependency/runtime cost. No image-input
capability is required. No host tool/model installation; runtimes remain containerized.

Ordinary sheet/rule context remains local with the inference runtime; no silent
hosted fallback or background upload. Keep request/time/retry/concurrency/context
bounds even without per-call API costs. Any optional hosted OCR-review task is a
separate, explicitly selected workflow with limited source payload and credential
handling; it does not authorize uploading character notes/campaign data. Never
read back credentials. Original documents stay local; human source comparison
does not imply a vision model in the application. Unsupported manual text/glyph
extraction is an evidence gap, not permission to add vision to this usage trial.

Next: confirm web template contract, choose a benchmarked local model and actual assets
and save policy/scorecard, then independently review the bounded technical plan.
Further administration remains paused. No new game-specific application code is
the fallback for an unsupported template or rule.
