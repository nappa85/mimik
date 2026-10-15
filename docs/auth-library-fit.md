# Authentication library fit — 2026-10-13

## Selection and blockers

**The requested pinned combination is not integration-compatible.** Preserve application `rusqlite 0.37.0` with `bundled`; do not upgrade it as part of auth selection.

| Candidate | Observed result | Decision |
| --- | --- | --- |
| `tower-sessions-rusqlite-store =0.15.0` + application `rusqlite =0.37.0` | Cargo resolution fails: `libsqlite3-sys 0.38.2` versus `0.35.0`, both `links = "sqlite3"` | Reject this store for the current application graph. Maintained does not imply compatible. |
| `axum-login =0.18.0` + `tower-sessions =0.15.0` | Both crates coexist and compile separately, but connecting `SessionManagerLayer` to `AuthManagerLayerBuilder::new` fails with E0308 | **No GO for this exact pair.** A session storage adapter cannot fix this version/type mismatch. |
| `axum-login =0.18.0` + `tower-sessions =0.14.0` + application SQLite + crypto candidates | Locked build and construction of the connected auth/session layers pass | Demonstrated compatible boundary, **not an approved fallback release selection**. Review the 0.15 memory-ordering fix before considering 0.14. |
| Published `tower-sessions-sqlx-store =0.15.0`, `sqlite` enabled, + application SQLite | Cargo resolution fails with SQLite native-link conflict | Reject as a drop-in alternative. It also implements sessions **core 0.14**, despite its own 0.15 version number. |
| `argon2 =0.6.0`, `getrandom ="0.4"`, `zeroize ="1"` | Resolve/build with current SQLite; resolved versions 0.6.0 / 0.4.3 / 1.9.1 | Suitable first-account-foundation candidates for independent review; established crypto, no bespoke crypto implementation. |

**Conditional later direction:** use established `axum-login`/`tower-sessions` APIs after choosing a mutually supported release pair. The demonstrated native-link conflicts justify considering a **narrow implementation of the library's `SessionStore` trait over Mimik's existing Store/SQLite boundary**. It would supply persistence only, not a new auth/session/cookie protocol. No adapter was implemented or approved here. Using 0.15 requires a compatible auth release or separately reviewed upstream dependency change; silently patching 0.18.0 is not the pinned release tested here.

Scope: public library/source research and isolated probes only. `TODO.md` and `AGENTS.md` were read. Research/probe plan was independently approved; coordinating fresh reviewer `ses_ecb23dee0ffeSmwUooH8laKP7n` subsequently accepted the report/evidence (review record in `reviews/2026-10-13-auth-foundation.md`). This research did not change application dependencies/code/database or credentials, and does not mark session integration production-ready. Subsequent Stage-A crypto integration is documented separately in `accounts.md`.

## Exact sources, release dates, maintenance

Retrieved **2026-10-13**. Dates below are registry `created_at` (publication timestamps, UTC), not inferred from GitHub activity. Version JSON, checksum-verified registry tarballs, and upstream snapshots are retained under the probe's `sources/` directory.

