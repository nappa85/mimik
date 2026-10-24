# Mimik 0.1.0: supported release scope

Mimik is a sheet-first tabletop RPG companion for players at a physical table. This archive
holds one self-contained Linux x86_64 binary. It runs on one machine and serves browsers on the
same private network. No Node.js, Python, GPU, API key or internet access is needed.

## What this release supports

- **Accounts.** A local Owner, created once from the command line, administers the instance
  and does not play. The Owner invites players, reissues invitations, issues player password
  resets and disables players in the browser (`/admin`). Invitations and resets come with a
  copyable link and a QR code. Passwords need at least 15 characters for the Owner and 12 for
  players, with no required character classes.
- **Campaigns.** Players create campaigns bound to one exact game and edition, request to join,
  and the creator manages additional Masters. A player who leaves keeps read-only access to
  their sheet, which stays frozen.
- **Private sheets.** Each sheet belongs to its player and campaign, with complete revision
  history. Two kinds:
  - runtime sheets from a published game definition: scalar and list fields, advisory derived
    values and checks (never blocking a save, never a legality ruling), read-only catalogue
    browsing through the sheet, number cells that accept sums such as `84 -15`, change notes
    with Undo, unknown values and per-field and per-cell saving (each list cell, new row and
    row action saves on its own; Remove is followed by "Row removed · Undo"), and a
    quick-add line ("3 torches") on lists whose definition declares one;
  - native sheets for the Italian 2025 MÖRK BORG core rules, once that exact source is
    registered;
  - on both, a "Not validated" flag in the header of any saved revision whose checks did not
    all pass or that no checks cover (it never blocks a save).
- **Owner materials.** The Owner registers games and editions at `/admin/rulesets`, and reviews
  imported source pages against the original page images at `/admin/materials`. Source import,
  definition publication and catalogue publication use the command line.
- **Unauthenticated prototype.** `--no-auth` serves the old single-user editor on
  `127.0.0.1` only. It is not protected by accounts and does not see private campaign data.

No language model file is part of this release. The archive carries the pinned official
llama.cpp CPU and Vulkan builds (`llama/`, b11609, MIT; pinned and hash-checked, its source is at the upstream commit named in `SOURCE.txt`) for the
optional interpretation assistant, which is off unless started with `--assistant` and proposes
readings the player must confirm; it never applies sheet changes on its own. Its scope is
Linux `x86_64` on the CPU: the model server runs only confined by bubblewrap, where the
start-time probe shows unprivileged user namespaces work; otherwise the assistant stays off and
`/admin/assistant` says how to enable them (it never runs the model server unconfined). Level 1 is verified on Ubuntu 24.04 with
`kernel.apparmor_restrict_unprivileged_userns` switched off (the CI runner) and on the dev host
(Gentoo, no such restriction); with Ubuntu 24.04's default restriction on, CI records the
probe's outcome without certifying it. On a machine with a hardware GPU the assistant tries
the GPU path first (the Vulkan build, `--assistant-device`) and falls back to the CPU; the GPU
path is not certified: CI does not cover it, and its live checks have been run on the dev
host's GPU only (RX 6650 XT, Mesa RADV; `results.md`). The documented minimum host for the bundled build (Ubuntu 22.04 `x86_64`) is not
yet verified on a VM.

## Run

1. Unpack `mimik-0.1.0-linux-x86_64.tar.gz` into a writable directory and `cd` into it.
2. Create the Owner. The password file must be a regular file you own with no group or other
   permissions; its bytes are the password exactly:

   ```sh
   mkdir -p private
   (umask 077; printf '%s' 'a long owner passphrase' > private/owner.password)
   ./mimik bootstrap-owner --login owner --display-name Owner --password-file private/owner.password
   ```

3. Start the server with `./mimik` and open the printed LAN address (port 3000) on the server
   or on a phone on the same network; `localhost` also works on the server. Sign in as the
   Owner, invite players from `/admin`, and play with a player account.

Options:

- `--loopback`: serve this machine only.
- `--lan-address 192.168.1.20`: pick the LAN interface when automatic selection is wrong.
- `--external-url https://mimik.example` or `MIMIK_EXTERNAL_URL`: the public root origin
  behind a TLS reverse proxy. Run with `--loopback` and proxy to `http://127.0.0.1:3000`,
  preserving the external Host header. Path prefixes are not supported.
