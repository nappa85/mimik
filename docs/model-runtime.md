# Model runtime

The interpretation assistant's model server (step 2c-2a; ASSIST-27, ASSIST-29 to ASSIST-34,
ASSIST-40 to ASSIST-42): with `--assistant`, Mimik starts the pinned official llama.cpp
`llama-server` as its own child process, confined at the highest level that works, reaches it
only through a checked endpoint with a per-launch token, and primes and calibrates it before
any job runs. Linux `x86_64`, CPU build only. The GPU path and confinement level 2 (Landlock)
are step 2c-2b (`assisted-sheet-plan.md`); macOS and Windows are not built. What the assistant
does with the server is in [assisted-entry.md](assisted-entry.md), "Interpretation assistant".

| Concern | Code |
|---|---|
| Pinned build | `schemas/llama-runtime-v1.json`, `src/assistant/runtime/manifest.rs`, `tools/llama_runtime.py` |
| Verified descriptors (model, runtime files) | `src/assistant/runtime/files.rs` |
| Runner, spawner thread | `src/assistant/runtime/runner.rs` |
| Seccomp program | `src/assistant/runtime/seccomp.rs` |
| bubblewrap preflight and profile | `src/assistant/runtime/bwrap.rs` |
| Start-time probe, `assistant-runner` entry | `src/assistant/runtime/probe.rs` |
| Launcher (levels 1 and 3), the child's lifetime | `src/assistant/runtime/launch.rs` |
| Endpoint check | `src/assistant/runtime/endpoint.rs`, transport in `src/assistant/transport.rs` |
| Token | `src/assistant/runtime/token.rs` |
| Pidfile, generation directories | `src/assistant/runtime/rundir.rs` |
| Output drain | `src/assistant/runtime/drain.rs` |
| Priming, calibration, budgets | `src/assistant/runtime/calibration.rs`, `server.rs`, `budgets.rs` |
| Generations and transitions | `src/assistant/runtime/manager.rs` |
| Thread count | `src/assistant/runtime/threads.rs` |
| Owner page | `src/auth/assistant.rs`, `templates/admin_assistant.html` |

## Options

`--assistant` turns it on (off by default: no child, no probe, no route, the page says
"Off"). `--assistant-model` (`local:<name>.gguf`, default the Granite 4.2 3B `Q4_K_M` file
with its SHA-256 compiled in), `--assistant-model-sha256` (required for any other file),
`--assistant-model-alias` (default the file name without `.gguf`, lower case). `--models-dir`
(default `<data dir>/models`, mode 0700, owned by Mimik's user; the model is placed there by
hand). `--assistant-runtime` (default `llama/` beside the binary; it must hold exactly the
pinned files either way). `--assistant-threads` (default the usable physical cores minus one,
at least 1; physical cores are the distinct `core_cpus_list` sets in sysfs among the CPUs of
Mimik's affinity mask, capped by its cgroup's `cpu.max` quota, ASSIST-41). The data
directory is the database's folder; `run/` lives there.

## Pinned build

The manifest pins release b11609 (commit `fcac190`), its CPU asset
`llama-b11609-bin-ubuntu-x64.tar.gz` (size and SHA-256), the 22 files Mimik runs from it
(each under its SONAME, no links: `llama-server`, `libllama-server-impl.so`,
`libllama-common.so.0`, `libmtmd.so.0`, `libllama.so.0`, `libggml.so.0`, `libggml-base.so.0`,
the 14 `libggml-cpu-<variant>.so` backends and `LICENSE`; the RPC backend is not shipped)
with size, SHA-256 and mode, the system libraries they need (`libc.so.6`, `libm.so.6`,
`libstdc++.so.6`, `libgcc_s.so.1`, `libgomp.so.1`, `libssl.so.3`, `libcrypto.so.3`; symbol
floors GLIBC 2.34, GLIBCXX 3.4.30, OpenSSL 3.0.0), the documented minimum host (Ubuntu 22.04
`x86_64`), the source archive (`git archive` of the commit, tar SHA-256) and the CI test model
(`ggml-org/models-moved` at `499bc88…`, `tinyllamas/stories260K.gguf`, upstream MIT). ggml
loads every `libggml-cpu-<variant>.so` beside the server and uses the best one the CPU
supports. Mimik embeds the manifest (a test asserts it equals the file).

## Files

