# Independent review: genuine local Jev-like decisions

## Plan and runtime fit

User requested actual Jev-like tests after the general-model RAG probe and called
out context focus and preload. Independent plan reviewer
`ses_ec15571d1ffeUVS77iJVfcT4jO` approved bounded container setup/downloads, with
pre-scoring conditions: unambiguous label precedence, shared logical inputs,
controlled adjacent cache pairs/invalidation, and enforceable native deadlines.

Main used maintained AnyJev `Decider`/`LlamaCppBackend` at immutable source commit,
Vulkan-built llama-cpp-python0.3.36 official sdist hash and existing verified GGUFs.
Trained Kev0.8 uses reference `Checkpoint`/`Server.probs`, CPU-only Torch2.8,
unrounded probabilities and actual trained adapter/pointer head. Base/adapter/head
artifacts verified; loaded class confirmedQwen3_5TextModel with no vision modules.
No remote code trust or unsafe pickle fallback. Packages remain optional
container-only evaluation tooling, not a new application scripting language.

Build context was explicitly empty/allowlisted. User interrupted for disk cleanup;
that separately reviewed operation is in `2026-10-15-temporary-cleanup.md`.
Setup then resumed without recreating large host `/tmp` targets. No API account,
user database, private manuals or credentials were used.

## Cases and evaluator

Independent evaluator/case reviewer `ses_ec113e796ffevPfgt9gOEb5C0W` found the18
labels/source preparation coherent and no gold leakage. Required fixes:

- High: cache hit/miss expectations and probability parity were only observed,
  not enforced. Fake-native main-run regression failed before fix; expected native
  flags/counter deltas, main cold/warm and changed/fresh1e-4 parity now gate the
  persisted verification outcome. Failed observations are retained, not repaired.
- Medium: started/finished ledger lacked timestamps. Regression reproduced missing
  fields; wall/monotonic/elapsed fields added for scored and cache calls, with
  complete72-row exhausted/dead-worker denominators tested.
- Medium: presentation-order tie-breaking could manufacture flips. Uniform-vector
  regression failed before fix; semantic-ID tie-breaking and explicit tied IDs
  now make ties presentation-independent. No actual top ties occurred in this probe.
- Main found a timeout pipe-close ResourceWarning; a real sleeping-child regression
  failed before fix. Deadline now terminates/reaps and closes native-process pipes.
- HF public-tree metadata0600 caused a UID1000 bootstrap permission warning;
  only the derived public cache-tree mode was corrected, with retained permission
  regression. Model contents/hashes were unchanged.

Recheck GO before scored inference. Reviewer confirmed no accidental accuracy gate:
experiment verification depends on execution/cache integrity, not model correctness.
55 helper tests/lints passed. Preservation guard also covers verification sidecars.

## Executed results review

Fresh independent results reviewer `ses_ec0a4052cffefsf5qyoh1AVbDb` read cases,
state/prompt/probability logs, native stats, manifests and verification outputs.
Recomputed results:

| Configuration | Correct observations | Median native decision wall time | Order-flip cases |
|---|---:|---:|---:|
|4B-Instruct raw|58/72|0.4706s|4/18|
|4B-Instruct L0|52/72|1.8924s|2/18|
|8B raw|38/72|0.8227s|5/18|
|8B L0|56/72|3.3386s|2/18|
|Kev0.8 CPUFP32|44/72|6.5171s cold /2.1183s warm|3/18|

360 scored observations over18 unique cases, not360 independent examples.
16 additional unscored cache calls and four verification records passed.
All three run verification files pass, all native decisions completed. Same-order
repeat winners agree36/36 per configuration. External order flips include both-
wrong changes; internal AnyJev order diagnostics are not substituted for that metric.

Results review verified identical logical state/question/option texts across
backends, no target labels in inputs, actual AnyJev raw/L0 levels/priors/rotations,
and reconstructed L0 probability correction within2.23e-16. Required source anchors
are present except deliberately withheld evidence. Thus context/full expected
labels coincide here and do not measure accidental retrieval-miss behavior.

Kev native36cold misses/36warm hits and all four value/revision/evidence/template
invalidation patterns are correct. Cold/warm and changed/fresh probe distributions
match exactly; no application revision-authorization claim follows from token-keyed
cache reuse. Container exit0 after8m44s confirms completion despite later console-
attachment timeout; persisted logs do not show a model timeout.

## Conclusions and limits required by review

- L0 helps8B but hurts4B accuracy in this content-free/full-cycle configuration.
- Wrong confident outputs remain; Kev0.8 has **zero observations>=0.8/0.9**, not
  a measured zero-risk or calibration advantage.
- All configurations miss the wrong-focus-delta case; evidence absence is still
  mishandled by several AnyJev runs. Labels are advisory, not legality or writes.
- Note case does not isolate injection causality because including notes changes
  retrieval relative to its clean comparison.
- GPU AnyJev clears KV; repeated decisions do not establish optimized prefix-cache
  performance. CPUKev warm means native state-cache hit, not cold model loading.
- Four-choice task results cannot be compared as accuracy gains over prior
  free-form JSON/patch tasks. No dialogue/citation-generation/whole-book/RPG
  production-quality claim.
- Native neural prefill/readout timing is not separately instrumented. Main added
  an independently labelled non-neural preparation microprobe with identical state
  hashes; it does not retroactively decompose model timings.

Final report accuracy recheck GO by the same results reviewer: all counts,
probabilities, memory units and timing aggregates verified. Adopted two wording
corrections: earlier JSON test had28observations, not28unique cases; cross-backend
equivalence is logical text, not identical native token encoders. Supplemental
preparation timings are explicitly separate from neural measurements.

Final Ruff/Cargo formatting,55 working-source helper tests, locked offline source
package compilation/member allowlist and55 tests from unpacked source passed.
All inference containers exited0, no model server or public ports remain, and no
host temp build targets were recreated. Installed Rust app/binary/data unchanged;
no new release or browser/provider integration claim. Raw local artifacts and
optional container images retained for reproducibility.
