# Independent review: actual local RAG benchmark

## Plan and execution scope

User requires local inference + RAG over extracted material for actual Mimik use;
API credentials are only for optional OCR review. User requested sustained work and
actual benchmark results while away. No paid/work API or private material access.

Independent plan reviewer `ses_ec1dd3debffewWNUmmwChpepYl` approved pinned download/
bootstrap, with pre-scoring conditions: complete stale-proposal metadata; frozen
semantic rubric; scoped deterministic FTS query/ranking/no gold leakage; deadlines,
persisted attempts and source/runtime verification. Incorporated before scored runs.

## Evaluator and source/package review

Fresh implemention/evaluator reviewer `ses_ec1d63984ffe7FZBFD6mf44GzP` identified:

- B1/high: no independent answer-schema validation. Permanent malformed-schema
  regression failed before fix; maintained jsonschema now validates every answer.
- B2/high: malformed envelopes could abort/lossily drop evidence. Permanent HTTP
  error retention regression failed before fix. Every started/finished attempt is
  flushed; raw bounded body/status/elapsed survives errors, missing choices/content,
  invalid JSON, truncation, timeout and next-attempt recovery are tested.
- B3/medium: total bound unenforced. Dispatch budget plus absolute Linux SIGALRM
  request deadline added with budget-crossing regression. Unstarted cases cannot
  silently count as successful.
- Mechanical score is not semantic approval: exact typed patches and citation IDs
  are checked, but every final answer needs separate assistant source inspection.
  Summaries refuse final_correct until explicit adjudication. Contradictory-prose
  false positives are retained as tests of that reporting gate.

Recheck GO before inference; all43 helper regressions currently pass in containers.
No new application provider protocol/runtime dependency was introduced.

Narrow Cargo demo-data inclusion approved because evaluator helper tests require
the five self-authored AGPL fixtures. Source checker now requires them without
broadening private corpus/image/credential admission. Unpacked source exposed an
existing normalized-Cargo README fixture gap: permanent explicit-readme regression
failed before fix; fixture now copies README, retaining all exclusions. Independent
fix GO; normal43 and unpacked-source43 tests, Ruff and verified source-package
compilation/member allowlist passed. No Rust application behavior changed and no
new shipping binary deployed.

## Diagnostic approvals

Same evaluator reviewer approved independent labelled top1, state-derived typed
schema and generic focused-prompt screens, then thinking+larger-output-budget and
plain-text screens with unchanged frozen baselines. Full actual schema/prompt/
parameters retained. No answer coercion, hidden repair or gold-derived constraints.
Separate diagnostics are fixture-informed, not external held-out evidence, and
combined mode/budget/prompt changes do not isolate single causal effects.

Reviewer also approved container-only aria2 resume of official8B artifact after
curl transfer was slow, with exact byte/hash checks before loading. aria2v1.36.0
installed only in downloader container; no host installs/credential use. Third
instruction-tuned4B candidate approved separately with pinned third-party quant
README and Apache-2.0 upstream provenance, no weight redistribution claim. Its
download/execution status remains pending at this checkpoint.

## Independent results review

Fresh results reviewer `ses_ec1a6e2f9ffef6OGiLzzl4il30` read all completed final outputs
and sources. Confirmed4B6/28 accepted,8B7/28 source-supported versus6 mechanical
and8 core readback/task outcomes, plain4B4/7 and8B5/7; all failures retained.
Focused4B notes gets manual task credit despite mechanical false negative from
benign extra maximum fields (2/7 reviewed versus1/7 mechanical).

An initial alleged typed-schema violation was not reproduced: independently
validating all7 answers against recorded schemas yielded zero errors. Reviewer
rechecked exact schema branches and **withdrew the finding**. Schema-valid no-ops
are semantic failures, not evidence of broken constrained decoding.

Verified risk findings retained: thinking4B actually follows injected notes in its
proposal;8B endorses stale edits; valid citation IDs accompany unsupported claims.
No inference result establishes legality, source approval or safe automatic writes.

Report accuracy GO for completed4B/8B conclusions: reviewer recomputed timings,
nearest-rankp95, throughput, memory units, annotation counts and28/28+7/7 paired
payload/context consistency. Adopted wording corrections: seconds-scale rather
than usability certification; pending-proposal metadata included in input description.
Raw data/annotations: ignored `target/local-rag-benchmark-20261015/`.

## Final candidate and cleanup

Initial900s acquisition ended INPR/exit7. Same evaluator reviewer independently
approved one final180s absolute wall-clock resume with a supervisor that stops the
container in cleanup; completed in104.78s including startup/refresh/provenance fetch
and exact artifact verification. No further extension used. Third candidate then
ran unchanged28structured+7plain jobs. All35 stopped normally without parse errors.

Results reviewer `ses_ec1a6e2f9ffef6OGiLzzl4il30` independently inspected all35 final
outputs and checked paired context/payload consistency:5/28 full strict/source-
supported structured successes and3/7 full source-supported prose successes (5/7
correct core prose outcomes, but two contain unsupported generalizations). Improved
stale recognition/arithmetic is acknowledged without masking address, incomplete
patch and citation/source-support failures. Acceptance patterns/rationales added
to local `adjudications.json`; no output repair/coercion or changed rubric.

Total135 completed inference attempts across three baselines, diagnostics, CPU
control and startup smoke. All five inference servers stopped; isolated Docker
network removed; host sampler stopped. Weights/licenses/raw synthetic results and
runtime arguments/logs remain local, outside archives. No API credentials, work
accounts, private manuals or user database accessed; installed application unchanged.

This evidence does not approve a production harness, real-manual RAG, model tool
dialogue or gameplay legality. Final three-candidate accuracy recheck GO: reviewer
reproduced all135 attempt/adjudication counts, final candidate timings and memory
units. Adopted nonblocking correction: final candidate's own idle GPU baseline is
0.92GiB (not the earlier0.89GiB), so its approximate increment is3.48GiB. No remaining
report blocker.
