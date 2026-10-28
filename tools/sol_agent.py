"""Run Sol (openai/gpt-6.1-sol through opencode) as Mimik's implementer inside bubblewrap.

usage:
  python3 -I tools/sol_agent.py run <worktree> <task file> <state dir> [-- command...]
  python3 -I tools/sol_agent.py question <state dir>      print the pending question, if any
  python3 -I tools/sol_agent.py answer <state dir> <text>

`run` options come from the environment: CPUS (default 2; at most 2 on weekdays 09-18),
MODEL (default openai/gpt-6.1-sol), TIMEOUT seconds (default 7200), SOL_HOSTS (comma list
replacing the egress allowlist), SOL_LOG_HOSTS=1 (log every CONNECT host to stderr). A
command after `--` runs in the same sandbox instead of the agent, to check it.

Confinement:
- Files. Read-only: /usr, the system library and binary directories, a short list of /etc
  files, the Rust toolchain (~/.rustup, ~/.cargo/bin), the Cargo registry and the opencode
  login file. Read-write: the worktree (every `.env*` file in it is masked by an empty
  file), a fresh per-run HOME and mailbox, and <state dir>/target as CARGO_TARGET_DIR.
  Nothing else of the user's home, /media, /run or other worktrees. The main checkout is
  refused, and the worktree's .git file points outside the sandbox, so git does not work.
- Network. A new, empty network namespace. Inside, 127.0.0.1:3128 forwards to a Unix
  socket served by this process on the host, an HTTPS proxy that allows CONNECT only to
  the egress allowlist on port 443. Host loopback, the LAN and abstract sockets are not
  reachable. Cargo runs offline.
- Process. All other namespaces are new, nested user namespaces are disabled, the
  caller's environment is cleared, and tools/sol_seccomp.py's filter applies.
- CPU. Affinity and nice 10. A process inside could widen its own affinity: no cgroup
  is writable on this host, so the limit is cooperative.

Mailbox: the agent runs `ask "question"`. The coordinator reads it with `question` and
replies with `answer`. Both refuse symlinks and non-regular files, and bound the size.
Question text is untrusted data.
"""

import contextlib
import datetime as dt
import os
import selectors
import shutil
import signal
import socket
import stat
import subprocess
import sys
import threading
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
HOME = Path.home()
AUTH = HOME / ".local/share/opencode/auth.json"
# Observed 2026-10-28: with a ChatGPT login, opencode reaches the model only at chatgpt.com.
# auth.openai.com refreshes that login. The model list (models.opencode.ai) and npm are
# refused, and opencode works without them.
EGRESS = ("chatgpt.com", "auth.openai.com")
ETC = (
    "resolv.conf",
    "hosts",
    "nsswitch.conf",
    "passwd",
    "group",
    "ld.so.cache",
    "ld.so.conf",
    "localtime",
    "ssl",
    "ca-certificates",
)
MAX_QUESTION = 16 * 1024
PROXY_PORT = 3128

FORWARDER = (
    r"""
import os, socket, sys, threading
def pipe(a, b):
    try:
        while data := a.recv(65536):
            b.sendall(data)
    except OSError:
        pass
    finally:
        for s in (a, b):
            try: s.shutdown(socket.SHUT_RDWR)
            except OSError: pass
def serve(client):
    upstream = socket.socket(socket.AF_UNIX)
    try:
        upstream.connect("/proxy/sock")
    except OSError:
        client.close(); return
    threading.Thread(target=pipe, args=(client, upstream), daemon=True).start()
    pipe(upstream, client)
listener = socket.create_server(("127.0.0.1", %d))
if os.fork():
    listener.close()
    os.execvp(sys.argv[1], sys.argv[1:])
while True:
    client, _ = listener.accept()
    threading.Thread(target=serve, args=(client,), daemon=True).start()
"""
    % PROXY_PORT
)

ASK = r"""#!/bin/sh
# ask "question": wait for the coordinator's answer (up to an hour).
[ $# -ge 1 ] || { echo 'usage: ask "question"' >&2; exit 2; }
rm -f /mailbox/answer
printf '%s\n' "$*" > /mailbox/question.tmp && mv /mailbox/question.tmp /mailbox/question
i=0
while [ ! -f /mailbox/answer ]; do
    i=$((i + 1))
    [ $i -le 3600 ] || { echo "no answer within an hour; decide yourself and say so"; exit 1; }
    sleep 1
done
cat /mailbox/answer; rm -f /mailbox/answer /mailbox/question
"""


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(2)


def overlaps_weekday_hours(start, seconds):
    """Whether [start, start + seconds] touches Monday-Friday 09:00-18:00."""
    end = start + dt.timedelta(seconds=seconds)
    day = start.replace(hour=0, minute=0, second=0, microsecond=0)
    while day <= end:
        if day.weekday() < 5:
            opens, closes = day.replace(hour=9), day.replace(hour=18)
            if start < closes and end > opens:
                return True
        day += dt.timedelta(days=1)
    return False