| Release | Publication timestamp | Source and license |
| --- | --- | --- |
| axum-login 0.18.0 | 2025-07-20T15:23:10.473411Z | [Registry](https://crates.io/api/v1/crates/axum-login/0.18.0); [release manifest](https://docs.rs/crate/axum-login/0.18.0/source/Cargo.toml); [MIT text at publication commit](https://github.com/maxcountryman/axum-login/blob/93ec03235d3e8eecf0ca9fec9d24840903541caa/LICENSE) |
| tower-sessions 0.15.0 | 2026-02-01T01:52:39.750984Z | [Registry](https://crates.io/api/v1/crates/tower-sessions/0.15.0); [release manifest](https://docs.rs/crate/tower-sessions/0.15.0/source/Cargo.toml); [MIT text](https://github.com/maxcountryman/tower-sessions/blob/92ad0268f4948a026a287979a5f1545f04d4a457/LICENSE) |
| tower-sessions-core 0.15.0 | 2026-02-01T01:52:24.509243Z | [Registry](https://crates.io/api/v1/crates/tower-sessions-core/0.15.0); same publication commit/MIT as tower-sessions 0.15.0 |
| tower-sessions 0.14.0, compatibility control | 2025-01-01T17:45:12.321985Z | [Registry](https://crates.io/api/v1/crates/tower-sessions/0.14.0); [release APIs](https://docs.rs/tower-sessions/0.14.0/tower_sessions/); upstream publication commit `b58463dc93fac49669e33f4b98af5455f0f332c2` |
| patte rusqlite-store 0.15.0 | 2026-10-04T17:28:54.689253Z | [Registry](https://crates.io/api/v1/crates/tower-sessions-rusqlite-store/0.15.0); **[primary GitHub rusqlite-store directory](https://github.com/patte/tower-sessions-rusqlite-store/tree/bf9fe85308023f23290d6c22af26eeb24d482796/rusqlite-store)**; [manifest](https://github.com/patte/tower-sessions-rusqlite-store/blob/bf9fe85308023f23290d6c22af26eeb24d482796/rusqlite-store/Cargo.toml); [MIT text](https://github.com/patte/tower-sessions-rusqlite-store/blob/bf9fe85308023f23290d6c22af26eeb24d482796/LICENSE) |
| SQLx store 0.15.0 | 2025-01-01T18:21:00.197933Z | [Registry](https://crates.io/api/v1/crates/tower-sessions-sqlx-store/0.15.0); [published manifest](https://docs.rs/crate/tower-sessions-sqlx-store/0.15.0/source/Cargo.toml); [MIT text](https://github.com/maxcountryman/tower-sessions-stores/blob/b34a2f363217c0c557ee332c8847f4e2d1b5e6b4/LICENSE) |
| argon2 0.6.0 | 2026-08-27T16:54:18.571817Z | [Registry](https://crates.io/api/v1/crates/argon2/0.6.0); [release source/license files](https://docs.rs/crate/argon2/0.6.0/source/); MIT OR Apache-2.0; Rust 1.85 minimum |
| getrandom 0.4.3, resolved from 0.4 | 2026-06-17T18:01:59.979552Z | [Registry](https://crates.io/api/v1/crates/getrandom/0.4.3); [release source/license files](https://docs.rs/crate/getrandom/0.4.3/source/); MIT OR Apache-2.0; Rust 1.85 minimum |
| zeroize 1.9.1, resolved from 1 | 2026-10-06T17:49:21.148259Z | [Registry](https://crates.io/api/v1/crates/zeroize/1.9.1); [release source/license files](https://docs.rs/crate/zeroize/1.9.1/source/); Apache-2.0 OR MIT; Rust 1.85 minimum |

GitHub API snapshots show all four auth/session/store repositories **unarchived**, with these `pushed_at` values and default-branch HEADs:

| Repository | `pushed_at` | HEAD |
| --- | --- | --- |
| maxcountryman/axum-login | 2026-06-19T09:12:29Z | `151c72d7a1b4646830f86b4332e6bd6e34d719a7` |
| maxcountryman/tower-sessions | 2026-08-05T14:47:45Z | `28c6d21dec5a7718855be398e239b44293d4c359` |
| patte/tower-sessions-rusqlite-store | 2026-10-04T17:27:36Z | `bf9fe85308023f23290d6c22af26eeb24d482796` |
| maxcountryman/tower-sessions-stores | 2026-09-14T19:53:15Z | `d18c9bf76f1d4fb73130dbe5aa643197f14b5d2d` |

These are maintenance/activity signals, not response-time guarantees or security certification. The patte repository and its recent release are the relevant maintained rusqlite-store source, not an assumed directory in maxcountryman's stores repository. The other store repository's [current SQLx manifest](https://github.com/maxcountryman/tower-sessions-stores/blob/d18c9bf76f1d4fb73130dbe5aa643197f14b5d2d/sqlx-store/Cargo.toml) already names SQLx 0.9 and sessions-core 0.15 while still declaring store version 0.15.0: **that is not the published 0.15.0 artifact**, which depends on SQLx 0.8/core 0.14. Do not substitute repository HEAD for a pinned registry release. Non-SQLite SQLx backends could avoid SQLite links, but add a separate database deployment and have not been probed/selected.

## API fit: current user, epoch invalidation, rotation, persistence

### axum-login 0.18.0 — source inspection

Exact [backend source](https://github.com/maxcountryman/axum-login/blob/93ec03235d3e8eecf0ca9fec9d24840903541caa/axum-login/src/backend.rs) and [session source](https://github.com/maxcountryman/axum-login/blob/93ec03235d3e8eecf0ca9fec9d24840903541caa/axum-login/src/session.rs); identical packaged files are `sources/axum-login-0.18.0/src/{backend,session}.rs`. Public [AuthnBackend](https://docs.rs/axum-login/0.18.0/axum_login/trait.AuthnBackend.html), [AuthUser](https://docs.rs/axum-login/0.18.0/axum_login/trait.AuthUser.html), and [AuthSession](https://docs.rs/axum-login/0.18.0/axum_login/struct.AuthSession.html) API references are versioned.

- `AuthnBackend::authenticate(credentials)` and `get_user(&UserId)` return an async `Result<Option<User>, Error>`. Credentials/password policy and application account storage are supplied by Mimik, not by the middleware.
- On each auth session reconstruction with a recorded ID, `from_session` calls `get_user` (session.rs:146–170). It does not trust a user snapshot serialized at login. Returning `None` for a disabled/missing user means no authenticated user; that branch **does not itself flush** the saved session. Re-enabling an account without changing its session hash could therefore revive that old session.
- `AuthUser::session_auth_hash() -> &[u8]` is stored at login and compared against the freshly fetched user's bytes using `subtle::ConstantTimeEq`; mismatch clears auth data and calls `Session::flush`. It accepts an application-provided epoch representation; no new hashing algorithm is needed. Candidate: persisted account epoch, exposed as fixed-width bytes in the current user, increased transactionally on disable/reset/password change. This is invalidation metadata, not the password verifier or a bearer credential.
- Global epoch revocation becomes effective when a subsequent request reconstructs auth state. It does **not** cancel an already-running request or provide transactional authorization for a write racing account disable. Protected writes will need fresh account/grant validation at their mutation boundary; do not store authoritative campaign grants in session data.
- `login` cycles the underlying ID only when the prior auth hash is absent (session.rs:112–125). It is not an unconditional rotation API for authenticated account switches/privilege changes. `authenticate` alone does not log in. `logout` flushes the session (128–139).
- `AuthzBackend` can expose typed permissions, but campaigns/resources and creator/player/Owner distinctions remain application domain decisions. Generic permission middleware cannot establish campaign-specific sheet access by itself.
- The [published manifest](https://docs.rs/crate/axum-login/0.18.0/source/Cargo.toml) requires `tower-sessions ^0.14.0`, default features off, and Axum `^0.8.1`. The probe used Axum **0.8.9**, matching the application's current lockfile. Sessions 0.14 and 0.15 structs/traits are distinct Rust types.

### tower-sessions 0.15.0 and the compatible 0.14 boundary — source inspection

Versioned [Session](https://docs.rs/tower-sessions/0.15.0/tower_sessions/struct.Session.html), [SessionManagerLayer](https://docs.rs/tower-sessions/0.15.0/tower_sessions/struct.SessionManagerLayer.html), [SessionStore](https://docs.rs/tower-sessions/0.15.0/tower_sessions/trait.SessionStore.html), and [core source](https://docs.rs/crate/tower-sessions-core/0.15.0/source/src/session.rs). The same named persistence/rotation/configuration APIs exist in the inspected packaged 0.14 sources; do not interpret shared names as cross-version type compatibility.

- Cookie holds the library-generated session ID; data remains server-side in a pluggable store. `insert/get/remove`, `save/load/delete`, `flush`, `cycle_id`, and `set_expiry` cover the needed primitives. `cycle_id` retains data, removes the old store ID, sets the ID to `None` and marks modified; `save` then invokes `create` for a fresh ID. It is not an atomic, transactionally coupled account mutation.
- Store interface: `create(&mut Record)`, `save(&Record)`, `load(&Id) -> Option<Record>`, `delete(&Id)`. `Record` carries ID/data/expiry. Collision-safe `create` must be implemented: the default delegates to `save` and warns, providing no collision protection. `load` must reject expired/invalid sessions, and backend failures must remain errors. `ExpiredDeletion` adds bounded periodic cleanup; deletion task failures need observation.
- Layer builders configure cookie name, HttpOnly, SameSite, secure, path/domain, expiry and always-save. Inspected defaults: HttpOnly **true**, SameSite **Strict**, Secure **true**, path `/`, no Domain/explicit expiry, always-save **false**. Local HTTP and deployed HTTPS must have an explicit reviewed configuration rather than relying on browser-specific localhost behavior.
- `Expiry::OnSessionEnd`, `OnInactivity(Duration)` and `AtDateTime` distinguish browser-session, sliding and fixed expiry. Persistent cookies plus persistent server storage are both required for restart/remembered-session behavior. `with_always_save(true)` updates inactivity expiry on read-only requests, at the cost of writes. Cookie lifetime alone does not revoke server records.
- Response middleware saves modified/nonempty sessions or always-save sessions except on server-error responses, emits the new cookie, and returns 500 on save failures (service.rs:225–273). Auth/database changes are not automatically rolled back with session persistence failure. Explicit session saves and error handling will need integration tests.
- Optional `signed`/`private` features use maintained `tower-cookies` controllers. They were **not enabled/tested**; signing/encryption and key rotation are not implied by `cycle_id` (ID rotation). Plaintext here means the opaque ID cookie is unencrypted, not that session data/passwords are placed in the cookie.
- Upstream [concurrent-session warning](https://docs.rs/tower-sessions/0.15.0/tower_sessions/#data-races-under-concurrent-conditions) documents lost updates between requests. Session data is not appropriate for authoritative account/campaign state. The [0.15 changelog](https://github.com/maxcountryman/tower-sessions/blob/92ad0268f4948a026a287979a5f1545f04d4a457/CHANGELOG.md) additionally says **“Fix memory ordering race. #254”**. A passing 0.14 constructor probe does not settle that fix's impact or approve reverting to 0.14.

### Maintained rusqlite store — source inspection and demonstrated conflict

The patte [implementation](https://github.com/patte/tower-sessions-rusqlite-store/blob/bf9fe85308023f23290d6c22af26eeb24d482796/rusqlite-store/src/lib.rs) implements sessions-core **0.15**. It provides `RusqliteStore::new(tokio_rusqlite::Connection)`, optional validated table name, `migrate`, expiry filtering, deletion and MessagePack record serialization. `create` transactionally checks collisions and writes the record, updating the caller's ID. `tokio-rusqlite` provides asynchronous access to its connection; this is a separate connection-management dependency/boundary from Mimik's existing storage.

Its primary manifest requires `rusqlite ^0.40.2`, `tokio-rusqlite ^0.8.0`, `rmp-serde`, `async-trait`, `time`, `thiserror`, and core's `deletion-task` feature. Its own library has no feature flags; **bundled is enabled in its dev/example dependency, not its normal dependency**. Native SQLite/build portability therefore needs deliberate feature unification, but turning bundled off cannot solve the conflicting native `links` identities. The minimal app-version/store manifest fails before compilation. This remains a useful maintained reference for a future storage-trait implementation, not a code-copy authorization or evidence of current deployability.

## First-account crypto candidates, features and cost

- `argon2 0.6.0` is RustCrypto's maintained Rust implementation, default Argon2id v19, with PHC serialization and verification APIs. [Exact 0.6 usage](https://docs.rs/argon2/0.6.0/argon2/#password-hashing) uses `password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash}` and `hash_password(password)` with generated salt; do not import a remembered 0.5 example/API. Default features are `alloc`, `getrandom`, `password-hash`; `parallel`/Rayon and `kdf` were not enabled. Default parameters in `src/params.rs` are **19 × 1024 KiB memory, 2 iterations, 1 lane**. This is source configuration, not a measured RSS/time bound; input PHC parameters influence verification cost. Later admission, bounded concurrency and a blocking-work boundary need review/measurement.
- `argon2`'s optional `zeroize` feature was **not enabled** in this baseline. Merely depending on `zeroize` does not enable it or wipe every password copy. A later integration should decide that feature and use established `Zeroizing`/`Zeroize` APIs at actual ownership boundaries; remeasure the enabled graph. The probe only constructed Argon2, obtained OS random bytes in a dummy buffer and cleared that buffer, without exercising hashing or password-policy logic.
- `getrandom 0.4` resolved to **0.4.3**, with its default system backend; no `wasm_js` or custom RNG backend was enabled. Versioned [platform/backend source](https://docs.rs/crate/getrandom/0.4.3/source/src/lib.rs) documents target-specific OS sources and errors. This candidate is appropriate for direct invite/reset token entropy, subject to later application design; randomness errors must fail closed. No invite/reset protocol was built.
- `zeroize 1` resolved to **1.9.1**, default `alloc`, without derive macros. [API/source](https://docs.rs/zeroize/1.9.1/zeroize/) uses volatile writes/compiler fences; clearing one allocation does not erase clones, logs, swap or serialized data.
- Portability is **source-inspected**, not cross-platform verified: RustCrypto Argon2 is `no_std` with optional allocation and requires 32/64-bit targets; zeroize supports no-std allocation configurations; getrandom depends on supported OS entropy backends. Native bundled SQLite requires a working target C toolchain. Only **x86_64-unknown-linux-gnu with Rust 1.95.0** was built/run. No Windows/macOS/WASM, network sharing, browser cookie behavior, memory/time budget or production artifact was verified.

## License fit and dependency graph measurements

The four library/store upstream LICENSE texts were read at their **publication commits**, not merely inferred from registry SPDX fields. They are standard MIT texts (axum-login copyright 2023 Max Countryman; sessions 2022 Max Countryman; patte store 2024 Patrick Recher; SQLx store 2024 Max Countryman). Their permission/copyright notices must be retained. MIT is compatible with the application's AGPL-3.0-only distribution terms; it does not change the application's own license.

MIT texts were also read for the actual resolved Argon2/getrandom/zeroize versions from `license-evidence/`. Each offers MIT as an alternative to Apache-2.0: select MIT and retain the relevant notices, consistent with existing project licensing practice. Upstream library license hashes/paths are in `upstream-license-checksums.json`; crypto license copies are retained separately in `license-evidence/` (that inventory does not hash them). Upstream API responses identify exact license URLs. Workspace registry archives omit some root LICENSE files, so the probe fetched their upstream publication-commit copies instead of treating omission as no license.

Measured isolated graphs, **not incremental application/shipping costs**:

| Graph | Count and meaning |
| --- | --- |
| Coexistence-only 0.18/0.15 + SQLite/crypto | **129** metadata package/version entries including probe root and target-specific packages; includes sessions/core 0.14 **and** 0.15. It is not a valid integration graph. |
| Connected 0.18/0.14 + SQLite/crypto | **120** all-target metadata entries including root (**119 third-party package/version entries**); **114** unique Linux normal/build package/version entries including root (**113 third-party**). |

Evidence: `compatible-metadata.json`, `graph-summary.json`, `aligned14-metadata.json`, `count-summary.json`, `aligned14-linux-tree.log`, feature logs, `aligned14-license-inventory.json`, and `license-file-inventory.json`. Counts deduplicate package/version identities, not distinct crate names, and are not compiled binary size measurements. The compatible control retains just `libsqlite3-sys 0.35.0`/`rusqlite 0.37.0`. Multiple getrandom generations remain due to upstream RNG dependencies (0.2.17, 0.3.4 and direct 0.4.3 in the coexistence graph); direct 0.4 does not replace those transitively.

The aligned metadata license-expression set includes MIT, Apache-2.0, BSD-3-Clause, Zlib, `MIT AND BSD-3-Clause`, `(MIT OR Apache-2.0) AND Unicode-3.0`, dual/triple alternatives (including an optional LGPL alternative), `MIT/Apache-2.0`, and Unlicense/MIT. `UNSPECIFIED` is the unpublished probe root, not a third-party dependency. MIT alternatives avoid choosing LGPL. Apache/BSD/Zlib/Unicode mandatory branches require their appropriate notices; these metadata expressions are compatible candidates for an AGPLv3 application, **not a full per-file transitive-license clearance**. The probe copied available root license/notice files but did not run cargo-about, dependency advisory audit, or distribution-notice generation. Actual integrated features/licenses must pass existing project gates; no licensing exclusion is approved here.

## Reproducible container evidence

Parent `target/` was verified with `ls` before probe creation; `git check-ignore target/auth-fit-20261013/probe.py` confirmed ignored status. Only the probe directory was bind-mounted writable. Public crate downloads, generated locks/builds/logs, source evidence and isolated license copies remain beneath it; no app tree, `.env`, manual, data directory or Docker build context was mounted.

Existing image: `mimik-publication-ci:20261013`, image ID `sha256:bbd06698cce7a554118cfe7f0aad34dd3b5919503e6913c9161e6e4c1abfffe8`. Container toolchain evidence: `toolchain.log`, Rust **1.95.0**, LLVM 22.1.2, x86_64 Linux. No host tool/dependency installation occurred.

Runner scripts: `probe.py` (public-source fetch/coexistence/native-link probe), `followup.py` (connected-layer mismatch/control/SQLx probe), `evidence.py` (publication-commit licenses, resolved-source snapshots and Linux graph counts). Run from the repository root, substituting its absolute probe path:

```sh
docker run --rm \
  --mount type=bind,src=/home/marco/Progetti/mimik/target/auth-fit-20261013,dst=/probe \
  --mount type=volume,src=mimik-auth-fit-cargo-20261013,dst=/opt/cargo/registry \
  --workdir /probe mimik-publication-ci:20261013 python3 /probe/probe.py
```

Use the same command with `/probe/followup.py` or `/probe/evidence.py`. The first two actual runs mounted the named cache at `/usr/local/cargo/registry`; logs revealed Cargo actually uses **`/opt/cargo/registry`**, so those mounts did not cache downloads. The evidence run and the reproducible command above use the corrected mount. It holds public registry/cache content only. Build outputs remain in each isolated crate's target directory.

| Actual command / scope | Exit | Evidence |
| --- | --- | --- |
| `cargo build`, rusqlite/store conflict | **101**, expected unsupported graph | `conflict-build.log` — exact 0.38.2 vs 0.35.0 native-link error |
| `cargo generate-lockfile`; `cargo build --locked`, coexistence main | **0 / 0** | `compatible-lock.log`, `compatible-build.log`; build 18.464 s, not an integration success |
| `cargo run --locked`, coexistence constructors/dummy entropy/clear | **0** | `compatible-run.log`; no auth requests or persistent-store behavior tested |
| `cargo build --locked --example layer_mismatch` | **101**, expected unsupported pair | `layer-mismatch.log` — E0308, sessions 0.14 expected versus 0.15 supplied |
| `cargo generate-lockfile`; `cargo build --locked`, connected 0.14 control | **0 / 0** | `aligned14-lock.log`, `aligned14-build.log`; build 17.267 s |
| `cargo run --locked`, connected control | **0** | `aligned14-run.log`; reject-all dummy backend, layer construction only |
| `cargo build`, published SQLx store with SQLite/current app SQLite | **101**, expected unsupported graph | `sqlx-conflict-build.log`; solver diagnostic 0.28.0 vs 0.35.0, not a selected/resolved SQLx graph |
| `cargo metadata --locked --format-version=1`; `cargo tree --locked -e features` | **0** for both successful graphs | metadata and feature logs; `results.json`, `followup-results.json` retain command/cwd/exit/duration |
| `cargo tree --locked --target x86_64-unknown-linux-gnu --prefix none --format '{p}' -e normal,build` | **0** | `aligned14-linux-tree.log`, `count-summary.json` |
| Container `python3 /probe/validate.py` | **0** | `validation.log`; retained expected exit codes/conflict diagnostics, aligned versions/counts, unspecified-license root scope and Python runner syntax passed |

Expected failures are retained reproducible compatibility probes, not hidden failures in a deployable application graph. No application test suite, independent final review, live auth/epoch invalidation, persistent store/restart, password hashing, browser tests, Clippy, release build, audit or production gates were run for this research slice.

## Handoff for independent review

- [x] Inspect maintained primary rusqlite store and published manifests/licenses; retain dated sources.
- [x] Demonstrate native-link conflict without upgrading application SQLite.
- [x] Catch coexistence-versus-integration false positive with a concrete layer connection.
- [x] Verify a connected 0.14 control and compile current first-account crypto candidates; record real feature/count/license inventory.
- [x] Independent adversarial review accepted research evidence; see `reviews/2026-10-13-auth-foundation.md`.
- [ ] Choose a supported auth/session release pair, including disposition of the 0.15 memory-ordering fix; the requested pinned pair remains blocked.
- [ ] Only then review/design a narrow existing-Store persistence trait adapter, justified by observed maintained-store conflicts; implement nothing from that design yet.
- [ ] At integration, retain appropriate regression coverage for chosen version wiring, epoch disable/reset/re-enable behavior, missing users, rotations, expired/colliding records, save failures, restart persistence and revocation/write races, and run actual licensing/audit/release gates.

No custom authentication crypto or session protocol was created. Probe-only findings are evidence for selection decisions, not shipping claims.
