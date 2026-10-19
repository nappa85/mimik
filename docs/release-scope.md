# Supported release scope

## What a green `deployable` check means

The checked release is a **Linux x86_64 self-deployed companion**, built and smoke-tested on Ubuntu 22.04 (glibc2.35), with authenticated trusted IPv4 LAN and configured root-origin HTTP(S) reverse-proxy workflows. The old unauthenticated editor remains loopback-only. It contains private campaigns/native/runtime sheets, account administration/history and the limited Italian-2025 MÖRK BORG exploration adapter plus strict prepared-source review/lookup. See corresponding-source coverage/setup documents for limits.

The registry CLI supports immutable game/edition IDs, exact aliases and v2
unknown-game prepared-source imports with canonical reviewed lookup. Source/record
identity registration is not generic sheet support or campaign authorization.
Database migration to schema5 preserves legacy sheets and review evidence; older
binaries reject the newer schema. Account/campaign/private-history capabilities
were added in schemas10–12 below; soft deletion/recovery remain roadmap features.
Refer to `docs/runtime-registry.md` in corresponding source.

Schema6 adds strict bounded definition-draft CLI/storage with exact source
references and revision checks. Import/show/list operations never publish a
definition or enable a generic sheet. See `docs/definition-drafts.md`; no new
browser draft editor, provider or campaign/account flow is certified.

Schemas7–9 add immutable prepared-source finalization and operator-only definition
comparison/publication/reapproval/withdrawal with retained evidence snapshots.
Schema9 also upgrades interrupted-session schema7/8 guard definitions. Wiring
status is separate from source review and rules legality; partial mappings and
unreviewed evidence are explicit. No generic editor is enabled by publication.
See `docs/definition-publication.md`. Artifact process smoke includes typed stale
ticket rejection, mapped original images, evidence invalidation, version reuse/new
allocation, reapproval, withdrawal and restart persistence. Browser checks cover
existing source-review paths, not a publication GUI.

Schema10 adds the trusted local-operator account CLI: protected-file Owner
bootstrap, invitations/enrollment, password resets/disable and non-secret
summaries. Argon2 PHC verification and epoch revocation primitives are implemented;
Campaign authorization/private generic editing are provided by schema12 below.
Account creation does not protect the existing unauthenticated web prototype.
Unpacked process checks exercise credential purpose/one-use/supersession, stale
epochs, password replacement, disable and restart persistence. See `docs/accounts.md`.

Schema11 introduced authentication with released tower-sessions0.15,
SQLite persistence, login/rotation/logout and current account-epoch revocation.
Explicit `--no-auth` selects the unauthenticated editor prototype, labelled
as not secured by account setup. Current network/proxy behavior is described below.
See `docs/http-auth.md`. Required unpacked and both Chromium auth stages exercise
real forms/cookies, strict rejections, rotation/account switching, restart,
reset issuance revocation, disable and logout/replay; concurrent expiry/storage/
admission faults are separately verified in unit/connected-Router tests. The
opt-in `auth-test-support` feature is excluded from the default shipping build.

Schema12 adds the usable authenticated campaign workspace, revision-safe requests/
membership/creator-controlled additional Masters, self-custodied native/runtime
sheets and complete private creation/edit/freeze history. New private records are
isolated from prototype tables. Custodian/current-Master authorization applies to
values/history; production Owner is administration-only even with old grants.
Departure preserves campaign
linkage and freezes values for both roles, including after rejoining. Runtime
creation requires current exact-pair wiring; existing pins retain structural editing
independently of missing/stale evidence. Global source/context/legacy paths remain
denied. Transfer/reactivation/rollback/delete/purge remain future
slices. See `docs/campaign-workspace.md`. Process and both Chromium gates exercise
real native/runtime create/edit/history, joining, role changes, privacy, departure/
sticky rejoin and restart. Boundary/corruption/race/capacity checks are additionally
unit/connected-Router evidence, not browser certifications of those interleavings.

