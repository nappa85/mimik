# Independent review: accuracy roadmap and legacy Pathfinder diagnostic

## Plan review

Fresh read-only reviewer `ses_ec067d832ffeEHa4s0hrFPnII5`: **GO** for bounded
inspection/stdlib diagnostics and documentation, before execution. Scope reviewed:
AGENTS/TODO, source-quality workflow and roadmap; no private corpus inspected by
the plan reviewer. Main retains implementation ownership.

Required amendments and disposition:

- Medium, legacy configs may contain credentials: allowlisted README and three
  implementation modules only; no config/env/log/credential inspection.
- Medium, unbounded/biased sampling: file/byte/record caps and two purposive windows;
  denominators/bias recorded, no corpus error rate.
- Medium, traversal/execution: reject symlink components, explicit regular files,
  no imported legacy modules, unsafe formats, startup hooks or write-capable DBs.
- Medium, metadata is not fidelity: historical visual glyph-loss evidence separate;
  no new PDF review/approval or exception-fidelity claim.
- Medium, size/complexity/layout/runtime conflation: separate axes and exact edition
  prerequisites; MÖRK BORG lighter mechanically, not assumed easier OCR.
- Low, duplicated infrastructure: extend existing Rust source store/history;
  structured blocks for review, Markdown/indexes derived; no new DB chosen.

Required stagewise tests, held-out cases, unsupported-answer and coverage measures,
calculator limits and preserved loadable-demo priority are future experiments,
not passed checks.

## Executed scope and findings

See [diagnostic](../accuracy-experiments.md#executed-legacy-artifact-diagnostic).
Stdlib read-only counts: seven Markdown sizes; complete bounded JSONL field/count
scan of chunks and index metadata; two <=64 KiB text prefixes sampled without
printing passages. Legacy files unchanged. No graph query (`graphify` unavailable),
PDF visual comparison, embeddings/search, inference, hosted call or app test.

- Medium: all 12,956 records in each JSONL lack page/region/source-hash/review fields.
  Block direct trusted-evidence adoption; use existing reviewed import.
- Medium: 39 records exceed intended 1,500 characters; maximum 68,888. Local
  embedding code caps inputs at 512 tokens. Actual vector coverage/backend unknown.
  Legacy defect remains unfixed; token-aware bounds and permanent failing-first
  regressions prerequisite to Mimik replacement.
- Medium: splitter omits heading levels 3/4 from breadcrumbs/body; post-retrieval
  source filtering risks candidate starvation. No answer/recall claim or legacy
  fix; heading/source-isolation tests required for replacement.
- Informational: old retrieval already uses FAISS+BM25; vectors do not repair
  extraction. No vector dependency/service selected.

## Change/results review and verification

Fresh reviewer `ses_ec061ce0effeVfhRnhd2f2M7q2`: **GO**, no blocking findings.
Read the documentation and allowlisted legacy README/three modules; independently
recomputed both JSONL inventories, seven Markdown sizes and both bounded sample
windows using stdlib/no-symlink regular-file access. All reported counts reproduced;
largest observed JSONL line 69,056 bytes. Did not test file-content equality or
actual vectors. Verified code-based provenance/header/token-limit/filter observations
and appropriately limited fidelity claims. No private passages printed.

Low, nonblocking: Stage C's bundled comparison cannot attribute gains to individual
components. Disposition: require fixed-model/evidence/budget paired ablations before
component-causality claims; added explicitly. No experiment was run or result changed.

Documentation checks passed: six changed documents checked for trailing whitespace,
final newlines and conflict markers; 11 local links checked including anchors.
`git diff --check` passed but these files are untracked, so the explicit document
checks supply coverage for them. No application code changed; no Rust/build/browser/
model checks implied. Real evidence review, live optional API review, retrieval/
model evaluation and app integration remain pending.