def cpu_list(requested, seconds, now=None):
    """The CPUs to pin to: the first `requested` of the allowed set. A run that may last into
    weekday hours (09-18) gets at most 2, since the limit is set once, at launch."""
    now = now or dt.datetime.now()
    if requested < 1:
        fail("CPUS must be at least 1")
    if requested > 2 and overlaps_weekday_hours(now, seconds):
        fail("this run may reach weekday hours 09-18, which allow at most 2 cores (CPUS)")
    allowed = sorted(os.sched_getaffinity(0))
    if requested > len(allowed):
        fail(f"CPUS={requested} but only {len(allowed)} CPUs are available")
    return allowed[:requested]


def is_secret_name(name):
    return name in {".env", ".envrc"} or name.startswith(".env.")


def secret_files(worktree):
    """Every `.env`, `.envrc` or `.env.*` file anywhere under the worktree (all of it is
    exposed, so nothing is skipped; links are not followed)."""
    found = []
    for root, _dirs, files in os.walk(worktree):
        found += [Path(root, f) for f in files if is_secret_name(f)]
    return sorted(found)


def client_hello_sni(data):
    """The server name in a TLS ClientHello, or None when `data` is not a complete
    ClientHello with exactly one host_name entry."""
    try:
        if data[0] != 0x16 or data[5] != 0x01:
            return None
        record = int.from_bytes(data[3:5], "big")
        if len(data) < 5 + record:
            return None
        pos = 5 + 4 + 2 + 32  # record header, handshake header, version, random
        pos += 1 + data[pos]  # session id
        pos += 2 + int.from_bytes(data[pos : pos + 2], "big")  # cipher suites
        pos += 1 + data[pos]  # compression methods
        end = pos + 2 + int.from_bytes(data[pos : pos + 2], "big")
        pos += 2
        names = []
        while pos + 4 <= end:
            kind = int.from_bytes(data[pos : pos + 2], "big")
            size = int.from_bytes(data[pos + 2 : pos + 4], "big")
            body = data[pos + 4 : pos + 4 + size]
            if kind == 0:  # server_name
                inner = 2
                while inner + 3 <= len(body):
                    name_type = body[inner]
                    length = int.from_bytes(body[inner + 1 : inner + 3], "big")
                    if name_type == 0:
                        names.append(body[inner + 3 : inner + 3 + length].decode("ascii"))
                    inner += 3 + length
            pos += 4 + size
        return names[0].lower() if len(names) == 1 else None
    except (IndexError, UnicodeDecodeError):
        return None


