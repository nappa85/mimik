import datetime as dt
import importlib.util
import os
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location("sol_agent", Path(__file__).with_name("sol_agent.py"))
agent = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(agent)

WEDNESDAY_NOON = dt.datetime(2026, 10, 28, 12, 0)
WEDNESDAY_NIGHT = dt.datetime(2026, 10, 28, 21, 0)


class Cpus(unittest.TestCase):
    def test_weekday_hours_allow_at_most_two_cores(self):
        with self.assertRaises(SystemExit):
            agent.cpu_list(3, 60, WEDNESDAY_NOON)
        self.assertEqual(len(agent.cpu_list(2, 60, WEDNESDAY_NOON)), 2)

    def test_evenings_allow_more_and_pick_from_the_allowed_set(self):
        allowed = sorted(os.sched_getaffinity(0))
        if len(allowed) < 3:
            self.skipTest("fewer than 3 CPUs")
        self.assertEqual(agent.cpu_list(3, 3600, WEDNESDAY_NIGHT), allowed[:3])

    def test_a_run_that_may_reach_weekday_hours_gets_the_weekday_cap(self):
        before_nine = dt.datetime(2026, 10, 28, 8, 59)
        with self.assertRaises(SystemExit):
            agent.cpu_list(3, 7200, before_nine)
        friday_night = dt.datetime(2026, 10, 30, 21, 0)
        self.assertTrue(agent.overlaps_weekday_hours(friday_night, 62 * 3600))  # into Monday
        self.assertFalse(agent.overlaps_weekday_hours(friday_night, 24 * 3600))


class Secrets(unittest.TestCase):
    def test_env_files_are_found_at_any_depth_but_not_in_build_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            names = (".env", "a/b/.env.local", "target/.env", "s/target/.env.prod", "a/.envrc")
            for name in (*names, "a/env.txt", "a/.environment"):
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text("DUMMY=1")
            found = [p.relative_to(root).as_posix() for p in agent.secret_files(root)]
            self.assertEqual(found, sorted(names))


def client_hello(server_name):
    """The first flight of a real TLS client naming `server_name`."""
    context = ssl.create_default_context()
    incoming, outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
    tls = context.wrap_bio(incoming, outgoing, server_hostname=server_name)
    try:
        tls.do_handshake()
    except ssl.SSLWantReadError:
        pass
    return outgoing.read()


class TlsName(unittest.TestCase):
    def test_the_server_name_is_read_from_a_real_client_hello(self):
        self.assertEqual(agent.client_hello_sni(client_hello("ChatGPT.com")), "chatgpt.com")

    def test_a_truncated_or_foreign_first_flight_has_no_name(self):
        hello = client_hello("chatgpt.com")
        self.assertIsNone(agent.client_hello_sni(hello[:40]))
        self.assertIsNone(agent.client_hello_sni(b"GET / HTTP/1.1\r\n\r\n"))


