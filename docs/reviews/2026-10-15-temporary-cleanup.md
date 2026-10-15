# Authorized temporary/build-cache cleanup

User interrupted benchmark setup because `/tmp` was almost full, then explicitly
asked to check project `./target/` as well. Independent read-only plan and scope
reviewer `ses_ec150a996ffecxTvh0G7MKDJU1` approved the bounded paths below, conditional
on identity, active-use and mount checks. No wildcard temp wipe or Docker prune.

## Checks

- First two Cargo target roots contained only `.rustc_info.json`, `CACHEDIR.TAG`,
  `debug/` and empty `tmp/`. No source/user data or unique retained artifacts.
- `findmnt -R /tmp` showed only the tmpfs, no nested cleanup mounts. Literal roots
  were checked as ordinary, nonsymlink directories immediately before deletion.
- No cargo/rustc/rustdoc/ninja/cmake/Mimik processes were reported at that check.
  Targeted/global open-file and cwd checks found no matching paths.
- lsof warnings were confined to unrelated `/run/docker/netns/*` objects; only
  two unrelated PostgreSQL containers were active, with no cleanup-path binds.
  Reviewer explicitly accepted those complementary checks, rather than treating
  an unresolved target-path permission error as proof of inactivity.
- Every retained source, runtime binary and vendor file in the ten validation
  copies below was hash-compared with its project `target/dist-*` archives.
  All ten matched, with no extras outside generated Cargo targets.
- Each validation copy's root-owned compiled target was cleared in an existing
  Python container with network disabled, read-only container root, no Docker
  socket and only DAC_OVERRIDE restored after dropping capabilities. Only the
  15 approved compiled-cache directories were mounted writable. Symlink-resistant
  `shutil.rmtree` was required; no symlink following or ignored errors. Then empty
  targets and validated copies were removed by their literal host paths.

## Deleted temporary roots

| Literal root under `/tmp/opencode/` | Root-owned cache cleared first, relative to root |
|---|---|
| `mimik-f145-target` | ordinary user-owned Cargo target, removed directly |
| `mimik-f2-target` | ordinary user-owned Cargo target, removed directly |
| `mimik-definitions-final-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-definitions-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-registry-final-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-registry-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-regions-final-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-regions-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-ui-final-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-ui-validation-20261012` | `mimik-0.1.0/target` |
| `mimik-context-validation` | `source/mimik-0.1.0/target` |
| `mimik-release-validation` | `source/mimik-0.1.0/target` |

## Deleted project build caches

Under `/home/marco/Progetti/mimik/` only:

- `target/debug`
- `target/release/build`
- `target/release/deps`
- `target/release/examples`
- `target/release/incremental`

Preserved `target/release/mimik`, release archives, current distribution,
`target/local-rag-benchmark-20261015` weights/raw data/metrics/provenance, user data
and all other unapproved paths. Installed binary before/after SHA256:
`420fa9b141efeea297d216555ddf60720f2c28274329df61887ae9fe1664f510`.

## Observed recovery

| Operation | Filesystem availability increase |
|---|---:|
| Two old temporary Cargo targets |5,370,257,408bytes|
| Ten validated temporary copies |6,243,995,648bytes|
| Project build caches |12,866,035,712bytes|

Total observed `/tmp` recovery:11,614,253,056bytes, approximately10.8GiB. `/tmp`
changed from93% to24% used, approximately13GiB available. Project-cache recovery
was approximately12.0GiB; its filesystem reported approximately46GiB available
afterward. These are measured operation deltas, not an attribution of all changes
on the shared project filesystem to this cleanup.

No application code/data migration or release installation occurred. Existing
archives retain the deleted unpacked source/vendor copies for reproduction.
Future benchmark builds should remain container-local, not large host `/tmp`
targets. Independent post-operation review GO: approved caches are absent, retained
binary/archive/model/results metadata and binary digest verified. Reviewer's later
disk snapshot was `/tmp`23%used/~13GiBfree and project filesystem90%used/~43GiBfree;
shared-filesystem activity accounts for variation from the earlier snapshot.
Ten temporary-copy post-deletion approval relies on implementer's exhaustive
pre-deletion comparisons and literal-path operation evidence, not an independent
reconstruction after those copies were removed. No outstanding cleanup blocker.
