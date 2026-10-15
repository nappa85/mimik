# Invitation sharing, readable dates and LAN access

The final sane-default steering below supersedes earlier --auth command references:
plain mimik is authenticated LAN, --no-auth is explicit loopback prototype, and --auth
is removed (no compatibility alias). Database default remains data/mimik.sqlite3 with
automatic directory creation; explicit CLI/environment overrides remain optional.

## User direction

The user requires easy self-deployment on their Wi-Fi/LAN, not loopback-only normal
operation. Sharing should use a copyable link plus QR code, not two duplicate token
fields. All displayed Unix timestamps become readable datetimes. Keep routine use
simple; preserve existing account/campaign boundaries without added approval flows.

## Sharing and presentation

- One primary invitation/reset link, explicit Copy link button with success/failure
  feedback and selectable fallback; optional advanced manual token only once.
- Link token goes in the fragment (`/auth/enroll#token=...` or `/auth/reset#token=...`)
  so the browser opens/prefills the correct form without sending it in GET requests
  or referrers. Small local script removes the fragment with replaceState, prefills
  the existing masked field and never redeems automatically/stores it elsewhere.
- Generate QR locally in Rust from that exact link, with quiet zone/high contrast.
  No external QR service or retrievable credential endpoint. GET/scanning is inert;
  password confirmation plus existing CSRF POST consumes the credential.
- Use platform Clipboard API where available, and native selected-field copy for
  plain-HTTP LAN contexts where async Clipboard API is unavailable. This is a narrow
  Mimik UI adapter, not new clipboard infrastructure. Never report success on failed
  copy; retain visible selectable link and instructions. No duplicate masked field.
- Use existing time crate for RFC3339 and readable UTC fallback, native Intl formatting
  for browser-local datetime/timezone. Cover issued expiry and private history dates.

## Network contract

- Normal CLI `--auth` automatically chooses the primary RFC1918 IPv4 LAN interface,
  advertises its address and serves Wi-Fi clients plus existing localhost access.
  Explicit `--loopback` retains development-only loopback; `--lan-address` can select
  a local private interface when automatic selection is ambiguous. These require
  auth, reject conflicting/subcommand combinations before database access. Anonymous
  prototype stays loopback-only and is never exposed through this change.
- Maintained if-addrs discovers interface IPv4/netmask; std UDP local-address route
  selection sends no packets and is used only to prefer the kernel-selected route.
  If no route is available, use the sole valid private interface or require selection.
- Binding on IPv4 wildcard preserves localhost and LAN access. Auth middleware uses
  real Axum ConnectInfo socket peers: permit loopback or private IPv4 within the
  selected interface subnet; reject other/public peers. Missing peer info fails in
  LAN mode. Forwarded/X-Forwarded headers do not establish authority or peer identity.
- Host allowlist is exactly configured LAN IP:3000 plus documented localhost names.
  Preserve exact single Host/Origin POST checks, CSRF/session/KDF limits and existing
  production Owner/player separation. Links use canonical trusted configured origin,
  not arbitrary request Host/forwarded headers. Mode has no public-address option.
- Initial supported network scope is authenticated HTTP on a trusted IPv4 LAN,
  direct connections; IPv6, public hosting, TLS/reverse-proxy termination are not
  silently inferred. No extra user approval/configuration ritual for ordinary Wi-Fi.

## Maintained-library fit

- qrcode=0.14.1, default-features=false, svg only: MIT OR Apache-2.0, QR encoding/SVG
  and quiet zone, portable pure Rust, no image dependency/runtime service. Registry
  release2024-07-05; nonarchived upstream pushed2025-08-25. Stable narrow capability
  covers encoding; no custom QR implementation. Verify actual audit/notices/features.
- if-addrs=0.15.0: MIT OR BSD-3-Clause, released2026-02-08, Unix libc and Windows
  windows-sys already in graph; handles platform address/netmask enumeration. Small
  Mimik-specific preference/subnet admission uses std socket/address primitives.
- rqrr=0.11.0 without image (dev-only): (MIT OR Apache-2.0) AND ISC, released2026-09-02;
  independent pure-Rust decode of generated bitmap confirms link payload. Never part
  of normal shipping dependency graph. Existing time/Intl cover dates, Axum peer info
  covers actual connection attribution; no vendor protocol/new auth framework.

## Acceptance and release gates

Record independent adversarial plan and implementation reviews. Permanent failing-
first browser regression for duplicate fields/no Copy link, actual insecure-LAN copy
button, fresh recipient link prefill/hash removal/enrollment/reset, no GET mutation,
no token in request URLs/referrers/logs/cookies/audit, QR independent decoding and
desktop/mobile responsive rendering, local/UTC date fallback/timezone bounds.
Unit/process tests validate private address/netmask selection, real peer/subnet/Host/
Origin/forwarded denial, unsafe/missing configuration, loopback/prototype isolation
and all existing role/revocation/race/persistence checks.