class Egress(unittest.TestCase):
    def test_only_allowlisted_hosts_on_port_443(self):
        with tempfile.TemporaryDirectory() as tmp:
            proxy = agent.Proxy(Path(tmp, "sock"), ["chatgpt.com"], False)
            self.assertTrue(proxy.allowed("chatgpt.com:443"))
            self.assertTrue(proxy.allowed("ChatGPT.com:443"))
            for target in ("chatgpt.com:80", "example.com:443", "chatgpt.com", "x.chatgpt.com:443"):
                self.assertFalse(proxy.allowed(target), target)
            proxy.server.close()

    def tunnel(self, request, *after):
        """Send `request` and the `after` chunks through a proxy allowing chatgpt.com; return
        the reply and the upstream addresses the proxy tried to reach."""
        dialled = []

        def fake_connect(address, timeout=None):
            dialled.append(address)
            raise OSError("no network in tests")

        with (
            tempfile.TemporaryDirectory() as tmp,
            mock.patch.object(agent.socket, "create_connection", fake_connect),
        ):
            proxy = agent.Proxy(Path(tmp, "sock"), ["chatgpt.com"], False)
            proxy.start()
            client = socket.socket(socket.AF_UNIX)
            client.settimeout(5)
            client.connect(str(Path(tmp, "sock")))
            client.sendall(request)
            reply = client.recv(64)
            for chunk in after:
                client.sendall(chunk)
            try:
                client.recv(64)
            except OSError:
                pass
            client.close()
            proxy.server.close()
        return reply, dialled

    def test_a_tunnel_whose_tls_name_differs_is_closed_before_any_upstream_connection(self):
        reply, dialled = self.tunnel(
            b"CONNECT chatgpt.com:443 HTTP/1.1\r\n\r\n", client_hello("attacker.example")
        )
        self.assertTrue(reply.startswith(b"HTTP/1.1 200"))
        self.assertEqual(dialled, [])

    def test_a_tunnel_naming_the_allowed_host_is_dialled(self):
        _, dialled = self.tunnel(
            b"CONNECT chatgpt.com:443 HTTP/1.1\r\n\r\n", client_hello("chatgpt.com")
        )
        self.assertEqual(dialled, [("chatgpt.com", 443)])

    def test_an_oversized_or_malformed_request_is_refused_without_dialling(self):
        reply, dialled = self.tunnel(b"CONNECT chatgpt.com:443 HTTP/1.1\r\n" + b"X" * 9000)
        self.assertTrue(reply.startswith(b"HTTP/1.1 431"))
        reply, _ = self.tunnel(b"CONNECT chatgpt.com:443 HTTP/9\r\n\r\n")
        self.assertTrue(reply.startswith(b"HTTP/1.1 405"))
        self.assertEqual(dialled, [])

    def test_connections_beyond_the_limit_are_dropped_and_the_proxy_keeps_serving(self):
        with tempfile.TemporaryDirectory() as tmp:
            proxy = agent.Proxy(Path(tmp, "sock"), ["chatgpt.com"], False)
            proxy.start()
            idle = []
            for _ in range(agent.Proxy.MAX_TUNNELS):
                sock = socket.socket(socket.AF_UNIX)
                sock.connect(str(Path(tmp, "sock")))
                idle.append(sock)
            extra = socket.socket(socket.AF_UNIX)
            extra.settimeout(5)
            extra.connect(str(Path(tmp, "sock")))
            self.assertEqual(extra.recv(64), b"")  # dropped
            for sock in idle:
                sock.close()
            threading.Event().wait(0.3)
            again = socket.socket(socket.AF_UNIX)
            again.settimeout(5)
            again.connect(str(Path(tmp, "sock")))
            again.sendall(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n")
            self.assertTrue(again.recv(64).startswith(b"HTTP/1.1 403"))
            proxy.server.close()

    def test_a_refused_host_gets_403_without_a_connection(self):
        with tempfile.TemporaryDirectory() as tmp:
            proxy = agent.Proxy(Path(tmp, "sock"), ["chatgpt.com"], False)
            proxy.start()
            client = socket.socket(socket.AF_UNIX)
            client.connect(str(Path(tmp, "sock")))
            client.sendall(b"CONNECT 127.0.0.1:22 HTTP/1.1\r\n\r\n")
            self.assertTrue(client.recv(64).startswith(b"HTTP/1.1 403"))
            client.close()
            proxy.server.close()


class Sandbox(unittest.TestCase):
    def test_the_sandbox_shares_nothing_and_binds_only_what_it_needs(self):
        run, state, tree = Path("/s/run-1"), Path("/s"), Path("/w/tree")
        args = agent.bwrap_args(tree, run, state, 2, [tree / ".env"])
        for flag in (
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--clearenv",
            "--new-session",
            "--die-with-parent",
        ):
            self.assertIn(flag, args)
        self.assertNotIn("--share-net", args)
        sources = {args[i + 1] for i, a in enumerate(args) if a in ("--bind", "--ro-bind")}
        home = str(agent.HOME)
        self.assertNotIn(home, sources)
        self.assertNotIn(home + "/.cargo", sources)
        writable = {args[i + 1] for i, a in enumerate(args) if a == "--bind"}
        self.assertEqual(
            writable,
            {f"{run}/home", f"{run}/mailbox", f"{run}/proxy", f"{state}/target", str(tree)},
        )
        mask = args.index(str(tree / ".env"))
        self.assertEqual(args[mask - 2 : mask], ["--ro-bind", f"{run}/empty"])
        self.assertGreater(
            mask, args.index(str(tree)), "the mask must come after the worktree bind"
        )


class StateDir(unittest.TestCase):
    def test_a_state_dir_inside_the_worktree_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp, "tree")
            tree.mkdir()
            (tree / "Cargo.toml").touch()
            (tree / ".git").write_text("gitdir: elsewhere\n")
            task = Path(tmp, "task.txt")
            task.write_text("t")
            with mock.patch.object(agent, "AUTH", task), self.assertRaises(SystemExit):
                agent.run_agent([str(tree), str(task), str(tree / ".sol-state")])

    def test_a_target_link_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp, "state")
            state.mkdir()
            (state / "target").symlink_to(tmp)
            with self.assertRaises(SystemExit):
                agent.prepare(Path(tmp), Path(tmp, "task"), state)


