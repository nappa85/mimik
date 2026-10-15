# Multi-book hierarchy prototype and bounded local screen

2026-10-15. Optional Python research tooling only; no shipping application/schema
change. User explicitly requested execution while away and clarified that a game
uses multiple manuals. This is not a completed broad model tournament.

## Delivered prototype

`tools/evidence_forest.py` validates and assembles supplied structural records. It
does **not extract or discover hierarchy from PDFs**. Existing jsonschema4.25.1
handles shapes; Mimik-specific code handles book-local ancestry/source pins,
references, access and stale derived context. No graph database/tree dependency.

- Source-set forest over separate core/GM/bestiary/expansion book trees, with exact
  enabled revision pins and an explicit same-edition policy. Enabling expansions
  does not automatically override a core rule. Nonempty precedence policy remains
  unsupported rather than guessed.
- Stable IDs independent of title/path, source positions across100+pages, explicit
  root/known/unresolved parent states, source-span containment and cycle checks.
- Batch continuation with identical-overlap deduplication and conflicting-overlap
  rejection. A class spell list references descriptions rather than becoming their
  structural parent; ancestry itself does not imply rule applicability.
- Cross-book references are separate from parents. Disabled, inaccessible,
  revision-mismatched or missing targets do not disclose their content.
- Repairs preserve original snapshots and stable identities. Conservative whole-
  forest fingerprints invalidate all derived artifacts, including ancestor and
  cross-book reference consumers. Cached reads recheck current access.

17 permanent hierarchy tests cover these properties. Functional flat baseline
produced19semantic failures and one reference-field error over the original13tests;
the implemented validator passed those tests. Review then found public mutable
indexes bypassing invalidation: a permanent regression failed before detached
inspection views fixed it. Additional subclass/reference-consumer tests are retained.

This establishes supplied-structure handling, not correct recovery of headings,
tables, footnotes or rule scope from real manuals. No real source was approved.
There is no Rust/UI adapter or persisted application evidence-store integration.

## Model screening actually executed

Case set:18previously inspected development cases plus6new original synthetic
scope probes, two option orders, one repeat. **24unique cases/48observations per
model**, not48independent cases. Existing cases use real SQLite FTS retrieval over
the synthetic demo; the six probes supply oracle hierarchy/source-scope facts.
They test use of supplied structure, not extraction/hierarchy discovery.

Native maintained AnyJev raw readout on three existing pinned Q4_K_M GGUFs using
Vulkan llama-cpp-python0.3.36; no L0 priors/rotations/adaptive reasoning. Same
question/options and exact logical state across models. Labels stay evaluator-only.

| Reference | Completed / scheduled | Correct / scheduled | New scope correct / scheduled | False-supported / supported predictions | Native median / p95 |
|---|---:|---:|---:|---:|---:|
| Qwen3-4B-Instruct-2507 |48/48|38/48|9/12|6/18|0.460 /0.550s|
| Qwen3-8B |48/48|26/48|7/12|1/11|0.798 /0.966s|
| Qwen3-4B |47/48|28/48|9/12 (11completed)|8/16|0.462 /0.549s on completed calls|
| Trained Kev0.8 |0/48|Not executed|Not executed|Not applicable|Not executed|

Qwen4's final logical decision hit the absolute deadline; failed verification and
unknown physical count are preserved. Do not compare its completed-only timing as
if the full run passed. 143observed native evaluations plus one unknown count,
within144conservative reservations. Kev's48outcomes are explicitly derived `not_run`
records, not inference. Historical Kev results are separate and were not pooled.

Interpretation: the existing4B-Instruct reference remains stronger on this small
development screen by overall count, while8B has fewer false-supported predictions.
That is a coverage/precision trade-off, not an established general winner. All
three still miss the wrong-focus-delta case in both orders. Instruct4 misses the
long-chapter ancestry check in both orders and one sibling-list check; these are
end-to-end decision failures on this supplied representation, not proof that it
cannot understand hierarchy in another context design. No model selected for
deployment; no real RPG/source-fidelity accuracy gain demonstrated.

## Admission/setup outcomes and limits

The ten-candidate pool remains a shortlist. Six was an execution attempt target,
not a promised six completions; the attempt target was unmet.