Full locked Rust/helper tests, strict lint/audit/licenses/features/default release,
allowlisted sources/vendor packaging, checksums/default equality and unpacked flows,
delivered-source offline empty-cache rebuild. Add isolated container-network server
and separate client/browser integration to CI; it must run fail-closed, with LAN
network isolated from external access. Distinguish those connected-client checks
from a physical phone/router test (not available automatically). Synthetic data only;
no user's database/passwords/cookies/manuals inspected/reset. Install verified binary
atomically and update supported scope/operator instructions/TODO handoff.

## Required independent-review amendments

Reviewer `ses_ec58d4a63ffeSmFUhjprY4xLv6` gave conditional GO with these contracts:
- Recoverable redemption errors retain a valid-shaped token only in the current
  masked form, clear passwords, give useful status/retry feedback and a same-purpose
  fresh-form link carrying the fragment. Mark only trusted credential retry pages
  for422/429/503/expired captured admission responses; unrelated/authentication errors
  retain static normalization. Existing CSRF/credential checks still gate every POST;
  a fresh GET repairs an expired/revoked form session, not an implicit POST admission.
- Validate selected IP against enumerated local interfaces and contiguous masks.
  Preferred route must match a valid interface; otherwise require a unique candidate
  or explicit selection, never first-enumerated fallback. Explicit selections obey
  the same validation. Real socket-peer admission precedes session/database work on
  all routes, missing ConnectInfo fails in LAN mode. Test actual disallowed peers.
- Exactly one allowed Host and one POST Origin equal to that Host's HTTP origin;
  crossed valid origins are denied. Reject conflicting absolute URI authority/scheme
  and duplicate headers; forwarding never changes canonical sharing origin/peer.
- Existing isolated checks explicitly run --auth --loopback. Isolated LAN separate-
  client job verifies unpacked shipping binary and is an explicit fail-closed
  deployable prerequisite. No secure-origin flags or clipboard grants in the plain-
  HTTP LAN copy test. Include actual rendered QR readability, independent payload
  decoding, revoked-session fragment redirects and invalid/out-of-range date fallback.

## External URL steering and retry correction

User additionally requests `external_url` for public links/QR behind a reverse proxy,
not a separate `internal_url`. Add `--external-url` and `MIMIK_EXTERNAL_URL` over the
existing CLI/environment configuration style; internal listener selection is already
covered by normal LAN discovery, --lan-address or explicit --loopback.

- Existing maintained url crate (already locked transitively) parses an optional
  HTTP(S) origin. Reject credentials, query, fragment and non-root path prefixes.
  Canonical links/banner use that origin; no external URL means the detected LAN URL.
- Exact configured external Host/scheme is an additional allowed authority/origin.
  POST Origin still must equal the expected scheme+Host for the specific admitted
  Host (not any member of a whitelist). Never infer scheme/host/peer from forwarding
  headers. Root-origin reverse proxies are supported; arbitrary subpath rewriting
  is not silently claimed.
- TLS may terminate at the reverse proxy. Use existing cookie library to mark
  session Set-Cookie Secure on the configured HTTPS authority, preserving direct
  trusted-LAN HTTP use. This is a narrow trusted-origin cookie-policy adapter, not
  a new session/cookie protocol. Raw headers/cookie secrets are never logged.
- Real backend peer admission remains loopback/selected LAN subnet. Behind a proxy,
  the proxy must enforce original-client network access; Mimik does not claim to
  infer it from untrusted forwarding headers. external_url does not itself expose
  a listener or install TLS certificates.
- Add real configured HTTPS proxy/browser proof, native HTTPS Origin forms, Secure
  cookie and canonical link/QR equality; retain insecure-LAN copy coverage separately.

Fresh implementation reviewer `ses_ec56e2003ffeIRUywwIlmjODIg` found a genuine retry
navigation blocker: a same-path fragment-only link can stay in the old document.
Recovery links must include a fixed non-secret fresh-form query to force document
GET; form POST actions stay on the canonical query-free purpose route. Test actual
late expiry/revocation, GET/fresh CSRF/token prefill/hash removal/empty passwords and
explicit successful submission. A CSRF failure before captured admission may offer
a recovery link only (valid-shaped caller-supplied token, no POST admission/credential
query or session mutation); other unrelated errors remain static.

## Sane-default steering

User requires authentication by default, `--no-auth` as the explicit opt-out, and no
retrocompatibility alias: remove --auth entirely. Normal no-flag serving becomes
authenticated LAN. Development --loopback and --dev-owner-play need no redundant
auth flag. No-auth always selects the existing isolated loopback prototype; reject
LAN/external-url/Owner-play options in that mode. Local operator subcommands remain
separate and do not require network discovery/auth sessions; reject explicitly
supplied serving flags with subcommands before database access. Bootstrap remains
the existing local CLI workflow, not an automatically claimed Owner on the LAN.
Fresh no-flag startup gives clear bootstrap guidance when needed. Update synthetic
helpers to opt out for prototype checks and use --loopback for isolated auth checks.
Permanent CLI regressions cover defaults/removal/conflicts, serving options versus
subcommands and no-auth loopback-only behavior. Operator/CI docs use the new defaults.