@unittest.skipUnless(shutil.which("bwrap") and shutil.which("opencode"), "needs bwrap and opencode")
class LiveSandbox(unittest.TestCase):
    """Runs the real launcher with a probe command: dummy secret, no model, no network."""

    def test_the_real_sandbox_holds(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp, "tree")
            (tree / "deep/target").mkdir(parents=True)
            (tree / "Cargo.toml").touch()
            (tree / ".git").write_text("gitdir: /nonexistent\n")
            (tree / "deep/target/.env").write_text("DUMMY_SECRET=not-real\n")
            auth = Path(tmp, "auth.json")
            auth.write_text("{}")
            task = Path(tmp, "task.txt")
            task.write_text("t")
            probe = (
                "import os, socket\n"
                "def link(f):\n"
                "    try:\n"
                "        return os.readlink('/proc/self/fd/' + f)\n"
                "    except OSError:\n"
                "        return None\n"
                "extra = [l for f in os.listdir('/proc/self/fd') if int(f) > 2\n"
                "         for l in [link(f)] if l and not l.startswith('/proc/')]\n"
                "print('fds', extra)\n"
                "print('secret', len(open('deep/target/.env').read()))\n"
                "s = socket.socket(); s.settimeout(2)\n"
                "print('loopback22', s.connect_ex(('127.0.0.1', 22)))\n"
                "import ctypes\n"
                "libc = ctypes.CDLL(None, use_errno=True)\n"
                "print('ptrace', libc.ptrace(0, 0, 0, 0), ctypes.get_errno())\n"
                "print('home', sorted(os.listdir(os.path.expanduser('~'))))\n"
            )
            env = dict(os.environ, CPUS="1", TIMEOUT="60")
            with mock.patch.dict(os.environ, env):
                code = (
                    "import sys, runpy; from pathlib import Path; "
                    f"m = runpy.run_path({str(Path(agent.__file__))!r}); "
                    f"m['AUTH'] = Path({str(auth)!r}); "
                    "sys.exit(m['run_agent'](sys.argv[1:]))"
                )
                out = subprocess.run(
                    [
                        sys.executable,
                        "-I",
                        "-c",
                        code,
                        str(tree),
                        str(task),
                        str(Path(tmp, "st")),
                        "--",
                        "python3",
                        "-I",
                        "-c",
                        probe,
                    ],
                    capture_output=True,
                    text=True,
                    timeout=120,
                    env=env,
                )
            lines = dict(line.split(" ", 1) for line in out.stdout.splitlines())
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertEqual(lines["secret"], "0")
            self.assertNotEqual(lines["loopback22"], "0")
            self.assertEqual(lines["ptrace"], "-1 1")
            self.assertEqual(lines["fds"], "[]")  # nothing inherited beyond stdio
            self.assertNotIn("Progetti", lines["home"])


class Mailbox(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.state = Path(self.tmp.name)
        (self.state / "run-1/mailbox").mkdir(parents=True)
        (self.state / "latest").symlink_to("run-1")
        self.box = self.state / "run-1/mailbox"
        self.outside = self.state / "outside.txt"
        self.outside.write_text("owner data\n")

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_question_is_read(self):
        (self.box / "question").write_text("which test?\n")
        self.assertEqual(agent.question([str(self.state)]), 0)

    def test_a_question_symlink_is_refused(self):
        (self.box / "question").symlink_to(self.outside)
        with self.assertRaises(OSError):
            agent.question([str(self.state)])

    def test_a_fifo_question_is_refused_without_blocking(self):
        os.mkfifo(self.box / "question")
        with self.assertRaises(SystemExit):
            agent.question([str(self.state)])

    def test_an_oversized_question_is_refused(self):
        (self.box / "question").write_text("x" * (agent.MAX_QUESTION + 1))
        with self.assertRaises(SystemExit):
            agent.question([str(self.state)])

    def test_an_answer_replaces_a_planted_symlink_without_writing_through_it(self):
        (self.box / "answer").symlink_to(self.outside)
        self.assertEqual(agent.answer([str(self.state), "use the CPU path"]), 0)
        self.assertEqual(self.outside.read_text(), "owner data\n")
        self.assertFalse((self.box / "answer").is_symlink())
        self.assertEqual((self.box / "answer").read_text(), "use the CPU path\n")

    def test_a_swapped_mailbox_directory_is_refused(self):
        (self.state / "run-1/mailbox").rename(self.state / "run-1/old")
        (self.state / "run-1/mailbox").symlink_to(self.state)
        with self.assertRaises(OSError):
            agent.answer([str(self.state), "x"])


if __name__ == "__main__":
    sys.exit(unittest.main())
