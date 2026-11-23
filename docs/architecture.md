# Architecture

Mimik is a sheet-first tabletop RPG companion for players at a physical table. It is one
standalone Rust binary (`mimik`, AGPL-3.0-only) that serves server-rendered HTML to browsers on
the local network and stores everything in one SQLite file. No Node.js, Python, GPU or internet
access is needed at run time. The optional interpretation assistant (off by default,
`--assistant`) runs the bundled, pinned official llama.cpp `llama-server` (the Vulkan build on
a hardware GPU, else the CPU build, in `llama/` beside the binary) as Mimik's own confined child
process, never a container
([model-runtime.md](model-runtime.md)).

## Runtime

- **HTTP:** Axum 0.8 on Tokio (`src/main.rs`, `src/web.rs`, `src/auth/mod.rs`). Port 3000.
- **HTML:** Askama 0.14 templates in `templates/` (38 files), escaped by default. Business logic
  is on the server; forms work as native HTML forms.
- **Browser scripts:** bundled in the binary from `assets/` and served locally:
  `account-sharing.js` (copy link, QR, local dates; account and history pages),
  `source-regions.js` (material review), `sheet-assist.js` (saved runtime sheet pages, under
  their strict CSP; `docs/assisted-entry.md`). htmx 2.0.8 and response-targets 2.0.4 are served only
  by the unauthenticated prototype (`src/web.rs`). The authenticated router serves no htmx
  (`src/auth/mod.rs`, fallback 403). Provenance and licences: `assets/README.md`.
- **CLI:** clap 4 (`src/main.rs`, `src/account_cli.rs`). Without a subcommand the binary
  serves; subcommands are local operator tools that share the server's domain transitions:
  - accounts: `bootstrap-owner`, `invite-account`, `enroll-account`, `issue-account-reset`,
    `complete-account-reset`, `disable-account`, `show-account`, `list-accounts`,
    `verify-account-password`;
  - registry: `register-ruleset`, `register-ruleset-alias`, `list-rulesets`;
  - sources: `import-source`;
  - definitions: `import-definition-draft`, `show-definition-draft`, `list-definition-drafts`,
    `compare-definition`, `approve-definition`, `show-definition-version`,
    `withdraw-definition`;
  - catalogues: `import-catalogue-draft`, `show-catalogue-draft`, `compare-catalogue`,
    `publish-catalogue`, `show-catalogue-version`, `verify-catalogues`,
    `rebuild-catalogue-index`.

### Serving modes

| Mode | Flags | Router | Listener |
|---|---|---|---|
| Authenticated LAN (default) | none; `--lan-address` picks the interface | `auth::serve_with_options` | `0.0.0.0:3000`; peers admitted from the chosen private IPv4 subnet or loopback (`src/network.rs`) |
| Authenticated loopback | `--loopback` | same | `127.0.0.1:3000` |
| Behind a TLS reverse proxy | `--external-url` or `MIMIK_EXTERNAL_URL` (root HTTP(S) origin) | same | as above |
| Unauthenticated prototype | `--no-auth` | `web::app` | `127.0.0.1:3000` only |

Authenticated startup refuses to serve until an Owner exists (`bootstrap-owner`). The
prototype keeps the legacy identity/notes records (no new ones are created), isolated
from the private campaign tables. `--assistant` turns the interpretation assistant on, with its
`--assistant-model*`, `--models-dir`, `--assistant-runtime`, `--assistant-threads` and `--assistant-device` options
([model-runtime.md](model-runtime.md)); builds with the `dev-endpoint` feature (never a
release) also take `--assistant-dev-endpoint` and `--assistant-dev-token-file` for tests. The
hidden `assistant-runner` subcommand is the model server's runner and the start-time probe.

The authenticated boundary (`src/auth/mod.rs`) admits at most 8 concurrent requests, times a
request out after 5 s, rate-limits new form sessions, sign-in and credential redemption with
one process-wide quota (burst 10, then one per 6 s), checks Host and
Origin against the configured authority, and sets `Cache-Control: no-store`,
`X-Content-Type-Options: nosniff`, a CSP including `frame-ancestors 'none'`,
`X-Frame-Options: DENY` and `Referrer-Policy: same-origin`. The router-wide body limit is
8 KiB; sheet routes have their own limits (table in `docs/campaigns.md`, "Limits").
Sheet pages and sheet JSON answers carry `SHEET_CSP`; Owner material pages, scripts,
images, successful POST redirects and 409/422 draft pages use that strict policy too.
Rebuilt refusals and the unauthenticated navigation's login redirect keep only
`frame-ancestors 'none'` (response matrix in `docs/sources-and-retrieval.md`).

