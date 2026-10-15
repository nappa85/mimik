# Accuracy-first harness experiments

2026-10-15. User authorizes disposable harness experiments: sheet-as-context defines
the product, not a fixed prompt or implementation. Accuracy is the viability
priority. This record separates the executed artifact diagnostic from proposed
experiments. No new inference, extraction, API call or app integration was performed.

> **2026-10-15 assessment:** benchmark-design findings B1–B8 and the code findings
> C1–C12 relevant to these experiments (English stemmer on Italian text, wrapped
> stored text, evidence missing region ids, no Rust retrieval) are in
> [harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md).

## Corpus choice

Materials may span player/GM/bestiary/expansion books. Model them as an explicit
version-pinned enabled source set over separate book trees, with cross-book
references and explicit conflict/supersession policy; never merge trees or assume
an expansion automatically overrides core text. The optional supplied-structure
[prototype and partial screen](local-hierarchy-screen-2026-10-15.md) now exercise
this boundary. They do not recover actual PDF hierarchy or integrate the app.

Keep MÖRK BORG as the first playable trial; add a bounded Pathfinder 2e stress test,
not all seven books as the first support scope. Corpus size, mechanical complexity,
extraction difficulty and runtime capacity are separate variables. MÖRK BORG has
fewer rules but its stylized layout need not be easier to extract.

Use the Italian legacy/pre-remaster core source identified in [sources.md](sources.md),
not remembered remaster rules. Exact errata coverage remains unknown. Other
Pathfinder books need their own identification before inclusion. Keep the Italian
2025 MÖRK BORG source distinct from English editions; do not mix conflicting sources.

Proposed real-game evaluation: 12 unique MÖRK BORG cases and 24 Pathfinder cases,
with separate development cases (6 and 12 respectively). Cover resource changes,
action costs, prerequisites, exceptions/negations, a table lookup, missing situation
facts and unavailable evidence. Include irrelevant context, conflicting source
versions and stale-state controls. Pathfinder should exercise linked rules, not
complete character creation or every spell/feat. Visually review an initial batch
of at most 30 source pages per game; if required dependencies exceed it, record the
gap and explicitly rescope or expand review. Never silently omit dependencies to
meet the cap. These are proposed bounds, not existing reviewed cases or statistical
proof of general reliability.

The loadable synthetic demo remains prerequisite to testing the actual UI/tool
loop. Its repeatedly inspected seven/18-case fixtures are development/regression
material, not fresh held-out evidence.

## Executed legacy-artifact diagnostic

Read-only inspection of `~/temp/pathfinder/README.md` and
`rag/{extract,index,retriever}.py`; no startup scripts/imported legacy modules,
configuration secrets, databases, unsafe serialization, downloads or APIs. The
legacy graph was listed but not queried: `graphify` was unavailable on PATH.
No install/rebuild was attempted; direct inspection was used.

Python stdlib parsing used explicit regular files under the legacy root, rejecting
symlink components. Bounds: seven Markdown files, each <=16 MiB and total <40 MiB;
two JSONL files, each <=64 MiB, <=50,000 records, <=128 KiB per line. Text sampling
read at most 64 KiB from each of two named Markdown files. Outputs contained counts,
field names/book identifiers, not passages. Legacy files were unchanged.

| Observation | Result |
|---|---|
| Markdown inventory | 7 files, 10,340,102 bytes total (sizes only) |
| Chunk JSONL | 12,956 records, 10,986,792 bytes |
| Index metadata JSONL | Same count/size/field counts; content equality not tested |
| Fields present | Only `id`, `source`, `section`, `text`, in every record of both files |
| Source counts in each JSONL | Core 4,796; player's guide 2,080; GM guide 1,535; bestiaries 1,738/1,282/1,509; reference sheet 16 |
| Duplicate IDs within each file | 0 |
| Oversized records | 39 above 1,500 characters, maximum 68,888, in each file |
| Reference-sheet lines 1–100 | 100 lines; 80 table lines; 69 empty cells |
| Core-book lines 180–240 | 61 lines; 8 heading lines |
| Replacement/private-use glyphs in these samples | 0 / 0; does not establish glyph preservation |