The runtime directory's `cpu/` must hold exactly the manifest's files: an extra, missing or
changed (size or SHA-256) file, a link, or a file or folder writable by group or others
refuses the start with that reason (owner Mimik's user or root). The model and every runtime
file are opened beneath their folder's descriptor with `openat2` (`RESOLVE_BENEATH`,
`RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS`) and `O_NOFOLLOW | O_NONBLOCK`, checked with
`fstat` (regular, owned by Mimik's user, not writable by group or others) and hashed from that
descriptor; a hash is reused while the `fstat` identity (device, inode, size, modification and
change times) is unchanged. The child loads those descriptors, so a file swapped after the
check is not what runs. Level 3 also requires the runtime directory and every ancestor not to
be writable by group or others (it loads the libraries by path). A model larger than
`MemAvailable` minus 1 GiB is refused ("not enough memory for this model").

## Runner

Mimik spawns `mimik assistant-runner --spec-fd 3` from one long-lived thread (the
parent-death signal follows the spawning thread); the spec (parent PID, kept descriptors,
the exact environment, the exec) arrives on descriptor 3. The runner sets `PR_SET_PDEATHSIG`
(SIGKILL), exits if its parent is no longer Mimik, sets `RLIMIT_CORE` 0 and `RLIMIT_FSIZE`
64 MiB (a larger write kills the server with `SIGXFSZ`), closes every descriptor it was not
handed, and executes in place with only `PATH`, `LANG` and `HOME`: `bwrap` on level 1; on
level 3 the verified llama-server descriptor, moved to number 1023 (close-on-exec) with
`RLIMIT_NOFILE` set to 1023, after installing the seccomp program, by `execveat(1023, "",
…, AT_EMPTY_PATH)`. Descriptors are moved to their numbers between fork and exec with
`fcntl(F_DUPFD_CLOEXEC)` then `dup2` only.

## Confinement levels

Tried in order; the active level and why each other level was skipped are on the page and in
the log.

1. **bubblewrap.** `bwrap` from `/usr/bin/bwrap` or `/bin/bwrap` (never `PATH`), refused if
   set-ID or carrying file capabilities; every option used is checked in its help text.
   Only `user.max_user_namespaces = 0` rules the level out before its probe; the probe
   decides otherwise, and a failed probe names `kernel.unprivileged_userns_clone = 0` or
   `kernel.apparmor_restrict_unprivileged_userns = 1` as a possible cause (a distribution's
   AppArmor profile for `bwrap` may still allow it). Profile:
   `--unshare-all --die-with-parent --new-session --clearenv` with `PATH`, `LANG` and `HOME`
   (`/tmp`), `--cap-drop ALL`; read-only binds of `/usr/lib`, `/usr/lib64`, `/lib`, `/lib64`
   (or their links), `/etc/ld.so.cache` and `/sys/devices/system/cpu`; `--dev /dev`, `--proc
   /proc`, `--tmpfs /tmp` (64 MiB) and `--tmpfs /dev/shm` (256 MiB) with `--size` when the
   installed bwrap has it (otherwise the page says "temporary space unbounded"); the
   generation's directory bound read-write by descriptor at `/run/llama`; each runtime file
   bound read-only by descriptor under `/opt/llama`; the model by descriptor at
   `/models/model.gguf`; the token by `--ro-bind-data` (mode 0400) at `/run/token`; the root
   remounted read-only; the seccomp program by `--seccomp`; `--json-status-fd`, whose first
   line gives the sandbox's PID-1 reaper (`child-pid`) and its PID namespace. Nothing else of
   the host is visible. The command inside is Mimik's own binary (bound by descriptor) as a
   shim: it writes a marker on a dedicated pipe, closes every descriptor but 0–2 and executes
   llama-server (or the probe). The server listens on `/run/llama/llama.sock`.
2. **Landlock with seccomp:** not in this release (step 2c-2b); the page names it skipped.
3. **Unconfined (seccomp only)**, on `127.0.0.1` with a random port: "not confined: TCP
   egress allowed, and the whole filesystem readable by Mimik's user is readable by the model
   server". The model is `--model /proc/self/fd/8`; the token is a 0600 file in the
   generation's 0700 directory, deleted once `/health` answers; `HOME` is an empty private
   directory there. A child that cannot bind its port exits and a new port is tried, at most
   3 per attempt.

**Seccomp (both levels), pure Rust** (`seccompiler` plus four hand-written first
instructions, which `seccompiler` cannot express): anything but `AUDIT_ARCH_X86_64` is
killed; every x32 syscall number is refused; `clone3` answers `ENOSYS` (glibc falls back to
`clone`). Default allow; `EPERM` for `fork`, `vfork`, `clone` without `CLONE_THREAD`,
`socketpair`, sockets other than TCP over IPv4 or IPv6 (protocol 0 or TCP) and stream Unix
sockets, `MSG_FASTOPEN` on `sendto`, `sendmsg` and `sendmmsg`, `TCP_FASTOPEN_CONNECT`,
`pidfd_open`, `pidfd_getfd`, `process_madvise`, `kcmp`, `ptrace`,
`process_vm_readv`/`writev`, `io_uring_*`, `bpf`, `perf_event_open`, the key calls, mounts and
the new mount API, `pivot_root`, `unshare`, `setns`, module and `kexec` calls, and
`prctl(PR_SET_PDEATHSIG)` and `prctl(PR_SET_NO_NEW_PRIVS)`. On level 3 `execve` is refused and
`execveat` is allowed only on descriptor 1023 with exactly `AT_EMPTY_PATH`: that descriptor is
close-on-exec and no new descriptor can take its number (the limit is 1023 and an
unprivileged process cannot raise it), so no exec is possible after the first, from any
thread. Seccomp sees no `connect()` address and is not network confinement.

The start-time probe (`mimik assistant-runner --probe-inner`) runs Mimik itself in place of
the server under each level's full profile (the same binds, descriptors, seccomp program and
token channel): it reads the model's magic, the token and binds the socket or port, reports
one JSON line and answers one connection, on which the endpoint check runs. `mimik
assistant-runner --probe --runtime <dir> --models-dir <dir> --model local:<name>
[--model-sha256 <hex>] [--data-dir <dir>]` prints the result as one JSON line and exits 0 only
at level 1 (the CI and owner check). A probe that fails for a definite cause skips its level;
one that is only inconclusive (no report within 30 s, a missing pidfd) is tried again, three
times in all, and a level that stays inconclusive is kept for the real start to decide.