## Modules (`src/`)

| Module | Responsibility |
|---|---|
| `main.rs`, `account_cli.rs` | Argument parsing, serving modes, operator subcommands |
| `lib.rs` | Module wiring, `AppError`, legacy prototype character input |
| `store.rs` | `Store`: one rusqlite connection behind a mutex, run on `spawn_blocking`; migrations; `busy_timeout` 5 s, `foreign_keys = ON` |
| `web.rs` | Unauthenticated prototype router: legacy characters (view and notes), source routes, bundled assets |
| `network.rs` | LAN/loopback selection, peer admission, canonical external URL |
| `auth/` | Sign-in, sessions, request admission, security headers, error pages; Owner pages `/admin` (accounts), `/admin/materials` (source review), `/admin/rulesets` (registry), `/admin/assistant` (model runtime) |
| `accounts/` | Accounts, Owner binding, invitations, resets, disable, epochs; Argon2 hashing; protected credential files; browser transport |
| `campaigns/` | Campaigns, membership and requests (`domain.rs`); private sheets (`sheets.rs`, `snapshots.rs` is the sole writer); value admission (`values.rs`, `transport.rs`); routes (`web.rs`); advisory formulas view (`formulas.rs`); history change summary (`history_diff.rs`); catalogue reading through a sheet (`catalogue.rs`, `catalogue_web.rs`); assisted entry: lenient numbers (`entry.rs`), change notes (`notes.rs`), switch and Undo (`assist.rs`, `assist_web.rs`), per-field save (`unit.rs`, `unit_web.rs`, list units `rows.rs`), quick-add reading (`quick.rs`), interpretation context, routes and apply (`interpret.rs`, `interpret_web.rs`) |
| `assistant/` | Interpretation assistant: schema v2, instructions and fixed prefix (`contract.rs`), phrase lexer (`lexer.rs`), reply validation (`output.rs`), request bounds (`request.rs`), `rig-core` client and bounded transport (`interpreter.rs`, `transport.rs`), jobs and runtime events (`jobs.rs`), breaker (`breaker.rs`), model spec (`model.rs`), the model runtime (`runtime/`: manifest, verified files, runner, seccomp, sandbox (`hako.rs`, `sandbox.rs`), probe, launcher, endpoint check, token, generations, drain, calibration, budgets, manager; [model-runtime.md](model-runtime.md)), model downloads (`download/`, `downloads.rs`, `download_rows.rs`; [model-downloads.md](model-downloads.md)), development endpoint for tests (`development.rs`) |
| `definitions/` | Declarative game definitions: strict decode, JSON Schema admission, revisioned drafts, immutable publication and withdrawal, private pins; CEL formula engine (`formula/`) and its admission |
| `catalogues/` | Read-only game catalogues: decode, admission, drafts, sealed publication with projections and name index, read-only lookups |
| `sources/` | Prepared-source import, page/unit review evidence, FTS5 lexical search, region previews, Owner material operations, prototype source routes |
| `registry.rs`, `registry_admin.rs` | Immutable game/edition identities and legacy aliases; Owner browser registration |
| `form_text.rs`, `local_file.rs`, `pagination.rs`, `presentation.rs` | CR LF → LF form text, bounded descriptor-based local reads, 50+1 keyset pages, date presentation |

## Storage

One SQLite database, default `data/mimik.sqlite3` (`--database`, `MIMIK_DATABASE`). rusqlite
0.43 with bundled SQLite (FTS5 included). Migrations are embedded (`src/store.rs`, `MIGRATIONS`)
and applied in one `IMMEDIATE` transaction; `PRAGMA user_version` holds the schema version
(`SCHEMA_VERSION` in `src/store.rs`). A database from a newer schema is refused. There are no down
migrations.

