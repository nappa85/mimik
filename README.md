# Mimik

Mimik is an open-source, sheet-first companion for tabletop role-playing players at a physical
table. It keeps the character sheet useful and brings the relevant rules to the player without
interrupting the game: it improves paper and pencil rather than replacing it. It is built in
Rust with server-rendered HTML and a small amount of bundled htmx. Mimik's original code is
licensed under AGPL-3.0-only; see `LICENSE` and the licensing section of the
[release scope](docs/release-scope.md).

## Current status

- **Accounts and campaigns.** Default serving is authenticated. A local Owner, created once
  from the command line, administers the instance at `/admin` and does not play. Players are
  invited, create or join campaigns bound to one game and edition, and keep private sheets with
  complete revision history. See [auth and sessions](docs/auth-and-sessions.md) and
  [campaigns](docs/campaigns.md).
- **Game definitions.** Games are described by data, not code: an Owner registers games and
  editions, imports strict definition drafts and publishes them after an explicit comparison
  of the field wiring against the original page images; that approval does not imply the
  source evidence was reviewed ([runtime definitions](docs/runtime-definition-contract.md)).
  Derived values and checks use a bounded CEL profile and are advisory: they never block a save
  ([formulas](docs/formulas.md)). Read-only [catalogues](docs/catalogues.md) can be browsed
  through a sheet.
- **Assisted entry.** Each cell saves on its own; number cells accept sums such as `84 -15`,
  changes record notes with Undo, unknown values are accepted, and catalogue-backed names get
  suggestions and "Which one?" ([assisted entry](docs/assisted-entry.md)).
- **Sources.** Owner-supplied manuals are imported locally with page images, preserved tables
  and a review history ([sources and retrieval](docs/sources-and-retrieval.md)). In normal
  operation manuals never leave the machine; the optional extraction reviewer
  (`tools/review_extraction.py`), when explicitly invoked, sends page blocks and page images to
  the HTTPS endpoint the owner selects.
- **Optional local model assistant** (`--assistant`, off by default): a pinned llama.cpp
  server run as a confined child proposes readings and reminders that the player confirms;
  proof-of-concept quality, not certified ([model runtime](docs/model-runtime.md),
  [evaluation tooling](docs/evaluation-tooling.md), [results](docs/results.md)).

## Run

```sh
mkdir -p private && (umask 077; printf '%s' 'a long owner passphrase' > private/owner.password)
cargo run -- bootstrap-owner --login owner --display-name Owner --password-file private/owner.password
cargo run
```

Open the LAN address printed at startup (`localhost` also works). `--loopback` serves this
machine only; `--external-url` (or `MIMIK_EXTERNAL_URL`) sets the public origin behind a TLS
reverse proxy; `--database` (or `MIMIK_DATABASE`) overrides the default `data/mimik.sqlite3`.
`--no-auth` serves the old unauthenticated single-user prototype on loopback only. Forms work
without JavaScript; small bundled scripts handle sharing and local date formatting.

Development checks (CI runs these and more inside containers; see
[release scope](docs/release-scope.md)):

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --example fake_llama_server
cargo test --locked --all-targets --all-features
ruff check tools && python -m unittest discover -s tools -p "test_*.py"
```

## Documentation

- [Work tracker](TODO.md): start here when resuming work.
- [Architecture](docs/architecture.md), including the docs map.
- [Decisions in force](docs/decisions.md), [known limitations and follow-ups](docs/limitations.md),
  [experiment and benchmark results](docs/results.md).
- [Supported release scope and CI gate](docs/release-scope.md).
- Active plan: [assisted sheet](docs/assisted-sheet-plan.md).

Rust and Cargo 1.99.1. Third-party htmx attribution is in `assets/README.md`.