## Endpoint check

Every connection is checked before its first byte, and the token goes nowhere else. The
transport is `hyper`'s HTTP/1 client connection over a stream Mimik opened itself (no pool,
proxy, redirect, retry or decompression exists to turn off). Level 1: Mimik checks that `llama.sock` in the generation directory is a socket (not
following links) and connects through `/proc/self/fd/<directory descriptor>/llama.sock` (no
`sun_path` length limit); `SO_PEERCRED`
must give Mimik's UID and a PID that is the reaper or whose `PPid` is it, in the reported PID
namespace (`/proc/<pid>/ns/pid`, or, where unreadable, `NSpid`), exactly one namespace deep with
a small inner PID, while the reaper's pidfd shows it alive. Level 3: before and after
`connect()`, every listening socket that could take a connection to `127.0.0.1:<port>`
(`127.0.0.1`, `0.0.0.0`, `::`, `::ffff:127.0.0.1` in `/proc/net/tcp` and `tcp6`) must be open
in the child's `/proc/<pid>/fd`; none yet is "not ready", any foreign one fails the attempt.
The `/proc` scans run off the async runtime.

## Token

32 bytes from the OS random source, base64url, new for every launch, held in memory and zeroed
when dropped; never on a command line or in a log. llama-server reads it with
`--api-key-file`; `/health` is the only request without it.

## Output

The child's stdout and stderr are drained by two threads with a 4 KiB line bound and never
copied into Mimik's log. Fixed patterns update a fixed-size summary (so memory stays bounded
however much the child prints): bind failures, listening, the build number, the offload line,
device and model-load errors (which name a failed start). Nothing the child prints can make
an exit a sandbox failure: at level 1 that is decided only by the shim's marker never
arriving (bwrap failed before the exec); once the marker arrived, or when it cannot be read,
the exit counts at the same level. An exit is classified after both readers ended (5 s at
most).

## Generations