| Migration | Holds |
|---|---|
| 001–002 | Prototype characters (002's native sheet table is dropped by 038) |
| 003–004, 007, 009 | Sources, pages, units, review events, coverage; source finalization and page hashes; integrity retrofit |
| 005 | Registry: games, editions, legacy aliases, character/source ruleset links (038 deletes the seeded game when nothing refers to it) |
| 006, 008 | Definition drafts; definition versions, approvals, withdrawals |
| 010–011 | Accounts, Owner, credentials, account events; auth sessions |
| 012 | Campaigns, members, requests, private sheets, private sheet history, campaign events (largest schema migration) |
| 013–016 | Owner administration-only rule; web attribution sidecars for accounts, materials and registry; registry events |
| 017 | Derived FTS5 search text and index over reviewed units, with indexing failures |
| 018 | Formula profile 1 (version bump only, no table change) |
| 019 | Catalogues: drafts, versions, entry types, entries, unit references, seals (trigger-bound to the hashed content), name FTS index |
| 020 | Private sheet change notes (assisted entry) |
| 021 | Change notes rebuilt to admit row addresses (`row:<list>/<host>`) and 6-byte `meta:` addresses |
| 022 | The assistant's model setting: models the owner switched to (spec, SHA-256, size, alias, trial measurements) and the single active/previous choice ([model-runtime.md](model-runtime.md), "Owner model setting") |
| 023 | The owner's model downloads: the confirmed downloads with their licence snapshot ([model-downloads.md](model-downloads.md)) |
| 024–027 | Source documents, owner units, review operations and suggestions; source uploads; upload fingerprint key |
| 028 | Owner units may be `Text` or `Table` |
| 029 | Append-only `source_page_words`: the word boxes of a page, written at import for the table shapes |
| 032 | `assistant_reply_lengths`: the owner's reply length per model file, apart from the model rows and the choice ([model-runtime.md](model-runtime.md), "Reply length") |
| 038 | Removes the native MÖRK BORG sheet: drops `mork_borg_sheets` and the registry `native` column, narrows the private-sheet insert trigger to runtime pins and deletes the seeded MÖRK BORG pair unless something refers to it; `migrate()` first refuses a database holding native sheet data |

## Areas

- **Accounts, sign-in, sessions, LAN and proxy:** [auth-and-sessions.md](auth-and-sessions.md).
- **Campaigns, private sheets, history:** [campaigns.md](campaigns.md).
- **Game definitions:** [runtime-definition-contract.md](runtime-definition-contract.md).
- **Derived values and checks (CEL `cel-mimik-1`):** [formulas.md](formulas.md).
- **Catalogues:** [catalogues.md](catalogues.md).
- **Assisted entry (sums, notes, switch, Undo, unknown values, per-field save, quick-add):**
  [assisted-entry.md](assisted-entry.md); remaining steps in
  [assisted-sheet-plan.md](assisted-sheet-plan.md).
- **Sources, review and retrieval:** [sources-and-retrieval.md](sources-and-retrieval.md).
  Lexical search uses SQLite FTS5 (`unicode61 remove_diacritics 2`, bm25) over reviewed units
  only (`src/sources/search.rs`); it is reachable from the prototype's `/sources/search`, not
  from the authenticated workspace. There is no vector search.

## Tooling outside the binary

- **Python helpers (`tools/`, Python 3.12, Ruff):** release checks (`package_release.py`,
  `verify_source_package.py`, `smoke_release.py`, `browser_smoke.py` with its
  `*_browser.py` scenarios, `lan_smoke.py`), source preparation and review prototypes
  (`extract_manual.py`, `review_extraction.py`, `prepare_review.py`, `source_bundle.py`,
  `evidence_forest.py`), profilers (`*_profile.py`), and benchmark prototypes (tool-loop,
  local RAG, decision models and router, model screens, `fetch_models.py`, queue runners).
  Benchmarks follow the prototype tier: they are not part of or required by the runtime
  binary, but ship in the corresponding source package (`Cargo.toml` `include`). Tests are `tools/test_*.py`.
- **Development examples (`examples/`):** `source_fixture` (synthetic source bundle for tests
  and smoke), `formula_eval` (JSON-lines bridge to the formula engine for Python prototypes;
  excluded from the source package), `qr_decode`, profilers; demo packs
  `sheet-harness-demo/` and `tool-loop-demo/`.
- **Schemas (`schemas/`):** definition and catalogue JSON Schemas and examples, the
  SHA-256-locked formula conformance fixture, `source-kinds.json` shared by Rust and Python, and
  the assistant's output schema v2 with its instructions (`interpretation-v2.json`,
  `interpretation-v2-instructions.txt`, SHA-256-pinned in Rust and Python; LF-only through
  `.gitattributes`).
- **Containers (`containers/`):** every tool runs in a pinned container, never on the host
  (one exception: the pinned official llama-server build may run natively on the dev host for
  runtime-manager tests, ASSIST-40).
  `ci.Dockerfile` (Ubuntu 22.04, Rust 1.99.1, rustfmt, Clippy, actionlint 1.7.7, uv with Python
  3.12.12, Ruff, cargo-audit 0.22.2, cargo-about 0.9.2), `browser.Dockerfile` (Playwright 1.56
  Chromium), `extraction.Dockerfile` (Marker, CPU), `decision-benchmark.Dockerfile` and
  `model-screen.Dockerfile` (local model evaluation).

