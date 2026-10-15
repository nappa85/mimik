# HTTP authentication/session fit — 2026-10-13

## Recommendation and review boundary

**Candidate GO for independent selection review: released `tower-sessions =0.15.0`
with the existing RustCrypto `argon2 =0.6.0` account backend and a narrow
Mimik account-ID/epoch binding.** Session IDs, cookie handling, record lifecycle,
rotation and expiry remain library-owned. Prefer this released abstraction over
an unreleased Git auth dependency. This is research evidence, not implementation
approval or a deployability claim.

The existing Stage-B wording in `auth-campaign-plan.md` says authentication remains
library-owned. **The recommended candidate explicitly revises that scope:** Mimik
would own the small authenticated-account reconstruction/binding boundary using
its existing account verifier and account state; `tower-sessions` owns the session
protocol. Do not silently treat this as an `axum-login` integration or introduce
custom cookie parsing, bearer-ID generation, cryptography or a second SQLite store.
The coordinating main agent must obtain independent approval of that scope change
and a concrete integration/persistence plan before application work.

Research/isolated-probe scope was approved by independent plan review
`ses_eca913472` (as supplied by the coordinating request). Read `TODO.md`,
`AGENTS.md`, `auth-library-fit.md` and `auth-campaign-plan.md`. Owned writes are
this report and `target/auth-session-fit-20261013/**` only. TODO/review-record
updates belong to the coordinator. Fresh coordinating reviewer accepted this
research/selection and explicitly revised account-binding scope; amended Stage-B
plan subsequently received GO. Implementation/artifact evidence is separate in
`reviews/2026-10-13-http-auth.md` and `http-auth.md`.

| Candidate | Connected result | Selection disposition |
| --- | --- | --- |
| Registry `axum-login =0.18.0` + sessions `=0.14.0` | Request-path checks pass | **No GO recommended:** published core 0.14 lacks #254. Passing functional probes/audit do not establish safety of omitting the known upstream fix. |
| Git axum-login `151c72d7a1b4646830f86b4332e6bd6e34d719a7` + registry sessions `=0.15.0` | Request-path checks pass using the commit's exact changed API | **Conditional GO alternative:** preserves auth reconstruction abstraction, but accepts unreleased API/source and release/vendoring/maintenance cost. No patch to stable 0.18.0 was made. |
| Registry sessions `=0.15.0` + current Argon2 + Mimik ID/epoch binding | Request-path checks pass | **Preferred candidate GO**, conditional on explicit scope revision and review of Mimik's small binding, account/write races and persistence boundary. |

The registry 0.18/0.15 type mismatch and SQLite-store native-link conflicts remain
as established in `auth-library-fit.md`. No new store or duplicate SQLite graph
was built here. These probes contain no SQLite dependency at all.

## Published artifacts, exact commits, license and maintenance

Retrieved 2026-10-13 from the public registry and GitHub. Registry responses still
identify newest stable releases as axum-login **0.18.0**, tower-sessions and
tower-sessions-core **0.15.0**, Argon2 **0.6.0**. Tarballs were downloaded from
`https://static.crates.io/crates/<crate>/<crate>-<version>.crate`, checked against
the version API SHA-256, unpacked, and inspected. `sources/` preserves the registry
responses, archives, packaged sources/manifests and upstream snapshots;
`source-summary.json` records checksums and publication commits.

| Artifact | Registry publication UTC | `.cargo_vcs_info.json` publication commit |
| --- | --- | --- |
| axum-login 0.18.0 | 2025-07-20T15:23:10.473411Z | `93ec03235d3e8eecf0ca9fec9d24840903541caa` |
| sessions 0.14.0 | 2025-01-01T17:45:12.321985Z | `b58463dc93fac49669e33f4b98af5455f0f332c2` |
| sessions-core 0.14.0 | 2025-01-01T17:44:43.374622Z | same `b58463dc…` |
| sessions 0.15.0 | 2026-02-01T01:52:39.750984Z | `92ad0268f4948a026a287979a5f1545f04d4a457` |
| sessions-core 0.15.0 | 2026-02-01T01:52:24.509243Z | same `92ad0268…` |
| argon2 0.6.0 | 2026-08-27T16:54:18.571817Z | `b1e0ad6fe229b1ba74e4696c7359ab45d7e931f0` |

