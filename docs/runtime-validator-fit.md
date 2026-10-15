# Runtime validator fit sample — 2026-10-12

Historical isolated-probe checkpoint. Subsequent actual application integration
is documented in `definition-drafts.md`; statements below describe the earlier
probe, not current application dependency status.

Status: isolated container probe complete; independent fit-result reviewer
`ses_ed0693a12ffeZ0fkOTfuwu8gvQ` approved GO for integration-candidate selection.
**No application dependency or runtime-game feature was integrated.** This evaluates
the library boundary, not the unimplemented admission/approval/renderer code.

## Reproducibility and isolation

- Probe directory: `/tmp/opencode/mimik-validator-fit-20261012`, separate from the
  application, with `Cargo.toml`, generated `Cargo.lock`, `src/main.rs`, synthetic
  `existing-schema.json` and probe-only `about.toml`. Keep this local probe for
  integration handoff; it is not in the shipping application package or normal CI.
- Exact versions: optional `jsonschema = "=0.58.6"`, defaults disabled; existing
  app's `serde_json = "=1.0.152"`. `schema` feature enables only that dependency.
- Container `mimik-ci`, image
  `sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f`.
  Rust1.95.0/Cargo1.95.0, Linuxx86_64. Release thin LTO/strip, separate
  `target/baseline` and `target/schema` preserve both binaries.
- Mount only the probe, container Cargo-cache volume, and an individually
  allowlisted read-only application notices template when needed. No workspace
  root, secrets, manuals, host Cargo home or Docker socket mounted. Repeat runtime
  samples with network disabled and read-only probe mount.
- Commands: container `cargo generate-lockfile`, `cargo fmt --check`, separate
  locked baseline/schema builds and runs; separate strict Clippy configurations
  with `-- -D warnings -D clippy::all -D clippy::pedantic`; `cargo audit --deny
  warnings`; locked `cargo metadata --filter-platform x86_64-unknown-linux-gnu`
  plus reverse feature tree; `cargo about generate --locked --fail --features
  schema --config about.toml /checks/about.hbs -o target/NOTICES.md`.

## Observed capability/feature results

- All executable assertions pass, including fresh no-network runs.
- Exact Serde i64 round trips at both extrema and around2^53 preserve tokens.
  `jsonschema` validates these integer types, but also accepts1.0,1e0 and the
  unsigned positive overflow value2^63 as Schema integers. Mimik must separately
  enforce its signed-token/i64 admission; a Schema `integer` check alone is not
  that contract. Fractional1e-1 fails both; negative overflow admission rejects
  without pretending f64 preserves its exact mathematical value.
- `.offline()` construction rejects HTTP references and a `file:` reference to
  a known-existing valid synthetic schema; embedded local `$defs` succeeds.
  Network-denied execution complements this test, not proof of no attempted
  outbound access by arbitrary future integration code.
- Missing required key, unknown property and invalid enum independently reject
  with expected keyword and instance/schema paths. Combined-invalid iteration
  retains all three reasons. This tests library diagnostics, **not** an implemented
  32-item/256-character application formatter or raw-draft UI.
- Schema feature has66 active target-filtered packages including probe, versus6
  for Serde-only baseline. `jsonschema` active feature set is empty. `reqwest`,
  `rustls` and `tokio` are absent from that resolved graph; disabled defaults were
  verified, not merely assumed. Lockfile contains80 entries including other
  targets/root. Declared crate MSRV1.85.0; build verified only on Rust1.95, not a
  Rust1.85 execution test.
- Formatting, strict all+pedantic Clippy for both configurations and dependency
  audit passed. Locked schema notices passed **after** the reviewed probe-only
  `borrow-or-share` MIT-0 acceptance below.

## Measured sample

256 fields, 16 nullable enums × 128 choices = 2,048 choices, and 240 text values
of 4,000 ASCII characters (no newline-containing text in this probe). Trusted
generated schema 42,542 bytes; valid body 963,891 bytes (about 92% of 1 MiB),
combined-invalid body 963,884 bytes.

| Measurement | Observed |
| --- | --- |
| Serde-only stripped release executable | 381,000bytes |
| Schema sample stripped release executable | 6,186,848bytes |
| Delta for this standalone probe | 5,805,848bytes |
| Offline compile sample, three runs | 0.724 / 0.760 / 1.844ms |
| 100 valid validations | 5.151 / 5.490 / 12.589ms |
| Independent and combined diagnostics cases, including clones | 1.747 / 1.989 / 5.085ms |
| Whole schema probe, in-process | 15.391 / 19.013 / 34.332ms |
| Schema process including launch, probe and exit | 20.367 / 24.700 / 41.201ms |
| Serde-only process including launch/exit | 2.818ms |

These are shared-host, short sampled measurements, not worst-case guarantees,
p95 statistics or a hard memory ceiling. Serde baseline is footprint/process
startup only, not an equivalent throughput implementation. Application binary
delta may differ through shared dependencies; no actual Mimik size was changed.
High-cardinality sample does not exhaust all admitted label lengths/Unicode,
invalid inputs or recursive diagnostic expansion. No peak-memory measurement.

## License finding and reviewed disposition

Initial cargo-about failed correctly: probe lacked its own license declaration,
and `borrow-or-share`0.2.4 uses MIT-0, not accepted by the application allowlist.
Independent review `ses_ed06ed686ffeSIxz16e0jApuZi` approved declaring original
probe code AGPL-3.0-only and a **probe-only** `[borrow-or-share] accepted=["MIT-0"]`
entry, retaining full transitive/build/dev coverage. No global suppression and
no modification of application `about.toml`.

Primary evidence: [SPDX MIT-0](https://spdx.org/licenses/MIT-0.html),
[published0.2.4 license](https://docs.rs/crate/borrow-or-share/0.2.4/source/LICENSE).
MIT No Attribution grant is compatible in principle with AGPL; original-code
AGPL obligations remain. Version archive SHA-256 reported by reviewer:
`dc0b364ead1874514c8c2855ab558056ebfeb775653e7ae45ff72f28f8f3166c`.
The corrected locked notices gate passed. This is not shipping license approval
for an unexamined future combined application graph.

## Proposed go/no-go and remaining gates

The sample supports **proceeding with jsonschema as the integration candidate**:
generic capability, exact typed integer boundary and offline controls fit; the
measured standalone cost is explicit rather than assumed. Independent fit-result
review approved the candidate, not application integration. Add the dependency
only in a reviewed implementation step.
Recheck the real generated profile, minimum/maximum admitted workloads, actual
combined feature/audit/license graph and binary/memory cost. The application
admission wrapper, cancellation bounds, source identity migration, publication,
canonical values, browser editor and complete release gates remain unimplemented.
