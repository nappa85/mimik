# Review record: harness/RAG assessment and Rust code review

2026-10-15. Findings record for [harness assessment](../harness-assessment-2026-10-15.md).

## Scope and independence

- Rust code review: fresh-context, read-only general-purpose subagent over `src/`,
  `migrations/`, `tests/` and `Cargo.toml`. It made no edits, did not build/run,
  and read no `.env`, `data/` or credentials. Main session spot-checked C1 (`LIKE`
  lookup), C3 (`html2text` width 120) and C6 (single mutex, no `journal_mode`).
- Benchmark-design findings B1–B8: main-session analysis of
  `tools/local_rag_benchmark.py` and the 2026-10-15 reports. Not independently reviewed.
- External landscape/model candidates: web search results, not reproduced locally.
- Not performed: builds, tests, inference, downloads, browser checks or fixes.

## Findings and disposition

| ID | Severity | Disposition |
|---|---|---|
| B1–B3 (contract/derivation design) | High | Open; user decided 2026-10-15: tool loop + host-filled metadata, declarative formulas in Rust (JS rejected). Not implemented |
| B4 (sample size) | High | Open; ≥100 layered cases proposed |
| B5 (temperature) | Medium | Open |
| B6 (English stemmer on Italian) | Medium | Open; requires failing-first retrieval regression |
| B7 (toy corpus), B8 (no tool loop tested) | Medium | Open |
| C1 (no retrieval/model in app) | High | Open; next-step proposal |
| C2 (plumbing share) | High (scope) | User decided admin continues in parallel; accuracy bar gates model-dependent work only |
| C3 (wrapped text, ASCII-only `LIKE`) | Medium | Open; needs failing-first regression before fix |
| C4 (evidence lacks region ids) | Medium | Open |
| C5 (duplicate PDF blobs) | Medium | Open |
| C6 (single connection mutex) | Medium | Open |
| C7 (unauthenticated review actor) | Medium | Open; loopback-only mitigates, does not resolve |
| C8–C10 | Low | Open |
| C11 (hardcoded MÖRK BORG) | Medium | Known; already tracked as exploration fixture |
| C12 (no FTS5/LLM crate) | Low | Informational |
| P1–P2 (process weight) | Medium | P2: user approved lighter prototype tier, added to `AGENTS.md`. P1 open |

## Documentation change-set review

Fresh-context read-only reviewer over the new/edited docs and TODO. Verdict: **no
blocking findings**. It verified the cited code lines (C1, C3–C7, `pdf_hash`
non-unique, benchmark lines 38/391/210), report faithfulness (24/28 top-3; Stage
line) and all relative links/anchors in 12 files.

| ID | Severity | Finding | Disposition |
|---|---|---|---|
| M1 | Medium | C8 overstated: crop path uses a geometry-only projection | Fixed: mitigation stated |
| M2 | Medium | New model screening lacked the existing download precondition | Fixed in assessment and TODO |
| M3 | Medium | Contract-not-retrieval conclusion stated as fact | Fixed: framed as hypothesis (assessment, decision-models) |
| M4 | Medium | JavaScript derivation presented as already allowed | Fixed: open user decision (assessment, discovery) |
| L1 | Low | "Superseded" before user confirmation | Fixed: "alternative, pending user choice" |
| L2 | Low | `rig` 0.43 identity unverified (`rig-core`) | Fixed: marked unverified |
| L3 | Low | Review count 27→31, TODO size, stale example | Fixed |
| L4 | Low | Stage line "strict lint" inaccurate | Fixed to match report |
| L5 | Low | This record lacked the doc review | Fixed: this section |
| L6 | Low | Admin pause, ingestion-vision confirmation, Qwen3.5 Vulkan check missing from TODO | Fixed |
| L7 | Low | New handoff ambiguous next to older paragraph | Fixed: separator |

B1–B8 were checked against the benchmark code by this reviewer only for cited lines; their analytic weight remains main-session judgement.

No code finding (B/C/P) is fixed by this record. Review findings are not proof of correctness.