## CI (`.github/workflows/ci.yml`, workflow "Deployability")

Runs on every push, pull request and manual dispatch, on `ubuntu-22.04` (`llama-runtime` on
`ubuntu-24.04`), with read-only contents permission and pinned actions.

1. **llama-fetch** (in the `mimik-ci` container): `tools/llama_runtime.py fetch` downloads the
   pinned llama.cpp asset, the two CI test models and the source (`git archive` of the pinned commit),
   verifies every size and hash against `schemas/llama-runtime-v1.json`, and uploads them.
2. **checks** (needs llama-fetch; in the `mimik-ci` container): `cargo fmt --check`; `cargo clippy --locked
   --all-targets --all-features -- -D warnings` and the same with default features; `ruff check tools` and `ruff format --check
   tools`; `cargo build --locked --example fake_llama_server` (the launch tests run that
   binary; `cargo test` alone builds only a hashed copy); `cargo test --locked --all-targets
   --all-features`; build the `formula_eval`
   example; generate the synthetic source fixture; `python -m unittest discover -s tools`;
   `actionlint`; `cargo audit --deny warnings` (also on the lock with the mupdf registry entry restored); `cargo about generate --locked --fail`
   (third-party notices); `cargo build --release --locked`; `cargo package`,
   `vendored_fork.py package` (adds the mupdf fork) and `verify_source_package.py` (member allowlist); `cargo vendor`; `llama_runtime.py verify` of
   the fetched files (hashes, then the manifest's modes); `package_release.py` (stages exactly
   the manifest's files as `llama/`, their notice, `BUILD.txt` and `SOURCE.txt` (which points to llama.cpp's upstream commit; its
   source is not shipped, ASSIST-47)); the lib test binary (`--no-run --message-format=json`)
   as `confinement-tests`; a development build with the `dev-endpoint` feature (never
   packaged). Uploads the release archives with `SHA256SUMS`, the confinement test binary, the
   synthetic smoke inputs and that development binary.
3. **artifact-smoke:** verifies checksums, unpacks the runtime archive and runs
   `smoke_release.py --auth` against the packaged binary from an isolated directory; then
   rebuilds the source archive plus vendored dependencies with `--offline --locked`, no network
   and an empty Cargo home.
4. **browser-smoke:** runs `browser_smoke.py` in the Chromium container against the unpacked
   binary, offline, with dropped capabilities and resource limits;
   `review_browser.py` checks synthetic source review at desktop/phone sizes with and without
   JavaScript, even if the preceding browser smoke fails (unless cancelled); then
   `assist_model_browser.py` against the development binary and a fake model server.
5. **lan-smoke:** `lan_smoke.py` runs the unpacked binary on isolated container networks with
   separate clients, a denied subnet and an HTTPS reverse proxy.
6. **llama-runtime** (needs llama-fetch and checks; host steps, since Docker's seccomp profile
   refuses the namespaces the sandbox needs; no Rust toolchain): verifies and unpacks the release,
   reapplies the manifest's modes, lifts the Ubuntu AppArmor user-namespace
   restriction for the job, checks `llama-server --version` and `--help` for every flag, runs
   `mimik assistant-runner --probe` (fails below level 1), starts the unpacked release with
   `--assistant` and requires Ready at level 1 and a clean SIGINT exit
   (`tools/assist_release_smoke.py`), then the confinement test binary's
   seccomp, endpoint, runner and live tests against the unpacked runtime and the test model,
   with the release's own binary as the runner.
7. **deployable:** always runs and fails unless all six jobs succeeded.

What green means and what is not certified: [release-scope.md](release-scope.md).

## Docs map

- [decisions.md](decisions.md): decisions in force.
- [limitations.md](limitations.md): known limits and follow-ups.
- [results.md](results.md): experiment and benchmark conclusions still in use.
- [release-scope.md](release-scope.md): supported release and gate (shipped as the archive
  README).
- Reference: [auth-and-sessions.md](auth-and-sessions.md), [campaigns.md](campaigns.md),
  [runtime-definition-contract.md](runtime-definition-contract.md), [formulas.md](formulas.md),
  [catalogues.md](catalogues.md), [assisted-entry.md](assisted-entry.md),
  [model-runtime.md](model-runtime.md),
  [sources-and-retrieval.md](sources-and-retrieval.md).
- Active plan: [assisted-sheet-plan.md](assisted-sheet-plan.md).
- Prototype tooling: [evaluation-tooling.md](evaluation-tooling.md); held-out freeze record and
  ledger policy: [tool-loop-heldout-v3-freeze.md](tool-loop-heldout-v3-freeze.md).