Each launch has a generation number; its directory `<data dir>/run/<generation>/` is created
0700 (anything already there refuses the start) and removed with its contents, never
following links, when the generation stops. The pidfile `run/assistant.pid` is held under an
exclusive `flock` for Mimik's life ("another Mimik uses this data directory's assistant"); it
records the child's PID, start time and possible executables; at start a leftover's pidfd
is opened first, the three are checked after that, the kill goes through the pidfd and the
sweep waits (5 s at most) for it to end, then old generation directories are removed. A child
is owned from its spawn: any failure before the start completes, or dropping an unfinished
start, kills and reaps it. One manager
task serializes every transition, so two servers never coexist: a child is stopped with
SIGTERM through the runner's pidfd (on level 1 to bwrap, which takes the sandbox down with
it) and SIGKILL after 5 s, repeated until gone; it is gone, and its exit is published, only
when the runner has exited and, on level 1, the reaper's pidfd shows it gone. Nothing stops a
running child but a stop. On every
generation change, queued and running jobs end "the assistant restarted: ask again" (neutral
for the breaker); a child exit is never a breaker event. On shutdown Mimik stops the child.

An attempt is: start (preflight once per start from level 1: the pidfile, the run directory,
the model's size; then each level's probe), spawn and the endpoint check, `/health` within
60 s (250 ms polls), priming within 300 s, calibration within twice the path's run cap
and each calibration sample within the run cap (a sample past it makes the machine "too slow",
not a counted failure). CPU
transitions:

| Event | Action | Failure count |
|---|---|---|
| Sandbox-attributable failure (bwrap setup, no status before exec) | next level that passed its probe, at once | not counted |
| Any other failed attempt (model load, a changed file, an exit after exec, a refused endpoint, no `/health`, failed priming or calibration, invalid timings) | retry after 2 minutes at the same level | +1 |
| Child exits while ready | restart now | +1 only within 10 minutes of `ready` |
| Two consecutive run-deadline expiries without a completed exchange between | stop as hung, restart, measure again | as a child exit |
| Failure count reaches 4 | `unavailable` until Restart | — |
| Budgets do not derive | `unavailable` ("too slow on this machine: about N s needed, the cap is M s"), no retry | — |
| `ready` for 10 minutes | count back to 0 | reset |
| Restart (or Mimik's start) | stop, start again from level 1, breaker reset; it interrupts a preflight, probe or start in progress, and further presses are ignored until `ready` or `unavailable` | reset |

While `starting` or `unavailable` a job start answers 503 with the state and queues nothing;
entry, notes and Undo work as usual. Log lines (`assistant: …` on standard error) carry
states, levels, reasons and measurements, never prompt text, URLs with tokens or the token.

## Priming, calibration and budgets

Priming is the readiness gate: the fixed prefix with a minimal envelope and `max_tokens` 1
fills the prompt cache before any job is admitted. Then two samples, non-streamed, `max_tokens`
the model's, `ignore_eos`, no schema, each with a filler of exactly the path's byte cap (CPU
1,024) made of context items shaped as requests carry them, from the labels and values of the
shipped demo and 2b synthetic sheets (one opens with `[`, the other with `{`, so neither
reuses the other's cache), each counted by `/tokenize`. Timings must be present, finite and
positive, `predicted_n` equal to `max_tokens`, the tail `prompt_n` − Nᵢ not negative and the
cached tokens at least 75 % of priming's prompt tokens, or the attempt fails. The budgets
follow `assisted-sheet-plan.md` §1.4a "Budgets" (ASSIST-31, ASSIST-32): T(B) is the larger of
the two samples' estimates per candidate (CPU 1,024, 768, 512), R = max(15, 1.25 × T rounded up
to 5 s) within 90 s, D 1, Q = 120 − R. After a run-deadline expiry, or when a running job
is superseded or cancelled, the next job waits the larger sample's prompt term at B (the request may still be in prompt evaluation), and
admission counts that wait, so one expiry no longer makes the next job miss its deadline too.
The measurement and the budgets are on the page.

## Owner page

`/admin/assistant` (owner only): the state with its reason, the pinned build, path,
generation, failure count and last failure, the breaker, the active level with its wording,
notes, skipped levels with reasons, the budgets, and the priming and sample measurements.
Restart (POST, CSRF-checked) stops the server and starts again from level 1; a second press
while one runs starts nothing ("A restart is already running").

## Development endpoint

Builds with the `dev-endpoint` feature (never a release) keep `--assistant-dev-endpoint` and
`--assistant-dev-token-file` for tests: a hand-started server (or a fake) as the manager's
only child, primed and calibrated the same way, without an endpoint check (the page says so).
