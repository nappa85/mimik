# Catalogue plan step 0: calibration results (2026-10-18)

Prototype tier. Spike code: branch `catalogue-spike`, commit `0907b9c`
(`src/catalogue_spike.rs`). It is an ignored in-crate test and is **not merged**. Plan:
[catalogue-plan.md](catalogue-plan.md) §3 step 0, R8, R11, R18, R28.

## Method

- **Environment:** container `mimik-ci:latest` with `--cpus 6`, a release build,
  bundled SQLite 3.50.2, on a Ryzen 5 1600 shared host. Single runs on a Sunday evening.
- **Database:** a fresh database migrated by `Store::open` (schema 18), plus a prototype of
  migration 019:
  - the tables of D1/R2/R17/R20;
  - the unit/page/source/pair insert trigger;
  - the seal trigger with linear two-way `EXCEPT` equality over `json_each` (R1/R18), version
    order (R19), and the append-only comparison.
- **Synthetic source:** 1 finalized source, 1,024 pages × 16 units (16,384 units). The text is
  90 words per unit, and every 64th unit is 200 KiB, to exercise prefix reads.
- **Catalogues:** 8 linked catalogues, each with 8 entry types and 2 properties per entry. Every
  8th entry has owner text (2 MiB total, or 1 MiB at 8,192 entries). The other entries reference
  4 units each.
- **Publishing:** each publish runs in one `IMMEDIATE` transaction (projections, FTS rowid-range
  delete+insert, seal). The last publish runs against a second connection doing an
  `IMMEDIATE` "server save" with a 5 s busy timeout.
- **Requests:** 200 iterations over random catalogues and types.
- **Pin validation:** a 16-list × 16-property definition, 30 iterations.

## Results

| Measure | 4,096 entries (3.19 MiB) | 8,192 entries (3.38 MiB) | R8 criterion |
|---|---|---|---|
| Publish lock held (per catalogue) | 492–537 ms | 0.92–1.06 s | ≤ 2 s ✓ |
| … of which seal trigger | 301–327 ms | 573–603 ms | measured (R18) |
| … projections / FTS | 127–191 ms / 11–12 ms | 253–392 ms / 22–24 ms | — |
| Publish of v2 (+1 entry, append-only check) | 591 ms | 1.11 s | ≤ 2 s ✓ |
| Concurrent save wait during publish | 545 ms, succeeded | 1.16 s, succeeded | waits and succeeds ✓ |
| Process peak RSS (VmHWM, whole test, 8 contents in memory) | 174 MB | 241 MB | ≤ 256 MiB ✓ (thin margin at 8,192) |
| Browse page (50, keyset) p95 | 0.58 ms | 0.78 ms | ≤ 250 ms ✓ |
| Picker search (FTS, 20) p95 | 11.5 ms | 29.2 ms | ≤ 250 ms ✓ |
| Entry page (all-unit metadata + prefix reads) p95 | 0.40 ms | 0.42 ms | ≤ 250 ms ✓ |
| Badges for 50 keys p95 | 3.0 ms | 3.2 ms | — |
| Writer membership, 4,096 keys | 32 ms | 45 ms | — |
| Content parse (serde, proxy) / sha256 | 9.5 / 1.9 ms | 18 / 2.0 ms | — |
| Pin `validate` / `validate_for_write` p95 | 4.8 / 4.7 ms | same | inside the 250 ms budget |

**Negative checks:**
- a same-count key substitution is refused by the seal ("entries differ");
- a unit claimed on the wrong page is refused by the insert trigger.

## Decisions for D5

- **Entries per catalogue: 8,192** (R11). Both publish and requests meet R8 with a margin of
  about 2×. The RSS margin is thin, but it was measured for a whole test process holding eight
  contents. Step 2 must measure the real CLI process with fresh child accounting, and drops back
  to 4,096 if the CLI exceeds 256 MiB.
- **Owner text total: 1 MiB at 8,192 entries**, so the canonical draft stays under 4 MiB. The
  owner-text cap becomes "≤ 1 MiB total, ≤ 20,000 scalars per entry". It was 2 MiB at 4,096.
- The other D5 limits are unchanged.

## Limitations

- Single runs on a shared host, not worst-case bounds.
- Every unit was unreviewed, so badge values were uniform. The query shape is the same for
  other states.
- Names come from a 16-word vocabulary, so FTS hit counts are high. That makes the search
  timings pessimistic.
- The parse time is `serde_json`, not the real catalogue decode profile.
- Contention used a second connection in another thread of the same process. SQLite file
  locking is the same across processes, but a separate-process measurement is still owed in
  step 2.
- Sheet render bytes with the picker open were **not** measured, because no template exists
  yet. Step 4's markup assertions (R10/R22) carry that check.
- `private_pin::validate` was timed on a 16 × 16 list definition with one formula; heavier
  formula sets cost more.
