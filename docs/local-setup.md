# Local setup and owner-supplied content

## Deployment model

Mimik is intended for self-deployment on a trusted Wi-Fi/LAN. Plain `mimik` serves authenticated accounts/campaigns on the selected private IPv4 network and defaults to `data/mimik.sqlite3`; explicit `--loopback` is development-only. `--no-auth` selects only the loopback prototype. Optional `--external-url` / `MIMIK_EXTERNAL_URL` supplies the canonical origin for links/QR behind a root-origin reverse proxy, including TLS termination. See `deployment.md` for startup and proxy settings. Owner remains separate from campaign Master; no cloud upload or public manual-download endpoint is supplied.

## Content responsibilities

The instance owner must own or otherwise lawfully obtain the manuals they import and have the permissions required for their actual use. Ownership alone does not grant unrestricted copying, sharing, redistribution, or use of artwork. The owner is responsible for selecting content, checking its applicable terms, administering access, and using the instance lawfully.

Mimik does not supply commercial manuals, full extracted manual text, proprietary sheet artwork, or access to books the user has not obtained. The software license does not grant rights to imported content. Project contributors remain responsible for the material they themselves distribute; an owner-responsibility statement cannot guarantee that all possible claims against contributors are excluded.

AGPL's license and warranty/liability provisions remain in force. The local deployment policy does not remove AGPL's applicable distribution or modified-network-version obligations.

## Install and start

1. Obtain Mimik's source and a Rust toolchain.
2. Run `cargo run` from the project directory.
3. Open http://127.0.0.1:3000 locally.
4. Keep `data/` private. It is ignored by Git and contains the SQLite database by default. To use another location, set `MIMIK_DATABASE` to a local database path.

## Prepare the MÖRK BORG source set

1. Use a lawfully obtained core manual. The first adapter is pinned to the Italian first printing, October 2025, identified in `sources.md`; English 2019 is a separate source, not a silent fallback.
2. Keep manuals, locally extracted text, and official sheet assets in a private directory such as `data/manuals/` and `data/sheets/`. Do not add them to Git or software releases.
3. Record the document's title, language, printing, hash, and local path. Do not infer edition from its filename alone.
4. Supply the official blank sheet separately for adaptive HTML/CSS layout mapping. Preserve its recognizable sections while allowing long fields/mobile reflow; Mimik's current structured editor is not a reproduction of that sheet.
5. Before importing extracted rules into retrieval, review them against the PDF as described in `source-quality.md`. Preserve corrections, page references, and review state.

**Current implementation:** character persistence, structured MÖRK BORG fields/calculations, strict prepared-source import, review UI/history and reviewed-only lookup work locally. Follow `source-review-setup.md` for the containerized sample flow. General OCR, live API review and official-sheet layout mapping remain unverified/pending; preparing files never automatically approves evidence.

Commercial API review is opt-in and sends selected source pages/blocks outside the machine. The instance owner configures their endpoint, model, and environment key and is responsible for permitted uploads and provider costs/terms. No key is required for the default local sheet workflow.

## Backups and releases

Stop the application before copying its SQLite database for a simple backup. Backups of imported content stay under the instance owner's control. Software release packaging should include code, dependencies' required notices, and mappings; it should exclude local databases, manuals, full-text extracts, rendered source pages, indexes, and owner-specific paths/data.
