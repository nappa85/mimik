# Maintained-library selection decisions

## Shipping source-review slice

The implementation reuses Clap for CLI parsing, SHA-2/hex crates for identities, rustix for descriptor-relative no-follow file access, lopdf for native PDF structure/page bounds, png for bounded PNG verification, Ammonia for HTML sanitization, html2text for text projection, and the existing rusqlite/Axum/Askama stack for atomic storage/HTTP/HTML. Actual versions are locked in Cargo.lock and license-checked by cargo-about.

Custom code is limited to Mimik's source contract, exact raw/prepared-unit mapping, review states/history/coverage, optimistic concurrency and evidence eligibility. It does not implement a new PDF parser, HTML parser, hashing algorithm, provider protocol, or storage engine.

Two HTML parser versions are currently transitive requirements of maintained Ammonia and html2text releases. These libraries serve different demonstrated needs (safe structural HTML and table-aware plain text); replacing them with an ad-hoc parser solely to remove a transitive duplication would weaken fidelity/security. Reassess dependency convergence when upstream releases align.

Source-kind vocabulary is shared between Rust and the preparation prototype in `schemas/source-kinds.json`; duplicated Python/Rust kind lists were removed. Synthetic fixture generation is shared between integration tests and artifact smoke scenarios.

## Quality and release infrastructure

Use Cargo's package/vendor mechanisms, cargo-audit, cargo-about, rustfmt, Clippy all+pedantic, Ruff, actionlint, pathspec's established Git-pattern matching and Playwright/Chromium. Mimik-specific archive policy and smoke scenarios use standard archive/HTTP/process facilities. No custom generic linter/license analyzer/browser engine was introduced.

All new tool/dependency/model installations occur in containers. CI and browser base images/tool versions are pinned; source runtime is built on Ubuntu 22.04 and remains a standalone Rust binary without Python, Node or model runtime requirements.

## Licensing

Mimik is AGPL-3.0-only; dependencies retain their own terms. The specific MPL-2.0 cssparser/dtoa-short exception was independently reviewed and documented, with notices and vendored sources supplied. Other unaccepted licenses remain fail-closed. Preferred-form htmx source accompanies corresponding source; owner manuals/corpora never become source-release inputs.

## Deferred

Commercial provider support will select an established Rust abstraction such as genai/Rig after capability/footprint tests; broad vendor protocols are not hand-written now. Specialized decision-model/Vulkan runtime compatibility and executable plugin infrastructure remain separate evaluations.
## Bundled response-targets and application logo (2026-10-12)

The existing htmx 2 client uses maintained `response-targets` 2.0.4 for scoped error swaps, avoiding custom client error infrastructure. Bundled source/license and provenance are in `assets/README.md`; normal CI exercises 403/415/422/409 handling with Chromium. Server retarget headers preserve the editor for transport errors.

The root `logo.png` is the user-supplied application logo, embedded without modifying the original and explicitly allowlisted in corresponding source. This records supply/provenance, not authorship or redistribution permission; confirm those before public release.

## Runtime schema validation (2026-10-12, draft-admission integration)

