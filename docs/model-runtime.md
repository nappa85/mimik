# Model runtime

The interpretation assistant's model server (steps 2c-2a, 2c-2b-1 and 2d-1; ASSIST-27,
ASSIST-28, ASSIST-29 to ASSIST-36, ASSIST-40 to ASSIST-42, ASSIST-52, ASSIST-56): with `--assistant`, Mimik starts the pinned official
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
| Seccomp program | `src/sandbox/seccomp.rs` |
| Sandbox: shared construction, host check | `src/sandbox/mod.rs`, `src/sandbox/host.rs` |
| llama-server sandbox profile and allowlist | `src/assistant/runtime/hako.rs`, `src/assistant/runtime/sandbox.rs` |
| Start-time probe, `assistant-runner` entry | `src/assistant/runtime/probe.rs` |
| Launcher (levels 1 and 3), the child's lifetime | `src/assistant/runtime/launch.rs` |
| Endpoint check | `src/assistant/runtime/endpoint.rs`, transport in `src/assistant/transport.rs` |
| Token | `src/assistant/runtime/token.rs` |
| Pidfile, generation directories | `src/assistant/runtime/rundir.rs` |
| Output drain | `src/assistant/runtime/drain.rs` |
| Priming, calibration, budgets | `src/assistant/runtime/calibration.rs`, `server.rs`, `budgets.rs` |
| Generations and transitions | `src/assistant/runtime/manager.rs` |
| Model setting: model, trial key and report, bounds | `src/assistant/runtime/models.rs` |
| Trial, Switch, Switch back | `src/assistant/runtime/manager_model.rs`, `src/assistant/runtime/trial.rs` |
| Model spec grammar, reply-length bounds | `src/assistant/model.rs` |
| Stored setting (migrations 022, 032) | `src/assistant/records.rs`, `migrations/022_assistant_models.sql`, `migrations/032_assistant_reply_length.sql` |
| Thread count | `src/assistant/runtime/threads.rs` |
| Device choice (render nodes, listing, ranking) | `src/assistant/runtime/device.rs` |
| Owner page | `src/auth/assistant.rs`, `templates/admin_assistant.html` |
| Model downloads (step 2d-2) | [model-downloads.md](model-downloads.md) |

## Options

