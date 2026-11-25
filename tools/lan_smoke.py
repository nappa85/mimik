"""Fail-closed isolated Docker LAN/proxy gate for unpacked runtime and delivered helpers."""

import argparse
import os
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

from smoke_release import require, require_secret_free


def docker(*arguments, check=True, timeout=900):
    result = subprocess.run(
        ["docker", *arguments], capture_output=True, text=True, timeout=timeout, check=False
    )
    if check:
        require(result.returncode == 0, "Docker LAN gate operation failed")
    return result


def cpu_cap(value):
    """A container CPU cap: a whole number of CPUs from 1 to 64. The machine schedule allows
    two cores on weekdays 09:00-18:00, hence the default of 2."""
    if not (value.isascii() and value.isdigit()) or not 1 <= int(value) <= 64:
        raise argparse.ArgumentTypeError("--cpus must be a whole number from 1 to 64")
    return str(int(value))


PROXY_IMAGE = (
    "nginxinc/nginx-unprivileged:1.28-alpine"
    "@sha256:7377697a821c131a924a7105fafbe7414db4e9fcc77a6f08f776f33f141ec3f8"
)


def proxy_config(upstream):
    """The maintained reverse proxy (nginx) in front of the server, as an operator would run it.

    Response buffering is left at nginx's default (on) and compression at its default (off): the
    stream must be flushed because of its own `X-Accel-Buffering: no`, not because the proxy was
    tuned for it. Port 3443 is the configured root origin with a 60 s read timeout (above the
    15 s heartbeat). Port 3444 answers for the same origin but cuts an upstream that stays silent
    for 3 s, standing for a proxy or a sleeping link closing an idle stream. Port 3445 has a 20 s
    read timeout, above the 15 s heartbeat but below the time a held stream is watched. All pass wrong
    forwarding data (loopback, plain HTTP) that the server must ignore."""

    def listener(port, host, timeout):
        return f"""
    server {{
        listen {port} ssl;
        ssl_certificate /tls/fixture.crt;
        ssl_certificate_key /tls/fixture.key;
        location / {{
            proxy_pass http://{upstream}:3000;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_set_header Host {host};
            proxy_set_header X-Forwarded-For 127.0.0.1;
            proxy_set_header X-Forwarded-Proto http;
            proxy_set_header Forwarded "for=127.0.0.1;proto=http";
            proxy_read_timeout {timeout};
        }}
    }}"""

    return f"""worker_processes 1;
error_log /dev/stderr warn;
pid /tmp/nginx.pid;
events {{ worker_connections 256; }}
http {{
    access_log off;
    client_body_temp_path /tmp/client;
    proxy_temp_path /tmp/proxy;
    fastcgi_temp_path /tmp/fastcgi;
    uwsgi_temp_path /tmp/uwsgi;
    scgi_temp_path /tmp/scgi;{listener(3443, "$http_host", "60s")}{listener(3444, "mimik.example:3443", "3s")}{listener(3445, "mimik.example:3443", "20s")}
}}
"""


