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
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    source = args.source_root.resolve(strict=True)
    fixture = args.source_fixture.resolve(strict=True)
    require(
        not (source / "data").exists()
        and not (source / ".env").exists()
        and (source / "vendor").is_dir(),
        "Use unpacked allowlisted corresponding source/vendor, not an owner workspace",
    )
    tag = "mimik-lan-" + uuid.uuid4().hex[:12]
    primary, secondary, server = tag + "-primary", tag + "-secondary", tag + "-server"
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
                "--cpus=2",
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
            # Different subnet, allowed advertised Host, spoofed forwarding: real peer must win.
            containers.append(tag + "-denier")
            denied = docker(
                "run",
                "--name",
                tag + "-denier",
                "--network",
                secondary,
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
                "--cpus=2",
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