- `--database PATH` or `MIMIK_DATABASE`: database location; default `data/mimik.sqlite3`
  under the working directory.
- `./mimik --help` lists the operator subcommands (accounts, registry, sources, definitions,
  catalogues).
- `--assistant`: the interpretation assistant. Put the model file in `data/models/` (mode
  0700; the default is `granite-4.2-3b-Q4_K_M.gguf`, SHA-256 `e0406663…`; another file needs
  `--assistant-model local:<name>.gguf --assistant-model-sha256 <hex>`). Mimik runs the
  bundled `llama/` build itself and measures it before the assistant takes jobs; on a slow
  machine it stays off and says why. `./mimik assistant-runner --probe --runtime llama
  --models-dir data/models --model local:<name>.gguf` reports the confinement level this
  machine reaches.

Assistant requirements on the host:

- The bundled model server needs the OpenMP runtime (`libgomp.so.1`), which minimal Linux
  installs lack: `sudo apt install libgomp1` on Debian and Ubuntu, or your distribution's
  OpenMP runtime package. The GPU build also needs the Vulkan loader (`libvulkan.so.1`). If one
  is missing the assistant says which library and what to install; it finds out by reading the
  ELF dynamic sections of the bundled files (the server, its libraries and its backends),
  without running anything. This is a best-effort check: when it cannot tell, for example
  because a library lives in an unusual folder, the start goes on without a message.
- Mimik refuses a `llama/` folder, its files, or a models folder that group or others can
  write. Ubuntu's default umask (002) makes folders made with `mkdir -p` or copied with `scp`
  group-writable; unpacking the archive with `tar` keeps the archive's modes (0755) and is not
  affected. The page names the fix: `chmod -R go-w <llama folder>` and `chmod 700 data/models`.
  The folders Mimik makes itself (`data/`, `data/run/` and its generations) are 0700 whatever
  the umask, and so are the intermediate folders it creates for a `--database` path that did
  not exist.
- Ubuntu 23.10 and later restrict unprivileged user namespaces: with `--assistant-sandbox
  hakoniwa`, run `./mimik assistant-apparmor-profile` (or `--for <absolute path>` for another
  location) and install the profile as the assistant page says. Checked on Ubuntu 24.04, where
  the profile covers that one binary path only: a copy or hard link elsewhere is refused, a
  symbolic link to it works.

Ctrl+C stops the server. Stop it before copying the database file as a backup. Back up before
upgrading: a newer binary migrates the database in place, and older binaries then refuse it.
Text that earlier builds stored with CR LF line breaks is written with LF on its next save.

Manuals are not included. The Owner supplies lawfully obtained source material locally;
imported evidence is stored in the database, and existing sheets work without the original
PDFs.

## What the release gate checks

The CI workflow ends in one status, **`deployable`**, which passes only when every job below
succeeded for the same revision:

- **llama-fetch:** the pinned llama.cpp release asset, the test model and the llama.cpp source (a build-time input for the notice
  texts, never shipped), every size and hash checked against the manifest;
- **checks:** formatting, Clippy with all and pedantic lints denied, all Rust tests
  (`--locked --all-targets --all-features`), Ruff and the Python helper tests, workflow lint,
  `cargo audit --deny warnings`, third-party licence notices, release build, source-package
  member allowlist, vendored dependency sources and these archives with checksums (`llama/`
holding exactly the pinned files);
- **artifact-smoke:** the unpacked binary from these archives, run from an isolated directory:
  startup, assets, accounts, campaigns, native and runtime sheets, formulas, list editing,
  catalogues, assisted entry, rejections, restart and persistence, with synthetic inputs; then
  an offline rebuild of the corresponding source with no dependency cache;
- **browser-smoke:** the same binary in offline Chromium at 1280×720 and 390×844: forms,
  long text, overflow and reflow, sharing, source review and sheet editing, runtime sheets
  with and without JavaScript (the sheet module under its strict CSP);
- **lan-smoke:** the same binary on isolated networks with separate clients, a denied subnet
  and an HTTPS reverse proxy, including Secure cookies and rendered QR codes;