- Three existing GGUF references reached native inference: two complete runs and
  one deadline-limited run. Fourth reference Kev0.8 was not started at cutoff.
- Mapika2B pinned `ff99089b6810691bd79134ec6febb5d9afa6e619`: setup attempted,
  transfer incomplete; no verified snapshot and no model load/inference.
- Tacit4B pin `af3fa1dc3cd98f937ab2ccedd4df8613fce5f1e5`: queued behind Mapika,
  transfer not started. Its adapter remains unverified by actual execution.
- Clef/Flash, DJev, Mapika4B, Kev4B and Phi: documentation admission/research only;
  no runtime attempt here. Clef native head and DJev sampler must be preserved.
  Their AMD/CPU/quantization fit is not established by publisher CUDA benchmarks.

Observed transfers ~3MiB/s did not improve enough with maintained aria2 segmented
transport. Original HF container ran13:26:46.942–13:35:17.505UTC, then aria2
13:36:16.880–13:42:14.141UTC. **867.8sactive transfer;927.2selapsed**, exceeding the
per-candidate900selapsed cap by27.2s. Record this scheduling defect explicitly;
the1200saggregate setup bound was not exceeded. Neither partial file was loaded.
The shared elapsed-deadline arithmetic now has a permanent regression and fix;
the complete future live controller must still enforce that deadline across
metadata/snapshot operations and transport changes, not just per-command transfers.

Inference ran sequentially in offline containers with no published ports, user1000,
sixCPU limit,24GiBmemory/swap limit, hostRAM/VRAM admission and read-only model/tools
mounts. Outer/worker absolute cutoff13:50:46UTC stopped the incomplete run; Qwen4
container exited1 at13:50:46.213UTC after native worker timeout, not a successful
completion. Stopping/logging has observable subsecond scheduling overhead.

Container-only aria2 package1.37.0+debian-3 (GPL-2.0-or-later), image
`mimik-model-screen:20261015`, manifest
`sha256:550abfbd87bbb52cb34160d30ad990c4026012828026b3c97704e040b3c5f295`.
Empty allowlisted build context. No host package installation, credentials/private
manual/database access, hosted model call or source approval.

## Evidence, regressions and checks

Ignored local artifacts: `target/hierarchy-screen-20261015/`; scored records in
`runs/`. Keep raw ledgers, per-model readiness/provenance/verification/logs,
`launches.jsonl`, `cases.json`, explicit `*.finalized.jsonl`, `summary.json` and
`executed-evaluator.tar.gz`. Partial public weights remain outside source releases.

Frozen contract hash:
`216136015e57cb261f950a3ab2e2fba7149880aab4050a9d1a782375f9e4051c`.
Independent review verified contract/state hashes, archived scorer/screen byte
hashes and exact state in prompts. Current scorer/screen formatting differs only
syntactically; archived executed sources preserve reproducibility.

Independent review found/fixed red-first regressions for manifest admission,
startup/failure denominators, observed versus reserved counts and no-second-forward
guard, launcher limits, mutable forest indexes and source-package omissions.
Finalizer tests cover interrupted/unstarted outcomes and corrupt/duplicate input.
Final review additionally reproduced that altered ledger gold/stratum/hash metadata
could change scores: two permanent tests failed in five assertions before frozen-
metadata and contract-envelope validation fixed it. Existing observations matched
the frozen metadata; recomputed scores after the fix are unchanged.

Final gates passed Ruff check/format,88helper tests, Cargo formatting, locked offline
source-package verification compilation, source-member allowlist and88unpacked
helper tests. Fresh independent results/fix/report review GO scoped to this optional
prototype and honest partial execution; reviewer independently ran42focused tests
and reconstructed unchanged results. See
[review record](reviews/2026-10-15-hierarchy-screen-execution.md).
No shipping Rust unit/integration suite, browser, live extraction/API review or
app usage trial was run for these optional-tool changes.

## Next

Wire the tested elapsed-deadline helper into a fully bounded live setup controller,
then resume one small new trained candidate before larger downloads. Preserve multi-book hierarchy
as a first-class evidence contract and connect it to actual extraction in a separate
reviewed step. A fast, broad comparison still requires finished verified artifacts
and fresh real-game cases; this partial screen does not replace that work.
