# Mimik 0.1.0: supported release scope

Mimik is a sheet-first tabletop RPG companion for players at a physical table. This archive
holds one self-contained Linux x86_64 binary. It runs on one machine and serves browsers on the
same private network. Linux x86_64 is the only verified platform. No Node.js, Python, GPU, API
key or internet access is needed; the assistant and its model downloads are optional.

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
    quick-add line ("3 torches") on lists whose definition declares one. On lists whose
    definition links a catalogue, a name cell warns when the name is not in the catalogue and
    asks "Which one?" when several entries share it; the pick is stored as an advisory
    catalogue key beside the name; name suggestions come as you type. None of this needs a
    model. Saved runtime sheet pages load one script module under a strict CSP; every save,
    Undo, switch, quick-add, row action, catalogue warning, Browse and "Which one?" also works
    without JavaScript. Name suggestions and the assistant's readings and reminders need it;
  - native sheets for the Italian 2025 MÖRK BORG core rules, once that exact source is
    registered;
  - on both, a "Not validated" flag in the header of any saved revision whose checks did not
    all pass or that no checks cover (it never blocks a save).
- **Owner materials.** The Owner registers games and editions at `/admin/rulesets`, and reviews
  imported source pages against the original page images at `/admin/materials`. Source import,
  definition publication and catalogue publication use the command line.
- **PDF staging.** On Linux, the Owner uploads one PDF at `/admin/materials/uploads`, inspects
  it, selects any pages of it (a range list or all, up to 20,000 pages), renders, cancels/retries jobs and discards staging. Upload needs
  the sandbox's unprivileged user namespaces and successful availability probe; failure shows
  remediation and never selects an unconfined parser. macOS and Windows have no upload.
  Until source-import step 4, this stages pages only: it creates no source or lookup evidence.
  The synthetic upload worker is confined to explicit `dev-endpoint` browser-check builds.
- **Unauthenticated prototype.** `--no-auth` serves the old single-user editor on
  `127.0.0.1` only. It is not protected by accounts and does not see private campaign data.

## The optional assistant

Scope: **local endpoint, POC quality, not certified** (D-D: the model quality bar is waived;
`limitations.md`, ASSIST-L4). It is off unless started with `--assistant`.

- **What it does.** It reads a number scribble the cell could not take, and, when the Owner's
  Reminders setting is on (on by default on the GPU path, off on the CPU path), reminds the
  player after a saved decrease. Both are narrow proposals the player must confirm; the model never
  applies a sheet change, never judges legality, and its output is shown before any write.
  Without the flag there is no model child, no job route and no client.
- **Runtime.** The archive carries the pinned official llama.cpp b12110 CPU and Vulkan builds
  (`llama/`, MIT; hash-checked; source at the upstream commit named in `SOURCE.txt`). No model
  file ships. The model server runs only inside Mimik's own sandbox (Hakoniwa, built into the
  binary) where the start-time probe shows unprivileged user namespaces work; otherwise the
  assistant stays off and `/admin/assistant` says how to enable them. It never runs unconfined.
- **Owner setting.** At `/admin/assistant` the Owner sees the runtime's state, switches models,
  sets the reply length, turns reminders on or off and downloads models. A download is Owner-started and licence-confirmed,
  goes over HTTPS only to the pinned Hugging Face hosts, and is size- and hash-verified before it
  is used. It needs the operating system's CA certificate store (`model-downloads.md`).
  Mimik sends no sheet, note or player text in a download.
- **Verified.** Linux `x86_64` only. Confinement level 1 on Ubuntu 24.04 with
  `kernel.apparmor_restrict_unprivileged_userns` switched off (the CI runner) and on the dev
  host (Gentoo); with Ubuntu 24.04's default restriction on, CI records the probe's outcome
  without certifying it. There is no level 2, and level 3 is not used. The GPU path (Vulkan,
  tried first on a hardware GPU, falling back to the CPU) is not covered by CI and was measured
  on one machine only: an RX 6650 XT with Mesa RADV (`results.md`). The documented minimum host
  (Ubuntu 22.04 `x86_64`) is not yet verified on a VM. macOS and Windows have no assistant.
- **Open limits** (`limitations.md`): L1 (change notes are unsealed), ASSIST-L4 (no quality
  bar), ASSIST-L5 (unbounded history), ASSIST-L34 and ASSIST-L36 (hosts and GPUs not
  verified), ASSIST-L24 (no confinement fallback), ASSIST-L20 (the CPU path needs enough
  threads), C1 (no model file ships), ASSIST-L58 to ASSIST-L60 (download hosts, proxies, CA
  store).