Schema13 preserves existing records and excludes production Owner from gameplay
membership/supervision. `/admin` is the Owner entry; schema14 account controls are
described below; schema15 material review is described next. A separate player account is required for
normal play. Explicit `--dev-owner-play` enables development-only use of
retained explicit Owner grants, with a warning. Password validation binds to the
durable target role: Owner15/player12 minimum, no required character categories;
128 scalars/512 UTF-8 bytes maximum, non-control exact UTF-8. No verifier reset.
Directory, roster, requests, versions, sheet lists and history use authorized50+1
lookahead; campaign sections preserve independent cursors.

Schema14 adds authenticated Owner account listings/invitations/reissue/player-reset
issuance/disable and immutable web actor attribution, sharing CLI transitions and
credential rules. Enrollment/reset completion forms use captured
durable CSRF preflight/final checks, role-bound password minima, exact confirmation,
bounded SQL/KDF work and current credential/epoch checks. No automatic login/server
credential retrieval. Issuance now provides a Copy link button and locally generated
QR. Tokens use fragments, never GET/referrer transport; the form prefills and removes
the fragment without consumption. Trusted recoverable errors retain the current
token, clear passwords and provide an actual fresh-document GET recovery link;
unrelated errors remain static. Owner reset issuance/disable remains CLI;
valid CLI-issued Owner tokens can be browser-completed. Recovery/re-enable/material
uploads and character lifecycle remain later slices. Anti-framing CSP/XFO and
same-origin referrer policy preserve strict native-form Origin admission. Process/
Chromium gates include actual issuance/reissue/enroll/reset/disable/restart, visible
clipboard copy, privacy, desktop/mobile reflow and another-loopback-origin framing
rejection with an authenticated positive control. See `docs/accounts.md`.

Schema16 (planned as migration017 at merge) adds Owner-only browser ruleset
registration at `/admin/rulesets`, in addition to the unchanged registry CLI: a
50-row composite-cursor listing, exact game/edition registration and exact legacy
aliases through the same shared transitions. New editions/aliases append immutable
`registry_events`; exact repeats write none, and earlier registrations gain no
invented history. Browser writes add an insert-only `registry_web_events` Owner
attribution sidecar; strict Origin, durable CSRF and Owner rechecks guard the
transaction and audit failure rolls it back. Rejected input returns escaped422 forms.
Definition publication/withdrawal from the browser is not part of this scope.

Schema15 adds Owner-only imported-material inspection and evidence review at
`/admin/materials`, with independent50-row source/page/unit/history lists, original
full-page/crop comparisons, server-derived account reviewer and atomic immutable
review sidecars. Shared source transitions preserve raw evidence, expected revisions,
whole-page completeness checks and invalidation of dependent evidence. Native409/422
forms retain drafts/submitted revisions; scalar validation and320KiB route body bounds
support full Unicode corrections. Durable Owner and CSRF rechecks protect transactions;
crop responses recheck authority after rendering in a fresh transaction. Existing
source/account/private histories upgrade additively. Extraction/imports, registry
changes, finalization and publication remained CLI in schema15; material
uploads/wiring are later slices. See `docs/source-review-setup.md`. Synthetic process/desktop-mobile Chromium
and separate-client LAN/proxy gates include native review/conflict/completion and
original previews; controlled authority/race/overflow checks are unit/Router evidence.

Schema18 (migration018, CEL formula profile M2) and the M3 evaluation slice add
advisory derived values and checks to runtime definition sheets. Scope by sheet kind:

- **Native MÖRK BORG sheets:** unchanged; no formula evaluation.
- **Runtime sheets of scalar formula definitions** (`formula_profile: 1`, no list fields):
  publishable; derived values and advisory checks are computed on view, save and history
  from the stored values and the immutable pin, shown read-only (never submitted), and
  never block saving or rule on legality. Process/browser evidence: the unpacked
  Chromium gate publishes a synthetic scalar formula definition and checks create
  (nothing evaluated), saves that carry no derived name, a visible violation after a
  violating save, read-only computed values, keyboard-operable expression disclosures in
  the editor and in read-only history, and desktop/mobile reflow with an unbroken
  1,024-byte expression and a long derived text. Escaping, round-trip, create,
  failed-save, permit and unavailable-input cases are unit/connected-Router evidence.