`--assistant` turns it on (off by default: no child, no probe, no route, the page says
"Off"). `--assistant-model` (`local:<name>.gguf` by the grammar below), `--assistant-model-sha256`
(required for any file but the two defaults; both it and the alias need `--assistant-model`),
`--assistant-model-alias` (default the file name without `.gguf`, lower case). These name the
model of a database that has no stored setting; once the owner has switched models, the stored
setting wins and the flags are ignored (the page says so). **Default by device (ASSIST-55):**
without `--assistant-model` and without a stored setting, the manager picks the model at each
start (`default_model` in `src/assistant/runtime/models.rs`): on the GPU path the Qwen3.5-9B
`Q4_K_M` file (`Qwen3.5-9B-Q4_K_M.gguf`, Apache-2.0, SHA-256 compiled in; reply length 1,024 unless the owner stored one for this file)
when it is in the models folder, otherwise Granite 4.2 3B with "Qwen3.5-9B not found or not
readable in <absolute models folder>; using Granite 4.2 3B" on `/admin/assistant` (a warning,
not a block); the same when the 9B file fails its pinned hash at the load ("Qwen3.5-9B in
<absolute models folder> does not match its pinned hash"). A plain Restart keeps this fallback
and its note. "Try the GPU again" (even while Granite runs on the GPU) or Mimik's restart
checks the 9B file again. An explicit or stored 9B is refused instead, as any model whose file
changed. On the CPU path
(`--assistant-device cpu`, no usable GPU, or a GPU start that fell back) Granite 4.2 3B
(file `granite-4.2-3b-Q4_K_M.gguf`, SHA-256 compiled in; reply length 1,024 unless the owner stored one for this file), with a note when
the GPU was wanted. "Try the GPU again" picks again. An explicit flag or the stored setting is
never replaced; a Switch or Switch back ends the default, and "Switch back" returns to the
model that was running. `--reasoning off` is in every start's arguments, so Qwen3.5's thinking
is off for the 9B as for any model (checked in the launch arguments; no live run). `--models-dir`
(default `<data dir>/models`, mode 0700, owned by Mimik's user; the model is placed there by
hand). `--assistant-runtime` (default `llama/` beside the binary; it must hold exactly the
pinned files either way). `--assistant-threads` (default the usable physical cores minus one,
at least 1; physical cores are the distinct `core_cpus_list` sets in sysfs among the CPUs of
Mimik's affinity mask, capped by the tightest `cpu.max` quota of its cgroup and the cgroup's ancestors, ASSIST-41). `--assistant-device`
(`auto`, the default: the best hardware GPU, else the CPU; `cpu`: the CPU path only, no device
is ever listed; or a GPU's PCI address such as `0000:0c:00.0`, or `0c:00.0` for domain 0). The
data directory is the database's folder; `run/` lives there.

## Pinned build

The manifest pins release b12110 (the commit tagged v0.9.0, `cd959ba`), its CPU asset
`llama-b12110-bin-ubuntu-x64.tar.gz` (size and SHA-256), the 22 files Mimik runs from it
(each under its SONAME, no links: `llama-server`, `libllama-server-impl.so`,
`libllama-common.so.0`, `libmtmd.so.0`, `libllama.so.0`, `libggml.so.0`, `libggml-base.so.0`,
the 14 `libggml-cpu-<variant>.so` backends and `LICENSE`; the RPC backend is not shipped)
with size, SHA-256 and mode, the system libraries they need (`libc.so.6`, `libm.so.6`,
`libstdc++.so.6`, `libgcc_s.so.1`, `libgomp.so.1`, `libssl.so.3`, `libcrypto.so.3`; symbol
floors GLIBC 2.34, GLIBCXX 3.4.30, OpenSSL 3.0.0), the documented minimum host (Ubuntu 22.04
`x86_64`), the source archive (`git archive` of the commit, tar SHA-256; a build-time input for the notice texts only, never shipped, ASSIST-47) and the CI test model
(`ggml-org/models-moved` at `499bc88…`, `tinyllamas/stories260K.gguf`, upstream MIT). ggml
loads every `libggml-cpu-<variant>.so` beside the server and uses the best one the CPU
supports. The Vulkan asset `llama-b12110-bin-ubuntu-vulkan-x64.tar.gz` (SHA-256
`a5d74194…`, ASSIST-42) holds the same 22 files byte for byte plus `libggml-vulkan.so` (the
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
handed, and executes in place with only `PATH`, `LANG` and `HOME`: the sandbox on level 1 (the runner builds it and stays as its supervisor); on
level 3 the verified llama-server descriptor, moved to number 1023 (close-on-exec) with
`RLIMIT_NOFILE` set to 1023, after installing the seccomp program, by `execveat(1023, "",
…, AT_EMPTY_PATH)`. Descriptors are moved to their numbers between fork and exec with
`fcntl(F_DUPFD_CLOEXEC)` then `dup2` only.

## Backend loading (ggml)

The bundled build is `GGML_BACKEND_DL=ON` with `GGML_CPU_ALL_VARIANTS=ON`: ggml picks a CPU
backend at run time by `dlopen`ing each `libggml-cpu-*.so` it finds and scoring it, and a
candidate's constructors run while it is open (ASSIST-42, audited against the b12110 source,
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
  current-directory search is that folder, never Mimik's own cwd); at level 1 the sandbox's working
  directory is `/opt/llama`, and only the allowlisted binds are visible inside the sandbox;
- clears the environment and never sets `GGML_BACKEND_PATH` or `GGML_BACKEND_DIR`.

## Confinement levels

Mimik runs llama-server at level 1 or not at all (ASSIST-43, ASSIST-48; there is no level 2): when level 1 cannot be used, the
assistant is unavailable and `/admin/assistant` says how to fix it (allow
unprivileged user namespaces, the AppArmor note), with level 1's own reason. Level 3 is built and
unit-tested but switched off: no manager path probes or starts it, and `--probe` reports its
result labelled "not used". It returns later as its own step behind an explicit owner opt-in. The launcher itself refuses
to start level 3 whatever a configuration asks (a sandbox-class failure, neither counted nor
retried). This covers what Mimik starts (the manager,
Restart, the routes and the start-time probe, which runs level 1 only); `--probe` is the one
path that still runs level 3's probe, which starts Mimik's own probe child under seccomp,
never llama-server; the private `assistant-runner` entry points
(`--spec-fd`, `--probe-inner`) are an internal protocol between Mimik and its runner,
not a privilege boundary (a local user who can run them can run llama-server directly).
Why each level was not used is on the page and in the log.

1. **The Hakoniwa sandbox** (ASSIST-51; built as the next section describes). Only
   `user.max_user_namespaces = 0` rules the level out before its probe; the probe decides
   otherwise, and a failed probe names `kernel.unprivileged_userns_clone = 0` or
   `kernel.apparmor_restrict_unprivileged_userns = 1` as a possible cause (the page then gives
   the AppArmor profile command). Inside: no network, a minimal `/dev`, the read-only system
   library folders (`/usr/lib`, `/usr/lib64`, `/lib`, `/lib64`, `/usr/libexec/ld`), the dynamic
   loader cache and the CPU topology, `/proc`, `/tmp` (64 MiB) and `/dev/shm` (256 MiB), the
   generation's directory read-write at `/run/llama`, each runtime file read-only under
   `/opt/llama`, the model at `/models/model.gguf`, the token at `/run/token`. Nothing else of
   the host is visible. The server listens on `/run/llama/llama.sock`.
   On the GPU path the profile also binds the selected render node read-write at
   `/dev/dri/<node>` (the only device bound; the CPU path binds none), its PCI
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
thread. Level 1 allows exec: the closure executes llama-server inside the sandbox, and anything
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
when level 1's probe passed (the CI and owner check). `--model-sha256` may be left out only for
the two compiled-in files (Granite 4.2 3B and Qwen3.5-9B); any other file is refused without it. A probe that fails for a definite cause
(an unusable sandbox, a changed file, a refusal inside the level or by the endpoint check)
skips its level; one that is only inconclusive (no report within 30 s, a failed spawn or a
runner with no status line, a transient endpoint error such as `EMFILE`, `ENOMEM` or `EAGAIN`) is
tried again, three times in all. A level that stays inconclusive is kept for the real start to
decide, but `--probe` reports it `"ok": false, "inconclusive": true`, never as reached.

## The sandbox (Hakoniwa)

Level 1 is the Hakoniwa library's sandbox (ASSIST-51, DEP-24), built in the runner. Code:
`src/sandbox/mod.rs` (the serializable profile, container, closure and Landlock layer),
`src/sandbox/host.rs` (the host check), and `src/assistant/runtime/hako.rs` and `sandbox.rs`
(llama-server's profile and allowlist); the runner's `Exec::Hakoniwa` in `runner.rs`.
The shared `Profile` (also named `Plan` in the runner protocol) declares binds and their
identities/access, bounded tmpfs mounts and flags, private files, links/directories, workdir,
argv/environment, explicit Landlock rules and whether to mount and grant `/proc`. It has no
model, token or socket-directory requirement; those are inputs to the llama-server profile.
`Profile::validate()` runs before encoding and after decoding the runner spec. It refuses
non-absolute or unnormalised rule/bind/tmpfs/file paths, explicit `/proc` rules, writable
tmpfs mounts missing `noexec`, `nosuid` or `nodev`, execute grants outside read-only binds,
and private files outside writable tmpfs mounts. The `proc` field controls the automatic
proc mount and read grant. Profile authors remain trusted to select the allowlist, verify
identities, bound resources, and make mount ordering, overlaps, symlinks, root/workdir,
directory topology, commands and environment safe; validation does not resolve host paths
or check their ownership/content. Without proc, descriptor cleanup requires `close_range`
to succeed; its failure refuses the closure with descriptor exit code 115.

The runner (a fresh single-threaded process, checked before it forks) builds the container
and stays as its supervisor: new user, mount, PID, IPC, UTS, cgroup and network namespaces
(the network namespace stays empty: no interface, no DNS, no proxy), uid and gid mapped to
Mimik's own with `setgroups` denied, the root a private directory made read-only, and the
allowlist of `sandbox::system_items` and `GpuBinds::items`:
`/proc`; a 1 MiB tmpfs `/dev` with only `null`, `zero`, `full`, `random`, `urandom`, the
`fd` and standard-stream links and (GPU path) the one render node bound without `nodev`; no
`tty`, `pts`, `ptmx` or `console`; `/tmp` (64 MiB) and `/dev/shm` (256 MiB) tmpfs; the
generation's directory read-write at `/run/llama`; the runtime files, the model and the
token file read-only. How it is built, each point with its reason:

- Hakoniwa binds host paths and closes every descriptor above 2 in its child, so the
  verified descriptors are not bound. Each bind carries the `(device, inode)` the launcher
  verified (`files::Identity`); the closure compares what is mounted with it after the
  mounts and just before the exec and refuses to go on on any difference (an unreadable
  target counts as one). The descriptors never enter the sandbox.
- The token is the content of a file the runner has Hakoniwa write at `/run/token` into the
  sandbox's own `/run`, a 64 KiB tmpfs (memory) under the socket directory's bind. It reaches
  the runner in the spec memory file only: never a file on the host, a command line or an
  environment variable (a test scans the data and scratch directories for it). It is an ordinary file
  inside the sandbox, which the server can read (it needs to) and rewrite (harmless: it knows
  the token).
- The program is PID 1 of its namespace; its reaper is the runner's child outside it, which
  is the process the endpoint check pins (`child-pid`). The seccomp program already refuses
  `fork`, so the sandbox holds one process. The runner does not report the PID namespace's
  number (the reaper is not dumpable); the endpoint check then leaves that comparison to
  the parent and `NSpid` checks (`endpoint::NAMESPACE_UNREPORTED`). The runner reports the
  server's own host PID and start time (`inner-pid`, `inner-start`) too, and Mimik holds a
  pidfd on it: a forced stop kills and waits for the server first, then the reaper, and
  publishes the exit only then, because the reaper can end before the server has released its
  memory. The runner does not reap the reaper (a zombie can still be pinned) until Mimik has
  pinned both and said so on an acknowledgement pipe (a closed pipe makes it end the sandbox);
  each handle is attached to the failed-start guard the moment it is opened. The runner finds
  the server by pinning each candidate with a pidfd and reading its parent and start time from
  one read of its `stat`, and takes it only while its pidfd still reads as running. A server
  whose pid is gone, or reused (another start time), has ended and is never pinned or signalled;
  one whose start time cannot be read for another reason (`EMFILE`, `EACCES`, a garbled line)
  keeps its pidfd and is waited for until it exits, never signalled; a
  sandbox that ended before the report says so (`ended`, the reaper reaped after the server's
  own exit status), and the marker then classifies the exit.
- There is no shim. The closure runs in the program's process after the setup, in this
  order (`sandbox::STEP_ORDER`): a new session (no controlling terminal), the identity check, the
  bounding set dropped capability by capability (up to the kernel's last one) and the other
  sets cleared, Mimik's seccomp program installed (`no_new_privs` is already set by Hakoniwa),
  the marker pipe's end (standard input) moved to descriptor 3 close-on-exec, `/dev/null`
  put on descriptor 0, every descriptor above 3 closed, the runner's umask restored, and
  the marker written on descriptor 3 as the very last step before the exec, which closes it.
  A step that fails ends the closure at once, with no marker; so the marker proves that every
  step passed, and nothing the program does can write it (it never holds the descriptor). A
  failure before the marker is a sandbox failure; the exec itself failing, and anything the
  server does or exits with afterwards, is the server's. No exit code or output of the
  server can count as a sandbox failure
  (`live_no_exit_code_of_the_server_counts_as_a_sandbox_failure`).
- Landlock (ASSIST-49, DEP-25): after the capabilities and before seccomp, the closure
  restricts the server further with the `landlock` crate, best effort. Handled: the ABI 6
  filesystem rights, TCP bind and connect (no rule, so both are denied: `EACCES`, beyond the
  empty network namespace) and the abstract-Unix-socket and signal scopes. The rules come from
  the plan (selected by `hako::profile_rules`; `sandbox::landlock_rules` only adds `/proc`
  when enabled): execute only on the program the closure execs and the
  system library directories (the kernel opens the ELF interpreter with the execute right;
  the loader maps the runtime's libraries with `mmap`, which that right does not govern, so
  they get read and list only, as do the GPU data, the sysfs binds and the probe's own
  executable when it is not the program); the model read only; read and write (create,
  remove, sockets, no execute, no device nodes) on the socket directory, `/tmp` and
  `/dev/shm`; of `/run` only the token file, read; listing on `/dev`, `/dev/dri` (libdrm and
  Mesa enumerate it), the runtime's directory (ggml looks for its backends there) and the GPU
  path's `/sys` scaffolding; `/proc` read; the five device nodes and the render node as
  devices (read, write, `ioctl`). Every tmpfs of the sandbox is also mounted `noexec`,
  `nosuid` and `nodev`. What the mounts alone allow and the
  rules do not (a file in the writable `/dev` tmpfs, a write in `/proc`, listing `/`, `/etc`
  or `/sys`) is denied. The step's result (`full` with the kernel's ABI, `partial`, or `none`
  when the kernel has no Landlock) is appended to the marker line on the marker pipe, in the
  same write (`exec\n{"landlock":"full","abi":11}\n`): the runner cannot know it (Hakoniwa
  closes its descriptors above 2, and its own status line is written before the closure
  finishes) and the server cannot forge it (it never holds the descriptor).
  Readiness waits up to the bounded marker-reader deadline for a report; a missing or
  unreadable report fails the attempt rather than silently omitting the layer's status.
  The report reaches `Status.landlock` and the Confinement section of `/admin/assistant` ("Landlock: full for the ABI 6 rules (kernel ABI n)", "partial (kernel ABI n)" or "not
  available", each with a note: the full one says that newer rights such as ABI 9's Unix-socket
  connect are not asked for; the others say that the namespaces confine
  the server then, and below ABI 6 the rights that ABI supports) and the `ready` log line. "Not available" is exactly a
  version probe (`landlock_create_ruleset` with the version flag) that fails with `ENOSYS`
  (no Landlock in the kernel), `EOPNOTSUPP` (disabled at boot) or `EPERM` (a seccomp or
  service-manager filter around Mimik refuses it); the start goes on. Any other probe
  error, and every failure after a successful probe (a path that cannot be opened, a rule or
  the restriction refused, a ruleset not fully enforced), fails the start (exit 117, before the marker, a sandbox failure). After a successful probe the ruleset is built for exactly the probed ABI (capped at 6) with every right required, so the crate's own later detection failing is an error too.
  The three "not available" errors are simulated with a seccomp filter in the live tests. The
  seccomp program installs after it and does not touch the Landlock calls; the exec of the
  server needs only the execute right on its file and the loader.
  Starting a new session clears the previous report before preflight and namespace probing,
  so a failed restart never presents the stopped generation's protection as current.
- Missing libraries (a best-effort diagnostic, not the loader): before the sandbox starts, the
  `DT_NEEDED` names are read from the ELF dynamic section of every ELF file of the runtime
  build, the server, its libraries and the backends it loads with `dlopen` (`libggml-cpu-*.so`
  link `libgomp.so.1`, `libggml-vulkan.so` links `libvulkan.so.1`), out of the descriptors the
  runtime check opened and hashed (`libs.rs`, `object` crate, DEP-26); non-ELF files (the
  licence) are skipped. A name is found when it is another file of the build, or a regular
  file (a link must resolve) that is an ELF of the same class and machine in the folders the
  sandbox binds: the loader's default folders (`/usr/lib`, `/lib64` and the multiarch folders
  directly under them) or the path `ld.so.cache` gives (parsed read-only). A name found
  nowhere in a complete search is advisory only: the sandboxed start is always attempted, and
  the check changes no level, no confinement and nothing about whether the sandbox is required.
  If that start then fails (no `/health`, an exit before ready, a GPU listing or start failure),
  the failure's reason gets the hint "llama-server may need libgomp.so.1: install libgomp1
  (Debian/Ubuntu) or your distribution's OpenMP runtime" (on the GPU path with the GPU to CPU
  fallback reason, counted or not exactly as the failure itself); if the start succeeds nothing
  shows. Nothing is executed on the host: no dynamic loader run, no host-side
  process. The answer is "unknown", and the start goes on as it would without the check, when a
  file is over 256 MiB, cannot be read or parsed, or names something that is not a plain library
  name; when a name has candidates only in places the loader would not look; and when the search
  was incomplete (its budget of entries, folders and depth was reached, a folder could not be
  read, or the start was cancelled). Folders that are aliases of each other are walked once.
  Candidates are opened without blocking and must be regular files (a FIFO or a link to one is
  refused), and entries that are neither files, links nor folders are never opened. The
  `ld.so.cache` read is bounded (64 MiB, opened without blocking and refused unless a regular
  file; the container is checked: the new format at the start or after the old format's own
  table, the declared entry and string tables inside the file, every string offset inside the
  declared string table, each string at most 4 KiB, at most 100 000 entries, a total string
  budget, cancellable); a cache that cannot be read or recognised (an old-format-only one, a
  damaged one) is "unknown", which is not the same as a cache that parsed and is empty. The
  parsed cache is kept for the process while the file does not change; a transient read error
  (no descriptors, an I/O error) is not kept, a parse or a definite refusal is. Looking a name
  up is bounded too: the loader's default folders come first and a hit ends the lookup; only
  the names they do not settle go to the cache, indexed once by name with repeated paths
  dropped; candidate opens, the cache scan and the enumeration of the default folders count
  against budgets and watch for a cancel, and running out of either makes the answer "unknown".
  The hint belongs to one attempt (cleared when a verification, a probe or a start begins) and
  only decorates the failure of that attempt's start or `/health`, or of the GPU listing or
  probe: not a probe's reason, the level 3 policy skip, or the exit of a server that was ready.
- Mount flags: each bind is non-recursive, and a mount under a read-only bound directory
  (read from the host's mount table at the start) is masked by an empty read-only tmpfs, so
  no submount can stay writable under a read-only bind and the files the directory holds
  underneath it do not show; `nosuid` and (but
  device nodes) `nodev`, plus `noexec` when its source mount has it, given up front, because
  the kernel refuses an unprivileged remount that drops a locked flag and Hakoniwa's fallback
  for that refusal silently drops read-only; the fallback is not enabled, so a mount that
  cannot be made as planned fails the start (the atime flags are not given: the library has no
  option for them; `noatime` on `/` works on the dev host, other atime settings are untried,
  HAKO-L4). A test lists the sandbox's mounts and requires read-only on all but the socket
  directory, the temporary space, `/dev` and `/proc`.
- A sandbox that ends at once (a mount refused, or a swapped file) is classified by the marker
  instead of failing the start on a reaper that is already gone.
- Parent death: the runner dies with Mimik (`PR_SET_PDEATHSIG` and a parent check), Hakoniwa
  sets the signal in its two children right after their fork, so the whole tree dies with the
  runner (`live_sigkill_of_mimik_*`, and a sweep of kill delays over a start). Hakoniwa does
  not re-check its parent after setting the signal (HAKO-L5).

`mimik assistant-apparmor-profile` prints the `AppArmor` profile for the running binary's own
path (template `apparmor/mimik`, also for distribution packages); when the sandbox cannot
start and Ubuntu's restriction is on, the assistant page's message gives that command with
the `tee` and `apparmor_parser -r` steps. `--for <absolute path>` prints it for
another binary (packagers, test setups). Ubuntu 24.04's restriction and the profile were checked in a VM
(`results.md`; repeatable with `/media/data/mimik/ubuntu-vm/run.sh`). The Landlock layer on a kernel below ABI 6
ran once on that VM (ABI 4); a kernel with Landlock disabled at boot is only simulated (a seccomp filter
answering `ENOSYS`, `EOPNOTSUPP` or `EPERM` to its calls). `results.md` has the measurements.

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
an exit a sandbox failure: at level 1 that is decided only by the marker pipe ending
cleanly without the marker (the closure failed before the exec). The marker's arrival, a read error,
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
the runner's status wait, the probe's report wait, the retry pause, the probe's endpoint check and
hashing, every MiB, see the cancel within about 100 ms; a file read that stalls, on a slow or
hung disk, holds it longer) and the manager waits until it has all
finished, cleanup included, before the next session starts or the shutdown completes. The
leftover sweep is such work too, and puts the pidfile back itself. That wait has no deadline:
a cleanup waits for the runner and the reaper to exit, however long, keeping the pidfile entry
until then. Probe directories are
`run/probe-<sequence>-<level>/`. One manager
task serializes every transition, so two servers never coexist: a child is stopped with
SIGTERM through the runner's pidfd (on level 1 to the runner, which takes the sandbox down with
it) and SIGKILL after 5 s, repeated until gone; it is gone, and its exit is published, only
when the runner has exited and, on level 1, the reaper's pidfd shows it gone. Nothing stops a
running child but a stop. On every
generation change, queued and running jobs end "the assistant restarted: ask again" (neutral
for the breaker); a child exit is never a breaker event. On shutdown Mimik stops the child.

An attempt is: start (preflight once per start from level 1: the pidfile, the run directory,
the model's size; then each level's probe), spawn and the endpoint check, `/health` within
60 s (250 ms polls), priming within 300 s, calibration within 1,020 s (the two samples at
480 s each and the two `/tokenize` calls) and each calibration sample within 480 s (a sample
past it makes the machine "too slow", not a counted failure: a wait above 480 s already means
R over the 600 s ceiling). On the GPU path a start first lists the selected node again in its own
sandbox (it must still show one hardware GPU with the recorded description; `--device` gets
that listing's name), and after `/health` the child's offload line must show every layer on
the GPU (N = M, within 5 s; a device error or N < M fails the attempt). GPU transitions (any
failure on the GPU path, including those the CPU rows count or hold):

| Event | Action | Failure count |
|---|---|---|
| The GPU probe finds no usable device or level, or a GPU attempt fails (start, the listing check, `/health`, the offload line, priming, calibration, budgets that do not derive), or the GPU child exits while ready or is stopped as hung | stop and wait until gone, then the CPU path at once (its own probe, from level 1), the reason on the page | not counted |
| Restart | stop, start again from level 1, staying on the CPU path after a GPU failure | reset |
| "Try the GPU again" (owner, CSRF-checked; only while the GPU is wanted and either the CPU path runs or the GPU device default shows a fallback note) | as Restart, from the GPU path; a second press while it runs starts nothing | reset |

The CPU path stays until Mimik restarts or the owner presses "Try the GPU again". CPU
transitions:

| Event | Action | Failure count |
|---|---|---|
| Sandbox-attributable failure (sandbox setup, no status before exec) | `unavailable` with the fix (level 1 is the only level used), at once | not counted |
| Any other failed attempt (model load, a changed file, an exit after exec, a refused endpoint, no `/health`, failed priming or calibration, invalid timings) | retry after 2 minutes at the same level | +1 |
| Child exits while ready | restart now | +1 only within 10 minutes of `ready` |
| Two consecutive run-deadline expiries without a completed exchange between | stop as hung, restart, measure again | as a child exit |
| Failure count reaches 4 | `unavailable` until Restart | — |
| Budgets do not derive | `unavailable` ("too slow on this machine: about N s needed, the ceiling is 600 s", or "reply length too large for the context"), no retry; on the first start after a reply-length change the file's earlier length returns and starts (breaker not reset) | — |
| `ready` for 10 minutes | count back to 0 | reset |
| Restart (or Mimik's start) | stop, start again from level 1, breaker reset; it interrupts a preflight, probe or start in progress, and further presses are ignored until `ready` or `unavailable` | reset |

While `starting` or `unavailable` a job start answers 503 with the state and queues nothing;
entry, notes and Undo work as usual. Log lines (`assistant: …` on standard error) carry
states, levels, reasons and measurements, never prompt text, URLs with tokens or the token.

## Priming, calibration and budgets

Priming is the readiness gate: the fixed prefix with a minimal envelope and `max_tokens` 1
fills the prompt cache before any job is admitted. Then two samples, non-streamed, 64 tokens
(whatever the owner's reply length), `ignore_eos`, no schema, each with a filler of exactly the path's byte cap (CPU
1,024) made of context items shaped as requests carry them (ID, label and type, no value),
from the IDs and labels of the shipped demo and 2b synthetic sheets (GPU 4,096; one opens with `[`, the other with `{`, so neither
reuses the other's cache), each counted by `/tokenize`. Timings must be present, finite and
positive, `predicted_n` equal to 64, the tail `prompt_n` − Nᵢ not negative and the
cached tokens at least 75 % of priming's prompt tokens, or the attempt fails. The budgets
follow `assisted-sheet-plan.md` §1.4a "Budgets" (ASSIST-31, ASSIST-32): T(B) is the larger of
the two samples' estimates per candidate (CPU 1,024, 768, 512; GPU 4,096, 2,048, 1,024), R =
max(15, 1.25 × T rounded up to 5 s), at most the 600 s ceiling on both paths (a limit on stuck
requests, not a wait), with T using the measured per-token time g over the owner's reply
length L slowed by the context each token adds, g × L × (1 + α × L / 2), α 2.1e-4 on the CPU
path and 7.0e-5 on the GPU path (`Path::slowdown`, measured on 2,560-token replies,
`docs/results.md`; ASSIST-L47). The 64-token samples give g; the first token comes from the
prompt pass, which makes g about 1.6 % optimistic. The total wait is max(120 s, 2 × R): on the CPU path D 1 and Q =
total − R + busy, on the GPU path D = max(1, min(4, floor((total − R) / R))) and
Q = D × (R + busy), where busy is the calibrated prompt term (the time the server stays busy
after a request cancelled at R, which a queued job waits too), so D is 1 once R reaches 40 s. B must also leave room in the context: the fixed prefix (priming's
measurement) + N(B) + the reply length + 64 tokens of template margin within the 8,192-token
context, or the next smaller candidate is taken; with none left the start fails with "reply
length too large for the context" (a reply is never cut silently). After a run-deadline expiry, or when a running job
is superseded or cancelled, the next job waits the larger sample's prompt term at B (the request may still be in prompt evaluation), and
admission counts that wait, so one expiry no longer makes the next job miss its deadline too.
The measurement and the budgets are on the page.

**Prompt cache and reminders.** On both paths the server runs with `--cache-ram <MiB>`
(`launch::cache_ram_mib`) and `--ctx-checkpoints 4` (`CHECKPOINT_CAP`): room for two entries
of a whole context, so a reminder's prompt does not evict the interpretation prefix (one entry
thrashes; `docs/results.md`, "Prompt cache across prompts"). An entry is `ctx x KV per token + (1 +
cap) x recurrent-state size`, so the cache is `2 x (ctx x KV per token + (1 + cap) x state)`. The sizes
are measured constants for the two shipped default files, recognised by their pinned SHA-256
(`launch::entry_size`): Granite 4.2 3B 80 KiB per token and no recurrent state, Qwen3.5-9B 32 KiB per
token and 50.25 MiB per state. Any other file is unmeasured (there is no GGUF parser) and gets a
conservative bound: 80 KiB per token at least, more for a bigger file (file size / 24,000 KiB), and a
state of 1/64 of the file size. The start's memory check counts the cache and the slot's own
checkpoints ((1 + cap) states) beside the model, on both paths. That the user-boundary checkpoint
that restores the prefix survives the cap of 4 was measured on the GPU (`docs/results.md`, 3c-1b).
Priming's `prompt_n` and `prompt_ms` are kept in `Calibration::primed`. At Ready,
`reminder_plan::plan` tokenizes the reminder system message and the largest valid reminder reply
(outside the breaker) and `reminder_budgets::derive` gives the reminder's own run deadline and its
two busy terms, or the reason reminders are off for the generation, with what helps on the CPU path
(more threads, the GPU path): prompt and reply not fitting the context, deadline over the 600 s
ceiling, the unverified busy term over R plus an interpretation's busy term, or restore beyond R's
margin (both paths). The queue wait Q does not bound a reminder: its cost is an extra wait (below).
The busy terms are the extra wait X that a scribble reading admitted behind a running or dropped
reminder may have beyond Q, in whole seconds. Unverified (Y): the whole reminder prompt,
`p x (system message + N(B))`, plus restoring the interpretation's entry. Verified (X): restoring
the reminder's entry, the variable part and the user turn's template tail,
`p x (N(B) + TEMPLATE_MARGIN_TOKENS)`, plus the interpretation's restore (the restore of the
interpretation entry stays counted in X on purpose: it also falls in the next interpretation's run,
under the margin check). Y applies until a reminder of the generation has completed with at least
75 % of the reminder system tokens read from the cache (`Completed::cached_tokens`), and again after
any reminder cancelled after reaching the server or ending without a figure, until a later one
completes and verifies. Entry sizes count the whole saved entry: the interpretation's from prefix,
N(B) and `max_tokens`, the reminder's from system message, N(B) and the reply bound
(`launch::entry_mib`). b12110's cache token limit bounds the sum of all entries but is raised to the
memory limit divided by the average bytes per token (`docs/results.md`), so two whole contexts fit
while the size bound is at or above the model's real one. The restore cost is the development
host's measurement scaled by the entry's size (not measured on the running host): CPU
`RESTORE_S_PER_MIB`, GPU 0.53 ms per MiB of entry plus 10 ms.

**Reminder prime.** After the samples and the `/tokenize` step, a generation primes the reminder
prefix before Ready (`reminder_plan::prime`, its own progress step "preparing the reminder
prompt"): one reminder request through the same client path as real reminders
(`Interpreter::complete`, `RequestKind::Reminder`, a fixed synthetic envelope with no sheet data,
`max_tokens` 1), bounded by the reminder's run deadline, so the template start and the system
message match token for token. It runs on the GPU path always and on the CPU path only when the
owner's stored choice has reminders on at the start (the choice reaches the manager before it
starts; the development endpoint reads it after, so there an unset choice at the first start counts
as off). A CPU generation started with reminders off is `Reminders::NotPrepared`: the page says
"Restart to prepare them", and reminders count as off whatever the setting says until the next
start; turning them off takes effect at once. A failed or timed-out prime leaves the generation
Ready with reminders unavailable ("the reminder prompt could not be prepared"); after a timeout the
server is presumed busy for the bound (`--parallel 1` keeps evaluating), carried by the Ready event
into `busy_until` so the first admission counts it. After a prime the interpretation cache check is
armed, so the first interpretation verifies the interpretation entry's restore.

**Checks at run time.** After a reminder that reached the server (or the prime) the first
interpretation must report at least 75 % of the prefix read from the cache. A failed check (too
little, no figure, or that request expiring or failing, whatever the cause) switches reminders off
for the rest of the generation on both paths, with the reason, and a reminder already queued then
does not run; there is no re-prime fallback. A completed reminder with fewer than 75 % of the
reminder system tokens cached (or no figure) switches reminders off the same way without clearing
an armed interpretation check; when both fail the first reason is kept. The cached-token figure is
`Completed::cached_tokens`, the stream's usage, which llama.cpp fills from the same count as
`timings.cache_n`. `/tokenize` of the reminder prompt and the prime are the last steps of a
start's live progress (outside the breaker). Admission gives each scribble reading an allowance
of Q, or of Q and the reminder busy term counted at that moment (a reminder running, or its dropped
prompt still presumed busy) when reminders are effective; the job keeps its allowance and expires
against it, and the first job queued behind a reminder's busy time does not count toward D.
Scheduling is in
[assisted-entry.md](assisted-entry.md#reminders).

## Owner model setting

The owner picks the model on `/admin/assistant` (ASSIST-28, ASSIST-36); the start flags only
name the first one. The field takes one spec, at most 512 bytes of ASCII without white space
(`parse_spec` in `src/assistant/model.rs`; a refused spec names the broken rule and the field
keeps the input):

- `local:<name>.gguf`: one name of 1 to 128 characters from `[A-Za-z0-9._-]`, not starting
  with `.`, ending `.gguf`, not a Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1` to
  `COM9`, `LPT1` to `LPT9`, in any case, with or without an extension). A `/` or `\` in the
  name is its own refusal. The file is one already in the models folder.
- `hf:<org>/<repo>/<path>@<revision>`: a file to review and download first
  ([model-downloads.md](model-downloads.md)); a downloaded file is tried and stored as
  `local:hf-<sha256>.gguf`. The `--assistant-model` flag takes `local:` only.

**Trial.** "Try" (POST `/admin/assistant/model/try`) parses the spec, then the manager ends the
running generation (players see "starting"; there is never a second model server) and:
hashes the file (the SHA-256 it will be held to at every load, remembered by the file's
identity so the load does not read it again); starts the candidate on the path the runtime is
on (the GPU path unless it fell back or `--assistant-device cpu`; no fallback inside a trial)
at level 1; primes it; calibrates it (two 64-token samples) for the default reply length and
derives the budgets (the trial report shows the reply length the file will use: the length
stored for it, else the default); then sends five synthetic phrases shipped with Mimik (`trial.rs`; no
sheet or player text) as a job would be sent (the reply schema limits `ref` to `t` and the
phrase's context IDs), at the default reply length (1,024 tokens), each of which must be
schema-valid and untruncated. The largest `predicted_n` of the phrases is recorded as
information; the reply length of a file is the owner's to set (below), and a trial leaves it
as it is. It passes when the calibration and the phrases succeed ("too slow"
fails with the numbers). Whether a phrase's expected value was among the options is shown and
decides nothing (D-D). Bounds: each phrase 2 × R of a default-length reply on the candidate's
measured speed, at most the 600 s ceiling (phrases end on EOS), the whole trial including
hashing 90 minutes on either path; "Cancel"
stops it at any time, and so do a Restart and Mimik's shutdown. A failed, expired or cancelled
trial stops the candidate and the previous model starts again. A passed trial stops the
candidate too and the previous model starts again at once, so players are not left waiting for
the owner's decision: the page shows the result, with "Switch" and "Keep the current model"
(which only dismisses the result). One trial at a time (a second "Try" answers "a trial is
already running"); while it runs, Switch, Switch back and Keep are refused. Model changes
are admitted one at a time: a Try, Switch, Switch back or reply-length Set is refused ("a model
change or a restart is already being carried out") while another is pending or a Restart is
running, so a double click on Switch back swaps once. During Mimik's own first start or a wait
to retry, a Try or Set is admitted and interrupts that start. The page never disables these
buttons, nor Restart and "Try the GPU again": a refused press is answered with the notice. "Cancel" carries the number of the trial the page
showed and is refused when another trial runs now. A file of 0 bytes or over 8 GiB (the
stored size's limit) fails the trial naming why, before anything starts. An interrupted or
expired trial stops its candidate, then settles the launcher's work; a hash in progress is
cancelled at once, so it never delays the previous model. A model change does not reset the
circuit breaker (only Restart and Mimik's start do). The start rejects an
`--assistant-model-alias` outside 1 to 128 characters of `[A-Za-z0-9._-]` (the stored
setting's limit).

**Switch.** "Switch" carries the passed trial's key (`<number>.<generation>.<sha256>`), accepted
only if it names the latest finished passed trial (another hash, an older trial or an older
generation is refused as stale and nothing changes). The manager stores the model first
(`assistant_models`: spec, file name, SHA-256, size, alias, the trial's
measurements as JSON, with the SHA-256 of the prompt prefix they ran under; `assistant_model_choice`: the active and the previous model, who and
when), then starts a session with it: restart, priming, calibration for the reply length
stored for its file (else the default; read again at the Switch, in case it was set since the
trial). "Switch back" uses the previous model as the manager holds it in memory, which a
reply-length change keeps in step (`set_active_length`). A store failure changes nothing and the page says why. A stored model whose trial recorded
another prompt prefix than the current one (or none, as before ASSIST-56) is shown with a
line saying its speeds were measured with an older prompt and that trying it
again measures it with the current one; nothing is blocked or re-tried by itself. "Switch back" swaps the
active and the previous model in the same way and restarts with it. A model that fails to
become ready after a switch can be switched away from the same way. All actions are
owner-only, CSRF-checked, strict forms (an extra, duplicate or missing key is a 422) and
bounded at 4 KiB.

**Reply length.** "Longest reply" (POST `/admin/assistant/model/length`, owner-only,
CSRF-checked, strict form) sets the reply length (`max_tokens`) of the active model's file,
256 to 2,560 tokens in steps of 128 (`model::reply_length`: `MIN`, `MAX`, `STEP` and the
default 1,024 are the one place the stored CHECK and the runtime read). The route only
parses the text as a number; the runtime manager alone validates it (out of bounds or off the
step is refused as invalid, the same value again changes nothing). The length is stored per
model file: one row of `assistant_reply_lengths` (SHA-256 of the file, length, who, when), a
table of its own that neither has rows of `assistant_models` nor touches the model choice.
A file without a row uses the default; every model's effective length (session start, Switch,
Switch back, the trial report, the device default, the start flags' model) is the length
stored for its SHA-256. Setting a length is one upsert of the file's row, stored before
anything stops (a store failure changes nothing, restarts nothing and the page says why); it
does not change the active or the previous model or the device default (the Qwen3.5-9B stays
the GPU default and Granite the CPU's; after a GPU failure Granite runs with Granite's own
length). Switching to another file and back keeps each file's length, and a new trial of a file
keeps the length stored for it (its phrases and calibration still run at the default). Then the
server restarts once, primes and calibrates for the new length; it does not run the five
phrases and consumes no trial number. It applies to both paths: a value tuned on the GPU may
be "too slow" on the CPU fallback (the page shows R for the path running). A first start that
fails its budgets because of the length (the estimate over the 600 s ceiling, or a reply too
large for the context) restores the file's earlier value (the previous row, or no row when
there was none; stored first) and starts again, with a note naming the measured reason and
without resetting the circuit breaker (only Restart and Mimik's start do). Any other outcome
follows its normal path and keeps the new setting: ready, a sandbox or other failure, a
changed model file, a calibration sample timeout (a 64-token sample is independent of the
length), a Restart or another request. A reply too large for the context is never a GPU
failure: the GPU is not slow or broken, the length and the file do not fit the context, so on
the GPU path it is held as unavailable with its message and the owner fixes the length, instead
of falling back to the CPU (where, for the device default, Granite would run with its own
length and hide the problem). Admitted one at a time with Try, Switch, Switch back and Restart ("a model change
or a restart is already being carried out"), refused while a trial runs. Migration 032 created
the table and rebuilt the model tables without the old `max_tokens` column. A stuck server is
still declared after two run-deadline expiries in a row, which now takes up to 2 × R
(ASSIST-L63).

**Changed file.** A load whose file no longer has the recorded SHA-256 is not retried: the
assistant is unavailable with "the model file changed", until the owner tries a model again,
switches back or restarts after restoring the file.

## Owner page

`/admin/assistant` (owner only): the state with its reason, the pinned build, path,
generation, failure count and last failure, the breaker, the active level with its wording,
notes, skipped levels with reasons, the device (the selected GPU's description, in-sandbox
name, PCI address, render node and memory; render nodes not used, with why; "CPU only" under
`--assistant-device cpu`; why the GPU is not in use), the budgets, and the priming and sample
measurements. Restart (POST, CSRF-checked) stops the server and starts again from level 1; "Try
the GPU again" (POST `/admin/assistant/try-gpu`, CSRF-checked) is shown while the GPU is wanted
and either the CPU path runs or the GPU device default shows a fallback note; a second press of
either while one runs starts nothing ("A restart is already running"). The Model section shows the active model (spec, SHA-256, longest reply, and
whether it comes from the stored setting or the start flags), the previous model with "Switch
back", the "Longest reply" field (the bounds, the running path, "a reply can take up to R s",
the measured time per token and the longest a player can wait, the queue wait plus R, and that setting it does not run the five phrases), the Model field with "Try" (the
refusal and the kept input beside it), and the latest trial:
its progress block and Cancel while it runs; when it passed, the device, times, speeds, the largest
phrase reply, B, N(B), R, D and Q and each phrase; the reason when it failed or was cancelled. With the
development endpoint the model cannot be changed and the page says so. The Downloads
section (review, progress, pinned defaults, downloaded models) is described in
[model-downloads.md](model-downloads.md).

### Live progress

The page updates itself (`assets/assistant-live.js`, `src/auth/assistant_live.rs`). One
route, `GET /admin/assistant/status.json` (owner only, `no-store` from the global header
layer, no CSRF because it changes nothing; polling does not extend the session's absolute
expiry), answers `{"available": false}` without an assistant runtime, otherwise `signature`,
`poll_ms`, `progress` (or null) and the downloads' fields (`review`, `download`, `rows`; see
[model-downloads.md](model-downloads.md)). The script decides nothing: it sets `textContent`,
`value` and `max` from what the server sends and never writes markup.

- **Busy and polling.** Busy is the model server starting, restarting, a model change
  pending, a trial running, or a review or download running. Busy: `poll_ms` 2,000 and
  `progress` set while the runtime is starting (a download alone has only its own bar);
  idle: 15,000, so a crash restart, a change from another tab or the breaker still show up
  and polling never stops. The script pauses while the tab is hidden, polls at once when it
  shows again, chains `setTimeout` (no pile-up behind a slow answer), stops on 401 or 403
  with "Signed out: reload to sign in", and has a "Stop updating" button (WCAG 2.2.2).
- **Steps.** `Phase` (`runtime/progress.rs`) is the one name of every step, used by the
  runtime state and by a running trial, so the label, the step number and the page agree.
  The manager chooses the list where the work begins:

  | Entry | Steps |
  |---|---|
  | Fresh session (Mimik's start, Restart, Try the GPU again, Switch, Switch back, a length change, the previous model after a trial, the CPU after a GPU failure, a reverted length) | stopping the model server (only when one ran), checking the host, starting the model server, verifying the model file, loading the model, priming, calibration sample 1, sample 2 |
  | Retry after a failed attempt or a crash restart (no preflight, no probe) | starting, verifying, loading, priming, samples 1 and 2; "attempt n of up to 4" only when counted failures exist (a level skipped after an uncounted sandbox failure, or a crash after the ready window, is unnumbered) |
  | Waiting to retry | its own step with the exact time left (`RETRY_AFTER`) |
  | Trial | waiting for the model server to stop, hashing the file, checking the host, starting, loading, priming, samples 1 and 2, phrases 1 to 5 |

  The GPU fallback is labelled "on the CPU after a GPU failure", a reverted length "with
  the earlier length". The bar's basis is the step count of the list, fixed for the run;
  its value is the steps finished, so it is full only when the work is. A session sets its
  list before it waits for earlier work to settle: with the stop step when a model server
  may have run (a ready or starting one stopped for the owner, or a trial that just ended),
  without it otherwise, so no earlier step stays up during the stop. A trial always starts
  its own progress, keeping the start time of the one its admission set.
  The launcher reports the model file's verification (`StartEvent`, `Launcher::observe`);
  `files::hash` reports bytes every MiB. A remembered hash reports "verified (unchanged)".
  The probes of the host hash a first-seen file before the start, so bytes show there
  too, as text in the checking step.
- **Time.** "At most X left in this step" comes from the limit the code enforces: `/health`
  60 s plus what `loaded()` allows (5 s for the readers, and 5 s more for the offload line on the GPU path), priming
  300 s, each calibration sample 540 s (its timeout, and its `/tokenize` call sits outside
  that), each trial phrase `phrase_bound` once calibrated, the whole trial
  `trial_bound()`; waiting to retry is exact; hashing adds "about Y left at this rate" from
  the bytes read since the step's first report, over the file size. "About Y left (last time)" appears for a step measured in
  the last successful run of the same file, path and entry kind in this process (memory
  only; a failed attempt's partial times are never kept). Past it the page says "taking
  longer than last time (N s)", never a negative time. Phrases end on EOS, so they get only
  the bound.
- **Signature and reloads.** One function over one status snapshot (`signature`) names what
  changes the page: the state kind (not the step), the unavailable reason, the active and
  previous model with lengths, the note, the trial's number and outcome, restarting and
  pending, whether budgets exist, the downloads' state, and the generation only while idle.
  It is sent as the lowercase hex SHA-256 of that text (a reason holding a line break or a
  NUL would not survive an HTML attribute unchanged). The page's `data-signature` and the
  JSON carry the same string. On a change the script
  navigates with `location.replace("/admin/assistant")` (no form re-submission, no stale
  notice), at most once per 5 s (`sessionStorage`, in try/catch); while a form field has
  focus or a changed value it shows "The state changed: reload" instead.
- **Page.** The progress block (the step text in an `aria-live="polite"` element changed
  only when the step changes, elapsed and time left in a separate non-live line, and a
  `<progress>` named by the step text) is in the State section, or in the running trial's
  section in place of a status line. Without JavaScript the page is rendered at the current
  step and says to reload it.
- **Live check** (not in CI): `tools/assist_progress_browser.py` runs a release build with a
  real model server natively and Chromium in the browser container ([results.md](results.md)).

## Development endpoint

Builds with the `dev-endpoint` feature (never a release) keep `--assistant-dev-endpoint` and
`--assistant-dev-token-file` for tests: a hand-started server (or a fake) as the manager's
only child, primed and calibrated the same way, without an endpoint check (the page says so).