`THIRD-PARTY-NOTICES.md` includes the MuPDF 1.27.2 vendored-tree section: all licence
documents shipped by mupdf-sys 0.8.0, including disabled components and fonts, the full
hash-pinned IJG libjpeg 9f README and attribution with MuPDF's three modified files, and
the explicit GPLv3 text used for FreeType's GPLv2-or-later election. Packaging verifies
this section against the corresponding dependency sources and pinned local licence assets.

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
- `--assistant`: the interpretation assistant. It needs a model file in the models folder (by default `data/models/`, beside the database;
  mode 0700): download one of the two pinned defaults or another Hugging Face GGUF from
  `/admin/assistant`, or place a GGUF file there by hand. The default depends on the device: Qwen3.5-9B `Q4_K_M` on the GPU path (Granite
  if its file is missing or does not match its pinned hash), `granite-4.2-3b-Q4_K_M.gguf` (SHA-256 `e0406663...`) on the CPU
  path. Another file needs `--assistant-model local:<name>.gguf --assistant-model-sha256 <hex>`,
  and a model the Owner switched to on `/admin/assistant` replaces the flag. Related options:
  `--models-dir`, `--assistant-runtime`, `--assistant-threads` (default: physical cores minus
  one), `--assistant-device` (`auto`, `cpu` or a GPU's PCI address). Mimik runs the bundled
  `llama/` build itself and measures it before the assistant takes jobs; on a slow machine it
  stays off and says why. `./mimik assistant-runner --probe --runtime llama --models-dir
  data/models --model local:<name>.gguf [--model-sha256 <hex>]` reports the confinement level this machine reaches.

Assistant requirements on the host:

- The bundled model server needs the OpenMP runtime (`libgomp.so.1`), which minimal Linux
  installs lack: `sudo apt install libgomp1` on Debian and Ubuntu, or your distribution's
  OpenMP runtime package. The GPU build also needs the Vulkan loader (`libvulkan.so.1`). If one
  seems missing the assistant still tries to start, and if the start fails its reason adds which
  library may be missing and what to install; it finds out by reading the
  ELF dynamic sections of the bundled files (the server, its libraries and its backends),
  without running anything. This is a best-effort check: when it cannot tell, for example
  because a library lives in an unusual folder, no hint is added.
- Mimik refuses a `llama/` folder, its files, or a models folder that group or others can
  write. Ubuntu's default umask (002) makes folders made with `mkdir -p` or copied with `scp`
  group-writable; unpacking the archive with `tar` keeps the archive's modes (0755) and is not
  affected. The page names the fix: `chmod -R go-w <llama folder>` and `chmod 700 data/models`.
  The folders Mimik makes itself (`data/`, `data/run/` and its generations) are 0700 whatever
  the umask, and so are the intermediate folders it creates for a `--database` path that did
  not exist.
- Ubuntu 23.10 and later restrict unprivileged user namespaces: run `./mimik assistant-apparmor-profile` (or `--for <absolute path>` for another
  location) and install the profile as the assistant page says. Checked on Ubuntu 24.04, where
  the profile covers that one binary path only: a copy or hard link elsewhere is refused, a
  symbolic link to it works.

Ctrl+C stops the server. Stop it before copying the database file as a backup. Back up before
upgrading: a newer binary migrates the database in place, and older binaries then refuse it. External tools that read the database (Python's `sqlite3` on an older system, for example) need SQLite 3.44 or later.
Text that earlier builds stored with CR LF line breaks is written with LF on its next save.

Manuals are not included. The Owner supplies lawfully obtained source material locally;
imported evidence is stored in the database, and existing sheets work without the original
PDFs.

## What the release gate checks

The CI workflow ends in one status, **`deployable`**, which passes only when every job below
succeeded for the same revision:

- **llama-fetch:** the pinned llama.cpp release asset, the two CI test models and the llama.cpp source (a build-time input for the notice
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
  with and without JavaScript (the sheet module under its strict CSP), and name suggestions
  with "Which one?"; the assistant's page flow runs in a development build (never released)
  against a fake model server;
- **lan-smoke:** the same binary on isolated networks with separate clients, a denied subnet
  and an HTTPS reverse proxy, including Secure cookies and rendered QR codes;
- **llama-runtime:** on Ubuntu 24.04 from the unpacked archive, with the AppArmor user-namespace
  restriction switched off for the job (the probe's outcome with it on is recorded first, not
  gated): the bundled llama-server's version and flags, the release binary's start-time probe
  reaching level 1, and the confinement tests, run from the test build of the same revision
  against the unpacked `llama/` (seccomp, endpoint check, runner, and the server under the real
  profiles with the test model: token refusal, socket and peer checks, an adversary in the
  sandbox reaching nothing, forged sandbox errors keeping level 1, SIGKILL of Mimik ending the
  sandbox), with a minimum count of tests run so a filter matching too little fails; and the
  unpacked release started with `--assistant` (bundled `llama/`, the SmolLM2-135M
  test model, the CPU device), which must reach Ready at level 1 after priming and calibration and then stop on
  SIGINT with a clean exit and no llama-server left.

**Green means** these checks passed for this revision and these exact archives. It does not mean
a finished RPG companion.

**Not certified:** physical phone or router setups; Windows, macOS or other CPU architectures;
browsers other than Chromium; public internet exposure; path-prefix proxying; the assistant as a whole (local endpoint, POC quality, D-D) and the quality or
speed of any model's interpretations; inference on any GPU (the GPU path was measured on one RX 6650 XT only, outside CI); API providers; PDF extraction or OCR; accuracy of any real manual's extraction; fidelity to
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
  `SOURCE.txt`, and `llama/`: exactly the pinned files of the official llama.cpp b12110 CPU
  build in `llama/cpu/` and Vulkan build in `llama/vulkan/` (their hashes and modes are in the
  manifest built into `mimik`).
- `mimik-0.1.0-source.crate`: corresponding source: Rust code, templates, migrations, assets
  in preferred form, tests, helper tools, build configuration, documentation and the vendored
  mupdf fork (`vendor/mupdf`, with the `[patch.crates-io]` entry and path-form lock; DEP-29).
- `mimik-0.1.0-dependency-sources.tar.gz`: vendored Rust dependency sources (`vendored-crates/`)
  with checksums, licences and the source-replacement configuration.
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
cargo build --locked --example fake_llama_server
cargo test --locked --all-targets --all-features
```

## Publishing

Make `deployable` a required status on the release branch, require reviews and disallow
bypass for routine changes. Publish only the archives produced by a successful `deployable`
run of the same revision; never rebuild a different binary after the gate. A cancelled or
skipped run is not green.
