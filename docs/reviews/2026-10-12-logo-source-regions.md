# Logo transparency and source-region previews

## Plan review

Fresh read-only reviewer `ses_ed0c77683ffexlfpB6YnsY05y5` reviewed header transparency/title removal and derived original-render bounding-region PNG previews/downloads. Initial high finding: per-request image limits alone do not bound aggregate work. Medium finding: imports admit degenerate four-point polygons. Memory revision also required bounded metadata expansion and encoded output.

Disposition: one process-wide nonqueued semaphore acquired before SQLite blob copying; owned permit follows DB/worker jobs through cancellation and encoding. Busy requests return503/Retry-After1. SQL guards input lengths; lazy previews. Existing `png` header validation shared with import, no double decode. Selective geometry deserialization skips unit/provenance expansion and limits page count during deserialization. Reject invalid/missing/duplicate geometry, zero-area bounds, corrupt PNG and APNG. Scale top-left extraction coordinates using the matching PDF-page dimensions, with outward rounding. Limit normalized decoded/crop buffers and encoded output. Always retain full-page fallback guidance. Final plan independently approved with no remaining blockers.

## Scope and library decision

Root logo remains unchanged RGBA (alpha range0..255). Black background came from CSS, not PNG conversion. Remove it and visible duplicate title; invert white line artwork on the light header while preserving alpha. Accessible home-link name comes from image alt.

Reuse existing maintained `png`0.18 decoder/encoder: licenses/dependencies are already covered by Cargo notices. Mimik-specific glue selects provenance geometry and copies bounded rectangle rows; no new image framework/runtime needed. Crops are **derived bounding-region previews**, not polygon masks or new approved evidence. Expand indexed pixels/transparency and normalize16-bit samples to8-bit; do not claim lossless16-bit fidelity. Storage and review state remain unchanged.

## Resource limits

One admitted job, no waiter queue. Source PNG/prepared JSON each <=24MiB before SQLite copies; polygon <=1024bytes; projected page geometries <=10000. Decoder limit64MiB; PNG dimensions <=4096×8192 and12million pixels (shared import checks), normalized output/crop each <=48MiB; output writer <=4MiB, explicit error rather than truncated success. Release decoder/full decoded pixels before encoding.

512MiB is a conservative per-job planning budget, **not a demonstrated hard process-RSS limit**. Limits bound input, projection, decoder and major buffers; allocator/codec working overhead remains. Completed HTTP response bodies and slow-client transport memory are outside crop admission's aggregate bound, as in the existing local HTTP service. The encoded response itself is bounded to4MiB. Preview failure does not mean evidence is absent; full page remains accessible.

## Regression and verification

Before implementation, logo regression failed on duplicate visible title and region endpoint test failed404 versus expected PNG. Focused tests now pass: exact asymmetric pixel/scaling/read-only route, unknown/mismatched pairs, RGB/RGBA/grayscale/gray-alpha/indexed transparency, fractional boundaries, multiple-page metadata selection, bad/degenerate metadata/corrupt PNG, incompressible output cap and16-request nonqueued overload. Strict Clippy passed. Full/browser verification and independent implementation review pending.

## Implementation findings and disposition

Independent reviewer `ses_ed0b78e75ffe2ECkY3v4IYY3Yl` found high transient503 previews never automatically recover and medium PNG color metadata loss. Corrective plan independently reviewed/approved: Mimik-specific native-image recovery with four bounded retries, per-image timers/status, lazy-load preservation and full-page fallback; preserve supported gamma/chromaticities/sRGB/ICC, explicitly reject unsupported HDR metadata. Permanent browser forced-overlapping503 regression failed before retry implementation. Metadata regression failed on missing sRGB; strengthened raw gAMA/chromaticity comparisons also failed before fixing actual decoder-to-encoder mapping.

Demonstrated library gap: png0.18.1 decodes cICP/mDCV/cLLI but its encoder does not serialize these fields from `Info`. Region endpoint returns explicit422/full-page fallback for them. It decodes legacy gamma/chromaticities into `gama_chunk`/`chrm_chunk`, while encoding uses `source_gamma`/`source_chromaticities`; explicitly map those fields. ICC cloning/compression buffers are included in the conservative planning allowance, not covered solely by the output cap. Degeneracy policy rejects zero-width/height **bounding rectangles**; a collinear diagonal may still have a usable nonempty bounding rectangle because previews are not polygon masks.

Fresh final reviewer `ses_ed0a26cf5ffeHaL414sZ9YngI2` approved scoped implementation with no blockers. Low recommendation: dedicated delayed-script native-lazy failure test; existing forced-error probes are eager, while ordinary source previews remain lazy. Deferred as additional coverage, not a claim that this timing edge has been browser-tested.

Final container checks:51 Rust/13 helper tests, strict all+pedantic Clippy, rustfmt/Ruff, actionlint, current dependency audit, notices and release build passed. Two Chromium viewports passed header alpha/no-title, three overlapping503 recoveries, terminal422 five-request ceiling/no further requests and actual PNG download plus existing sheet/context/source scenarios. Final packaged artifact/source gates run after handoff documentation regeneration. Local supported scope only, no new source approval or live inference.

## Artifact verification and handoff

`target/dist-regions-20261012/` passed source-member allowlist, `sha256sum --check SHA256SUMS`, unpacked binary equality against rebuilt `target/release/mimik`, process smoke, updated Chromium helper at both viewports, and delivered-source/vendor offline release rebuild with networking disabled, empty Cargo home and fresh build target (1m39s). Same CI/browser container image IDs as the preceding creation/layout review; current helpers mounted read-only. Label `local-20261012-regions-reviewed` identifies a local worktree snapshot, not a Git commit. Final handoff-inclusive archives regenerated in `target/dist-regions-final-20261012/`; artifact identities reside in accompanying `SHA256SUMS`. No hosted CI or real manual approval claimed. Completed tests do not prove every source/layout correct.