Samples deliberately revisit earlier action-table/sidebar problem areas; they are
not random. Empty cells can be legitimate. No new visual PDF comparison or corpus
error-rate estimate was made. Action-cost glyph loss is **historically visually
confirmed**, not reconfirmed by these counts; see
[sources.md](sources.md#observed-quality-issues). Exception fidelity was not assessed.

Code observations:

- Existing retrieval combines FAISS dense search and BM25 through reciprocal rank
  fusion. Adding vectors alone cannot repair missing evidence.
- Marker output saves only `rendered.markdown`. Chunks discard page/region, source
  hash, extraction identity and review status; they cannot be adopted directly as
  reviewed Mimik evidence.
- Header splitting recognizes levels 1–4 but retains only levels 1–2 in breadcrumbs
  and omits heading lines from body text: a risk of lost rule-entry identity. Its
  effect on answers was not measured.
- Local sentence-transformer code sets `max_seq_length=512`; oversized chunks
  create truncation risk. No tokenizer/stored vector was inspected; actual lost
  coverage and active index backend remain unverified.
- Source filtering happens after global retrieval: test candidate starvation in
  future source-specific evaluation. Permission/source-set eligibility must filter
  evidence before disclosure to the model. No live retrieval recall was measured.

Findings block **direct reuse as trusted evidence**, not this documentation pass.
Legacy chunk/header behavior remains unfixed. Any Mimik replacement must first
retain failing-first regressions for oversized entries, heading identity and
symbol/table preservation. No legacy fix or regression test is claimed here.

## Evidence format: structured records, derived reading/search views

### Book-wide hierarchy is required, not a chunking convenience

User clarification: hierarchy can persist across hundreds of pages. Losing it can
make otherwise accurate text misleading. Preserve a book-level structural tree
before creating retrieval chunks. Each chunk references a stable node and its
ancestor path; neither a page break nor an extraction batch resets chapter scope.
Do not reduce this requirement to a Markdown heading prefix guessed per chunk.

Target node data: book/revision identity, stable node ID, parent ID, title/type,
ordered source block/region references, start/end positions (including within-page
boundaries), structural evidence and uncertainty. Unresolved parents/incomplete spans
must remain explicit during assembly rather than forcing a fabricated hierarchy.
Text and structure have separate findings/coverage; correct words under the wrong
chapter are an extraction failure. Preserve identity/history across repairs and
invalidate affected derived chunks/context/review results.

For example, a Spells chapter may contain a rules section, class spell lists and
spell descriptions as sibling sections. A class list links to a description; this
does not necessarily make the description its child or make the spell exclusive
to that class. Likewise, class/subclass structure must not leak one subclass's
restrictions into siblings. **Containment, scoped rule applicability and references
are different relationships.** Ancestors identify scope but do not automatically
make every parent statement an applicable rule for every descendant.

Proposed extraction/review sequence:

1. Build a tentative global outline from source bookmarks/contents, layout/typography
   and heading evidence. Bookmarks/contents may be incomplete or wrong; retain
   disagreements rather than treating them as unquestioned authority.
2. Extract/review bounded page batches carrying the open ancestor stack, stable IDs
   and source-boundary evidence. Keep boundary overlap and reconcile it without
   duplicate entries. Preserve running headers as layout, not new sections.
3. Reconcile the book-level tree across batches: chapter continuation, heading-level
   changes, repeated titles, attached tables/footnotes and explicit references.
   A multi-pass frontier model can propose repairs against the original images;
   its summary is not a replacement for the evidence or the full structural ledger.
4. Validate parent existence/acyclicity, ordering/source-span consistency, no silent
   missing blocks, duplicate handling and explicit unresolved assignments. Structural
   validity alone does not establish semantically correct parents/scope.
5. Index leaf passages with ancestor identities and source-backed scope context;
   retrieve relevant ancestor rules and references within budget, not an entire
   hundred-page chapter whenever one spell matches.

Before implementation, evaluate maintained extractor/document-tree capabilities
against these requirements and the existing source store; only demonstrated gaps
justify custom stitching. Required permanent failing-first regression fixtures:
100+ page chapter continuation, batch-boundary restart, same-named entries under
different parents, class/subclass scope, sibling spell lists/descriptions, detached
tables/footnotes, bad bookmarks, orphan/cyclic parents, invalid spans and repair
invalidation. All are planned tests, not implemented checks in this revision.

Markdown is useful for reading/model input, but insufficient as the sole canonical
extraction artifact. A vector database is an index, not an extraction format or
authority: it embeds whatever text we supply, including errors.

Extend existing Rust source import/review/correction history rather than create a
parallel evidence database. Preserve original PDFs and immutable extraction bundles
outside releases. Use structured JSON blocks/regions for interchange and existing
SQLite storage for admitted, versioned review records. Target capabilities:

- Source hash, game/edition/language, extraction tool/settings/revision.
- PDF page index, separately verified printed label, bounding regions and rendered
  page/crop references; raw output separate from corrected text.
- Section hierarchy, reading order, block type, table headers/cells/spans/footnotes,
  and rule-bearing symbol regions with reviewed semantic annotations.
- Stable unit IDs, review state/coverage and source-backed corrections.
- Surrounding/referenced-rule relationships where demonstrable. Distinguish inferred
  from source-explicit links; neither proves completeness.

These are targets, not a claim the current schema covers every table/relationship
semantic. Inventory support and demonstrate gaps before schema changes. This is
evidence structure, not an executable rules DSL.

Generate Markdown/plain text and lexical/vector indexes from eligible revisions.
Bind indexes to evidence IDs/revisions, model/tokenizer and chunking settings;
invalidate affected entries after correction. Embeddings never override review
state or caller/source-set eligibility. Check actual embedded input limits including
headings; split/reject overflow rather than silently truncate exceptions/tables.
Return relevant parent/neighbor evidence within budget; retrieved coverage is not
whole-book coverage.

### Optional frontier-model extraction review

Retain the user's original proposal: compare selected original page images with
structured extraction, especially symbols, numbers, tables, negations and reading
order. Proofreading Markdown without the original cannot reliably recover omissions.
The existing optional review prototype already supplies image/block comparison and
suggestion boundaries; extend only demonstrated gaps, not a second pipeline.

Evaluate a small human-checked batch containing known defects and intact controls:
missed defects, false alarms, correction correctness, cost and review effort. Freeze
independent human annotations before reading model output. Humans resolve findings
against images; model confidence/no-findings never auto-approves a page. Ordinary
dialogue stays local/text-only. A live review needs an explicitly selected endpoint/
model and bounded payload; this pass reads no credentials and uploads nothing.
See [extraction-prototype.md](extraction-prototype.md).

### Scale review through automation, not compulsory full human rereading

The goal is frontier-model comparison across the manual in bounded batches,
including original images, extraction blocks and global hierarchy, followed by
human attention to disagreements, unresolved structure, high-impact rule values,
and a stratified sample of **unflagged** material. Sampling only flagged pages
cannot estimate missed defects. Run a cross-batch structural audit in addition
to page-local text comparison; both must expose skipped/failed pages and retries.

Measure defect recall/false alarms by defect type and chapter/layout stratum,
hierarchy-parent/path accuracy and human minutes per page/manual. Use independently
source-checked evaluation annotations; agreement between models is not ground
truth. Escalate/review more of strata where missed errors occur. A cheaper first
pass plus stronger disagreement review is a candidate to compare, not assumed
equivalent to a frontier pass everywhere.

Predeclare sampling units, per-stratum counts and aggregation weights in the
execution plan. Targeted high-impact checks and random unflagged samples have
separate denominators; oversampling hard layouts must not distort a manual-wide
estimate. Report considered/admitted/executed model counts separately too, even
if fewer than six candidates can run faithfully on the target hardware.

Track separately: extraction coverage, automated checked coverage, human-reviewed
coverage, unresolved findings and structural completeness. Automated no-findings
must not relabel unsampled pages as human-reviewed/verified. Current source policy
still restricts verified advice to source-reviewed evidence. **Whole-book scalable
admission remains an explicit policy/implementation gap**: evaluate model-checked
material in a labelled research lane without silently admitting it to verified
lookup. Any later operational policy needs its own reviewed design. Routine
Owner preparation must not presuppose a complete human reread of every book.

## Sheet context is an assembled, revisioned working set

The sheet is the anchor, not the entirety of the situation or raw HTML pasted into
a prompt. Proposed generic context assembly:

1. Authorized canonical snapshot: actual typed values, stable field/entry IDs,
   labels/groups/types, missing versus empty, template and sheet revisions.
2. User intent or before/after diff, plus explicit situation facts. Keep temporary
   facts/assumptions separate from persisted character state.
3. Discoverable field catalog and focused projection, with bounded `read_sheet`
   expansion; do not force the model to guess omitted values.
4. Exact-edition passages and surrounding exceptions, evidence IDs/revisions/review
   status, and explicit gaps/conflicts.
5. Bounded operation results, pending questions/proposal, not an ever-growing chat
   transcript. Source text/character notes cannot grant tool authority.

Compare full-state with focused-plus-expandable state on identical cases. Selection
can hide dependencies; log included/omitted context and budget exhaustion. Recheck
live revisions before applying displayed proposals. Cache immutable rule context
for speed only, with version/source-set/permission scoping: caching does not make
wrong or stale context correct.

Generic candidates, not implemented here:

- Bounded query/unit/section/reference lookup, returning main rules and relevant
  linked context/exceptions within hard call/token budgets.
- Maintained-library-backed arithmetic with enumerated operations (add/subtract/
  multiply/divide/min/max/comparison), bounded numeric operands and explicit rounding
  where needed; no `eval`, arbitrary expressions or code execution. Carry operand
  provenance/units and proposed effect. Arithmetic does not certify the selected
  rule/formula/units. Zero division and overflow are errors, not invented results.
- Separate evidence sufficiency, missing facts, effects/calculation, typed patch
  construction and advisory source checking. Bound every stage; another model is
  a candidate checker, not independent ground truth.
- Exact before/after patches against stable IDs, structural/permission/revision
  checks and explicit user confirmation.

Reuse existing libraries and source/patch boundaries. Before implementation review
license, maintenance, API coverage, portability and runtime cost. No new package or
model is selected/installed by this record.

## Experiments and promotion gates

| Stage | Controlled comparison | Evidence required before promotion |
|---|---|---|
| A. Source preparation | Existing extraction versus structured preservation + optional image review | Visual checks of all used evidence; known symbol/table/negation defects become failing-first permanent regressions; report unresolved coverage |
| B. Retrieval | SQLite FTS baseline versus dense/hybrid, then optional reranker | Frozen human-labelled supporting units/dependencies; all-required-evidence recall at fixed context budget, source isolation, latency/memory |
| C. Context/tools | Existing single-call approach versus projection/expansion + bounded tools | Paired held-out cases; arithmetic, exact complete patch, grounding, abstention, total calls/time including failures |
| D. Model type | Broad, fast capability-specific screen of 6–10 diverse candidates, then top 2–3 on fresh held-out real-game slices | Same logical evidence/budgets; native decision heads/samplers preserved; pinned artifacts/license/runtime fit, all failed/blocked candidates and local hardware measurements |
| E. User trial | Loadable synthetic demo, then MÖRK BORG slice, then Pathfinder slice | Actual app dialogue/edit path, citations/diffs/questions, user-applied changes, preparation burden and task completion |

For B/C/D, use an **oracle-evidence control** with human-selected supporting passages
to distinguish retrieval misses from reasoning failures. It exposes no expected
answers/gold labels. Insufficient-evidence controls intentionally remain insufficient.
Report extraction, retrieval, context-selection and reasoning failures separately.

Stage C initially measures the bundle, not individual component causality. Before
attributing a gain to projection, expansion, arithmetic or checking, run paired
component ablations with the same model, evidence and budgets held fixed.

For Italian manuals, evaluate a language-appropriate lexical tokenizer; synthetic
English Porter settings are not automatically suitable. Start with existing SQLite
FTS. A dedicated vector server is not justified by 12,956 chunks alone. Compare
maintained embedded Rust-compatible vector options versus a service only after
dense/hybrid gains warrant integration; include invalidation, memory/disk,
portability, licensing and startup cost. FAISS is a prototype reference, not a
shipping dependency selection.

Freeze development/held-out splits and expected source/state outcomes before tuning.
Report unique cases separately from repeated observations. Track false-supported
decisions, precision **and coverage/recall**, missing-fact/rule discrimination, exact
patches and citation support. Refusing everything is not success. Confidence
thresholds require held-out calibration, not fitting to this tiny diagnostic.
Report failures/retries, p50/p95 time and peak memory.

Provisional engineering target: no observed unsupported "supported" outcomes or
wrong committed values on a promoted slice's held-out cases, with successful
assistance on >=80% of answerable cases. Report counts/denominators and human
adjudication. This is a research target, not a user-agreed SLA, gate waiver or claim
of general zero error. Distinguish ordinary user edits from model proposals. Retain
failures as permanent regressions; use fresh held-out cases after tuning on failures.
Revert experiments that fail to improve usefulness/reliability at acceptable cost.

The previous one-challenger restriction is superseded by the user's request for
fast, broad screening. See [model shortlist and tournament](decision-models.md#fast-broad-screening-2026-10-15).
Bound setup/inference effort rather than arbitrarily narrowing model diversity.
The supplied-structure hierarchy prototype and partial existing-model screen are
now recorded above. The next execution plan must finish bounded setup enforcement
and resume new trained candidates, then connect the prototype to actual extraction.
The loadable local demo/tool loop remains prerequisite to UI viability, not offline
screening. Neither requires Owner-authored whole-game rules.
