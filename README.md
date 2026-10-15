# Mimik

A proposed multi-system tabletop role-playing game (GDR/TTRPG) companion built in Rust with a server-rendered htmx UI.

Mimik is an open-source, sheet-first companion for players at a physical table: keep the sheet useful and bring relevant rules to the player without interrupting the game. Mimik's original code is licensed under AGPL-3.0-only; see `LICENSE` and [licensing notes](docs/licensing.md).

## Product brief

- Store editable character sheets using the official layout for each supported game and edition.
- Validate character data and calculate derived values using explicit rules.
- Suggest situational options with explanations of actions, spells, prerequisites, and relevant rules.
- Answer questions grounded in the user's manuals; evaluate Jev-style typed decision models and optional generative explanations.
- Begin with D&D, Pathfinder, Vampire (exact title to confirm), and MÖRK BORG.
- Expand through a plugin system.

## Current status

An initial Rust/htmx foundation is implemented: create, list, edit, and persist character identity/notes in SQLite, with server-side field validation and stale-edit protection. Assets are bundled locally; no Node.js or model service is required.

The first MÖRK BORG slice adds persistent abilities, HP, armor, load/resources, descriptive fields, and source-backed baseline calculations. In the explicit loopback `--no-auth` prototype, the home-page creation flow offers only loaded exact-edition sources with an implemented adapter (currently Italian 2025 MÖRK BORG); its empty database links to source setup. Normal authenticated startup requires Owner setup and uses the private campaign workspace. See [coverage](docs/mork-borg-coverage.md) for supported mechanics and audited pages.

Its contextual checklist explains ordinary base-rule options from a saved snapshot and explicitly declared facts. Unknowns/broken outcomes/special rules remain unresolved; it never spends resources, rolls dice, or claims full legality.

Local sources now have an import/review workflow: original page images, preserved table structure, separate corrections/history, coverage visibility, and reviewed-only exact-edition lookup. See [source setup](docs/source-review-setup.md). The registered local MÖRK BORG sample remains unreviewed.

Signed-in Owner source inspection and evidence review are available at `/admin/materials`, with original page/region previews, separate unit/completeness decisions, revision-safe drafts and authenticated review history. Imports, registry changes and definition publication use the local CLI.

A [runtime ruleset registry](docs/runtime-registry.md) supports stable game/edition registration, explicit legacy aliases, and version2 prepared-source imports for unknown games. Lookup uses canonical identities across aliases. Authenticated campaign permissions and pinned generic editing are implemented in the separate private workspace; registration alone does not enable a new character editor.

[Strict definition drafts](docs/definition-drafts.md) can be imported, revision-safely updated, shown and listed through local CLI. Bounded schema/reference validation rejects malformed or unsupported proposals. [Immutable publication](docs/definition-publication.md) records explicit wiring comparison/approval snapshots and invalidation/reapproval, separately from source review or game legality.

[Local operator accounts](docs/accounts.md) provide protected-file Owner bootstrap, invitations/enrollment, password resets and disablement. [Default authenticated serving](docs/http-auth.md), `mimik`, provides persistent sign-in/sign-out and epoch revocation. Its [campaign workspace](docs/campaign-workspace.md) offers creation/joining, explicit Master grants, personal native/runtime sheets, private history and departure freezing. Explicit `--no-auth` selects the **unauthenticated prototype, not secured by account setup**, isolated from these new private records.

In normal auth mode **Owner is administration-only** at `/admin`; use a separately
enrolled player account for campaigns. Development testing can explicitly opt in
with `--dev-owner-play`, preserving existing grants and records. Passwords
require at least15 Unicode scalars for Owner and12 for players, without mandatory
character categories. Lists show Next only when another authorized page exists.

Owner can now list accounts, invite/reissue, issue player password resets and
disable players in the browser. Invitations/resets have a Copy link button and QR;
opening prefills the form without consuming the credential. Expiry/history dates
are readable in the browser's local timezone. See the
[account workflow](docs/accounts.md).

Normal `mimik` starts authenticated Wi-Fi/LAN serving and prints the address to open.
The database defaults to `data/mimik.sqlite3`; no flags are needed for common use.
`--loopback`
is explicit development mode. Configure canonical links/QR behind a reverse proxy
with `--external-url https://mimik.example` or `MIMIK_EXTERNAL_URL`; listener settings
cover the internal address, so no separate internal_url is needed. See
[self-deployment](docs/deployment.md).

Use `--no-auth` only for the isolated loopback prototype. The former `--auth` flag is
removed; `--database` and `MIMIK_DATABASE` remain optional location overrides.

The owner-supplied official blank sheet informed the editor's adaptive section grouping; this structured editor is not an approved official sheet. Legacy identity/notes records remain accessible only in prototype mode. The authenticated workspace creates private campaign-bound native or published runtime sheets with complete edit/freeze history; it does not assign legacy records automatically. Recovery/lifecycle operations and model-assisted definition wiring remain next work. No inference model is integrated into the application. Lookup is a literal reviewed-only `LIKE` search with no ranked/semantic retrieval. See the [harness assessment](docs/harness-assessment-2026-10-15.md) for findings and proposed next steps.

## Run

```sh
cargo run
```

After one-time Owner bootstrap, open the LAN address printed at startup (localhost
also works). For explicit development loopback use `cargo run -- --loopback`; for the
unauthenticated prototype use `cargo run -- --no-auth`. `MIMIK_DATABASE` or `--database`
selects another database; otherwise `data/mimik.sqlite3` is used. Ctrl+C stops the server
gracefully. Normal forms support manual entry without JavaScript; sharing and local
date formatting use small bundled scripts.

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

## Project documentation

- [Existing tools and sources](docs/research.md)
- [Proposed architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Questions and decision log](docs/discovery.md)
- [Decision-model research](docs/decision-models.md)
- [Persistent tasks and session handoff](TODO.md) — start here when resuming work.
- [Target hardware and inference direction](docs/hardware-and-runtime.md)
- [Local source inventory](docs/sources.md)
- [PDF extraction and review workflow](docs/source-quality.md)
- [Local setup and owner-supplied manuals](docs/local-setup.md)
- [Layout-aware extraction and optional owner-key API review](docs/extraction-prototype.md)
- [MÖRK BORG implementation coverage](docs/mork-borg-coverage.md)
- [Rust provider abstractions](docs/provider-abstraction.md)
- [Adaptive official-sheet presentation](docs/sheet-presentation.md)
- [Maintained dependency decisions](docs/dependency-decisions.md)
- [Supported release/CI scope](docs/release-scope.md)
- [Independent reviews](docs/reviews/)

CI uses isolated containers for formatting, strict Clippy all+pedantic, regressions, lint/audit/licenses, release packaging, offline source rebuild and real-process/Chromium artifact checks. Configure `deployable` as a required repository status when hosting the project.

Rust and Cargo 1.95.0 are available in the development environment. Third-party htmx attribution is in `assets/README.md`.