Use selected `jsonschema`0.58.6 with `default-features = false`, no optional features,
explicit Draft2020-12 and `.offline()`. Primary sources:
[versioned API](https://docs.rs/jsonschema/0.58.6/jsonschema/) and
[feature graph](https://docs.rs/crate/jsonschema/0.58.6/features), inspected2026-10-12.

- **Fit/capability:** maintained general JSON Schema validator, reusable compiled
  validation and structured error paths; use trusted embedded/host-generated
  schemas. Serde handles decoding but does not provide generic schema validation
  or reject duplicate keys in `Value`; its visitors can supply narrow bounded
  admission. Avoid writing a generic schema engine or JSON tokenizer.
- **Maintenance/license:** published MIT release dated2026-10-06; upstream has
  current draft/API documentation. Compatible in principle with AGPL; transitive
  license/audit gates still required. Recent release is not a correctness audit.
- **Portability:** upstream documents native/WASM variants; Mimik only claims its
  container-verified Linux release target. Verify MSRV against Rust1.95 before use.
- **Runtime/dependency cost:** defaults enable HTTP/file resolution, reqwest/rustls
  and TLS provider dependencies. Disable them and assert offline reference failure.
  Core still includes regex/fancy-regex, fraction, referencing and schema-value
  infrastructure: reduced-feature, not dependency-free or measured lightweight.
  No arbitrary-precision feature needed for signed i64 storage; verify extreme
  integer behavior instead of trusting float-based constraints.
- **Boundaries:** one trusted envelope schema and host-generated simple value
  schemas; bounded Mimik reference/lifecycle/i64 checks remain custom domain work.
  Do not compile user-authored JSON Schema/regex or accept remote references.

Historical isolated candidate probe is recorded in `runtime-validator-fit.md`;
the application was unchanged **at that checkpoint**. Current draft admission
integrates the pinned version/defaults disabled and explicit offline host schemas.
Actual combined Linux graph240 packages, jsonschema features empty, no reqwest/
rustls; audit/notices passed. See `definition-drafts.md` for real-profile limits,
resource samples and measured6,019,744-byte release delta. Samples are not
worst-case latency/hard-memory guarantees. No new host tooling installed.

The probe-only MIT-0 disposition was superseded for application integration by
fresh license review `ses_ecfcce262ffeoxR1tVYwovgyoj`, approving locked borrow-or-share
0.2.4/checksum and a narrow package-specific application entry. Global accepted
licenses/build/dev/transitive coverage remain unchanged. Actual notices include
the crate/version/MIT No Attribution text. Re-review version/source/license changes;
artifact gates remain separate from license-policy approval.
## Lexical retrieval (2026-10-15)

Rules lookup uses SQLite FTS5, already compiled into the bundled rusqlite/libsqlite3-sys build (`ENABLE_FTS5`, SQLite 3.50.2, verified in the CI container), with the built-in `unicode61 remove_diacritics 2` tokenizer (Italian manuals; English `porter` rejected) and bm25 ranking. Search text is produced by the existing html2text dependency in `plain_no_decorate` raw mode (unwrapped, one table cell per line); no new crate or SQLite feature was added. Custom code is limited to eligibility filtering, quoting-only query construction and index maintenance. A vector index (`sqlite-vec` or brute-force Rust cosine) is deferred until an embedding runtime is selected; see `docs/hybrid-retrieval-plan.md`.

## Derived-field formula engine: `cel` and `antlr4rust` (selected 2026-10-15, recorded in M3)

Derived values and advisory constraints use the maintained `cel` crate (cel-rust), restricted
by Mimik to the admitted subset "cel-mimik-1". Selection evidence, the capability spike and
the combined-tree measurements are in `docs/derived-formulas-plan.md` §2.

- **Versions and pinning:** `cel = "=0.15.0"` with `default-features = false` (no regex,
  chrono or base64 encoders; Mimik owns JSON→CEL conversion, so `json` is unused).
  Its required parser runtime `antlr4rust` 0.6.0 is transitive. Both are locked: the
  `pinned_engine_versions_in_lockfile` test fails on any version change of either crate,
  so `cargo update` cannot move them silently, and the golden conformance fixture
  (`schemas/formula-conformance-v1.json`) is SHA-256-locked in the Rust and Python tests.
- **Licenses:** `cel` is MIT; `antlr4rust` is BSD-3-Clause. Both are in the existing
  `about.toml` accept list; `cargo about generate --locked --fail` passes unchanged. New
  transitive packages are listed in the plan §2; the transitive duplicates (`nom` 7/8,
  `bit-set`/`bit-vec`) are accepted like the html5ever precedent above and reassessed when
  upstreams converge.
- **Maintenance risk:** `antlr4rust` is a single community-maintained upstream for the
  generated parser runtime. Mitigations: the lockfile gate, `cargo audit --deny warnings`
  in CI, and a reviewed re-run of the conformance fixture on every bump.
- **Alternatives rejected:**
  - `evalexpr`: its own expression language, no static typing of declared fields and no
    standard semantics to cite; Mimik would have to specify and maintain the language.
  - `fasteval`: floating-point numeric evaluation only (no exact i64, strings, lists or
    enums).
  - `rhai`: a general-purpose scripting language with loops and functions; admitting it
    would mean disabling most of the language and still bounding a Turing-complete
    runtime.
  - A custom `winnow` parser (revision 1 of the plan, Appendix A): a Mimik-specific
    language with a Python reference implementation, i.e. two implementations to
    maintain. It was dropped once the user accepted CEL's truncating division and error
    absorption.
- **Custom code** is limited to the gaps cel-rust does not cover: admission (allowlisted
  constructs, static types, AST depth, nested-aggregate and static cost bounds), Mimik's
  aggregate function catalogue, typed JSON↔CEL conversion and the stable error-code
  mapping (`src/definitions/formula/`).
- **Exit path:** the engine is isolated behind `formula::{Compiled, Evaluation, Code}`.
  Replacing `cel` means a new profile ID, an engine passing the golden fixture (or a
  reviewed outcome diff under `formula_profile: 2`) and dual evaluation of existing pins
  until migration; pinned definitions keep their profile.
