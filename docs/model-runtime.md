# Model runtime

The interpretation assistant's model server (steps 2c-2a and 2c-2b-1; ASSIST-27, ASSIST-29 to
ASSIST-34, ASSIST-40 to ASSIST-42): with `--assistant`, Mimik starts the pinned official
llama.cpp `llama-server` as its own child process, confined at the highest level that works,
reaches it only through a checked endpoint with a per-launch token, and primes and calibrates
it before any job runs. Linux `x86_64`: the GPU path (the Vulkan build on one hardware GPU)
first, the CPU path (the CPU build) as the fallback. There is no confinement level 2 (ASSIST-48);
macOS and Windows are not built. What the assistant does
with the server is in [assisted-entry.md](assisted-entry.md), "Interpretation assistant".

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
| Device choice (render nodes, listing, ranking) | `src/assistant/runtime/device.rs` |
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
Mimik's affinity mask, capped by the tightest `cpu.max` quota of its cgroup and the cgroup's ancestors, ASSIST-41). `--assistant-device`
(`auto`, the default: the best hardware GPU, else the CPU; `cpu`: the CPU path only, no device
is ever listed; or a GPU's PCI address such as `0000:0c:00.0`, or `0c:00.0` for domain 0). The
data directory is the database's folder; `run/` lives there.

## Pinned build

The manifest pins release b11609 (commit `fcac190`), its CPU asset
`llama-b11609-bin-ubuntu-x64.tar.gz` (size and SHA-256), the 22 files Mimik runs from it
(each under its SONAME, no links: `llama-server`, `libllama-server-impl.so`,
`libllama-common.so.0`, `libmtmd.so.0`, `libllama.so.0`, `libggml.so.0`, `libggml-base.so.0`,
the 14 `libggml-cpu-<variant>.so` backends and `LICENSE`; the RPC backend is not shipped)
with size, SHA-256 and mode, the system libraries they need (`libc.so.6`, `libm.so.6`,
`libstdc++.so.6`, `libgcc_s.so.1`, `libgomp.so.1`, `libssl.so.3`, `libcrypto.so.3`; symbol
floors GLIBC 2.34, GLIBCXX 3.4.30, OpenSSL 3.0.0), the documented minimum host (Ubuntu 22.04
`x86_64`), the source archive (`git archive` of the commit, tar SHA-256; a build-time input for the notice texts only, never shipped, ASSIST-47) and the CI test model
(`ggml-org/models-moved` at `499bc88…`, `tinyllamas/stories260K.gguf`, upstream MIT). ggml
loads every `libggml-cpu-<variant>.so` beside the server and uses the best one the CPU
supports. The Vulkan asset `llama-b11609-bin-ubuntu-vulkan-x64.tar.gz` (SHA-256
`8aff1888…`, ASSIST-42) holds the same 22 files byte for byte plus `libggml-vulkan.so` (the
Vulkan backend, which ggml loads from beside llama-server like the CPU backends and which needs
the system's Vulkan loader `libvulkan.so.1` and a Vulkan driver); the manifest pins it as the
`vulkan` build, shipped in `llama/vulkan/` (the CPU files are shipped twice, once per build
folder). The Vulkan loader `libvulkan.so.1` and the GPU driver (Mesa or the vendor's) are
system dependencies: the release never bundles them (the packaged archive holds no
`libvulkan`, and the manifest's file list is exact), `libggml-vulkan.so` resolves the loader
through `ld.so.cache` and the sandbox binds the system's library folders read-only. The
headers (`vulkan.hpp`) compiled into `libggml-vulkan.so` are an SDK build input of the upstream
binary, not shipped source (REL-2): only their licence text is an external notice,
Vulkan-Headers' MIT text pinned by URL and SHA-256 at tag `vulkan-sdk-1.4.357.0` (the release
does not record the SDK version of its Linux Vulkan build; the Linux job installs the unpinned
`vulkan-sdk` package of LunarG's rolling repository, and that tag is the one its workflow names
for the Windows job), which `llama-fetch` downloads and checks and packaging copies into
`THIRD-PARTY-NOTICES.md`. Mimik
embeds the manifest (a test asserts it equals the file).

## Files

Each build's folder (`cpu/`, `vulkan/`) must hold exactly the manifest's files for that build: an extra, missing or
changed (size or SHA-256) file, a link, or a file or folder writable by group or others
refuses the start with that reason (owner Mimik's user or root). The model and every runtime
file are opened beneath their folder's descriptor with `openat2` (`RESOLVE_BENEATH`,
`RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS`) and `O_NOFOLLOW | O_NONBLOCK`, checked with
`fstat` (regular, owned by Mimik's user, not writable by group or others) and hashed from that
descriptor; a hash is reused while the `fstat` identity (device, inode, size, modification and
change times) is unchanged. The child loads those descriptors, so a file swapped after the
check is not what runs. Folders are opened at their real path (links in earlier components,
such as a symlinked home, resolved once) without following any link. Level 3 also requires
the runtime directory and every ancestor of its real path to be owned by root or Mimik's
user and not writable by group or others. The runtime is resolved once per attempt: opened,
its real path read back from that descriptor, that exact path walked from `/` by descriptor
without following links and ending at the same folder; the pinned files are opened and hashed
beneath that descriptor, and level 3 executes the verified descriptor with `LD_LIBRARY_PATH`
set to the checked build directory's descriptor (`/proc/self/fd/<n>`) and the runner `fchdir`'d
into it, so neither the configured path nor Mimik's cwd is used again (see "Backend loading").
A failed attempt's processes end before the next spawn (probe, retry, start or next level). A model larger than
`MemAvailable` minus 1 GiB is refused ("not enough memory for this model").

## Runner

Mimik spawns `mimik assistant-runner --spec-fd 3` from one long-lived thread (the
parent-death signal follows the spawning thread); the spec (parent PID, kept descriptors,
the exact environment, the exec) arrives on descriptor 3, a memory file written whole before
the spawn (1 MiB at most; at most 64 descriptors, the spec's included, are moved, which the
pinned build's level 1 fits with room to spare). The runner sets `PR_SET_PDEATHSIG`
(SIGKILL), exits if its parent is no longer Mimik, sets `RLIMIT_CORE` 0 and `RLIMIT_FSIZE`
64 MiB (a larger write kills the server with `SIGXFSZ`), closes every descriptor it was not
handed, and executes in place with only `PATH`, `LANG` and `HOME`: `bwrap` on level 1; on
level 3 the verified llama-server descriptor, moved to number 1023 (close-on-exec) with
`RLIMIT_NOFILE` set to 1023, after installing the seccomp program, by `execveat(1023, "",
…, AT_EMPTY_PATH)`. Descriptors are moved to their numbers between fork and exec with
`fcntl(F_DUPFD_CLOEXEC)` then `dup2` only.

## Backend loading (ggml)

The bundled build is `GGML_BACKEND_DL=ON` with `GGML_CPU_ALL_VARIANTS=ON`: ggml picks a CPU
backend at run time by `dlopen`ing each `libggml-cpu-*.so` it finds and scoring it, and a
candidate's constructors run while it is open (ASSIST-42, audited against the b11609 source,
`ggml/src/ggml-backend-reg.cpp`). `ggml_backend_load_best` searches, in order: `GGML_BACKEND_DIR`
(a compile-time path, not set in this build); the executable's directory (`/proc/self/exe`'s
folder); and the current working directory. It is not governed by `LD_LIBRARY_PATH`.
`ggml_backend_load_all` also reads `GGML_BACKEND_PATH` and `dlopen`s it. So Mimik:

- hands level 3 the verified server by descriptor, whose `/proc/self/exe` resolves to the
  checked build folder (the executable-directory search is that folder);
- hands over the verified build directory as a descriptor, kept open across the exec on
  purpose, and sets `LD_LIBRARY_PATH=/proc/self/fd/<n>` to it, so the loader opens the
  libraries by name beneath that descriptor (its inode), never a mutable pathname;
- `fchdir`es the runner into that same descriptor before seccomp and the exec, at level 3 (the
  current-directory search is that folder, never Mimik's own cwd); at level 1 bwrap does the
  `chdir` into `/opt/llama`, and only the allowlisted binds are visible inside the sandbox;
- clears the environment and never sets `GGML_BACKEND_PATH` or `GGML_BACKEND_DIR`.

## Confinement levels

Mimik runs llama-server at level 1 or not at all (ASSIST-43, ASSIST-48; there is no level 2): when level 1 cannot be used, the
assistant is unavailable and `/admin/assistant` says how to fix it (install bubblewrap, allow
unprivileged user namespaces, the AppArmor note), with level 1's own reason. Level 3 is built and
unit-tested but switched off: no manager path probes or starts it, and `--probe` reports its
result labelled "not used". It returns later as its own step behind an explicit owner opt-in. The launcher itself refuses
to start level 3 whatever a configuration asks (a sandbox-class failure, neither counted nor
retried). This covers what Mimik starts (the manager,
Restart, the routes and the start-time probe, which runs level 1 only); `--probe` is the one
path that still runs level 3's probe, which starts Mimik's own probe child under seccomp,
never llama-server; the private `assistant-runner` entry points
(`--spec-fd`, `--shim`, `--probe-inner`) are an internal protocol between Mimik and its runner,
not a privilege boundary (a local user who can run them can run llama-server directly).
Why each level was not used is on the page and in the log.

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
   the host is visible. bwrap's reported reaper is accepted only as the runner's child in the
   reported PID namespace, checked through its pidfd; when it cannot be verified the start
   fails and bwrap is killed, which ends the sandbox only through `--die-with-parent` (every
   profile has it). The command inside is Mimik's own binary
   (bound by descriptor) as a shim: it writes a marker on a dedicated pipe, closes every
   descriptor but 0–2 (`close_range`, else each one `/proc/self/fd` lists; when neither works
   it executes nothing) and executes llama-server (or the probe); an exec failure is written
   to stderr and exits 127 after the marker, so it counts at the same level. The server listens on `/run/llama/llama.sock`.
   On the GPU path the profile also binds the selected render node read-write at
   `/dev/dri/<node>` (`--dev-bind`, the only device bound; the CPU path binds none), its PCI
   device's sysfs folder read-only at its own path, the links libdrm resolves to it
   (`/sys/dev/char/<major>:<minor>` and `/sys/class/drm/<node>`), an empty `/sys/bus/pci`, and
   read-only the Vulkan loader's and driver's data folders that exist (`/usr/share/vulkan`,
   `/etc/vulkan`, `/usr/share/libdrm`, `/usr/share/drirc.d`); llama-server gets
   `MESA_SHADER_CACHE_DISABLE=true` (the sandbox's `/tmp` is private to one launch and bounded,
   so a shader cache could only fill it). The server can issue any ioctl on that one render
   node, so its exposure to the kernel's graphics driver is wider than on the CPU path. The
   sandbox is otherwise the same: no network, no other processes and no access to the user's
   files; it reads the runtime and the model, read-only system libraries, system driver data
   (Vulkan ICD manifests, libdrm, drirc), the card's sysfs attributes and the CPU topology
   (`/sys/devices/system/cpu`). The CPU path reads the same way the read-only system library
   folders (`/usr/lib`, `/usr/lib64`, `/lib`, `/lib64`, `/usr/libexec/ld`), the dynamic loader
   cache and the CPU topology, without the driver data and the card's sysfs. This is accepted
   by default and shown on the page (ASSIST-45, ASSIST-L46); `--assistant-device cpu` removes
   it. The list is verified on the dev host's RX 6650 XT with Mesa RADV (the traced start and the
   three live tests, `results.md`); another driver or GPU that needs more falls back to the
   CPU, and tracing other drivers (AMDVLK, Intel, NVIDIA) is the outstanding item
   (ASSIST-L34). The whole PCI sysfs folder is exposed read-only (the trace shows only
   `vendor`, `device`, `subsystem_vendor`, `subsystem_device`, `uevent` and the `device` and
   `drm` links read): a fingerprinting exposure only, to be narrowed to those files
   (ASSIST-L44). All Vulkan ICD manifests are visible, not only the selected card's driver
   (ASSIST-L45).
3. **Unconfined (seccomp only), not used in this release (ASSIST-43)**: what follows describes
   the switched-off code. On `127.0.0.1` with a random port: "not confined: TCP
   egress allowed, and the whole filesystem readable by Mimik's user is readable by the model
   server". The model is `--model /proc/self/fd/8`; the token is a 0600 file in the
   generation's 0700 directory, deleted once `/health` answers; `HOME` is an empty private
   directory there. A child that cannot bind its port exits and a new port is tried, at most
   3 per attempt.

**Seccomp (both levels; the exec rule at level 3 only), pure Rust** (`seccompiler` plus four hand-written first
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
thread. Level 1 allows exec: the shim executes llama-server inside the sandbox, and anything
executed there reaches only what the sandbox holds. Seccomp sees no `connect()` address and is
not network confinement.

The start-time probe (`mimik assistant-runner --probe-inner`) runs Mimik itself in place of
the server under each level's full profile (the same binds, descriptors, seccomp program and
token channel): it reads the model's magic, the token and binds the socket or port (on the GPU
path it also opens the bound render node read-write), reports one JSON line and answers one
connection, on which the endpoint check runs. `mimik
assistant-runner --probe --runtime <dir> --models-dir <dir> --model local:<name>
[--model-sha256 <hex>] [--data-dir <dir>]` prints the result as one JSON line once its probes'
cleanup has ended (level 3's result with `"used": false`; the level reached is 1 or none) and
exits 0 only
when level 1's probe passed (the CI and owner check). A probe that fails for a definite cause
(an unusable bwrap, a changed file, a refusal inside the level or by the endpoint check)
skips its level; one that is only inconclusive (no report within 30 s, a failed spawn or bwrap
with no status line, a transient endpoint error such as `EMFILE`, `ENOMEM` or `EAGAIN`) is
tried again, three times in all. A level that stays inconclusive is kept for the real start to
decide, but `--probe` reports it `"ok": false, "inconclusive": true`, never as reached.

## Hakoniwa sandbox (comparison build)

Builds with the `sandbox-hakoniwa` feature (off by default, DEP-24) can run level 1 through
the Hakoniwa library instead of bubblewrap: `--assistant-sandbox hakoniwa` (also
`assistant-runner --probe --sandbox hakoniwa`; the live tests take `MIMIK_LIVE_SANDBOX`).
bubblewrap stays the default. Code: `src/assistant/runtime/hako.rs`; the runner's
`Exec::Hakoniwa` in `runner.rs`.

The runner (a fresh single-threaded process, checked before it forks) builds the container
and stays as its supervisor: new user, mount, PID, IPC, UTS, cgroup and network namespaces
(the network namespace stays empty: no interface, no DNS, no proxy), uid and gid mapped to
Mimik's own with `setgroups` denied, the root a private directory made read-only, and the
same allowlist as the bubblewrap profile (`bwrap::system_items` and `GpuBinds::items` feed both):
`/proc`; a 1 MiB tmpfs `/dev` with only `null`, `zero`, `full`, `random`, `urandom`, the
`fd` and standard-stream links and (GPU path) the one render node bound without `nodev`; no
`tty`, `pts`, `ptmx` or `console`; `/tmp` (64 MiB) and `/dev/shm` (256 MiB) tmpfs; the
generation's directory read-write at `/run/llama`; the runtime files, the model and the
token file read-only. Differences from the bubblewrap profile, each with its reason:

- Hakoniwa binds host paths and closes every descriptor above 2 in its child, so the
  verified descriptors are not bound. Each bind carries the `(device, inode)` the launcher
  verified (`files::Identity`); the closure compares what is mounted with it after the
  mounts and just before the exec and refuses to go on on any difference (an unreadable
  target counts as one). The descriptors never enter the sandbox.
- The token is a 0600 file in the generation's directory bound read-only at `/run/token`;
  the launcher unlinks the name once the runner has reported the sandbox (the bind mount
  keeps the inode), so it is on disk only for the moments of the start.
- The program is PID 1 of its namespace; its reaper is the runner's child outside it, which
  is the process the endpoint check pins (`child-pid`). The seccomp program already refuses
  `fork`, so the sandbox holds one process. The runner does not report the PID namespace's
  number (the reaper is not dumpable); the endpoint check then leaves that comparison to
  the parent and `NSpid` checks (`endpoint::NAMESPACE_UNREPORTED`).
- There is no shim. The closure runs in the program's process after the setup: a new
  session (no controlling terminal), the identity check, every capability set cleared
  (ambient, bounding, inheritable, permitted, effective), Mimik's seccomp program installed
  (`no_new_privs` is already set by Hakoniwa), then the marker. The marker pipe's write end is
  the program's standard input until the closure writes the marker on it, replaces
  descriptor 0 with `/dev/null` and closes every other descriptor, so nothing the program
  does can write it. A failure before the marker (a mount, the identity check, capabilities,
  seccomp) is a sandbox failure; the exec itself failing, and anything the server does or
  exits with afterwards, is the server's. No exit code or output of the server can count as
  a sandbox failure (`live_no_exit_code_of_the_server_counts_as_a_sandbox_failure`).
- Mount flags: each bind is `nosuid` and (but device nodes) `nodev`, plus `noexec` when its
  source mount has it, given up front, because the kernel refuses an unprivileged remount that
  drops a locked flag and Hakoniwa's fallback for that refusal silently drops read-only; the
  fallback is not enabled, so a mount that cannot be made as planned fails the start. A
  test lists the sandbox's mounts and requires read-only on all but the socket directory,
  the temporary space, `/dev` and `/proc`.
- A sandbox that ends at once (a mount refused, or a swapped file) is classified by the marker
  instead of failing the start on a reaper that is already gone.
- Parent death: the runner dies with Mimik (`PR_SET_PDEATHSIG`), Hakoniwa sets the signal in
  its two children, so the whole tree dies with the runner (`live_sigkill_of_mimik_*`).

Not tested: Ubuntu's AppArmor restriction of unprivileged user namespaces and the draft
profile `apparmor/mimik` (it needs an Ubuntu 24.04 VM); Landlock as an extra layer inside the
namespaces (the dev kernel has none); a real GPU. `results.md` has the comparison.

## Device (GPU path)

Render nodes come from sysfs: each `/sys/class/drm/renderD*` whose `device` resolves to a PCI
device (its `subsystem` is `pci` and its folder is named by a PCI address), whose `dev` numbers
match a character device `/dev/dri/<node>`. The PCI address is a device's identity across
restarts and tells two identical cards apart. Each node (under a PCI address choice, only that
one) is listed by the Vulkan build's `--list-devices` inside the level-1 GPU profile with only
that node bound (30 s at most; the output format is pinned by tests from the pinned build's
`"  %s: %s (%zu MiB, %zu MiB free)"` and its `(none)`, any other output refuses the node). A
listing counts only when it is read to its end without error within 64 KiB (longer output is
refused, never cut) and its process exits successfully within the 30 s; both streams are
drained and the sandbox is killed when the listing ends or the time runs out.
Software renderers (`llvmpipe`, `lavapipe`, `SwiftShader`, by description: the name leads it or
opens a parenthesised driver) are excluded before any count, and each list must then show
exactly one hardware GPU (a card that two Vulkan drivers both list, such as RADV and AMDVLK,
shows two and runs on the CPU: ASSIST-L45). Ranking under `auto`: the PCI
address chosen last while that card is still listed, else dedicated devices (sysfs reports
`mem_info_vram_total` above 0) before the others, then by listed memory, ties by the lower PCI
address. `boot_vga` is not used: it marks the firmware's primary display, not a discrete card.
Why each other node was not used is on the page. The GPU path runs at level 1 only; the
single-GPU rule of the TCP level comes with level 3's return (ASSIST-L25).

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
device and model-load errors (which name a failed start). The summary has no text field and no
line is ever kept, forwarded or logged, at any verbosity (a test feeds request-looking lines
with a marker phrase). The GPU path starts llama-server with `--verbosity 4` because the pinned
build prints the offload line only from that level; in the live check a job's marker phrase
appeared in neither stream at that level (`results.md`). Nothing the child prints can make
an exit a sandbox failure: at level 1 that is decided only by the shim's marker pipe ending
cleanly without the marker (bwrap failed before the exec). The marker's arrival, a read error,
unexpected bytes, or a reader still running after the readers' wait (5 s) all count the exit
at the same level.

## Generations

Each launch has a generation number; its directory `<data dir>/run/<generation>/` is created
0700 (anything already there refuses the start) and removed with its contents, never
following links, when the generation stops. The pidfile `run/assistant.pid` is held under an
exclusive `flock` for Mimik's life ("another Mimik uses this data directory's assistant"); it
records the child's PID, start time, possible executables and its owner's key (the generation
number, or a probe's own key above any generation), followed by a SHA-256 line over them; an
entry is cleared only by its owner. A record writes the entry over the old one, syncs, then
truncates, so an interruption leaves the old entry, the new one, or a torn one that never
parses; a record that fails fails the start. The pidfile has its own lock (its writes never
hold the launcher's state lock). An empty pidfile means no child; one that cannot be read or
does not parse refuses the start ("the pidfile run/assistant.pid cannot be read") and leaves
the generation directories in place, as nothing tells what ran. At
start, off the async runtime, a leftover's pidfd is opened first, the three are checked after
that, the kill goes through the pidfd and the sweep waits (5 s at most) for it to end; a
leftover still running keeps its entry, refuses the start ("a model server from an earlier run
has not exited yet") and leaves the old generation directories in place until a later start
finds it gone. A child is owned from its spawn: any failure before the start completes kills
and reaps it, kills and waits for the sandbox's reaper (without a deadline, saying so in the
log after 5 s), and clears the entry it recorded; the next spawn waits until that has ended. Every probe's and start's blocking part and every child's watcher is in-flight work:
the owner's Restart or shutdown cancels it (it stops before spawning or recording anything;
bwrap's status wait, the probe's report wait, the retry pause, the probe's endpoint check and
hashing, every MiB, see the cancel within about 100 ms; a file read that stalls, on a slow or
hung disk, holds it longer) and the manager waits until it has all
finished, cleanup included, before the next session starts or the shutdown completes. The
leftover sweep is such work too, and puts the pidfile back itself. That wait has no deadline:
a cleanup waits for the runner and the reaper to exit, however long, keeping the pidfile entry
until then. Probe directories are
`run/probe-<sequence>-<level>/`. One manager
task serializes every transition, so two servers never coexist: a child is stopped with
SIGTERM through the runner's pidfd (on level 1 to bwrap, which takes the sandbox down with
it) and SIGKILL after 5 s, repeated until gone; it is gone, and its exit is published, only
when the runner has exited and, on level 1, the reaper's pidfd shows it gone. Nothing stops a
running child but a stop. On every
generation change, queued and running jobs end "the assistant restarted: ask again" (neutral
for the breaker); a child exit is never a breaker event. On shutdown Mimik stops the child.

An attempt is: start (preflight once per start from level 1: the pidfile, the run directory,
the model's size; then each level's probe), spawn and the endpoint check, `/health` within
60 s (250 ms polls), priming within 300 s, calibration within twice the path's run cap plus
60 s (the two samples and the two `/tokenize` calls; GPU 180 s, CPU 240 s) and each
calibration sample within the run cap (a sample past it makes the machine "too slow",
not a counted failure). On the GPU path a start first lists the selected node again in its own
sandbox (it must still show one hardware GPU with the recorded description; `--device` gets
that listing's name), and after `/health` the child's offload line must show every layer on
the GPU (N = M, within 5 s; a device error or N < M fails the attempt). GPU transitions (any
failure on the GPU path, including those the CPU rows count or hold):

| Event | Action | Failure count |
|---|---|---|
| The GPU probe finds no usable device or level, or a GPU attempt fails (start, the listing check, `/health`, the offload line, priming, calibration, budgets that do not derive), or the GPU child exits while ready or is stopped as hung | stop and wait until gone, then the CPU path at once (its own probe, from level 1), the reason on the page | not counted |
| Restart | stop, start again from level 1, staying on the CPU path after a GPU failure | reset |
| "Try the GPU again" (owner, CSRF-checked; only while the GPU is wanted and the CPU path runs) | as Restart, from the GPU path; a second press while it runs starts nothing | reset |

The CPU path stays until Mimik restarts or the owner presses "Try the GPU again". CPU
transitions:

| Event | Action | Failure count |
|---|---|---|
| Sandbox-attributable failure (bwrap setup, no status before exec) | `unavailable` with the fix (level 1 is the only level used), at once | not counted |
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
shipped demo and 2b synthetic sheets (GPU 4,096; one opens with `[`, the other with `{`, so neither
reuses the other's cache), each counted by `/tokenize`. Timings must be present, finite and
positive, `predicted_n` equal to `max_tokens`, the tail `prompt_n` − Nᵢ not negative and the
cached tokens at least 75 % of priming's prompt tokens, or the attempt fails. The budgets
follow `assisted-sheet-plan.md` §1.4a "Budgets" (ASSIST-31, ASSIST-32): T(B) is the larger of
the two samples' estimates per candidate (CPU 1,024, 768, 512; GPU 4,096, 2,048, 1,024), R =
max(15, 1.25 × T rounded up to 5 s) within 90 s on the CPU path (D 1, Q = 120 − R) and 60 s on
the GPU path (D = max(1, min(4, floor((120 − R) / R))), Q = D × R). After a run-deadline expiry, or when a running job
is superseded or cancelled, the next job waits the larger sample's prompt term at B (the request may still be in prompt evaluation), and
admission counts that wait, so one expiry no longer makes the next job miss its deadline too.
The measurement and the budgets are on the page.

## Owner page

`/admin/assistant` (owner only): the state with its reason, the pinned build, path,
generation, failure count and last failure, the breaker, the active level with its wording,
notes, skipped levels with reasons, the device (the selected GPU's description, in-sandbox
name, PCI address, render node and memory; render nodes not used, with why; "CPU only" under
`--assistant-device cpu`; why the GPU is not in use), the budgets, and the priming and sample
measurements. Restart (POST, CSRF-checked) stops the server and starts again from level 1; "Try
the GPU again" (POST `/admin/assistant/try-gpu`, CSRF-checked) is shown while the GPU is wanted
and the CPU path runs; a second press of either while one runs starts nothing ("A restart is
already running").

## Development endpoint

Builds with the `dev-endpoint` feature (never a release) keep `--assistant-dev-endpoint` and
`--assistant-dev-token-file` for tests: a hand-started server (or a fake) as the manager's
only child, primed and calibrated the same way, without an endpoint check (the page says so).
