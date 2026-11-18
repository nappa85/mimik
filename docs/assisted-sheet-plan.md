# Assisted sheet plan: remaining steps

Direction (user, 2026-10-19): "Help the user as much as we can, but don't force anything;
improve paper and pencil, don't replace it." The player writes what they want; the app
computes, warns, suggests and offers Undo. Model output is only ever a confirmable proposal;
legality stays advisory and Rust does all arithmetic.

Decision IDs (D-…, U…, P…) are defined in `docs/decisions.md`. What is built (number entry,
notes and scribbles, switch and Undo, unknown values, per-field save, the busy page, the page
bound, quick-add, the validity flag, the sheet module with save on leaving a field, the
interpretation assistant of development builds) is described
in `docs/assisted-entry.md`; this plan only adds to it.

Remaining, in order (2b measured and reviewed, `docs/results.md`; ASSIST-27 to ASSIST-56
decided; Rust 1.99.1 and `rig-core` 0.45.0 in place, ASSIST-39; 2c-1, interpretation against a
hand-started local server, built, `docs/assisted-entry.md`; 2c-2a, the CPU runtime, 2c-2b-1,
the GPU path, and 2d-1, the owner model setting with local models, built,
`docs/model-runtime.md`; 2d-2, model downloads, built, `docs/model-downloads.md`): **3** bundles and reminders, **4**
suggestions and "Which Sword?", **5** docs and gate; each sub-step has its own branch, tests and reviews. The model runs GPU-first with
the CPU as the intended baseline (ASSIST-27; whether the default model meets the CPU caps at
the ASSIST-41 thread default is set by the native calibration), as an official `llama-server` build that Mimik
starts as a child and confines in the Hakoniwa sandbox where it can, never a container in production
(ASSIST-29, ASSIST-30, ASSIST-34); budgets come from a measurement at each start within fixed
caps (ASSIST-31, ASSIST-32). Linux x86_64 is the platform built and verified now.

Every step: its own branch; failing-first tests with the red run recorded; independent
implementation review before merge; every new dependency recorded with its licence, MSRV and
maintenance in the dependency decisions (for example `seccompiler`, `procfs`,
`rustls`, `rustls-platform-verifier`, and `hyper` if the Unix connector needs it); built and tested on 2 cores on weekdays; GPU and live
model runs only at night or at weekends (machine schedule). Each step keeps
the demo (`tools/assist_demo.py`) usable. Any step that adds per-cell markup reruns the
worst-case page fixtures in `src/campaigns/list_tests.rs` (tightest headroom today 2.0%, a new row kept read-only from a full list) and
never enlarges the budget.

**POC cuts** (no security or integrity risk): player `*` and `/`; interpreting text, enum or
boolean cells; notes and Undo for text cells; seals and note verification (D-T); dismissal
records; Undo of creation, row add and moves; forced Undo; live preview; a keyed rate
limiter; Gemma; changes to the native MÖRK BORG editor. Non-goals: legality rulings,
automatic application of model output, chat, manual RAG, `catalogue_ref` membership checks,
hosted providers, suggesting models other than the two pinned defaults, gated or private Hugging Face
repositories, split (multi-file) GGUF models.

## 1. Design for the remaining steps

### 1.4 Interpretation service

Built in 2c-1 and described in `docs/assisted-entry.md` ("Interpretation assistant"): the
`Interpreter` trait, stub and `rig-core` adapter over the bounded transport, schema v2 and the
fixed prefix, the phrase-number lexer, reply validation, the request bounds and related-fields
context with formula read sets, the runtime event interface, jobs, admission, the note-write
map, the breaker, the start, poll and apply routes, the note key, and the module's flow. Later
steps add:

- **2c-2:** the runtime paths of the client policy: on the sandbox path the destination is
  the current generation's socket path and nothing else; on the TCP paths the URL host must be
  the IP literal `127.0.0.1` (`::1` stays accepted only by the development endpoint) and the
  port the current child's; every connection is checked to reach the current child before its
  first byte (§1.4a, "Endpoint"; `reqwest`'s Unix-socket connector has no per-connection hook
  besides `connector_layer`, so the check's placement is a 2c-2 design point), and the token
  goes only there. The runtime manager feeds the event interface (generation started, ended
  with its reason, ready with B, N(B), R, D, Q); a child exit is never a breaker event. The
  owner's assistant page shows the runtime state and the breaker; priming, calibration and the
  2d trial are outside the breaker; only Restart and the app's start reset it.
- **2d-2 (built):** the binary contains TLS for downloads (`reqwest`, DEP-27), so the model
  client's plain-HTTP property rests on its URL guard, not on the dependency tree.
- **3:** reminder jobs have their own context and records (§1.5, step 3c) and a low-priority
  slot on the same worker, after interpretations, one per account, behind the owner's setting
  (ASSIST-60); the
  catalogue provenance extension (§1.5); bundles in the proposal freshness binding and in apply
  (step 3b). Interpretations keep prompt format D without catalogue text (step 3, deviation).
- **Not built:** token log-probabilities, if the runtime returns them, are not recorded on the
  job record (evaluation only; nothing is gated on them).

### 1.4a Model runtime, device and budgets (2c-2; ASSIST-27, ASSIST-29 to ASSIST-34, ASSIST-40)

Built for the CPU path (step 2c-2a) and the GPU path with its device choice and fallback
(step 2c-2b-1), at level 1 (level 3 built but not used, ASSIST-43):
[model-runtime.md](model-runtime.md) is the reference and wins where it differs from this
design.

Linux x86_64 is the only platform built and verified in 2c-2; macOS (Metal) and Windows
(Vulkan) runtime code is deferred with no platform designed out (ASSIST-34, §4 Later).
Production never needs a container (ASSIST-29); on the dev host the pinned official build runs
natively for these tests (ASSIST-40).

- **Pinned build (ASSIST-33):** a manifest, the data file `schemas/llama-runtime-v1.json`
  embedded into Mimik with `include_str!` (a test asserts the embedded copy equals the file),
  also read by `llama-fetch` and by packaging, so CI and the binary verify the same set, pins one llama.cpp build (b12110, ASSIST-42) for Linux x86_64: the official GitHub release assets for the Vulkan and
  the CPU build (`llama-b12110-bin-ubuntu-vulkan-x64.tar.gz`, SHA-256 `a5d74194…`, and
  `llama-b12110-bin-ubuntu-x64.tar.gz`, SHA-256 `368227ed…`; gzip tarballs), each asset's SHA-256, and the path and
  SHA-256 of every file Mimik runs from them. Before 2c-2 depends on them, the manifest entry
  records the exact asset names, the system libraries each binary needs (from its ELF
  dependencies: glibc version floor, `libvulkan.so.1`, others), how the CPU backend is chosen
  for the host's instruction set, and the documented minimum host; the release scope names
  that host. The runtime directory must hold exactly the manifest's files (an extra or missing
  file refuses the start), and neither it nor its files may be writable by group or others.
  On level 3 the shared libraries beside llama-server are loaded by path from that
  verified directory, inside the same-user boundary below.
