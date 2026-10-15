# Licensing

## Current choice

The user selected AGPL for now. Mimik's original code uses **AGPL-3.0-only**, with the complete GNU Affero General Public License version 3 in the root `LICENSE` file and the matching SPDX identifier in `Cargo.toml`. The full text was obtained from the SPDX license-list-data repository's `text/AGPL-3.0-only.txt`.

## Practical implications

AGPL permits development, use, modification, distribution, and commercial use. It does not require a Rust-only implementation or prohibit native libraries, plugins, or model services merely because those are used.

It does impose conditions:

- Distribution of covered software must satisfy AGPL's license, notice, and corresponding-source requirements.
- A modified covered version that supports remote network interaction must prominently offer those users its corresponding source under section 13.
- Covered derivative/combined works must satisfy applicable copyleft and dependency-license compatibility requirements. A proprietary closed-source derivative distribution is not an unrestricted option.

Therefore, AGPL fits the stated open-source direction, but it is not restriction-free. Dependency and plugin integration should be assessed against their actual licenses and mode of integration, not assumed compatible based only on being open source or a separate process.

The original copyright holders can choose a different license for their own work later. Contributions by others and incorporated third-party material can limit unilateral relicensing; a later license change is not automatically available for every contribution.

## Code versus content

The AGPL grant covers Mimik's original software. It does not relicense imported RPG manuals, official sheet artwork/templates, model weights, or third-party assets. The local manual corpus remains external to the repository and retains its own terms. Runtime imports should stay in ignored local data directories rather than being bundled into source releases by default.

The bundled htmx asset retains its Zero-Clause BSD license and attribution in `assets/htmx-LICENSE` and `assets/README.md`. Future model/runtime selection must record code, weight, and data licenses separately.

The source-review HTML stack brings cssparser 0.38.0 and dtoa-short 0.3.5 under MPL-2.0. Their exact license files were independently reviewed; MPL-2.0 permits AGPLv3 secondary licensing when no secondary-license incompatibility designation applies. Retain upstream notices/source and do not relabel all third-party code as Mimik-owned AGPL. `about.toml` permits MPL only for those two crate names; locked version changes require renewed review. Releases now accompany binaries with vendored dependency sources/configuration and preferred-form htmx source, and require an offline source rebuild check.

## Instance-owner policy

Mimik is locally deployed with owner-supplied content. Owners must lawfully obtain and have the required permissions for imported materials and are responsible for their content choices, access administration, and use. See `local-setup.md` for the setup process.

This documents responsibility; it is not a guarantee that every claim can be transferred to an owner. Contributors must also respect rights in what the project itself distributes. Local use and manual ownership do not automatically resolve all copyright questions.