- **llama-runtime:** on Ubuntu 24.04 from the unpacked archive, with the AppArmor user-namespace
  restriction switched off for the job (the probe's outcome with it on is recorded first, not
  gated): the bundled llama-server's version and flags, the release binary's start-time probe
  reaching bubblewrap, and the confinement tests, run from the test build of the same revision
  against the unpacked `llama/` (seccomp, endpoint check, runner, and the server under the real
  profiles with the test model: token refusal, socket and peer checks, an adversary in the
  sandbox reaching nothing, forged sandbox errors keeping level 1, SIGKILL of Mimik ending the
  sandbox), with a minimum count of tests run so a filter matching too little fails.

**Green means** these checks passed for this revision and these exact archives. It does not mean
a finished RPG companion.

**Not certified:** physical phone or router setups; Windows, macOS or other CPU architectures;
browsers other than Chromium; public internet exposure; path-prefix proxying; the quality or
speed of any model's interpretations, inference on a GPU, API providers; PDF extraction or OCR; accuracy of any real manual's extraction; fidelity to
official sheet layouts; any game's rules legality; tamper evidence of change notes (they are
not sealed or verified); protection against unbounded growth (no per-account write-rate limit
or storage guard for sheet history and notes).

## Licensing

- Mimik's original code is **AGPL-3.0-only**: the full text is in the root `LICENSE` (from SPDX license-list-data) and the identifier is in `Cargo.toml`. A distributed binary must come with its corresponding source. A modified version that serves remote users must offer them its source (AGPL section 13). Integrating a dependency or plugin is judged by that component's own licence and how it is integrated.
- The grant covers Mimik's code only. Imported manuals, official sheet artwork and templates, model weights and extracted corpora keep their own terms. They stay in ignored local data and are never release assets. Owners must lawfully obtain what they import.
- Third-party notices: Rust notices come from `cargo about`. The accepted licence list and the narrow per-crate exceptions are in `about.toml`: MPL-2.0 for `cssparser`, `dtoa-short`, and the test-only `cssparser-macros` and `selectors` (MPL-2.0 allows AGPLv3 as a secondary licence); MIT-0 for `borrow-or-share`. A change to any of these locked versions needs a new review. The bundled htmx 2.0.8 and response-targets 2.0.4 assets keep their 0BSD licences (`assets/*-LICENSE`, `assets/README.md`), and their preferred-form sources ship in the source archive. Third-party code is never relabelled as Mimik's AGPL code.
- `logo.png` (repository root, served at `/assets/logo.png`) is artwork supplied by the owner and embedded in the binary. Its authorship and redistribution rights are unconfirmed. Confirm them before any public release.
- Contributions by others and incorporated third-party material can limit a later relicensing.

## Archive contents

- `mimik-0.1.0-linux-x86_64.tar.gz`: `mimik`, this `README.md`, `LICENSE` (AGPL-3.0-only),
  `THIRD-PARTY-NOTICES.md` (Rust dependencies, then llama.cpp and each component compiled
  into `llama/` with its licence text), `htmx-LICENSE`, `response-targets-LICENSE`,
  `SQLITE-NOTICE.txt`, `BUILD.txt` (version, revision, Rust version, llama.cpp build),
  `SOURCE.txt`, and `llama/`: exactly the pinned files of the official llama.cpp b11609 CPU
  build in `llama/cpu/` and Vulkan build in `llama/vulkan/` (their hashes and modes are in the
  manifest built into `mimik`).
- `mimik-0.1.0-source.crate`: corresponding source: Rust code, templates, migrations, assets
  in preferred form, tests, helper tools, build configuration and documentation.
- `mimik-0.1.0-dependency-sources.tar.gz`: vendored Rust dependency sources with checksums,
  licences and the source-replacement configuration.
- `SHA256SUMS`: checksums of the three archives.

llama.cpp is a separately executed, pinned build dependency: its source is not shipped.
`SOURCE.txt` and `BUILD.txt` name the exact upstream tag, commit and asset hashes, and
`THIRD-PARTY-NOTICES.md` carries llama.cpp's MIT text and the component licences. Corresponding
source covers Mimik and the crates linked into it.

No manuals, databases, secrets, extracted text or rendered source pages are included. CI smoke
fixtures are synthetic and are not release content.

## Build from source

Unpack the `.crate`, then unpack the dependency sources into the package root. With Rust 1.99.1:

```sh
cargo build --release --offline --locked
cargo test --locked --all-targets --all-features
```

## Publishing

Make `deployable` a required status on the release branch, require reviews and disallow
bypass for routine changes. Publish only the archives produced by a successful `deployable`
run of the same revision; never rebuild a different binary after the gate. A cancelled or
skipped run is not green.