- **Model and runtime files (verified descriptors):** the model is opened relative to the
  models folder's descriptor with `openat2` (`RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`,
  `RESOLVE_NO_MAGICLINKS`) and `O_NOFOLLOW | O_NONBLOCK` (a FIFO cannot block), then `fstat`:
  a regular file owned by Mimik's user, not writable by group or others. Its SHA-256 is
  computed from that descriptor before the first load and again whenever its `fstat` identity
  (device, inode, size, modification and change times) differs from the last hashed one, so a
  retry does not re-read a 2 GiB file; that descriptor stays open and is what the child loads: level 1 binds it read-only (`--ro-bind-fd`); level 3 passes it inherited, as
  `--model /proc/self/fd/<n>` (a live check confirms that the pinned server's open and `mmap`
  accept that path). Runtime files are hashed the same way; level 1 binds them by descriptor
  and level 3 execute the verified llama-server descriptor (`execveat`). So the model and
  the executable cannot be swapped between check and load on any level; on level 3 the
  shared libraries are still loaded by path, so the runtime directory and every ancestor
  directory must not be writable by group or others (checked at start), and the library
  guarantee there is the same-user boundary below. Threat boundary: a process running as
  Mimik's user could still rewrite an inode in place; that is outside the boundary (it could
  rewrite Mimik itself). Windows reparse points and path rules come with the Windows runtime.
- **Flag vector:** the 2b vector (`tools/model_registry.py` `server_args` plus the screen's
  extra flags), with only the transport and device changed (ASSIST-30): `--model <verified
  descriptor path>`, `--alias <label>`, `--host <transport>` (the socket path or
  `127.0.0.1`), `--port <p>` (TCP levels only), `--offline --no-webui --no-slots --no-mmproj
  --jinja`, on the GPU path `--n-gpu-layers 99 --device <name>` (the selected device, below),
  on the CPU path `--n-gpu-layers 0`, `--fit off`, `--threads <n> --threads-batch <n>`
  (ASSIST-41: default the physical cores minus one, at least 1, configurable; 2b measured 2), `--ctx-size 8192 --parallel 1 --no-warmup --reasoning off`, and `--cache-ram` (0 on the GPU path; on the CPU path room for two whole-context entries, `launch::cache_ram_mib`), and the token (below). Measurement-relevant (a change needs a new
  measurement before it ships): `--jinja`, `--n-gpu-layers`, `--fit`, threads, `--ctx-size`,
  `--parallel`, `--cache-ram`, `--reasoning`, plus the per-request `cache_prompt: true`,
  `stream: true` and the greedy sampler (temperature 0, top-k 1, seed 0). `--parallel 1`: jobs
  run one at a time (ASSIST-32). The pinned b12110 `--help` (checked in a container,
  2026-11-06) lists `--api-key-file`, `--api-key` with the environment variable
  `LLAMA_API_KEY`, `--ignore-eos`, `--slots`/`--no-slots`, `-dev`/`--device`,
  `--list-devices`, and `--host` accepting Unix socket paths. The runtime check (CI, below)
  runs the pinned binary's `--version` and `--help` for every flag; behaviour that help text
  cannot show is checked live against the pinned binary, and a mismatch stops 2c-2 for a
  decision: serving and the token over the Unix socket, `/health` without a token, the
  per-request `ignore_eos`, the `/proc/self/fd` model path, the device list format and the
  offload line.
- **Packaging and CI (ASSIST-33, REL-2):** `package_release.py` gains an allowlist entry
  `llama/` holding exactly the manifest's files (anything else refuses the package), the
  llama.cpp build number in `BUILD.txt`, and `THIRD-PARTY-NOTICES.md` entries for llama.cpp
  and ggml (MIT) and for every other file shipped from the asset, by its own licence. ASSIST-47:
  the release does not ship llama.cpp's source; `SOURCE.txt` and `BUILD.txt` point to the
  exact upstream tag, commit and asset hashes. The pinned `git archive --format=tar` of the
  commit (its tar SHA-256 in the manifest) is a `llama-fetch` build-time input for the licence
  texts only. The `dev-endpoint` feature is absent from the release
  binary because the staged binary comes from `cargo build --release --locked` without
  features (tests build with `--all-features`); `smoke_release.py` in `artifact-smoke` checks
  the staged binary: it contains no `assistant-dev-endpoint` string, and the option is
  refused as unknown.
- **Test model (CI only, never shipped):** Hugging Face `ggml-org/models-moved` at revision
  `499bc8821c6b12b4e53c5bffcb21ec206f212d81`, file `tinyllamas/stories260K.gguf`, 1,185,376
  bytes, LFS SHA-256 `270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d`. That
  repository states no licence; its upstream, `karpathy/tinyllamas` at revision
  `0bd21da7698eaf29a0d7de3992de8a46ef624add`, is MIT. Both are recorded in the workflow and the
  manifest's test section. Only `llama-fetch` downloads it.
