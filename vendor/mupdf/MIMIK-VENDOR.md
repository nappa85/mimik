# Vendored mupdf 0.8.0 with the Mimik text-direction/structure fork

Owner decision 2026-11-05 (route A plan, "Wrapper gaps"): use a local fork of mupdf-rs until
the owner's upstream PR is merged and released; then return to the crates.io release.

How this directory was made (reproduce exactly to check it):

1. `mupdf-0.8.0.crate` from crates.io, SHA-256
   `185fb74927a40c569b7152c01c73f4c0206059b8b3d37c7eb007cc3f63453b64` (the `Cargo.lock`
   checksum), extracted.
2. `tests/` and `examples/` removed (upstream fixtures, about 11 MB), and their `[[test]]` and
   `[[example]]` sections removed from `Cargo.toml`; the upstream CI and git files
   (`.github/`, `.gitignore`, `.gitmodules`), `Cargo.lock` (unused for a dependency) and `Cargo.toml.orig` (it still listed the removed
   targets) removed. The unit tests inside `src/` need those
   fixtures; they run in the fork's own repository, not here.
3. The fork's change applied: `git diff v0.8.0 1adf801 -- src` in mupdf-rs
   (branch `mimik/text-direction-structure`, commits bd4b703, f84972e, fe37046, 230e6eb,
   1adf801), diff SHA-256 `c95716de35045d69b74ad2803a587d089e55734abaec92a2caf518d01d168c43`.
   It touches `src/lib.rs` and `src/text_page.rs` only: safe `TextLine::dir`,
   `TextChar::bidi`/`is_rtl`, `TextBlock::structure`, `TextStruct` and `StructureType`.

`mupdf-sys` is unchanged and still comes from crates.io (`=0.8.0`, MuPDF 1.27.2).
Licence: AGPL-3.0, as upstream (`LICENSE`).

Wiring: the root `Cargo.toml` has `[patch.crates-io] mupdf = { path = "vendor/mupdf" }`.
`cargo package` drops that entry and omits this directory (it is a nested package), so
`tools/vendored_fork.py package` adds both back to the source package (DEP-29). The exact file
list with SHA-256 is pinned in `vendor/mupdf.sha256` (outside this directory); the tool and
`verify_source_package.py` refuse any extra, missing or changed file. After an intended change
run `python tools/vendored_fork.py inventory`.