Primary version sources: [axum-login registry](https://crates.io/api/v1/crates/axum-login/0.18.0),
[sessions 0.14](https://crates.io/api/v1/crates/tower-sessions/0.14.0),
[sessions 0.15](https://crates.io/api/v1/crates/tower-sessions/0.15.0),
[core 0.14](https://crates.io/api/v1/crates/tower-sessions-core/0.14.0),
[core 0.15](https://crates.io/api/v1/crates/tower-sessions-core/0.15.0),
[argon2](https://crates.io/api/v1/crates/argon2/0.6.0).

`axum-login`'s **packaged** 0.18 manifest requires sessions `^0.14.0`. Its tarball
omits the repository-root LICENSE: the exact publication-commit
[MIT LICENSE](https://github.com/maxcountryman/axum-login/blob/93ec03235d3e8eecf0ca9fec9d24840903541caa/LICENSE)
was fetched and read, rather than substituting HEAD or assuming no license.
Sessions' top-level archive includes LICENSE; core's workspace artifact/license
is supported by its exact publication-commit root license. Read exact MIT texts
for both generations and the Git candidate. Copyright is 2023 Max Countryman for
axum-login, 2022 Max Countryman for sessions. Argon2's packaged and publication
`argon2/LICENSE-MIT` is RustCrypto copyright 2021–2026; MIT is an available choice
from MIT OR Apache-2.0. Retain applicable notices under the application AGPL scope.
License/diff SHA-256 evidence: `source-license-diff-checksums.json`.

Both auth/session repositories remain unarchived. Auth repo `pushed_at` is
2026-06-19T09:12:29Z, HEAD `151c72d7a1b4646830f86b4332e6bd6e34d719a7`;
sessions repo `pushed_at` is 2026-08-05T14:47:45Z, HEAD
`28c6d21dec5a7718855be398e239b44293d4c359`. These are activity signals, not support
SLAs. HEAD is explicitly distinct from published artifacts.

### Exact Git alternative and costs

Use `git = "https://github.com/maxcountryman/axum-login"`,
`rev = "151c72d7a1b4646830f86b4332e6bd6e34d719a7"`, default features off,
with registry sessions `=0.15.0`. The pinned commit is
[“Upgrade tower-sessions to 0.15.0 (#315)”](https://github.com/maxcountryman/axum-login/commit/151c72d7a1b4646830f86b4332e6bd6e34d719a7).
Its manifest still says version **0.18.0**; the Cargo Git source/revision is what
distinguishes it from registry 0.18.0. Never label it a published compatible release.

The preserved release-to-HEAD comparison lists 13 commits, including thread-safe
AuthSession state (`375856753e0fa7480f3e07013488c9fe678c2f65`) and the new require
builder API (`6e2581e25bb5fc46ed92914b77371c878c32d30e`). AuthSession state is now
private behind `Arc<tokio::sync::Mutex<_>>`; `user().await` replaces the public
user field, and login/logout use `&self`. Explicit rotation uses a separate
library `Session` extractor: there is no public `auth.session` field. The initial
stable-API probe failed with E0609/E0615 (`upstream15-run.log`); the exact-API
follow-up passes. This is an API-boundary regression probe, not a stable-library
patch. The resolved source, pinned manifest, LICENSE and comparison are retained.

Costs: network Git source acquisition on first resolution, a full pinned checkout
unless vendored, provenance distinguished from registry checksums, inclusion in
allowlisted source/vendor archives and notices, explicit offline-rebuild gates,
and later migration/re-review when an actual compatible release is published.
The MIT LICENSE is unchanged in bytes at the two inspected commits, but that is
not evidence that the API/behavior is unchanged. Vendoring/offline artifact builds
were **not** tested here. A Git advisory scan is not review of unpublished changes.

## #254: exact fix and what it does not fix

Primary sources: [PR #254](https://github.com/maxcountryman/tower-sessions/pull/254),
its retained `.diff` and API file list, and the **checksum-verified released**
`tower-sessions-core-{0.14.0,0.15.0}/src/session.rs` comparison. PR merged
2026-02-01T01:42:17Z, merge commit
`3cd7a259729d75629e95fd7ad4fd6e4e1ac2ce58`. Author's stated rationale: effects
must precede the release-write to be ordered before an acquire-load that observes
the updated `is_modified`.

The entire relevant PR change is in `insert_value` and `remove_value`:

```rust
// 0.14 insert_value, while holding the record mutex:
is_modified.store(true, Ordering::Release);
record_guard.data.insert(key.to_string(), value)

// 0.15:
let previous = record_guard.data.insert(key.to_string(), value);
is_modified.store(true, Ordering::Release);
previous

// 0.14 remove_value:
is_modified.store(true, Ordering::Release);
Ok(record_guard.data.remove(key))

// 0.15:
let previous = record_guard.data.remove(key);
is_modified.store(true, Ordering::Release);
Ok(previous)
```

`is_modified()` uses an Acquire load. The response service samples that flag,
then awaits `is_empty()` and conditionally saves the session. 0.14's flag release
therefore does not publish the **following** data mutation via that atomic;
0.15's ordering does. The record mutex also synchronizes access: this report does
not claim Rust memory unsafety, an observed auth bypass, or that every normal
awaited handler loses data on 0.14. The functional probes do not force weak-memory
interleavings and do not reproduce #254 dynamically. The exact source defect/fix
is established; a source-level ordering comparison is not a failing-first runtime
regression. No unfounded “safe to downgrade” conclusion is drawn from that gap.

The released session-file diff contains precisely these two changes plus a mutex
guard lifetime annotation and `rand::thread_rng().gen()` → `rand::rng().random()`.
Core `src/lib.rs` is identical between these archives. Evidence:
`sources/released14-15-src-session.rs.diff` and `sources/pr254.diff`.

**Separate requests still lose updates in 0.15.** Each request constructs its own
Session/record; a shared store does not give record updates a database revision/CAS
or shared cross-request mutex. The upstream
[concurrent-request warning](https://docs.rs/tower-sessions/0.15.0/tower_sessions/#data-races-under-concurrent-conditions)
still applies. `common/lost.rs` demonstrates this on the actual library request
path: initialize counter=0, barrier-synchronize two requests after both load it,
both insert 1 and return success, then GET observes 1 rather than 2. This is
observed for all three candidates, including 0.15. The retained probe asserts the
known limitation; it is not a green application regression or a fix. Authoritative
accounts, epochs, grants, sheet revisions and history must remain transactional
domain data, not mutable session authority.

## Connected request evidence and honest limits

The probes invoke actual Axum/Tower request services using `Router::oneshot`,
library-generated Set-Cookie IDs and a library MemoryStore. No TCP listener or
browser is involved. The test harness carries only the response cookie's first
name/value pair back as a request Cookie header; application cookie handling is
entirely library middleware. `common/account.rs` makes a dummy PHC with current
Argon2 0.6 defaults and verifies submitted dummy passwords on a blocking worker.
This is not the application's production account admission/worker implementation.

`common/library.rs` runs axum-login's `authenticate`, `login`, per-request
`get_user` reconstruction/hash comparison and `logout`; explicit library
`Session::cycle_id()` runs for every successful login. `common/direct.rs` stores
only a typed `{id, epoch}` binding in library session data, obtains the current
enabled account for each protected request, rejects absent/disabled/mismatched
accounts and flushes invalid sessions, and uses library rotation/logout APIs.
Epoch is nonsecret positive integer invalidation metadata; equality here is not
password or bearer-secret comparison. Both use the same Argon2 dummy backend.

All three final runs assert:

- anonymous and incorrect-password rejection;
- successful password authentication/login, new ID after anonymous seed;
- next GET reconstructs auth with a fresh current-account lookup (counter checked);
- explicit repeat-login rotation, old-ID rejection and new-ID acceptance;
- logout and replay rejection;
- enabled-account epoch change rejection;
- disabled-account session and password-login rejection;
- re-enable at the changed epoch does not resurrect the old session.

These checks do not verify SQLite persistence/restart, expiry/cleanup, account
switches, missing/deleted accounts, malformed records, store errors, account
disable/reset racing KDF/session-save/domain-write boundaries, cancellation,
CSRF/origin checks, throttling, browser cookie/TLS behavior, LAN deployment or
production release artifacts. Library `get_user(None)` does not automatically
flush axum-login auth data; epoch advancement prevents revival in the tested
re-enable scenario. Direct binding flushes rejected data explicitly.

The dummy backend fixes one account ID and accepts only generated hashes; real
integration must reuse Stage A's bounded PHC admission, secret-free projections,
Zeroizing password ownership and nonqueued worker permit. The probe's dummy User
Debug/PHC storage is not a production account projection or logging model.

## Feature, lock, graph and license/advisory measurements

All probes pin Axum `=0.8.9` (current app lock version), Argon2 `=0.6.0` with
`zeroize` plus its normal alloc/password-hash/getrandom defaults. Sessions disable
defaults and explicitly enable `axum-core,memory-store`; axum-login disables its
optional macro/require-builder defaults. No signed/private cookie features,
session deletion task, SQLx, rusqlite, Rayon or new crypto backend was enabled.
Probe-only upstream-api feature selects the exact Git API, not a dependency patch.
Shipping persistence selection must remeasure after removing memory-store.

| Isolated graph | All-target metadata incl. root | Linux normal/build incl. root | Third-party Linux entries |
| --- | ---: | ---: | ---: |
| registry login 0.18/sessions 0.14 | 108 | 101 | 100 |
| pinned Git login/sessions 0.15 | 111 | 101 | 100 |
| direct released sessions 0.15 | 108 | 98 | 97 |

Counts deduplicate package/version/source tree entries; they are **not application
incremental costs or binary/RSS measurements**. Git and registry login have the
same declared version but different source identities. `graph-summary.json`
records lock SHA-256; each candidate retains Cargo.lock, final metadata, feature
tree, Linux normal/build tree, license inventory and available license-file copies.
No application lock was modified or combined application graph claimed.

All three `cargo audit --json` runs exit0 with **0 vulnerabilities, no warnings**;
RustSec DB commit `64a0302dc836df15a10ac27ebdcdf8f9b4ce845a`, 1309 advisories,
last update 2026-10-13T14:18:24+02:00. #254 is still a source concern even though
0.14 has no reported advisory. All three `cargo about generate --locked` runs
exit0, empty stderr, using isolated accepted-license/target configuration matching
the application's generic accepted set; notices are retained in `*-notices.log`.
No exclusion/clarification was added. MIT alternatives avoid LGPL; mandatory
Apache/BSD/Unicode notices remain represented. This is tool-generated probe graph
licensing evidence, not final integrated-distribution review.

## Reproduction and handoff

Existing image `mimik-publication-ci:20261013`; only the probe bind and public
registry-cache volume are mounted. Parent target directory was inspected before
creation. No host tool installation, private data/manual/credential mount or
Docker build context. Git acquisition is public and container-local.

```sh
docker run --rm \
  --mount type=bind,src=/home/marco/Progetti/mimik/target/auth-session-fit-20261013,dst=/probe \
  --mount type=volume,src=mimik-auth-fit-cargo-20261013,dst=/opt/cargo/registry \
  --workdir /probe mimik-publication-ci:20261013 python3 /probe/checks.py
```

Scripts: `research.py` (registry/archive/license/PR source), `upstream-source.py`
(pinned auth source/comparison), `run.py` (initial probes), `evidence.py` (API-correct
follow-up and counts/license inventory), `checks.py` (final connected/lost-update
checks, notices, audit), `summarize.py` (advisory/commit/hash summaries). Source
research reruns query current HEAD separately; manifests stay exact-pinned.

Final `check-results.json`: all nine connected/notices/audit commands exit0.
Connected commands took approximately 5.825s / 6.662s / 5.806s respectively,
including incremental debug compilation and dummy KDF work; these are not latency
or memory budgets. Initial Git API mismatch is retained, followed by successful
exact-API runs. No application tests, strict Clippy/format gates, release build,
offline source rebuild or browser checks were run for this research slice.

Coordinator handoff:

- [x] Verify current published artifacts versus distinct HEAD; exact sources/license evidence.
- [x] Establish released 0.14/0.15 #254 diff and distinguish cross-request lost updates.
- [x] Exercise all three connected request paths with dummy accounts/current Argon2.
- [x] Capture exact locks, features, normal counts, notices and advisory results.
- [ ] Obtain independent final research review; record findings/disposition in docs/reviews and TODO.
- [ ] Select preferred direct candidate with explicit authentication-ownership revision, or accept the exact Git alternative's unreleased-source cost.
- [ ] Independently review concrete Stage-B account-binding/HTTP and narrow existing-Store SessionStore persistence plan before implementation.

That next plan needs typed secret-free current-account context, fail-closed
reconstruction and storage errors, explicit all-login/account-switch rotations,
bounded/expiry-filtered/collision-safe records and observed cleanup, plus a
transactional fresh enabled/epoch/grant check at every protected mutation. Account
verification completing before disable must not make subsequent stale session
authority acceptable; in-flight reads/writes need a documented boundary. Retain
failing-first regressions for discovered implementation issues and tests for
save/create/delete failures, revocation races, logout replay, expiry, corruption,
restart and supported HTTP/TLS/CSRF/origin configuration. Existing SQLite adapter
research justifies a narrow library trait adapter, not a duplicate store or custom
session protocol. These are remaining plan obligations, not work executed here.