def network_address(container, network):
    value = docker(
        "inspect",
        "--format",
        '{{(index .NetworkSettings.Networks "' + network + '").IPAddress}}',
        container,
    ).stdout.strip()
    require(value != "", "LAN fixture lacks an interface address")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--source-fixture", required=True, type=Path)
    parser.add_argument("--ci-image", default="mimik-ci")
    parser.add_argument("--browser-image", default="mimik-browser")
    parser.add_argument("--proxy-image", default=PROXY_IMAGE)
    parser.add_argument(
        "--cpus",
        type=cpu_cap,
        default="2",
        help="CPU cap of every container, and the decoder's CARGO_BUILD_JOBS (default 2)",
    )
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    source = args.source_root.resolve(strict=True)
    fixture = args.source_fixture.resolve(strict=True)
    require(
        not (source / "data").exists()
        and not (source / ".env").exists()
        and (source / "vendored-crates").is_dir(),
        "Use unpacked allowlisted corresponding source/vendored-crates, not an owner workspace",
    )
    tag = "mimik-lan-" + uuid.uuid4().hex[:12]
    primary, secondary, server = tag + "-primary", tag + "-secondary", tag + "-server"
    proxy = tag + "-proxy"
    created = []
    containers = []
    with tempfile.TemporaryDirectory(prefix="mimik-lan-verification-") as temp:
        verification = Path(temp)
        try:
            for name in (primary, secondary):
                created.append(name)
                docker("network", "create", "--internal", name)
            containers.append(server)
            docker(
                "run",
                "-d",
                "--name",
                server,
                "--network",
                primary,
                f"--cpus={args.cpus}",
                "--memory=1g",
                "--pids-limit=256",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--mount",
                f"type=bind,src={binary},dst=/app/mimik,readonly",
                "--mount",
                f"type=bind,src={source / 'tools'},dst=/checks,readonly",
                "--mount",
                f"type=bind,src={fixture},dst=/fixture,readonly",
                args.ci_image,
                "python",
                "-B",
                "/checks/lan_server.py",
                "/app/mimik",
                "--source-fixture",
                "/fixture",
            )
            address = network_address(server, primary)
            for _ in range(120):
                result = docker(
                    "exec",
                    server,
                    "python",
                    "-c",
                    'import urllib.request; print(urllib.request.urlopen("http://127.0.0.1:3001/status", timeout=1).status)',
                    check=False,
                )
                if result.returncode == 0 and result.stdout.strip() == "200":
                    break
                require(
                    docker("inspect", "--format", "{{.State.Running}}", server).stdout.strip()
                    == "true",
                    "LAN fixture stopped before readiness",
                )
                time.sleep(0.25)
            else:
                raise AssertionError("LAN fixture readiness timed out")
            docker("network", "connect", secondary, server)
            other = network_address(server, secondary)
            mounts = ["--mount", f"type=bind,src={source / 'tools'},dst=/checks,readonly"]
            # The reverse proxy: a self-signed certificate for the configured origin, made in a
            # throwaway container, and nginx on the same network as the server.
            tls = verification / "tls"
            tls.mkdir()
            owner = f"{os.getuid()}:{os.getgid()}"
            (tls / "nginx.conf").write_text(proxy_config(address))
            containers.append(tag + "-tls")
            docker(
                "run",
                "--name",
                tag + "-tls",
                "--user",
                owner,
                "--network=none",
                f"--cpus={args.cpus}",
                "--mount",
                f"type=bind,src={tls},dst=/tls",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--read-only",
                args.ci_image,
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "/tls/fixture.key",
                "-out",
                "/tls/fixture.crt",
                "-days",
                "1",
                "-subj",
                "/CN=mimik.example",
                "-addext",
                "subjectAltName=DNS:mimik.example",
            )
            containers.append(proxy)
            docker(
                "run",
                "-d",
                "--name",
                proxy,
                "--user",
                owner,
                "--network",
                primary,
                f"--cpus={args.cpus}",
                "--memory=256m",
                "--pids-limit=64",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--read-only",
                "--tmpfs",
                "/tmp",
                "--mount",
                f"type=bind,src={tls / 'nginx.conf'},dst=/etc/nginx/nginx.conf,readonly",
                "--mount",
                f"type=bind,src={tls},dst=/tls,readonly",
                args.proxy_image,
            )
            proxy_address = network_address(proxy, primary)
            for port in (3443, 3444, 3445):
                for _ in range(80):
                    ready = docker(
                        "exec",
                        server,
                        "python",
                        "-c",
                        "import socket,sys; "
                        f"socket.create_connection(({proxy_address!r}, {port}), timeout=1).close()",
                        check=False,
                    )
                    if ready.returncode == 0:
                        break
                    time.sleep(0.25)
                else:
                    logs = docker("logs", proxy, check=False)
                    require_secret_free((logs.stdout + logs.stderr).encode(), ())
                    print((logs.stdout + logs.stderr).strip())
                    raise AssertionError(f"Reverse proxy port {port} did not open")
            # Loopback: the same stream and probe checks from inside the server's own container.
            loopback = docker(
                "exec",
                server,
                "python",
                "-B",
                "/checks/lan_client.py",
                "--address",
                "127.0.0.1",
                "--origin",
                "http://127.0.0.1:3000",
                "--streams-only",
                check=False,
            )
            if loopback.returncode:
                require_secret_free((loopback.stdout + loopback.stderr).encode(), ())
                print((loopback.stdout + loopback.stderr).strip())
            require(loopback.returncode == 0, "Loopback Master stream gate failed")
            print(loopback.stdout.strip())
            # Different subnet, allowed advertised Host, spoofed forwarding: real peer must win.
            containers.append(tag + "-denier")
            denied = docker(
                "run",
                "--name",
                tag + "-denier",
                "--network",
                secondary,
                f"--cpus={args.cpus}",
                *mounts,
                args.ci_image,
                "python",
                "-B",
                "/checks/lan_client.py",
                "--address",
                other,
                "--origin",
                f"http://{address}:3000",
                "--denial-only",
            )
            print(denied.stdout.strip())
            containers.append(tag + "-client")
            client = docker(
                "run",
                "--name",
                tag + "-client",
                "--user",
                f"{os.getuid()}:{os.getgid()}",
                "-e",
                "HOME=/tmp",
                "--network",
                primary,
                f"--cpus={args.cpus}",
                "--memory=4g",
                "--shm-size=1g",
                "--pids-limit=1024",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                *mounts,
                "--mount",
                f"type=bind,src={verification},dst=/verification",
                "--entrypoint",
                "python",
                args.browser_image,
                "/checks/lan_client.py",
                "--address",
                address,
                "--origin",
                f"http://{address}:3000",
                "--proxy-address",
                proxy_address,
                "--verification",
                "/verification",
                check=False,
            )
            if client.returncode:
                require_secret_free((client.stdout + client.stderr).encode(), ())
                print((client.stdout + client.stderr).strip())
            require(client.returncode == 0, "Separate-client LAN/proxy browser gate failed")
            print(client.stdout.strip())
            containers.append(tag + "-decoder")
            decoder = docker(
                "run",
                "--name",
                tag + "-decoder",
                "--network=none",
                f"--cpus={args.cpus}",
                "-e",
                f"CARGO_BUILD_JOBS={args.cpus}",
                "--mount",
                f"type=bind,src={source},dst=/workspace",
                "--mount",
                f"type=bind,src={verification},dst=/verification,readonly",
                "-e",
                "CARGO_HOME=/tmp/empty-lan-cargo",
                args.ci_image,
                "cargo",
                "run",
                "--offline",
                "--locked",
                "--example",
                "qr_decode",
                "--",
                "/verification",
                check=False,
            )
            require_secret_free((decoder.stdout + decoder.stderr).encode(), ())
            require(decoder.returncode == 0, "Independent rendered QR decoding failed")
            print(decoder.stdout.strip())
            logs = docker("logs", server)
            require_secret_free(
                (logs.stdout + logs.stderr).encode(),
                (b"Synthetic initial password 2026!", b"abcdefghijkl", b"mnopqrstuvwx"),
            )
            print("Unpacked isolated LAN/proxy gate passed; no physical-phone/router test claimed.")
        finally:
            failed = False
            for container in reversed(containers):
                try:
                    result = docker("rm", "-f", container, check=False, timeout=30)
                    if result.returncode and "no such container" not in result.stderr.lower():
                        failed = True
                except Exception:
                    failed = True
            for network in reversed(created):
                try:
                    result = docker("network", "rm", network, check=False, timeout=30)
                    if result.returncode and "no such network" not in result.stderr.lower():
                        failed = True
                except Exception:
                    failed = True
            require(not failed, "LAN gate resource cleanup failed")


if __name__ == "__main__":
    main()
