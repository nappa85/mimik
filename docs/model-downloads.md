# Model downloads

The owner downloads a model into the models folder from `/admin/assistant` (step 2d-2;
ASSIST-28, ASSIST-35, ASSIST-58, DEP-27). A download is owner-started, goes only to Hugging
Face, is licence-confirmed, bounded, hash-verified and published without ever replacing a file.
It carries no sheet, note or player text: `src/assistant/download/` takes typed specs and
imports no sheet, database, campaign or account type (a test scans for it), and only that
module uses `reqwest` (also scanned). The model client stays on hyper and refuses every
non-loopback URL.

| Concern | Code |
|---|---|
| `hf:` grammar, pinned defaults | `src/assistant/model.rs` (`parse_spec`, `pinned`) |
| Client: HTTPS, redirect policy, resolver, proxy, errors | `src/assistant/download/client.rs` |
| Hub metadata and licence admission | `src/assistant/download/hub.rs` |
| Review snapshot and token | `src/assistant/download/review.rs` |
| Files: safe open, publish, fault points | `src/assistant/download/disk.rs` |
| Task, reconciliation | `src/assistant/download/task.rs` |
| Rows (trait) and the database rows | `src/assistant/download/rows.rs`, `src/assistant/download_rows.rs`, `migrations/023_assistant_downloads.sql` |
| Wiring, routes, panel | `src/assistant/downloads.rs`, `src/auth/assistant_downloads.rs`, `src/auth/assistant_downloads_view.rs`, `templates/admin_assistant.html`, `assets/assistant-downloads.js` |
| Test servers | `src/assistant/download/fake_hub.rs`, `fake_proxy.rs` |

## Spec and storage

`hf:<org>/<repo>/<path>@<revision>`: `org` and `repo` each 1 to 96 of `[A-Za-z0-9._-]`,
starting and ending alphanumeric, without `--` or `..`; `path` segments of 1 to 128
`[A-Za-z0-9._-]` joined by `/`, none `.` or `..`, ending `.gguf`, not a split file; `revision`
exactly 40 lowercase hex digits. URLs are built by path segment, never by concatenation.

A downloaded file is `models/hf-<sha256>.gguf` and is tried and recorded in the model table as
`local:hf-<sha256>.gguf` (the table never holds an `hf:` spec); `assistant_downloads` keeps the
provenance (spec, file name, hash, size, the licence snapshot, who and when, `confirmed` or
`complete`). The two pinned default files (Granite 4.2 3B, Qwen3.5-9B; ASSIST-55) are stored
under their `local:` names so the device default finds them, and must match the hash and size
compiled into Mimik. A file whose hash equals a download's is adopted without a transfer; a
different file under the name is never replaced.

## Review and confirmation

Try with an `hf:` spec, or Review on a pinned default, starts the one live review in the
background (the request returns at once; the page polls). It reads the Hub API at the revision
(JSON of at most 1 MiB, 20 s): the commit equals the revision, `private` and `gated` are false,
the file has LFS data, its SHA-256 and size (1 byte to 8 GiB) are well formed. The licence is
the card's `license` identifier (1 to 64 of `[A-Za-z0-9._+-]`) and the licence file: a
repository-relative path the card names, else a root `LICENSE`, `LICENSE.md` or `LICENSE.txt`;
fetched from `resolve` at the revision (at most 1 MiB, 30 s) and shown escaped. An external
link is shown as text and never fetched; a named file that is missing or oversize refuses the
review; no licence file is "none", allowed with a warning. The result is one immutable
snapshot under a random token, confirmable for 30 minutes; Confirm records exactly that
snapshot. No model-body request happens before Confirm.

## Transfer

One download and one partial file (`models/.partial/<sha256>`, mode 0600, created
exclusively, never through a link; resumed only if regular, Mimik's, not group or other
writable, with one hard link). Free space of the remaining bytes plus 1 GiB is checked at the
start and on every resume. A resume cuts the partial back one 1 MiB chunk (a power-loss tail)
and sends `Range`; a reply that is not 206 with the exact `Content-Range` restarts from zero
once. A body over the confirmed size fails the download and deletes the partial. Connect 30 s,
idle read 60 s, a progress floor of 1 MiB per 5 minutes after which it stops and waits. After
the last byte: `fsync`, a visible `verifying` state hashing the whole file (cancellable), then
publication by `renameat2(RENAME_NOREPLACE)` (or `link` then `unlink` where unsupported), `fsync`
of both directories, and the row set `complete`. A failed directory sync undoes the
publication. Every blocking file operation runs off the async executor; Cancel is accepted at
once even while a connection, a write or a hash is pending. Only a command naming the running
download interrupts it: a Cancel carries the id the page showed (a stale page cancels nothing),
a Discard of another id is answered without touching the transfer, and a second Start, Resume
or Repair is answered "another download is running". A body whose size is not announced, or
differs from the confirmed one, is refused before any byte is kept; a resume is refused while
another download's partial exists.

