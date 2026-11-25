# Master view: remaining work (CAMP-14, ROAD-8)

Slices (a), (b) and (c1) are built: the schema and classification, seen marks, the plain-HTML
workspace, the notification hub, the snapshot probe, the event stream, the strict-policy
progressive-enhancement client (DEP-33), the automatic acknowledgement and the loopback browser
suite (`tools/master_browser.py`, in CI's `browser-smoke`). The reference is `docs/campaigns.md`
("Master workspace and seen marks"), `docs/assisted-entry.md` ("Master workspace script and the
opened acknowledgement"), `docs/auth-and-sessions.md` ("Event stream lifecycle"),
`docs/release-scope.md`, CAMP-14 to CAMP-17 and DEP-33 in `docs/decisions.md`, and ROAD-8 in
`docs/limitations.md`. Only slice (c2) remains.

## Slice c2: LAN, loopback and HTTPS-proxy verification

- Test direct LAN HTTP, loopback and the configured root-origin HTTPS proxy: Host and
  Secure-cookie selection, forwarded headers never trusted, actual incremental delivery (not just
  headers), Wi-Fi sleep or proxy idle closure recovering by full snapshot. No public-internet or
  physical-device certification implied.
- Extend `tools/lan_client.py`/`lan_smoke.py` and their helper tests with separate-client
  push/reconnect, denied subnet/wrong Host/foreign Origin, HTTPS Secure cookies and proxy
  flushing/idle behavior. Run against the unpacked artifact, not a dev binary, in CI's
  `lan-smoke` job (the browser suite already runs in `browser-smoke`). Preserve the fail-closed
  gate: formatting, strict Clippy, locked Rust tests, helper lint/tests, audit/notices, release
  packaging/startup/edit/persistence and browser/LAN jobs.
- When it lands, update `docs/release-scope.md` ("Master workspace": the LAN and proxy
  verification), ROAD-8 in `docs/limitations.md`, and delete this plan.

## Risks to resolve before release

Proxy buffering, background tab suspension and HTTP/1 browser connection limits can delay
updates; the fallback status and Reload link must stay truthful. Only checks actually run can
establish release scope.
