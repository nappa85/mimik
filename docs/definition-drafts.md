# Strict runtime definition drafts

Implemented: bounded proposal admission and revisioned **draft-only** CLI/storage.
Explicit publication is a separate operator workflow in `definition-publication.md`.
Generic editor, campaign binding and provider calls remain future work. Drafts never enable sheets or approve source evidence; new generic
definitions have no prototype compatibility exceptions.

## Operator workflow

Register an exact nonnative pair first (`runtime-registry.md`), then:

```sh
./target/release/mimik import-definition-draft --file proposal.json --expected-revision 0
./target/release/mimik show-definition-draft --id 1
./target/release/mimik list-definition-drafts --game-id my-game --edition-id first --after-id 0
./target/release/mimik import-definition-draft --file corrected-proposal.json --expected-revision 1
```

Revision0 means create only; existing tuples conflict. Successful updates/no-ops
increment once; stale/overflow writes do not mutate. Row IDs are positive database
IDs distinct from definition slugs scoped to game/edition. Identity is immutable.
List returns at most100 summaries, ascending IDs, exclusive cursor. CLI JSON uses
exact integer tokens; consumers must not round through JavaScript `Number`.
`schemas/definition-example.json` is an original synthetic unresolved proposal
for registered `synthetic-game / first`, not an approved real-game sheet.

## Closed profile and references

Trusted host schema: `schemas/definition-v1.json`, not uploaded JSON Schema.
Required root: contract_version1, game_id, edition_id, definition_id, title,
sources, fields, groups. Controls: text/multiline/integer/boolean/enum. No scripts,
decimals, nested/repeating controls, unknown properties or uploaded schema hooks.
Derived fields, constraints and list fields follow the formula profile
(`formula_profile: 1`, see `runtime-definition-contract.md` and
`derived-formulas-plan.md` §6). The optional last member `catalogues` (1–8 unique
`{"id": slug}` links, author order kept) links published catalogues of the same pair; each
must have a sealed version when the draft is saved (`runtime-definition-contract.md`,
"Linked catalogues"). Each field occurs exactly once in ordered groups; hints only
auto/main/sidebar. Field/group IDs have separate namespaces; choices unique per
field. IDs follow the registry slug grammar; whitespace-only labels/reasons fail.

Sources contain positive **database IDs**, lowercase PDF SHA-256 and explicit
core/sheet/supplement roles. Mapped fields have1–4 database source/page/unit tuples;
the joined unit belongs to that page/source and selected sheet-role source with
the exact same registered pair/PDF hash. Every write verifies this transactionally.
Unresolved mappings require a reason. Unreviewed source can inform proposals;
draft mapping is not wiring/rule approval. Reads do not reapprove relationships;
publication binds current artifact/review snapshots and reports stale wiring.

Optional help and non-enum options accept omission/null, canonicalizing to
explicit null. Non-null options are enum-only; enums need1–128 choices. This is
intentional metadata semantics, not a prototype exception. Text is not normalized.

## Resource and diagnostic boundary

- Input and canonical UTF-8 each <=262,144bytes; canonical overflow rejected
  before DB-worker submission.
- Depth12 containers,32,768 nodes;32 keys/object and64 scalars/key. Fields/groups/
  sources arrays256/32/32, group references256, options128/enum and2,048 total,
  roles3, regions4/field and1,024 distinct.
- ID/hash strings64 scalars then strict ASCII/domain checks; labels/title100,
  help2,000, reasons5,000. Serde may allocate an escaped string before the visitor
  checks its length; the raw cap bounds that allocation.
- Duplicate keys (including escaped equivalence), fractional/exponent tokens,
  out-of-i64 integers and trailing JSON fail. Positive u64 callbacks are checked
  into i64, not rejected indiscriminately.
- At most32 path/code diagnostics, each256 scalars after terminal escaping,
  separate truncation flag/static marker. Raw error messages/values are not echoed.
- One nonqueued admission job/process, acquired before file read/worker submit.
  Owned permit survives cancellation across read/parse/DB handoff. Cancelled
  queued writes may commit atomically; cancellation cannot stop synchronous work.
  Separate processes rely on SQL transactions/uniqueness/revisions; losers can be
  conflict or typed busy/storage errors, never suppressed failures.
- Shared descriptor reads retain NOFOLLOW/NONBLOCK, regular-file and limit-plus-one
  checks. Existing byte callers own their allocation; bounds apply before expansion.
  No new HTTP endpoint or claimed HTTP retry behavior.

## Canonical identity and database constraints

Hash SHA-256 over `b"mimik-definition-v1\0" + canonical bytes`, lowercase hex.
Typed property order fixed; sources sorted by numeric ID, roles core/sheet/
supplement, regions by source/page/unit tuple. Fields/groups/choices preserve
order. Optional null spelling is fixed; no float/unknown members. This is Mimik
encoding, not general JSON/JCS. A permanent independent hash vector guards it.

Schema6 STRICT table enforces positive integer/autoincrement IDs/revisions,
registry FK, immutable identity, JSON-object content/meta identity/byte bounds,
NUL-free ASCII slugs and exact64-byte lowercase-hex hashes. Full reads verify
canonical bytes/hash/identity. Migration is additive/atomic; no reset was needed.
User-authorized prototype wiping remains preferable to future constraint leases.

## Actual integration measurements

jsonschema0.58.6 defaults disabled, explicit Draft2020-12/offline. Actual Linux
graph240 packages, jsonschema features empty, no reqwest/rustls; existing Tokio
remains. Lockfile257 entries including other targets. Audit/notices pass, including
freshly reviewed borrow-or-share0.2.4 package-specific MIT-0; no global ignoring.

Release14,717,816bytes vs prior registry8,698,072: delta6,019,744bytes (69.2%).
Standalone Rust runtime retained; Python profiler is optional container tooling.
`tools/definition_profile.py` uses a fresh wrapper per child and Linux RSS KiB.
Fresh corrected shared-host samples, whole launch/DB/schema/parse/storage/output/
exit costs (not validator-only):

| Case | Input bytes | Outcome | Process ms | Peak RSS KiB |
| --- | ---: | --- | ---: | ---: |
| Registry baseline, no schema init | 0 | success | 7.067 | 15,248 |
| 256 fields /2,048 enum choices | 122,386 | draft | 43.252 | 17,452 |
| 128 text fields near canonical cap | 260,993 | draft | 54.384 | 17,688 |
| 256 wrong nullable types | 125,198 | expected rejection | 19.169 | 15,720 |

Rejection requires exit1 and the same top-level nullable/type diagnostic, not
any nonzero exit. The narrow helper is coupled to current CLI Debug formatting;
format changes fail until reviewed. Fake storage/config/panic/signal/wrong or
disconnected diagnostics have retained helper regressions.

These are single samples with no sources/unresolved mappings, not worst-case or
hard-RSS guarantees. Mapped-source correctness has tests, but32-source/1,024-ref
timing is not certified. Do not subtract baseline as validator-only overhead.
Artifact results: `reviews/2026-10-12-definition-admission.md`, `../TODO.md`.