- **Runtime sheets with list fields (M4):** publishable within the render-unit budget
  (32,768 units; `list-form-exceeds-limit` otherwise). Rows are server-rendered fieldsets
  with host IDs `r<revision>-<k>`; on the create page (and the legacy full form) Add,
  Remove, Move up and Move down re-render the unsaved draft and Save persists it. On a saved
  sheet (assisted sheet slice 1.5b) each row cell saves on its own on Enter, a new-row form
  adds one row, and Remove, Move up and Move down write at once; Remove is followed by "Row
  removed · Undo". History revisions show a read-only change summary against the previous
  revision. Create and edit POSTs accept 5 MiB (after a pre-body
  principal check); drafts over 1 MiB decoded are not echoed. Process/browser evidence:
  `smoke_release.py --auth` publishes the synthetic list definition and checks Add, the
  host ID, a derived list sum and the change summary; the unpacked Chromium gate (1280 px
  and 390 px) checks Add on an empty create page, Enter saving through the top Save, host
  IDs, Move up and down by keyboard with focus after each op, disabled edge moves, Remove,
  then on the saved sheet Enter in a row's Quantity saving that cell alone, the new-row
  form, a keyboard Move, Remove with Undo, a violation, read-only history with its change
  summary, no script, and no overflow with a 32-property row and a 3,000-character note.
  Budget, echo, escaping, precedence, writer and history cases are unit/connected-Router
  evidence. Not certified: item catalogues, per-property `editable: false`, value-level
  diffs or revert, and physical phone setups.
- **Line-break migration note (M4):** textarea-backed fields and every campaign-transport value
  are now stored with LF, and their length limits count typed characters. Single-line inputs
  such as printed labels and credentials are unchanged. Text saved by earlier builds may hold CR LF; it is
  not migrated in place, and its next save writes it as LF in a new revision, while history
  keeps the earlier revision.

No model, tool loop or provider evaluates or proposes formula changes; `formula::preview`
is library-only. See `docs/runtime-definition-contract.md` and
`docs/derived-formulas-plan.md` §16.

Normal no-flag startup discovers the primary private IPv4 LAN interface and accepts real
socket peers from that subnet or loopback. Host/Origin checks remain exact; missing
LAN peer attribution and forged forwarding data cannot establish authority.
`--loopback` explicitly restricts development serving; `--lan-address` selects an
enumerated private interface when needed. The prototype cannot be exposed by these
flags. Canonical `external_url` is configured with `--external-url` or
`MIMIK_EXTERNAL_URL`; it accepts an HTTP(S) origin, not credentials/query/fragment or
path prefixes. No internal_url is needed. A root-origin TLS proxy preserves external
Host, supplies actual TLS and governs original-client LAN access; Mimik uses the
configured scheme and cookie library to set Secure for that HTTPS authority while
direct LAN HTTP remains usable. Forwarding headers never select authority/scheme.
Readable RFC3339/UTC fallback dates become browser-local datetimes/timezones.
Native sign-in failures retain the form/login and clear passwords; browser document
errors have a shared branded alert/status page, while explicit API/fetch responses
remain plain with unchanged statuses. Login identity whitespace/ASCII case is
canonicalized without truncating authentication input or altering password bytes.
Original durable CSRF/session admission is checked before/after password verification;
expired/revoked forms cannot rotate into a new authenticated session. Wrong/unknown/
disabled credentials remain indistinguishable; errors never emit session cookies.
See `docs/deployment.md`. Required separate-client LAN/proxy checks use the unpacked
shipping binary, ordinary insecure-HTTP Copy button (no app clipboard grants or
secure-origin overrides), fragment prefill/retry/GET nonconsumption, real denied peers,
native HTTPS forms/Secure cookies and independent decoding of rendered desktop/mobile/
proxy QR screenshots. Source helper unit/controlled KDF races remain distinguished
from those physical socket/browser flows.