- **CI workflow (2c-2 deliverables, `.github/workflows/ci.yml`):**
  - a new required job `llama-fetch` (in the `mimik-ci` container like `checks`, with
    `timeout-minutes: 15`): fetches the
    pinned assets, builds the source archive with `git archive` of the pinned commit, fetches
    the test model, verifies every hash, unpacks the assets (gzip tarballs, with Python's
    `tarfile` refusing links outside the archive, under `target/`), and uploads the verified `llama/` files, the source archive and
    the test model as artifacts;
  - `checks` gains `needs: llama-fetch`, downloads that artifact before packaging, packages
    only it (since `upload-artifact` drops file modes, every job that unpacks re-hashes each
    file against the manifest and reapplies the manifest's modes), and uploads the
    `linux-release` tarball with its `SHA256SUMS` (as today) and the confinement test binary,
    built with the workflow's own flags (`cargo test --locked --all-features --no-run
    --message-format=json`) and located from that output (built in the Ubuntu 22.04 image, so
    it also runs on 24.04);
  - a new required job `llama-runtime` with `needs: [llama-fetch, checks]` (so there is no
    cycle: `checks` never waits for `llama-runtime`), on a runner pinned by label
    (`ubuntu-24.04`), with `timeout-minutes: 30`, as host steps, not inside the `mimik-ci`
    container (Docker's default seccomp profile refuses the namespaces the sandbox
    needs). It uses no Rust toolchain. It verifies the `linux-release` tarball
    against `SHA256SUMS`, unpacks it under `target/`, reapplies the manifest's modes, and runs
    everything from that unpacked release. If `kernel.apparmor_restrict_unprivileged_userns` exists it logs its value and lifts it with
    `sudo sysctl` for the job's duration only (an absent key is logged and skipped). It then
    runs:
    - the flag check;
    - the start-time probe for the CPU build only, as `target/rel/mimik assistant-runner
      --probe --runtime target/rel/llama --models-dir target/models --model
      local:stories260K.gguf --model-sha256 270cba1b…`, which prints one JSON line (level
      reached, per-resource results, process shape) and exits 0 only at level 1; anything
      lower fails the job (CI covers the CPU build only; the GPU path is a dev-host
      measurement, and release builds never accept software renderers);
    - the real CPU-build llama-server under the level-1 profile with the test model, started
      by the confinement test binary through the runner with the same arguments: `/health`
      without a token, token refusal and acceptance over the Unix socket in
      `target/data/run/<generation>/`, the model bound from the verified descriptor
      (`--ro-bind-fd`), one completion, the peer-shape assertion, and cancellation
      mid-generation. The test model's `n_ctx_train` is recorded with its pin, and the CI run
      uses a `--ctx-size` and a short prompt that fit it (the fixed prefix is not sent); its
      first run is itself a live check of the model's behaviour under `--jinja` and that
      context;
    - the confinement test binary (seccomp, endpoint and adversarial-helper tests) against
      the unpacked release's runtime; the adversarial helper is that same binary re-executed
      with a helper flag; the binary reads nothing from the source tree (fixtures embedded);
      it is run as `target/confinement-tests --exact` with the confinement test filter;
  - `deployable` requires `llama-fetch`, `checks` and `llama-runtime` with the existing jobs;
    every new action is pinned by commit SHA like the existing ones;
  - stub-transport integration coverage runs in a test build with the `dev-endpoint` feature,
    labelled as such and never called verification of the shipped binary;
  - not CI-covered, and said so: the GPU path (probe, device listing, live GPU checks),
    mid-prompt cancellation timing and the
    per-request `ignore_eos` with the real default model are dev-host measurements, repeated at
    every pin change and recorded in `docs/results.md`.
- **Runner:** Mimik spawns `mimik assistant-runner` (a hidden subcommand) from one dedicated,
  long-lived thread (the parent-death signal follows the spawning thread). The runner sets
  `PR_SET_PDEATHSIG` (SIGKILL), then checks that its parent is still the Mimik PID it was given
  and exits otherwise (the race before the signal is set); sets `RLIMIT_CORE` to 0 and
  `RLIMIT_FSIZE` to 64 MiB (a write past it raises `SIGXFSZ`, left at its default action, so
  the server is killed and the exit is handled as a child exit); clears its environment except a fixed minimal set (`PATH`,
  `LANG`, `HOME` pointing at an empty private directory, the token variable when that channel
  is used); keeps only the descriptors it was handed (Rust opens everything close-on-exec; the
  hand-over descriptors are listed by number); then executes the confinement level's command
  in place (the Hakoniwa sandbox built in the runner on level 1, `execveat` of the verified llama-server descriptor on
  level 3; either way the parent-death signal stays set). On level 3 the runner sets
  `PR_SET_PDEATHSIG` and `PR_SET_NO_NEW_PRIVS` before it installs the seccomp filter, and the
  filter refuses any later `prctl(PR_SET_PDEATHSIG)` and `prctl(PR_SET_NO_NEW_PRIVS)` call,
  so a compromised server cannot clear the signal; on level 1 the runner stays as the sandbox's
  supervisor, and the PID namespace contains the tree.
- **Confinement levels (ASSIST-34), tried in order; the active level and the reason for each
  skipped level are shown on the assistant page and logged:**
  1. **Hakoniwa** (ASSIST-51), unprivileged (user namespaces): the runner builds new user,
     mount, PID, IPC, UTS, cgroup and network namespaces (an empty network namespace: no
     network at all), a root of read-only binds of the system library directories, the Vulkan
     loader and driver files and the runtime files and model (each checked against the
     identity the launcher verified), bounded tmpfs mounts, a minimal `/dev` (on the GPU path
     plus the selected render node only), the token in the sandbox's memory-backed `/run`, a
     Landlock layer (ASSIST-49) and the seccomp program; the child listens on a Unix socket at
     `/run/llama/llama.sock`, a bind of a per-generation directory in Mimik's data directory
     (`<data dir>/run/<generation>/`, mode 0700, opened with `O_NOFOLLOW | O_DIRECTORY`),
     which Mimik reaches through `/proc/self/fd/<dir fd>/llama.sock`. A compromised server can
     write files into that directory, each bounded by `RLIMIT_FSIZE` but not in number
     (ASSIST-L26). The exact list and the reasons are in [model-runtime.md](model-runtime.md).
  3. **Unconfined** (seccomp only, TCP on `127.0.0.1`), used only when the owner has
     explicitly allowed it (ASSIST-43; not in 2c-2a): "not confined: TCP egress allowed, and
     the whole filesystem readable by Mimik's user is readable by the model server".
  There is no level 2 (ASSIST-48). The assistant runs at level 1, and at level 3 only with the
  owner's explicit permission (ASSIST-43; hardening is "where available", ASSIST-30); the page
  shows the level next to the assistant state and in the Model section beside "Try" (a model
  file is untrusted input to llama-server, contained only by this).
- **Seccomp (every level):** pure Rust only, adding no runtime shared-library dependency to
  the shipped binary: `seccompiler` (pure Rust, it does not link `libseccomp`) or hand-written
  BPF; `libseccomp` is not an option. A 2c-2 fit check confirms `seccompiler` can express an
  architecture check as the first rule and a raw x32 range refusal (`nr & 0x40000000`), which
  a default-allow filter needs so x32 calls cannot bypass the `clone` rule; otherwise those
  first instructions are hand-written BPF. Default action allow with the listed refusals returning `EPERM`
  (`ENOSYS` for `clone3`); the filter first checks the architecture (`AUDIT_ARCH_X86_64`,
  anything else killed), then refuses every x32 syscall number; `AF_NETLINK`, `AF_PACKET` and every family not named below are
  refused. `socket()` is allowed only for
  `AF_INET` or `AF_INET6` with `SOCK_STREAM` and protocol 0 or `IPPROTO_TCP`, and for
  `AF_UNIX` with `SOCK_STREAM` (the listening socket; flags `SOCK_CLOEXEC` and
  `SOCK_NONBLOCK` masked); `socketpair()` is refused unless the pinned server needs it (then
  `AF_UNIX` only), fixed by test; `fork`, `vfork` and `clone` without `CLONE_THREAD` are
  refused and `clone3` returns `ENOSYS` (on x86_64 glibc builds `fork` and `vfork` on `clone`
  or `clone3`, so the `clone` flag rule and the `clone3` refusal are what operate; `clone3`'s
  flags cannot be inspected; glibc then falls back to `clone` for threads), so the server cannot start other processes: process-tree
  containment on level 3, where the parent-death signal covers only the direct child
  (an `exec` without a fork only replaces the server in place). Exec after start is restricted
  too, because a non-leader thread that calls `exec` becomes the process leader without the
  parent-death setting (it is per task): `execve` is refused, and `execveat` is allowed only on
  the verified runtime descriptor's number with `AT_EMPTY_PATH`, that descriptor being
  `O_CLOEXEC` so it is gone after the first exec; `dup2`, `dup3` and `fcntl(F_DUPFD*)`
  targeting that number are refused if needed. 2c-2a made it sound: the descriptor is 1023 and `RLIMIT_NOFILE` is 1023, so no later descriptor can take the number ([model-runtime.md](model-runtime.md)). `pidfd_open`, `pidfd_getfd`, `process_madvise`
  and `kcmp` are refused (they would let the server reach Mimik's descriptors). TCP Fast Open
  is refused on every level: `sendto` whose flags carry `MSG_FASTOPEN`, `sendmsg` and
  `sendmmsg` whose flags carry it, and `setsockopt(TCP_FASTOPEN_CONNECT)` return `EPERM`, so a
  fast-open send on an unconnected TCP socket is not a way around the endpoint checks; `io_uring_setup`,
  `io_uring_enter`, `io_uring_register`, `ptrace`, `process_vm_readv`/`writev`, `bpf`,
  `perf_event_open`, `keyctl`, `add_key`, `request_key`, `mount`, `umount2`, `pivot_root`,
  `unshare`, `setns`, module and `kexec` calls are refused. Seccomp sees no `connect()`
  address, so it is never described as network confinement. Refused-call discovery on the
  dev host uses `strace` in a container on the GPU path (no dependency on the kernel log or
  root); calls the Vulkan driver needs are decided before 2c-2 completes.
  `SECCOMP_FILTER_FLAG_LOG` is optional.
- **Preflight** (every start, before level 1): the host check (unprivileged user namespaces not
  disabled); then a real probe run, `mimik assistant-runner --probe`: the Mimik binary re-executed under the
  full level profile (the same binds, descriptors, seccomp program and token channel as a real
  start, plus Mimik's own libraries bound) exercising each resource once: open the model
  through its descriptor path (including `--ro-bind-fd` of a regular-file descriptor), bind
  the socket path in its directory (level 1) or the port (level 3), read the token
  channel, and on the GPU path open the render node and enumerate the device; on level 1 it
  also asserts the process shape of the endpoint check (below). It does not prove
  llama-server's own system calls; the real server start does (CI with the test model, the dev
  host with the real model). It runs once for the CPU build and once for the GPU build (a
  level may work for one and not the other; each path uses the highest level its own build
  passed). A failure names its cause where it can be told: user namespaces disabled
  (`user.max_user_namespaces`, `kernel.unprivileged_userns_clone`), the Ubuntu AppArmor
  restriction on unprivileged user namespaces (`kernel.apparmor_restrict_unprivileged_userns`),
  or the probe's own error. Then level 3 is probed only if the owner has explicitly allowed it (ASSIST-43). A level that
  passes the probe can still fail at the real start; that failure is recorded with its cause
  and shown on the page, and the transitions below move down only for sandbox-attributable
  failures. Memory: the child has no memory cap (`RLIMIT_AS` is
  unusable with GPU drivers' address-space mappings, ASSIST-L23); the preflight refuses a model
  file larger than `MemAvailable` minus 1 GiB ("not enough memory for this model").
- **Token:** 32 bytes from the OS random source, base64url, new for every launch, held in
  memory. Channels, in order: level 1, `--ro-bind-data` from a pipe to a file inside the
  sandbox read with `--api-key-file`; level 3, `--api-key-file` on a 0600 file in a
  0700 private directory, deleted once `/health` answers (`LLAMA_API_KEY` in the runner's
  environment, readable only by Mimik's user, is the documented alternative), never in an
  argument. A pinned build without either channel would not start the assistant ("the model
  server cannot receive its token safely"); b12110 has both. It never runs without a token
  and never passes it on a command line. `/health` without a token is a live-checked property
  of the pinned build; every other request carries it.
- **Endpoint check, per connection, before its first byte:** level 1: `SO_PEERCRED` on the
  connected Unix socket (the kernel reports the peer PID in Mimik's PID namespace) must give
  Mimik's UID and a PID that is either the `child-pid` the runner reported for this generation
  (the sandbox's reaper) or the server inside, whose `PPid` or reported start time ties it to
  the sandbox. The
  namespace test must pass: the `st_ino` of `/proc/<pid>/ns/pid` equals the `pid-namespace`
  number reported (where readable), cross-checked by `NSpid` in `/proc/<pid>/status` (two entries, the
  inner one small); where `ns/pid` is not readable (it needs ptrace-read access, which a
  non-dumpable process denies), the `NSpid` and `PPid` tests decide. Mimik holds a pidfd on
  `child-pid` so its PID cannot be reused during the generation. A UID-only match is never
  accepted. Level 3: before and after `connect()`, every `LISTEN` entry for
  `127.0.0.1:<port>` in `/proc/net/tcp` (`procfs` crate) must have its socket inode open in
  the current child's `/proc/<pid>/fd` (with `SO_REUSEPORT` several listeners can share a
  port, so one foreign entry fails the attempt). While the child holds that listener no other process
  can listen on the port, so the connection reaches it; the accepted socket is not required
  (it enters the child's table only on `accept()`). No `LISTEN` entry yet means "not ready"
  (retried until the `/health` timeout); an entry owned by any other process fails the attempt
  at once. The port is picked by binding
  `127.0.0.1:0` and releasing it; a child that fails to bind it exits, and a new port is tried
  (at most 3 per attempt). The child's "listening" line is a hint for when to connect, never
  the proof.
- **Output:** stdout and stderr are each drained by a reader task with a 4 KiB line bound
  (longer lines truncated) and are never copied into Mimik's log. Only fixed patterns become
  events: listening, the build number, "offloaded N/M layers", model-load and device errors.
  The CPU child runs at the pinned build's default verbosity; the GPU child runs with
  `--verbosity 4`, the lowest level at which the pinned build prints the offload line. A live
  test sends a job with a marker phrase and checks the marker appears in neither stream (on
  the GPU path it was checked at verbosity 4, `results.md`).
- **Generations:** every launch has a generation number; the socket directory, the pidfile
  entry, the endpoint check, jobs and every exit or timeout event carry it, and an event from an
  older generation is ignored. One manager task serializes transitions: a child is stopped
  (SIGTERM, SIGKILL after 5 s) and the next one starts only after it is gone: on level 1 when
  the runner's pidfd and the server's start-time identity show it exited (the PID
  namespace, and with it every process and its GPU memory, is gone); on level 3 when a
  pidfd on the child signals exit. So two model servers never coexist (a GPU → CPU fallback and
  a trial included). On every generation change, every queued and running job ends with "the
  assistant restarted: ask again", neutral for the breaker. The pidfile (generation, PID,
  process start time, binary path) is held under an exclusive `flock`, so a second Mimik on
  the same data directory cannot run the assistant; at start a leftover is killed only when
  PID, start time and path all match. On shutdown Mimik stops the child as above.
- **Device (ASSIST-27, HW-1):** software ICDs (`llvmpipe`, `lavapipe`, `SwiftShader`, by
  description and driver) are excluded first, before any count of devices. Enumeration names
  (`Vulkan0`, `Vulkan1`) change with the set of visible devices, so the name passed to
  `--device` always comes from the listing in the same confinement the server runs in. Level 1:
  at each start the GPU build's `--list-devices` (30 s timeout; output format pinned by a test
  against the pinned build) runs inside the level-1 profile once per render node with only that
  node bound; after excluding software ICDs each list shows exactly one hardware GPU, and that
  in-sandbox name is what the real start passes. Whether binding one node really isolates
  enumeration is a 2c-2 live measurement: Mesa enumerates through sysfs and libdrm, so if the
  ICD still lists other cards, the level-1 profile also binds only the matching
  `/sys/class/drm` entries, or selects by PCI address inside the sandbox. Level 3 cannot isolate nodes (it
  does not restrict device opens at all): the GPU is used there only if exactly one hardware
  GPU is visible, counted from sysfs (`/sys/class/drm/renderD*` with their PCI devices), not
  from `--list-devices`, otherwise the CPU path is used with a page note (multi-GPU outside
  the sandbox is a limitation, ASSIST-L25). The node's PCI address (sysfs) is its stable
  identity across restarts and distinguishes two identical cards. Ranking (`auto`, level 1):
  a device counts as dedicated when sysfs shows `mem_info_vram_total` above 0 (amdgpu) or its
  PCI class and `boot_vga` mark a discrete card where available; dedicated devices rank before
  the others, then by reported memory, ties by PCI address; where sysfs says nothing, no
  ranking claim is made beyond memory. `--assistant-device cpu` forces the CPU and a PCI
  address selects that device. The real start checks that the sandboxed list still shows one
  hardware GPU with the recorded description. The GPU path loads every layer on the GPU: the
  child's offload line must show N = M, otherwise it is a GPU failure. A Vulkan build that
  cannot start (for example no Vulkan loader) is a GPU failure. The CPU path uses the CPU
  build.
- **Transitions** (attempt = spawn, endpoint check, `/health` within 60 s, priming within
  300 s, calibration within 1,020 s; owner actions are owner-only
  and CSRF-checked):

  | Event | Action | Failure count |
  |---|---|---|
  | Attempt fails on the GPU path (budgets that do not derive included), or the GPU child exits, or is stopped as hung | stop and wait until gone, start on the CPU path at the same level at once, showing the reason; CPU until Mimik restarts or the owner presses "Try the GPU again" | not counted |
  | Attempt on the CPU path fails with a sandbox-attributable failure at level 1 (a sandbox setup error, a status-descriptor failure before exec, the level's preflight failing) | next attempt at once on level 3 if the owner has explicitly allowed it and it passed its probe (ASSIST-43); none left: `unavailable` with the fix; the skipped level and its reason are recorded and shown | not counted |
  | Attempt fails on the CPU path, otherwise (model-load errors, hash mismatches, exits after exec keep the level) | retry after 2 minutes | +1 |
  | Child exits while ready (CPU path) | restart now | +1 only within 10 minutes of `ready` |
  | Two consecutive run-deadline expiries with no completed exchange between | stop as hung, wait until gone, restart, re-measure (whatever `/health` says); expiries never count toward the breaker | as a child exit |
  | Failure count reaches 4 (the first attempt plus 3 retries) | `unavailable` until Mimik restarts or the owner presses "Restart" | — |
  | Budgets do not derive on the CPU path ("too slow") | `unavailable` with the numbers; no retry; "Restart" measures again | — |
  | `ready` for 10 minutes | failure count back to 0 | reset |
  | Owner presses "Restart" (or Mimik starts) | stop, wait until gone, new attempt from level 1, the only way back up from a lower level; breaker reset | reset |

  While `starting` or `unavailable`, a job start answers 503 with the state, queues nothing,
  and number entry, notes and Undo work as usual. Logs carry states, levels and reasons, never
  prompt text, URLs with tokens or the token.
- **Cancellation:** a superseded or expired job's streamed request is dropped, which closes
  its connection; the pinned server is expected to stop at the next token, and during prompt
  evaluation at its next check of the connection. Live tests measure both: a superseding job's
  first token after cancelling a job mid-generation and mid-prompt (a 1 KiB prompt on the CPU
  path). If the server finishes prompt evaluation before noticing, the superseding job waits
  that long inside its own budget; only two consecutive deadline expiries stop the child.
- **Priming and calibration:** after `/health`, priming as in 2b (the fixed prefix with a
  minimal data envelope, `max_tokens` 1, which fills the prompt cache: about 117 s for Granite
  at 2 CPU threads in a container, about 1.3 s on the RX 6650 XT). Then two calibration
  samples: fixed synthetic jobs shipped with Mimik (no sheet or player text), each with a
  dynamic part of exactly the path's byte cap (CPU 1,024, GPU 4,096) built from filler whose
  token density matches real sheets (labels and values of the shipped demo and 2b synthetic
  sheets; the density is recorded), since every request is separately held to N(B) tokens;
  the two fillers differ from their first byte so neither reuses the other's cache;
  64 tokens, `ignore_eos: true` and no schema, so each sample generates exactly 64 tokens (the
  owner's reply length enters the estimate through the measured speed). From each sample: wall time W, `timings.prompt_n`, `prompt_ms` and the
  cached prompt tokens (the evaluated prompt tokens include any uncached tail of the fixed
  prefix and the envelope and template tokens around the filler), `predicted_n` and
  `predicted_ms`; and from `/tokenize`, each filler's token count N₁ and N₂ (equal byte
  lengths do not give equal token counts). All must be present,
  finite and positive, `predicted_n` must equal 64, each tail tᵢ = `prompt_n` − Nᵢ
  must not be negative, and the cached tokens must be at least 75 % of the fixed prefix's
  tokens measured by priming (2b saw 83 to 88 %), or the attempt fails (a timing criterion,
  not a content one). Both are outside the breaker. The grammar question is settled once, in
  2c-2, not at each start: a live measurement compares per-token generation time with and
  without the grammar on both paths; if the grammar costs more than 10 %, the shipped
  calibration measures g from schema-carrying samples (then `predicted_n` need only be at
  least 32 and generation time is g × the reply length), otherwise it stays as above.
- **Budgets (ASSIST-31, ASSIST-32; recomputed at every start, a latency estimate within hard
  caps, not a worst-case bound):** per sample i: per-token prompt time pᵢ = `prompt_ms` /
  `prompt_n`, per-token generation time gᵢ = `predicted_ms` / `predicted_n`, overhead oᵢ =
  max(0, W − `prompt_ms` − `predicted_ms`), tail tᵢ. The shared request limit uses the
  smaller count, N_cal = min(N₁, N₂) (conservative: a request may not exceed the leaner
  filler's density). For a candidate bound B, N(B) = floor(N_cal × B / cap), Tᵢ(B) = oᵢ + pᵢ ×
  (tᵢ + N(B)) + gᵢ × `max_tokens` with the same N(B) in both, and T(B) = max(T₁(B), T₂(B)). Candidates: GPU 4,096, 2,048, 1,024; CPU 1,024, 768, 512. B is the
  largest candidate with R(B) = max(15, round up to 5 s of 1.25 × T(B)) within the 600 s
  ceiling (both paths) and with room in the context (prefix tokens + N(B) + `max_tokens` + 64
  ≤ 8,192). Total wait = max(120 s, 2 × R); queue: GPU D = max(1, min(4, floor((total − R) /
  R))) and Q = D × (R + busy); CPU D = 1 and Q = total − R + busy, where busy is the busy-after-expiry slack (the calibrated prompt term, which a job queued behind one cancelled at R also waits), so Q + R ≤ max(120 s, 2 × R) plus D × busy, and the
  admission rule (`docs/assisted-entry.md`, "Interpretation assistant", Jobs) refuses as busy a start that cannot begin in
  time. Every request also obeys N(B) tokens (`docs/assisted-entry.md`), so its prompt is no longer than the
  calibration's; the hard run deadline holds when the estimate is wrong, and two consecutive
  expiries restart and re-measure (the transitions above). On the GPU path, no fitting
  candidate because of speed is a GPU failure (CPU fallback); a reply too large for the context
  is not (nothing about the path changes it): it is held as unavailable with its message. On the CPU path, if 1,024 does not fit, 768 and
  then 512 are taken; only when 512 does not fit is the assistant unavailable ("too slow on
  this machine: about N s needed, the ceiling is M s"), with the measurement shown; entry is never
  affected: "too slow" on the CPU path turns the assistant off for that machine until a
  Restart measures again. Expected outcomes, with the prompt term written out as p × (t +
  N(B)): t is the uncached tail of the fixed prefix (2b saw 170 to 250 of its about 1,460
  tokens uncached; the 75 % cache rule lets t reach about 365), N(B) the dynamic part. On the
  GPU path the 2b numbers (about 1,100 prompt and 80 generated tokens per second on the
  RX 6650 XT) give T about 4 s, so B 4,096, R 15, D 4, Q 60. On the CPU path the outcome is
  set by the first native calibration on this host, a measurement step of 2c-2 recorded in
  `docs/results.md`. The 2b container illustration under the old 90 s cap no longer applies: the outcome on a
  CPU is set by the native calibration and the owner's reply length (`docs/results.md`), and
  in 2c-2 the formulas are fixtures, not evidence.

### 1.5 Bundles and reminders (3)

- **Bundles:** up to 4 changes to integer, enum or boolean addresses (scalars or saved-row
  cells by host ID), no row adds or removes; one revision; notes sharing `bundle`; freshness
  as above. Undo on any member reverts all members, only if every member's current note is
  still that bundle and holds its `after`; otherwise nothing is reverted and the page lists
  what changed.
- **Reminders (D-W):** triggered by a saved decrease of any top-level integer field, with the
  assistant on and JavaScript present; the model's `relevant` flag judges relevance. Context:
  the decrease, numbers and enums, list-row names from properties with the `catalogue`
  member, and linked entry text (≤ 2,000 characters, quoted) when exactly one entry matches or
  the stored key resolves. Output `{relevant, row, hint ≤ 120, option?}`, the option being a
  bundle. Display: a dismissible hint (client-side only), never blocking or auto-applying.
  Injection is contained by the schema, the provenance extension below, the full display and
  confirmation.
- **Catalogue provenance extension (decided 2026-10-19, coordinator default):** in a reminder
  option, a number may also come from the text of the catalogue entry the reminder cites, and
  only if it appears literally in that entry's text as sent in the request; a reminder literal may
  also be the decrease's before or after value or their difference (step 3c). The cited entry is bound by catalogue
  version and entry key (part of the D-P read set). The option is displayed with its source
  ("from Shield: …"). Every other number must pass the step-2c rule (in the phrase, equal to
  the target's value, or a 2, 3, 4 or 100 admitted by a fraction or percent word); outside reminders the step-2c rule stays strict.
- **Catalogue member:** an optional `"catalogue": {"id": <link>, "entry_type": <id>?, "key":
  <property>}` on a text item property. `key` names a declared optional text property of the
  same list holding the remembered entry key (D-R); it is rendered as the link, never a
  control. Checked at the three step-3 catalogue-link sites (draft save, publication
  snapshot, readiness): the key property exists, is optional text, is not itself
  catalogue-backed, and is used by at most one hint. Resolved only through the immutable pin.
  Omitted when absent, so old hashes are kept.
- **Linking on view** (rules 1–2 here, 3–4 in step 4): names are normalised (NFKC, case-fold,
  diacritics removed, whitespace collapsed). (1) If the stored key resolves to an eligible
  entry (linked catalogue, type filter, not retired) whose normalised name equals the cell's,
  link it. (2) Otherwise link exactly one eligible exact match (D-O).

### 1.6 Names, "Which Sword?", suggestions (4)

- (3) Several matches: "Which Sword?" lists the candidates (name · type · catalogue); choosing
  one posts the cell route `cells/{f}/{r}/{p}` (ASSIST-6; the row index read at `base`
  and mapped to its host ID) with the entry key: one revision, no note; the link is
  remembered (D-R). (4) None: "Not in the catalogue" plus Browse; with JS, "Did you mean…?" on
  demand. Saving the name cell with a different name clears the key in the same revision.
  A dangling key just shows no link.
- **Suggest route** (JS): read rights and pin correspondence rechecked per request; no `JOBS`
  permit, a pool of 4, 429 at saturation; query 2–64 scalars and ≤ 256 bytes; prefix match on
  cached names, then FTS, then an on-demand typo pass (±2 characters of length, ≤ 20,000
  comparisons and 20 ms, outside the DB mutex); the type filter on every path; a name cache
  LRU-bounded at 32 MiB, names only; p95 ≤ 50 ms, measured. Picking a suggestion stores the
  name, plus its key when it is one of duplicates.
- **Combobox:** `aria-controls` and unique option IDs; while open, Enter selects and does not
  submit; IME composition respected; debounce, `AbortController` and a generation counter.

### 1.7 Warnings still to add

| Warning | Source | Stored | Shown |
|---|---|---|---|
| Assistant failure, expiry, busy, stale | job / apply | no | inline (JS) |
| Not in catalogue / which one | linking on view | no | next to the cell |

Every warning is non-blocking and tied to its control with `aria-describedby`.

### 1.8 Security summary

Model output passes the schema, the grammar, Rust schema and domain checks, literal
provenance, checked Rust evaluation, origin binding and read/write-set freshness, and is rendered escaped inside `<bdi>`. Rights are rechecked on every
request and inside every write transaction. Limits: raw ≤ 200, notes ≤ 256 per revision,
dynamic prompt ≤ 4 KiB on the GPU path and ≤ 1 KiB (related fields only) on the CPU path, the
measured job budgets within their caps and the record caps, the breaker, the suggest bounds.
The model client talks plain HTTP only to its own child: through the generation's Unix socket
(the sandbox) or a loopback IP literal, every connection checked to reach the child before its
first byte (the peer in the child's PID namespace under the reaper; the port's listener in
the child), with a per-launch token that never appears on a command line; it follows no
redirects and ignores proxy settings. The one exception is the development endpoint of 2c-1,
which has no endpoint check and exists only in builds with the `dev-endpoint` feature, never
in a release. The model server is a pinned official llama.cpp build and a model file, both
hash-verified and loaded from the verified descriptors on every level. Its confinement is one
of these levels, shown on the assistant page: the Hakoniwa sandbox (private namespaces, so it has no
network beyond its own loopback and sees only an allowlist of read-only paths plus the GPU
node; seccomp), or, only with the owner's explicit permission, unconfined (TCP egress allowed and
the user's whole filesystem readable). On every level seccomp refuses new processes and
non-TCP sockets; it is never described as network confinement. So Mimik sends player text
only to its child, but below the sandbox level a compromised model server (for example
through a malicious GGUF) could send it on. Model downloads are owner-started, HTTPS to an
enumerated list of Hugging Face hosts, bounded, licence-confirmed and hash-verified, and carry
no sheet or player text. No prompts, player text, presigned URLs or token in logs; no secrets.

## 2. Steps

### 2c-2 Model runtime

Split (user, 2026-10-22) into **2c-2a** (the CPU path at confinement level 1; level 3 built,
not used, ASSIST-43) and
**2c-2b** (the GPU path, 2c-2b-1; there is no level 2, ASSIST-48), each with its own branch and reviews; the machinery of
2c-2a is path-aware, so 2c-2b only adds to it.

#### 2c-2a CPU path, sandbox (level 3 built, not used: ASSIST-43)

Built (branch `assist-2c2`; reference [model-runtime.md](model-runtime.md)). Remaining before
it is marked complete:

- the official CPU asset on the documented minimum host (an Ubuntu 22.04 VM) and the AppArmor
  preflight on an Ubuntu VM;
- the `llama-runtime` CI job's first run (it needs a push).

#### Level 3 behind an owner opt-in (later step, ASSIST-43)

Level 3 is built but switched off in 2c-2a. Bringing it back: an explicit owner setting (never
automatic), shown on the page with its exposure, and these before it can be enabled:

- refuse a real runtime path holding `$` (the loader's dynamic string tokens in dlopen names),
  with docs and tests;
- the bind-deadline items: a robust bind test on a loaded runner, the "deadline passed, exit
  re-checked" branch exercised, and an early bind error decided at once;
- a cancel-aware drain settle in `bind_outcome` (100 ms slices);
- the live cwd-helper hardening: bounded ready wait and read, failing at once on refusal or
  exit, kill-on-drop guards.

#### 2c-2b-1 GPU path

Built and verified live on the RX 6650 XT (branch `assist-2c2b`; split from 2c-2b by the user,
2026-10-23; reference [model-runtime.md](model-runtime.md), results in `results.md`). The live
checks run on the dev host with the GPU fan on manual, evening or weekend, through
`tools/assist_gpu_live.py` (it refuses otherwise). Remaining:

- from the trace, bind only the sysfs files and ICD manifests that are read and limit the
  loader to the selected card's driver (ASSIST-L44, ASSIST-L45), after another driver (AMDVLK,
  Intel, NVIDIA) has been traced where one is available;
- re-measure the superseding-job timings on both paths with the fixed helper, then the GPU-path
  checks at each llama.cpp pin change (`tools/assist_gpu_live.py`).

### 2d-2 Model downloads

Merged (reference [model-downloads.md](model-downloads.md)). Remaining, in this order:

- the live checks, none in CI: (4) `tools/assist_model_browser.py` extended to the Downloads
  panel at two viewports; (5) the release binary refusing a certificate outside the system
  store on a clean machine (the unit tests cover the verifier with an isolated store).

### 3 Bundles and reminders

Scope: §1.5. Slices 3a (catalogue member, linking on view) and 3b (bundles: multi-change
proposals, apply and Undo) are built (`docs/assisted-entry.md`); slice 3c remains, with its own
branch, failing-first tests, implementation review and merge. Migration 034 (3b) carries the
op `reminded` and the bundle index; 3c's setting uses migration 035.

#### 3c Reminders

Three slices, in order: 3c-0 decides how the interpretation prefix survives a reminder, 3c-1
builds reminders with names only, 3c-2 adds catalogue entry text and the provenance extension
(its own review). Interpretations keep prompt format D without catalogue text (ASSIST-61).

##### 3c-0 Prompt cache across prompts (prototype tier, live)

With one slot and `--cache-ram 0` (`launch.rs`), a reminder's prompt replaces the slot's cache,
and the next interpretation re-evaluates its whole prefix inside its own run deadline R (R's
prompt term covers only `t + N(B)`): about 80 s on the CPU path at 5 threads (1,979 tokens in
78.5 s, `docs/results.md`), about 150 s at 2 threads, about 10 s for Qwen3.5-9B on the GPU path,
so a long interpretation expires and two expiries stop the child as hung. Measure on both paths
(CPU at 2 threads in the day; the GPU at night or the weekend, fan read first), with
`tools/assist_runtime_measure.py` extended: an interpretation's time to first token and total
time right after a reminder prompt of realistic size, (c) with llama.cpp's host-RAM prompt
cache (`--cache-ram <MiB>`, sized to two prefixes) against (a) re-priming the interpretation
prefix after every reminder (`calibration::prime`, held in `busy_until`). Record RAM cost,
times and the choice in `docs/results.md`; prefer (c) (llama.cpp's own mechanism) when it
restores the prefix: after a reminder, the interpretation's `cache_n` is at least 75 % of the
prefix (the calibration rule), not only a short time to first token. Also measure: on the
hybrid default models (Qwen3.5's linear-attention layers; check Granite 4.2) whether the reuse
point is a context checkpoint at or before the prefix's end; the size of one cache entry (a
whole prompt's state and checkpoints), so `--cache-ram` is sized from it, and that a
reminder → interpretation → reminder → interpretation sequence keeps the interpretation entry;
the selection rule of b12110 itself; the GPU path's host↔device copy time. `--cache-ram` is in
the measurement-relevant flag vector (`launch.rs`): choosing (c) re-checks calibration and
updates that vector's text and test. A shared system message is excluded (two conflicting
tasks, earlier review). The chosen mechanism and its measured cost feed 3c-1.

CPU result (2026-11-18, `docs/results.md`): (c) passes with Granite 4.2 on the CPU path (99 %
reuse); the GPU path is pending (fan on manual, night or weekend), with the tool recording reply
text and the per-step pass rule (denominator: `/tokenize` of the prefix), and stays on (a) until
measured. Because of (c), 3c-1 also: sizes `--cache-ram` per model and context (context × bytes
per token, or the largest interpretation entry plus the largest reminder entry, each prompt plus
reply bound); checks in b12110's source or by a run whether the 8,192-token limit bounds each
entry or their sum, and at Ready falls back to (a) or makes reminders unavailable when the two
entries do not fit; checks RAM headroom on the documented minimum host; tests a third prompt
kind (a cancelled reminder mid-prompt then an interpretation, reminders of two accounts,
calibration or a trial then a reminder then an interpretation); compares an interpretation's
reply after a restore with a fresh one; re-runs calibration with the flag and updates the
flag-vector text and test; takes the restore cost as the entry load time with its scaling
stated, not the dev host's 1 s.

##### 3c-1 Reminders with item names (live checks pending)

Built; described in [assisted-entry.md](assisted-entry.md#reminders) and
[model-runtime.md](model-runtime.md) ("Prompt cache and reminders"). Remaining, all live and none
done: on both paths (CPU at 2 threads in the day; the GPU at night or the weekend, fan read
first) run the reminder relevance class (`tools/assist_eval.py run-reminder`, 8 synthetic cases, a
smoke check only) and compare the reminder time and the extra wait of a scribble reading right
after one with the page's estimates; re-run calibration with `--cache-ram` on (the flag is in
the flag vector, but calibration has not been repeated with it) and replace the CPU restore
constant (`RESTORE_S_PER_MIB`) with the measured entry load time of the host; check RAM headroom
(model plus the sized cache) on the documented minimum host (an Ubuntu 22.04 VM; the preflight
counts the cache against `MemAvailable`, but no process memory figure was measured); test a third
prompt kind live (a cancelled reminder mid-prompt then an interpretation, reminders of two
accounts, calibration or a trial then a reminder then an interpretation; only the stub-level
schedule is tested) and compare an interpretation's reply after a restore with a fresh one; the
GPU part of 3c-0 (measure the re-prime against the host-RAM cache and decide; the GPU path stays
on the re-prime until then, and Qwen3.5's linear-attention layers are unmeasured). Run time
detection of a failed CPU restore is implemented but not exercised against a real server.

##### 3c-1b The prompt cache on the GPU path

Landed (reference: `docs/model-runtime.md`, "Prompt cache and reminders"): `--cache-ram` per model
on both paths with `--ctx-checkpoints 4`, the memory check counting the cache and the slot's
checkpoints, the cache check after a reminder on both paths (a failed one switches reminders off
on the CPU path and falls back to the re-prime on the GPU path), no re-prime while the cache
works, and the GPU restore cost (0.53 ms per MiB of entry plus 10 ms) in the extra wait, the
margin check and the page (the fallback re-prime counts in Q and the extra wait), with the `BeyondMargin` check on both paths.

Remaining:

- Reminders are unavailable on both paths of the dev host (`docs/results.md`, 3c-1b GPU entry):
  decide what the queue rule counts for a reminder, then re-measure.
- RAM headroom on the documented minimum host for the GPU path (Qwen about 5.3 GiB model +
  1,015 MiB cache + 251 MiB slot + 1 GiB margin).
- The tool also writes the thermal guard's minimum and maximum into its JSON.

##### 3c-2 Catalogue entry text and provenance (GPU path)

- **Context.** Rows gain `entry` when 3a links them: the catalogue page's text rule, plain
  text, ≤ 2,000 characters cut at a character boundary, none for "Description unavailable";
  dropped first under B (last row first). On the CPU path B leaves no room, so none is sent.
- **Provenance extension** (§1.5): a literal may also be a number in the cited row's entry text
  as sent in this request (the 2c phrase-number lexer; a sign or a die size such as `d6` is not
  part of the number); a cited row whose text was dropped or absent admits none; a number only
  in another row's text drops the option. The option shows "from <entry name>: …" with the
  number's surrounding words (≤ 64 characters) in `<bdi>`; `Proposal` gains that `source`,
  which enters the content hash with the catalogue reads (tag `mimik-proposal-v4`).
- **Read set.** Each entry text sent adds its catalogue ID, sealed version and entry key to
  `reads`; apply re-reads them through the catalogue store (latest sealed version and the
  row's link); a newer sealed version, a changed link or key → 409, and so does a failed or
  timed-out lookup (never read as unchanged or as no link).
- **Tests.** Duplicate names without a key → no entry text; entry text cut to 2,000;
  injection in entry text → at most a fully shown option; a number in the cited entry →
  allowed and shown with its source; a number only in an uncited row's text, or in a cited
  row's dropped text → dropped; a number in no source → dropped; a new catalogue version or a
  changed key → 409.

### 4 Suggestions and "Which Sword?"

Scope: §1.6. Tests: confinement (forged address, unlinked catalogue, other game, native,
unsupported or corrupt pin); the type filter on every path; 2-character prefix; typo distance
1 and 2; diacritics; query, typo-pass and cache-byte bounds; no `JOBS` 503; 429; escaping;
choosing among duplicates stores the key and the link persists; renaming clears the key; a
dangling key shows no link; p95 measured.

### 5 Docs, release scope, gate

- Update `docs/assisted-entry.md`, `architecture.md` (the model runtime is no longer absent),
  `evaluation-tooling.md` (its "no provider or model integration" scope),
  `release-scope.md` (schema 23, the sheet module under its strict CSP, the local model as
  confirm-only, the bundled llama.cpp build, its licence and source, the confinement levels and
  which were verified, model downloads over HTTPS, Linux x86_64 as the only verified platform,
  the GPU path measured on one RX 6650 XT), the runtime definition contract (the
  `catalogue` member), publication and dependency docs; replace the statements this work
  reverses: catalogue Q4 (not in the catalogue is a warning, "Which Sword?" remembered), the
  catalogue writer membership check (`catalogue_ref`, not built; the key is advisory),
  "no model proposes", the accuracy bar (waived, D-D), and "the LLM does not judge at play
  time" (narrow confirmable proposals and reminders).
- `cargo about`, audit, packaging (the runtime files verified against the manifest), and the
  unpacked smoke with the assistant off and once on the production runtime path (§1.4a, CI
  workflow); stub-endpoint coverage stays in the feature-enabled test build.
- Assistant scope stated as "local endpoint, POC quality, not certified (D-D)", with L1 and the
  other open limitations listed.

## 3. Verification

- Rust gate: fmt, strict Clippy, locked tests (red then green), audit, `cargo about`, release
  build and size delta.
- Browser (Chromium at 1280 × 720 and 390 × 844, demo data, with and without JS): per-cell
  Enter, buttons for selects and checkboxes, row actions, quick-add, notes, switch, Undo,
  unknowns, the two-tab sequence, derived and check refresh, CSP headers and no violations,
  combobox keyboard and IME, no overflow.
- Model runtime (host measurements, reported separately from unit, fake-server and CI
  evidence, never part of the CI gate except the `llama-runtime` job): on this host the CPU
  path at 2 threads and at the ASSIST-41 default (5 of 6 physical cores; evening or weekend)
  and the GPU path (Vulkan, RX 6650 XT, night or weekend), each from a native
  child in the sandbox: start to `ready`, priming and calibration samples, derived budgets,
  token refusal, cancellation, the GPU → CPU fallback, shutdown and orphan cleanup, the
  confinement tests with the adversarial helper. The AppArmor preflight on an Ubuntu VM;
  until run, labelled unverified. macOS (Metal) and Windows (Vulkan): deferred, not claimed.
- Model screen (prototype tier, `tools/assist_eval.py` with unit tests; results note with
  exact scope, denominators and limitations; reviewed; no pass bar, D-D): per path (CPU at 2
  threads, GPU), p50/p95 latency, schema-valid rate, output tokens; per class, correct within the first 3 and
  false-confident. Fast-path inputs never reach the model; results are per class, not general
  accuracy. `tools/assist_eval.py` is new in 2b; its cases are synthetic and independently
  labelled. Extra classes for 2c: valid divisions, overflow, unknown target, provenance
  failure, multi-option, related-field selection at 1 KiB, same-row cell references; injection rows expect an option at
  most, never a write. Quality numbers are per backend (CPU and GPU answers differ at
  temperature 0, `docs/results.md`).

  The 2b screen used the six interpretation classes (50 cases); the reminder class (8) belongs
  to step 3, so the table totals 58.

  | Class | n |
  |---|---|
  | Damage and heal phrases (EN/IT) | 16 |
  | Fractions and vague | 8 |
  | Other-field references | 6 |
  | Nonsense | 8 |
  | Injection in the player phrase (a legitimate request, shown as an option) | 4 |
  | Injection hidden in context | 8 |
  | Reminder relevance (step 3) | 8 |

- Unit, connected-router, browser and live-inference evidence are reported separately.

## 4. Risks and later

Risks: the CPU path may be too slow for a long reply length (§1.4a: R over the 600 s ceiling at
every bound is "too slow", and a reply-length change that comes out so goes back), and
`starting` takes priming of about 117 s plus two calibration samples of 64 tokens plus hashing the model before the assistant is
ready; all from container measurements; model quality is unknown and the bar is waived (the display
and confirmation are the safeguard); reminder prompts carry catalogue text (contained, not
prevented); one revision per Enter makes history long (no-ops write nothing, no cap; D-U);
`rig-core` is pre-1.0 with frequent breaking releases (pinned); HTTP client defaults
(proxies, redirects, retries) could send player text off the machine or repeat it, hence the
client policy and its tests. Runtime: the sandbox needs unprivileged user namespaces, which
some distributions disable or restrict (Ubuntu's AppArmor rule); those machines run no assistant unless the owner explicitly allows
unconfined (ASSIST-43), as shown on the page; a malicious GGUF exploiting llama-server is contained only as far as the active level
goes; the official Linux builds link against a distribution's glibc and Vulkan loader, which
may not match the host; the sandbox's path list is driver-specific (Mesa on AMD measured;
other drivers may need more paths, and then fall back to the CPU); other applications can take
the GPU's memory mid-use (the fallback moves to the CPU); budgets are estimates from two
samples per start, so throttling later shows up as deadline expiries and restarts; the 2b GPU
run had the fan forced to full, while production uses the driver's fan curve; one GPU (RX 6650
XT) is measured, other GPUs and Metal are not; downloads add TLS and a second network client
to the binary; Hugging Face file hosts can change, which stops downloads until the host list
is updated.

Later (interpretation, deferred by the 2026-10-20 plan review): escalation to a second
local model (needs a second pinned endpoint, a thread budget totalling 2, preloading and a
single job deadline); model tools such as `read_field` and `search_catalogue` (need
host-bound capabilities, the Context field policy, results in the bound read set, and an A/B
against plain context); confidence gating (needs a defined signal and calibration on a
held-out set of at least 100); opt-in interpretation traces with offline replay (need
owner-only access, byte caps, retention, purge and deletion on every path; replay means
re-validating a recorded answer, while re-running a model is scored against independently
labelled answers; a confirmed proposal is feedback awaiting adjudication, never automatic
gold).

Later: sealing and verifying notes (before release, L1); a per-account write-rate limit or
storage guard (before release, D-U); player `*` and `/`; text, enum and boolean
interpretation; notes and Undo for text; Undo of moves and creation, forced and arbitrary
restore; owner-reviewed reminder rules without a model (D-I); a model for quick-add; more
threads off-hours; the quality bar; macOS (Metal) and Windows (Vulkan) runtime, confinement,
lifecycle, endpoint check (no token to an unverified listener) and model-file rules, verified
on a host of each, and Windows confinement without administrator rights (AppContainer);
measurements on other GPUs; parallel slots on the GPU path (ASSIST-32); fraction words and number words across
languages (ASSIST-38); gated Hugging Face repositories (owner token) and split GGUF files;
partial GPU offload; retiring the legacy full-form edit POST; definitions
choosing a fallback for unknown (L-U4).

## 5. Open questions for the user

- None. The CPU-baseline question is settled by ASSIST-41 (threads default to the physical
  cores minus one, and the native calibration at that default decides the CPU budgets); if
  the measurement still fits nothing, it comes back to the user.