class Proxy(threading.Thread):
    """HTTPS CONNECT proxy on a Unix socket: only allowlisted hosts on port 443, and the
    tunnel's TLS ClientHello must name the same host (no fronting another site through an
    allowed CDN address). Bounded: MAX_TUNNELS at once, a header deadline, an idle limit."""

    MAX_TUNNELS = 32
    HEADER_SECONDS = 10
    IDLE_SECONDS = 600
    MAX_HEADER = 8192

    def __init__(self, path, hosts, log_hosts):
        super().__init__(daemon=True)
        self.slots = threading.BoundedSemaphore(self.MAX_TUNNELS)
        self.hosts = {h.lower() for h in hosts}
        self.log_hosts = log_hosts
        self.server = socket.socket(socket.AF_UNIX)
        # A relative name keeps long state paths under the 108-byte socket path limit.
        cwd = os.getcwd()
        os.chdir(path.parent)
        try:
            self.server.bind(path.name)
        finally:
            os.chdir(cwd)
        self.server.listen(64)

    def run(self):
        while True:
            try:
                client, _ = self.server.accept()
            except OSError:
                return
            if not self.slots.acquire(blocking=False):
                client.close()
                continue
            try:
                threading.Thread(target=self.guarded, args=(client,), daemon=True).start()
            except RuntimeError:
                self.slots.release()
                client.close()

    def guarded(self, client):
        try:
            self.serve(client)
        except OSError:
            pass
        finally:
            self.slots.release()

    def allowed(self, target):
        host, sep, port = target.rpartition(":")
        return bool(sep) and port == "443" and host.lower() in self.hosts

    def serve(self, client):
        with contextlib.closing(client):
            client.settimeout(self.HEADER_SECONDS)
            head = b""
            while b"\r\n\r\n" not in head:
                if len(head) > self.MAX_HEADER:
                    client.sendall(b"HTTP/1.1 431 Request Header Fields Too Large\r\n\r\n")
                    return
                chunk = client.recv(4096)
                if not chunk:
                    return
                head += chunk
            head, rest = head.split(b"\r\n\r\n", 1)
            parts = head.split(b"\r\n", 1)[0].decode("latin-1").split(" ")
            if len(parts) != 3 or parts[0] != "CONNECT" or parts[2] not in {"HTTP/1.0", "HTTP/1.1"}:
                client.sendall(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
                return
            target = parts[1]
            if self.log_hosts:
                print(f"sol-proxy: CONNECT {target}", file=sys.stderr)
            if not self.allowed(target):
                print(f"sol-proxy: refused {target}", file=sys.stderr)
                client.sendall(b"HTTP/1.1 403 Forbidden\r\n\r\n")
                return
            host = target.rpartition(":")[0].lower()
            # The ClientHello comes after the 200, so it is read before any upstream byte.
            client.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
            hello = rest
            while client_hello_sni(hello) is None and len(hello) < 16384 + 5:
                chunk = client.recv(4096)
                if not chunk:
                    return
                hello += chunk
            if client_hello_sni(hello) != host:
                print(
                    f"sol-proxy: refused TLS name {client_hello_sni(hello)} in {target}",
                    file=sys.stderr,
                )
                return
            try:
                upstream = socket.create_connection((host, 443), timeout=30)
            except OSError:
                return
            with contextlib.closing(upstream):
                upstream.sendall(hello)
                relay(client, upstream, self.IDLE_SECONDS)


def relay(a, b, idle):
    """Copy both ways until either side closes or nothing moves for `idle` seconds."""
    a.settimeout(idle)
    b.settimeout(idle)
    with selectors.DefaultSelector() as sel:
        sel.register(a, selectors.EVENT_READ, b)
        sel.register(b, selectors.EVENT_READ, a)
        while True:
            ready = sel.select(timeout=idle)
            if not ready:
                return
            for key, _ in ready:
                data = key.fileobj.recv(65536)
                if not data:
                    return
                key.data.sendall(data)


def bwrap_args(worktree, run, state, cpus, masks):
    home = str(HOME)
    args = [
        "bwrap",
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--seccomp",
        "9",
        "--ro-bind",
        "/usr",
        "/usr",
    ]
    for top in ("/bin", "/sbin", "/lib", "/lib64"):
        args += ["--ro-bind", top, top]
    for name in ETC:
        if os.path.exists(f"/etc/{name}"):
            args += ["--ro-bind", f"/etc/{name}", f"/etc/{name}"]
    args += ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]
    args += ["--bind", str(run / "home"), home]
    args += ["--ro-bind", str(run / "opencode.json"), f"{home}/.config/opencode/opencode.json"]
    args += ["--ro-bind", str(AUTH), f"{home}/.local/share/opencode/auth.json"]
    for part in (".rustup", ".cargo/bin", ".cargo/registry"):
        args += ["--ro-bind", str(HOME / part), str(HOME / part)]
    args += ["--ro-bind", str(run / "ask"), "/opt/sol/ask"]
    args += ["--ro-bind", str(run / "forward.py"), "/opt/sol/forward.py"]
    args += ["--bind", str(run / "mailbox"), "/mailbox"]
    args += ["--bind", str(run / "proxy"), "/proxy"]
    args += ["--bind", str(state / "target"), "/target"]
    args += ["--bind", str(worktree), str(worktree)]
    for mask in masks:
        args += ["--ro-bind", str(run / "empty"), str(mask)]
    proxy = f"http://127.0.0.1:{PROXY_PORT}"
    env = {
        "HOME": home,
        "USER": os.environ.get("USER", "sol"),
        "LANG": "C.UTF-8",
        "TERM": "dumb",
        "PATH": f"/opt/sol:{home}/.cargo/bin:/usr/bin:/bin",
        "CARGO_TARGET_DIR": "/target",
        "CARGO_BUILD_JOBS": str(cpus),
        "CARGO_NET_OFFLINE": "true",
        "HTTPS_PROXY": proxy,
        "HTTP_PROXY": proxy,
        "https_proxy": proxy,
        "http_proxy": proxy,
    }
    args += ["--chdir", str(worktree), "--clearenv"]
    for key, value in env.items():
        args += ["--setenv", key, value]
    return args


