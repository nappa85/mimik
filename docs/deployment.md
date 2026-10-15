# Self-deployment on your Wi-Fi/LAN

## Ordinary startup

After initial Owner bootstrap, run:

```sh
./mimik
```

Authentication and LAN serving are defaults. Mimik uses `data/mimik.sqlite3` under
the working directory and creates its data directory as needed; `--database` or
`MIMIK_DATABASE` is optional. It selects your primary private IPv4 interface and prints the address to open.
Localhost still works on the server. Connect phones/players to the same LAN and
use that printed address or scan an invitation QR code. Account/campaign access
still requires sign-in and the appropriate grants. The standalone runtime needs
no Node.js, Python, model service or internet connection.

For multiple interfaces/VPNs, select the desired local address explicitly:

```sh
./mimik --lan-address 192.168.1.20
```

The address must belong to a local private IPv4 interface with a valid netmask.
The application accepts direct clients from that subnet and loopback. It does not
choose an arbitrary interface by enumeration order. If a phone cannot open the
printed address, check its Wi-Fi network, router client isolation and the server
firewall's TCP3000 rule. Restart after the server's LAN address changes.

Development-only loopback is explicit:

```sh
./mimik --loopback
```

`--dev-owner-play` remains an independent explicit development option. `--no-auth`
explicitly selects the old unauthenticated editor prototype and always binds to
loopback; it cannot be combined with LAN/external-url/Owner-play options. The former
`--auth` flag is removed, with no alias. Operator subcommands remain local operations
and ignore server-only environment configuration. First startup requires one-time
Owner bootstrap; an absent Owner produces concrete CLI guidance, never a LAN takeover.

## Canonical external URL and reverse proxies

Set `external_url` with a flag or environment variable:

```sh
./mimik --external-url https://mimik.example
MIMIK_EXTERNAL_URL=https://mimik.example ./mimik
```

The CLI flag takes precedence. This URL is used in shared invitation/reset links,
QR codes and the startup address, including when Owner uses localhost internally.
Without it, the detected LAN address is used. There is no `internal_url`: listener
selection already controls the internal address, and the proxy has its own backend
address setting.

The external URL must be an HTTP(S) **origin**, optionally with a port. Credentials,
query/fragment and path prefixes are rejected; root-origin proxying is supported.
Mimik does not provision TLS certificates. An existing reverse proxy can terminate
TLS and forward to the Rust application over local HTTP.

For a proxy running on the same host:

```sh
./mimik --loopback --external-url https://mimik.example
```

Typical nginx location in an already-configured TLS server:

```nginx
location / {
    proxy_pass http://127.0.0.1:3000;
    proxy_set_header Host $http_host;
}
```

Preserve the external Host, including a non-default port. Mimik uses the configured
scheme for that authority, accepts matching native HTTPS form Origins and marks its
session cookies Secure. Direct trusted-LAN HTTP remains usable with its own cookie
policy. Forwarded/X-Forwarded headers do not choose the host, scheme or peer.

Backend connections still must come from loopback/the selected private subnet.
With a proxy, configure the proxy's original-client access rules for your LAN;
Mimik observes the proxy's socket address, not a trustworthy original-client address
from headers. For container deployments, put the backend and proxy on the same
private network or use Linux host networking; use the canonical external URL for
the address clients actually reach.

## Sharing and dates

Owner administration shows one copyable link, a **Copy link** button and its QR.
Scanning/opening prefills the recipient's masked code field; the code stays in the
URL fragment rather than entering GET requests/referrers. It is removed from the
address when the form loads. Explicit password confirmation consumes the one-use
credential; opening/previewing the link does not. Optional manual entry is available.

Recoverable errors keep the link in the current page, clear passwords and offer
**Open a fresh form** when session admission needs renewing. Expiry/history dates
use the browser's local datetime/timezone, with a readable UTC fallback.

Supported verification scope: Linux x86_64 trusted IPv4 LAN and configured root-origin
HTTP(S) proxy workflows, synthetic separate clients and Chromium desktop/mobile
viewports. Physical phones/router settings and other platforms/browsers require
their own verification. Owner source inspection/review is at `/admin/materials`;
imports and publication remain local CLI workflows. Account recovery/re-enable, uploads and remaining
character lifecycle features retain their existing roadmap status.