Green CI means required checks passed for the same revision; the unpacked binary passed startup/assets/edit/rejection/restart/persistence checks including synthetic source review, and corresponding sources rebuilt offline without a dependency cache. Required Chromium scenarios test actual htmx swaps, long-text persistence/expansion, desktop/mobile overflow, sanitized source images/review and reviewed-only lookup. It does not certify a finished full-feature RPG companion.

Not yet certified: physical phone/router configurations, Windows/macOS/other CPU architectures, other browsers, GPU/model inference, general Marker/OCR conversion, real API-provider calls, whole-manual extraction accuracy, official sheet-layout fidelity, public-client admission or path-prefix proxying. Browser verification is bounded to Chromium at1280×720 and390×844 with synthetic inputs and a synthetic TLS proxy; real-manual approval is not automated. Extend scope/checks when features ship.

## Run the runtime archive

Unpack `mimik-0.1.0-linux-x86_64.tar.gz` into a writable local working directory.
After Owner bootstrap, use `./mimik` and open the printed LAN address on the
server/phone; localhost remains available on the server. For development use
`./mimik --loopback`. For a same-host TLS proxy, use
`./mimik --loopback --external-url https://mimik.example` and proxy to
`http://127.0.0.1:3000`, preserving external Host. The environment alternative is
`MIMIK_EXTERNAL_URL=https://mimik.example`. Select a local interface explicitly with
`--lan-address 192.168.1.20` if automatic selection is unsuitable. The database defaults
to `data/mimik.sqlite3`; `--database` / `MIMIK_DATABASE` is optional. `--no-auth` selects
only the unauthenticated loopback prototype. The former --auth flag is removed.
Corresponding-source
`docs/deployment.md` covers firewall/Wi-Fi isolation and root-origin proxy setup.

It creates `data/mimik.sqlite3` under the current working directory. `MIMIK_DATABASE` may set another local database path. Ctrl+C shuts it down. Stop it before a simple database-file backup. No Node.js, Python, GPU, API key, or internet is required for runtime sheet editing.

Prototype new-character creation requires a registered exact-edition source matching an implemented adapter (currently Italian-2025 MÖRK BORG); its empty database links to `/sources/setup` only in explicit loopback `--no-auth` mode. Normal authenticated startup requires Owner setup and exposes the private campaign workspace, not global source routes. Registration requires the owner's prepared bundle and original PDF and is not evidence approval. Existing records remain editable without the original PDF files at runtime; imported comparison evidence is stored locally in SQLite. Manuals are not included in the release.

## Source and notices

The accompanying `mimik-0.1.0-source.crate` includes Rust code, preferred-form embedded asset source, templates, migrations, tests/helper source, configuration/build instructions and documentation. The accompanying `mimik-0.1.0-dependency-sources.tar.gz` includes Cargo-vendored sources/checksums/licenses and source-replacement configuration. `BUILD.txt`/`SOURCE.txt` identify revision; `SHA256SUMS` identifies all three archives.

Unpack the .crate, then unpack dependency sources into that package root. Use Rust 1.95.0:

```sh
cargo build --release --offline --locked
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
```

Rust dependencies can be built from the accompanying vendored archive without registry access. Optional Python/container tooling is described in source documentation. The runtime includes AGPL `LICENSE`, Rust notices, htmx's license and SQLite public-domain notice. Imported manuals, databases, secrets, full-text corpora and rendered source pages are not release assets. Separate CI-only smoke fixtures are synthetic and are not promoted as release content.

## Required repository configuration

Make the exact GitHub Actions status **`deployable`** required in the default/release branch ruleset, require reviews, and prevent bypasses for routine changes. YAML alone cannot enforce branch protection. Publish/promote only artifacts from a successful `deployable` run for the same revision; do not rebuild a different binary after the gate.

The final gate runs after failed/skipped/cancelled prerequisites and requires checks,
artifact-smoke, browser-smoke and lan-smoke all to succeed. The LAN job uses isolated
internal container networks and separate clients, including a real denied subnet and
configured HTTPS proxy. Whole-workflow cancellation is not green. This workspace
currently has no Git remote/active hosted CI; local equivalents are verification
evidence, not a fabricated hosted green run.