def prepare(worktree, task, state):
    """A fresh run directory under `state`; launcher-owned files are written before the
    sandbox exists, so the agent never had a chance to plant links in it."""
    state.mkdir(parents=True, exist_ok=True)
    target = state / "target"
    target.mkdir(exist_ok=True)
    # The sandbox binds it read-write: it must be a real directory, not a link elsewhere.
    if target.is_symlink() or not target.is_dir() or target.resolve() != target:
        fail(f"{target} must be a plain directory")
    run = state / f"run-{dt.datetime.now():%Y%m%d-%H%M%S}-{os.getpid()}"
    run.mkdir()
    for sub in ("home/.config/opencode", "home/.local/share/opencode", "mailbox", "proxy"):
        (run / sub).mkdir(parents=True)
    (run / "home/.local/share/opencode/auth.json").touch()
    (run / "home/.config/opencode/opencode.json").touch()
    shutil.copyfile(TOOLS / "sol_opencode.json", run / "opencode.json")
    (run / "empty").touch(mode=0o444)
    (run / "ask").write_text(ASK)
    (run / "ask").chmod(0o755)
    (run / "forward.py").write_text(FORWARDER)
    subprocess.run(
        [sys.executable, "-I", str(TOOLS / "sol_seccomp.py"), str(run / "seccomp.bpf")],
        check=True,
    )
    (state / "latest").unlink(missing_ok=True)
    (state / "latest").symlink_to(run.name)
    return run


def run_agent(argv):
    if len(argv) < 3:
        fail(__doc__)
    worktree, task, state = Path(argv[0]).resolve(), Path(argv[1]).resolve(), Path(argv[2])
    command = argv[4:] if len(argv) > 3 and argv[3] == "--" else None
    if not (worktree / "Cargo.toml").is_file():
        fail(f"not a Mimik worktree: {worktree}")
    if not (worktree / ".git").is_file():
        fail("run it on a git worktree, not the main checkout")
    if not AUTH.is_file():
        fail("opencode is not logged in")
    if shutil.which("opencode") is None or shutil.which("bwrap") is None:
        fail("opencode and bwrap must be installed")
    model = os.environ.get("MODEL", "openai/gpt-6.1-sol")
    timeout = int(os.environ.get("TIMEOUT", "7200"))
    cpus = cpu_list(int(os.environ.get("CPUS", "2")), timeout)
    hosts = os.environ.get("SOL_HOSTS", ",".join(EGRESS)).split(",")
    state = state.resolve()
    if state.is_relative_to(worktree) or worktree.is_relative_to(state):
        fail("the state dir must be outside the worktree, and not contain it")
    run = prepare(worktree, task, state)
    proxy = Proxy(run / "proxy/sock", hosts, os.environ.get("SOL_LOG_HOSTS") == "1")
    proxy.start()
    if command is None:
        command = ["opencode", "run", "--dir", str(worktree), "-m", model, "--agent", "coder"]
        command.append(task.read_text())
    args = bwrap_args(worktree, run, state, len(cpus), secret_files(worktree))
    args += ["python3", "-I", "/opt/sol/forward.py", *command]
    with open(run / "seccomp.bpf", "rb") as bpf:
        os.dup2(bpf.fileno(), 9)
        os.set_inheritable(9, True)

        def confine():
            os.sched_setaffinity(0, cpus)
            os.nice(10)

        child = subprocess.Popen(args, pass_fds=(9,), preexec_fn=confine)
        try:
            return child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            child.send_signal(signal.SIGTERM)
            child.wait()
            print(f"sol-agent: stopped after {timeout} s", file=sys.stderr)
            return 124
        finally:
            os.close(9)


def mailbox(state):
    """The latest run's mailbox directory, opened without following links below `state`."""
    state_fd = os.open(state, os.O_RDONLY | os.O_DIRECTORY)
    try:
        latest = os.readlink("latest", dir_fd=state_fd)
        if "/" in latest or not latest.startswith("run-"):
            fail("latest run link is not a run directory")
        run_fd = os.open(latest, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=state_fd)
    finally:
        os.close(state_fd)
    try:
        return os.open("mailbox", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=run_fd)
    finally:
        os.close(run_fd)


def question(argv):
    if len(argv) != 1:
        fail(__doc__)
    box = mailbox(Path(argv[0]))
    try:
        try:
            fd = os.open("question", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=box)
        except FileNotFoundError:
            return 1
        with os.fdopen(fd, "rb") as file:
            if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
                fail("the question is not a regular file")
            text = file.read(MAX_QUESTION + 1)
        if len(text) > MAX_QUESTION:
            fail("the question is too long")
        print(text.decode("utf-8", "replace"), end="")
        return 0
    finally:
        os.close(box)


def answer(argv):
    if len(argv) != 2:
        fail(__doc__)
    box = mailbox(Path(argv[0]))
    try:
        name = f".answer-{os.getpid()}"
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644, dir_fd=box)
        with os.fdopen(fd, "w") as file:
            file.write(argv[1].rstrip("\n") + "\n")
        os.rename(name, "answer", src_dir_fd=box, dst_dir_fd=box)
        return 0
    finally:
        os.close(box)


def main():
    commands = {"run": run_agent, "question": question, "answer": answer}
    if len(sys.argv) < 2 or sys.argv[1] not in commands:
        fail(__doc__)
    return commands[sys.argv[1]](sys.argv[2:])


if __name__ == "__main__":
    sys.exit(main())
