# Stage-B integrated Linux HTTP auth measurements

Measured 2026-10-13 on the shared Ryzen 5 1600 Linux host. Scope is the
approved Stage-B loopback HTTP router and its integrated default-release resource
and dependency graph. One warm sample per case; these are observations, not
worst-case, throughput, concurrency, or hard memory/latency bounds.

## Binary and dependency graph

The measured executable is the **post-fix default release supplied by the main
session**, from Docker volume `mimik-auth-build` at
`/workspace/target/release/mimik`, mounted read-only. It was not rebuilt by this
measurement task. Its size is **16,332,056 bytes**. The preceding account release
at `target/dist-accounts-final-20261013/` was **15,649,304 bytes** (main-session
baseline), giving **+682,752 bytes**, approximately **+4.36%**.

SHA-256 of the executable:
`4e76b815c80017df7330ceed9e32a4c4d27a455a669af77ad6c99f18507902f1`.

The integrated manifest/lockfile graph was resolved offline and locked for
`x86_64-unknown-linux-gnu`, with default application features and no additional
feature switches:

| Count definition | Packages including Mimik | Dependencies excluding Mimik |
| --- | ---: | ---: |
| Unique package/version entries reachable through normal edges | 261 | 260 |
| Unique package/version entries through normal + build edges | 276 | 275 |
| All lockfile package entries, including dev and other targets | 295 | 294 |

These count packages, not compilation units or loaded runtime modules. The two
tree counts deduplicate Cargo's repeated `(*)` entries. The feature tree excludes
dev edges; it is not a count of every entry in the lockfile.

Checks confirm released `tower-sessions=0.15.0` with only `axum-core`, without
MemoryStore or its default feature bundle; `governor=0.10.4` with only `std`;
no `axum-login`; and one `rusqlite=0.37.0` / `libsqlite3-sys=0.35.0` stack.
Rusqlite includes `bundled` and `functions`. The shipping binary and library
fingerprints report application `features=[]`: `auth-test-support` is declared
for tests but absent from shipping. Governor's integrated `std` graph includes
futures, timer and parking-lot dependencies, even though request throttling uses
the direct nonqueued API. Full raw feature/build-fingerprint evidence is below.

Licensing/notices and dependency audit had already passed in the main session,
per the measurement handoff. That is external verification provenance, not a
new audit/license check performed by this task. The existing plan and review
record are `http-auth-plan.md` and `reviews/2026-10-13-http-auth.md`; their current
planning text alone does not establish final implementation or release approval.

## HTTP request samples

Each row is **one timed request**, with a fresh server and fresh dummy database.
HTTP uses `127.0.0.1:3000`, no redirects followed, explicit cookies, matching
Host/Origin, and the existing `auth_request` helper. Time starts immediately
before that helper and ends after the complete response body is received and the
connection closed. Thus it includes loopback connection setup, form encoding
where applicable, transport and response decoding; response assertions are
outside the timer. No keep-alive connection or standalone KDF timing is claimed.

| Timed request | Exact expected outcome | Request latency (ms) | Whole-server-child lifetime peak RSS (KiB) |
| --- | --- | ---: | ---: |
| Warm `GET /auth/login`, existing anonymous cookie | 200, login form, unchanged CSRF, no Set-Cookie | 0.733 | 27,756 |
| Correct `POST /auth/login`, known enabled dummy Owner | 303 to `/auth/session`, rotated session cookie; subsequent status identifies that Owner | 59.202 | 31,844 |
| Password-mismatch `POST /auth/login`, same known login | 401, exact static `Invalid credentials or session.`, no Set-Cookie | 31.805 | 31,572 |
| Known-authenticated `GET /auth/session` | 200, known principal and logout form, no Set-Cookie | 0.792 | 31,652 |
| `POST /auth/logout`, current CSRF/session | 303 to `/auth/login`; subsequent old-cookie status returns exact static 401 | 5.308 | 31,920 |

All measured responses have `Cache-Control: no-store`. Positive form/cookie
checks use the existing helpers, including HttpOnly/SameSite Strict/path/max-age
cookie checks. Failures require the exact expected status and static body, not
just any nonzero status. Every server received SIGINT and exited **0** within
the shutdown deadline. Server logs passed secret-free checks and were discarded;
raw results contain no cookie, CSRF, password or verifier values.

