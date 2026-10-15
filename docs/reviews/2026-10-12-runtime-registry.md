# Runtime registry/source identity and campaign clarification

## Plan review

Reviewer `ses_ed03f567dffeJNPu8bwZdA1YMA` reviewed the bounded registry/source slice.
Initial high findings: enforceable complete associations/safe migration,
label/alias ambiguity/native protection, canonical activation on every character
path, duplicate-import association checks. Medium: version parsing/null/Unicode
limits. Revised plan approved before execution: junction tables with NOTNULL
composite foreign keys, no parent reconstruction, immutable registry/aliases,
bidirectional collision checks, canonical shared adapter resolver, identity checks
before duplicate early return, explicit v1/v2 transition and parsing rules.

Confirmed campaign requirements recorded: Master grants and player membership
are per campaign; new sheets belong to a campaign bound to exact game/edition.
Future generic creation requires that compatible campaign; unaffiliated legacy
prototype records are not silently assigned. No account/campaign implementation
in this slice.

## Implementation review and regressions

Fresh reviewer `ses_ed0228e1cffeYFA4pGBxBA3FqA` found medium blockers: lookup still
used raw labels instead of canonical associations; valid100-scalar Unicode labels
were rejected by the search byte limit. Correction plan independently approved.
Retained regressions failed before fixes: canonical alias aggregation returned
1 source instead of2; Unicode lookup returned Invalid. Fixed label/stable-ID
selectors, shared canonical/unresolved predicate for counts/coverage/matches,
coherent read transactions, explicit unsupported-ID behavior and scalar bounds.

Fresh recheck reviewer `ses_ed012ad53ffe1CLYKEfVVsePFu` approved implemented scope
with no blocking findings. Registry migration, native gating, identity parsing,
aliases, atomic backfill, duplicate guard, lookup and docs were read-only reviewed.
No extra schema/provider/auth dependency integrated; existing rusqlite/Serde/Clap
cover generic capabilities, custom code is Mimik identity/lifecycle behavior.

The initial CLI capability regression failed on unrecognized `register-ruleset`
before implementation. Normal CI retains12 registry tests: registration/idempotency,
alias/native collisions, missing/null/invalid v2 identities, Unicode lookup,
canonical alias aggregation and cross-edition coverage exclusion, unresolved legacy
compatibility/selectors/literal wildcards, malformed/missing/both selectors,
native-looking raw-label denial, alias backfill and injected-failure rollback,
fresh/migrated FK/NOTNULL guards, migration failure rollback, populated schema4
payload/sheet/PNG/review preservation, duplicate import association guard/reopen.

## Verification

63 Rust tests /13 helper tests, strict all+pedantic Clippy, rustfmt/Ruff,
actionlint, current dependency audit and locked notices passed; release built.
Process helper proves CLI register/idempotent/conflict/list/unknown import and
server-restart registry/source persistence. Two real Chromium viewports passed
existing sheet/conflict/source/region checks plus registered unknown-source
visibility and stable-ID lookup form submission. These direct release-binary
checks precede final packaged gates; no real-manual approval or live provider.

Container IDs as earlier local gates: CI
`sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f`, browser
`sha256:894398ec9fdbcabf61068d42e9d2983800fedffcc993de4a38990e08c5fb88a8`, with
current browser and process helpers mounted read-only. Synthetic fixtures:
`target/smoke-registry-20261012`. Final archives use worktree label
`local-20261012-registry-reviewed`, not a Git commit; identities in `SHA256SUMS`.
Unpacked/offline artifact gates remain active at this checkpoint.

### Artifact completion

`target/dist-registry-20261012/` passed source-member allowlist, archive checksums,
unpacked binary equality with `target/release/mimik`, isolated process smoke and
two Chromium viewports against the unpacked executable. Delivered source/vendor
rebuilt with `cargo build --release --offline --locked`, networking disabled,
empty Cargo home and fresh target in1m22s. No dependency changes; existing locked
vendor tree reused, its offline build verifies it covers the delivered sources.
Final handoff-inclusive archives regenerated/rechecked in
`target/dist-registry-final-20261012/`; accompanying hashes identify those exact
artifacts. All current application checks passed; no hosted CI run claimed.

## Remaining scope

Next independently reviewed change: bounded definition admission using evaluated
schema candidate and actual combined graph/license/resource checks; then immutable
publication/evidence approval and campaign-compatible generic editor. Campaign
accounts/Master grants, full sheet history/rollback, soft deletion and Owner-only
purge are deferred. Plan/review/test success does not prove universal rules fidelity
or public-release readiness; hosted CI/logo rights remain pending.