At start, before any command, the task reconciles the disk with the rows: a publication cut
short is completed (final file present with the right size and hash), a second link left by
the `link` fallback is removed, a fully received partial verifies without a request, and
partials no confirmed download owns are swept. A recovered publication is made durable (the file
and both directories are synced) before it is recorded; a row that cannot be reconciled is
logged and left recoverable, never aborting the start. Nothing is remembered in memory about a publication: a Discard decides about the file from
what is on disk and in the records. The file under the download's name is removed with the
record only when it exists, has the confirmed size, hashes to the confirmed SHA-256, is named
by no other record, is not used by the model setting (active or previous), and is not a pinned
default; otherwise it is left and the notice says so. A retried or late Discard therefore reaches
the same answer. Nothing resumes by itself.

## Gate and delete

A download, a delete, a discard of anything but the running download, a Try, a Switch and a
Switch back take one model-work gate in the runtime
manager: one at a time, never beside a trial (Restart does not touch model files and is not
gated). The running download is discarded under the permit of its own transfer. The active
model serves jobs during a download. The gate is held until the work an
interrupt abandoned (a blocked write, a publication, a cleanup) has finished and any discard
has settled, so nothing changes the models folder outside it; Cancel releases it only then.
Meanwhile a new Start, Resume or Repair is refused ("still finishing"), a Discard waits its
turn, and a Shutdown ends the task at once, leaving the gate to the abandoned work. A discard's
cleanup is tracked work the task polls beside commands, holding its permit and its caller's
answer until it settles; a Shutdown detaches it and tells only the callers still waiting that the
task has ended. Delete
removes a completed download's row and file unless another row names the file, the model
setting (active or previous) uses it, it is the active or previous model, or it is a pinned
default.

## Network policy

HTTPS only, port 443, no user information, host normalised (case, trailing dot) and matched
exactly against `SHIPPED_HOSTS` before a redirect hop is sent, at most two hops, only from
`resolve`; a 3xx from the API is refused; a rejected destination receives no request, also
through a proxy. The system proxy (environment, `NO_PROXY`) is for downloads only, with proxy
credentials only on the CONNECT. The resolver refuses loopback, private, link-local, shared,
multicast, unspecified and reserved addresses on direct connections (a proxy resolves for
itself). IPv6 transition forms are judged by the IPv4 address they carry, not rejected: NAT64
(`64:ff9b::/96` and the local-use `64:ff9b:1::/48`, with the RFC 6052 layout) and 6to4
(`2002::/16`) are how IPv6-only networks reach IPv4 hosts, so a public embedded address passes
and a private or loopback one does not; site-local, IPv4-compatible, Teredo and documentation
space are refused outright. Trust is the operating system's store through the platform verifier (a missing store
is reported with the package to install); tests trust only their own CA. No cookies, no
`Referer`, `Accept-Encoding: identity`, any other `Content-Encoding` rejected, no
decompression. Errors pass through a type that never holds a URL; presigned URLs are never
shown or logged. The host list holds candidates until the real redirect chain of the two
pinned files has been observed by hand (`limitations.md`).

## Pinned panel and repair

The Pinned defaults panel judges a file under a pinned name by its size only (a hash of up to
5 GB is not computed on a page load): "the size matches; its hash is checked when the model
loads", or "a different file has this name". A finished download whose file is gone or has
another size shows "Download again" (`POST /admin/assistant/download/repair`): the row is made
waiting again, keeps its licence snapshot, and the file is fetched and verified like any
download; a different file under the name is never replaced.

## Routes

Owner only, CSRF-checked, strict forms of at most 4 KiB: `POST /admin/assistant/model/review`,
`/admin/assistant/download/{confirm,cancel,resume,discard,repair}`, `/admin/assistant/model/delete`;
`GET /admin/assistant/downloads.json` (state for polling: no URL). Discard deletes the
partial and the row, so a new download needs a fresh review. Download ids are never reused
(`AUTOINCREMENT`), so a stale page's Cancel or Discard cannot reach a later download. Cancel
is acknowledged once the task has matched the running download (a stale id, an idle task or a
finished download does not match); the task is the only writer of the status. A refused
Start, Resume or Repair has its own notice: busy or finishing, another partial exists, no such
download, nothing to repair, or records that cannot be read. A second confirmation while a
partial exists is refused before any row is written.