Readiness is an untimed `GET /auth/login` that creates an anonymous session; the
same cookie is then used for an additional untimed warm login-form GET.
Correct-login and mismatch rows time their first credential POST after these
GETs, not a prewarmed in-server KDF. Status/logout rows perform an untimed correct
login and status read first. Correct-login and logout rows also include untimed
postcondition requests after their measured request. The warm login GET performs
no password verification; known status reconstructs identity without KDF.

RSS is Linux `getrusage(RUSAGE_CHILDREN).ru_maxrss`, collected after shutdown by
a **fresh Python wrapper per case**. Each wrapper asserts an initial zero child
peak and launches exactly one child: `mimik --auth`. It performs HTTP requests
itself and launches no CLI, Cargo, or other children. Dummy account bootstrap
and graph commands run in the parent before wrappers. Their resource usage does
not contaminate wrapper child accounting. RSS covers the entire server-child
lifetime, including startup, readiness, setup login where used, postconditions
and shutdown. It is not memory attributable only to the timed request and does
not include the Python wrapper or the container as a whole. The largest sampled
child peak is **31,920 KiB** (about **31.17 MiB**).

Correct/mismatch times differ in this single sample; no constant-time, relative
performance, percentile or worst-case inference follows. Shared hardware was
not isolated and no CPU governor, load, affinity or cold-cache control is claimed.

## Raw evidence and reproduction

Owned local scratch directory: `target/http-auth-profile-20261013/`.

- `profile.py`: parent preparation, offline graph capture and fresh wrappers.
- `samples.json` and each `<case>.json`: unrounded timings, child peak RSS,
  exact-response success, child count, exit code and discarded-log check.
- `normal-tree.txt`, `build-tree.txt`, `features-tree.txt`: Cargo output.
- `graph-summary.json`: counts, executable size/hash, manifest/lock hashes,
  baseline delta and graph exclusions.
- `release-fingerprints.json`, `selected-release-fingerprints.json`: actual
  release application and selected dependency build-feature provenance.
- `<case>/smoke.sqlite3` and associated scratch database files: synthetic-only
  account/session fixtures, private case directories. Do not print their stored
  verifiers or payloads. Dummy password input files were removed after bootstrap.

The run used existing image `mimik-publication-ci:20261013`
(`bbd06698cce7`) with networking disabled. Only scratch was host-mounted writable.
Tools, manifest, lockfile, toolchain file and source were mounted read-only;
the binary volume and existing `mimik-auth-cargo` registry cache were read-only.
No host tooling or dependencies were installed. The container runs as root
because its existing Python runtime is under `/root`; outputs may be root-owned.
Initial attempts failed before measurement because the image Python runtime was
inaccessible to the host UID and the registry volume was mounted at the wrong
level. The successful run mounted the registry at `/opt/cargo/registry`.

To reproduce, use an empty owned scratch directory and mount it as `/scratch`,
the script at `/scratch/profile.py`, `tools/` as `/helpers`, the four allowlisted
project inputs (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`) at their
matching `/workspace/` paths, the release volume at `/workspace/target`, and the
registry volume at `/opt/cargo/registry`. Set `PYTHONDONTWRITEBYTECODE=1`, disable
container networking and run `python /scratch/profile.py`. The script refuses
an occupied port and existing case directories. `python /scratch/profile.py graph`
refreshes only graph/hash/fingerprint evidence without rerunning HTTP samples.

## Verification boundary and review handoff

Measurement checks above passed. The final scratch script also passed container
`ruff format --check --isolated /scratch/profile.py` after a formatting-only
patch; the initial format check requested reformatting. No application format,
lint or test suite was run by this measurement task. Integrated request timeout/concurrency bounds
are verified by tests in the main implementation session, per its handoff;
this sequential measurement does not verify those limits. No browser,
unpacked-artifact, archive/checksum equality, delivered-source rebuild, live
provider/manual, campaign authorization, LAN or full-artifact certification is
claimed here. `--auth` remains the loopback-only Stage-B scope described by the
plan; login does not establish gameplay authorization.

Fresh coordinating independent reviewer `ses_ec9fdfe61ffe83j1C4Y4YaiDzH` accepted
the scratch script, raw graph/fingerprints, response assertions, latency/RSS
attribution and this document with no blocking findings. Review is recorded in
`reviews/2026-10-13-http-auth.md`. Measurements remain observations, not release
approval; subsequent artifact gates are recorded separately. The measurement
task wrote only its owned scratch path and this document, without application or
tracker changes.
