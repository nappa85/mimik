"""LAN verification must clean every owned resource after timeout or failure."""

import argparse
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import lan_smoke


class CleanupTests(unittest.TestCase):
    def invoke_timeout(self, fail_network=False, decoder=False, cpus="2"):
        calls = []
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "mimik"
            binary.write_bytes(b"synthetic")
            source = root / "source"
            (source / "vendored-crates").mkdir(parents=True)
            fixture = root / "fixture"
            fixture.mkdir()
            args = argparse.Namespace(
                binary=binary,
                source_root=source,
                source_fixture=fixture,
                ci_image="ci",
                browser_image="browser",
                proxy_image="proxy@sha256:0",
                cpus=cpus,
            )

            def run(command, **kwargs):
                calls.append(command)
                if command[1] == "inspect":
                    text = "172.20.0.2" if "primary" in command[-2] else "172.21.0.2"
                elif command[1] == "exec":
                    text = "200"
                else:
                    text = "passed"
                if command[1] == "run" and (
                    (decoder and "qr_decode" in command)
                    or (
                        not decoder
                        and "/checks/lan_client.py" in command
                        and "--denial-only" not in command
                    )
                ):
                    raise subprocess.TimeoutExpired(command, 900)
                if (
                    command[1:3] == ["network", "rm"]
                    and fail_network
                    and command[-1].endswith("secondary")
                ):
                    return subprocess.CompletedProcess(command, 1, "", "synthetic removal failure")
                return subprocess.CompletedProcess(command, 0, text, "")

            with (
                patch("lan_smoke.argparse.ArgumentParser.parse_args", return_value=args),
                patch("lan_smoke.uuid.uuid4", return_value=SimpleNamespace(hex="0123456789abcdef")),
                patch("lan_smoke.subprocess.run", side_effect=run),
                patch("builtins.print"),
            ):
                with self.assertRaises((subprocess.TimeoutExpired, AssertionError)):
                    lan_smoke.main()
        return calls

    def test_browser_timeout_explicitly_removes_client(self):
        calls = self.invoke_timeout()
        self.assertTrue(
            any(
                command[1:3] == ["rm", "-f"] and "mimik-lan-0123456789ab-client" in command
                for command in calls
            )
        )

    def test_browser_uses_verification_directory_owner_without_widening_permissions(self):
        calls = self.invoke_timeout()
        browser = next(
            command
            for command in calls
            if command[1] == "run"
            and "/checks/lan_client.py" in command
            and "--denial-only" not in command
        )
        self.assertIn("--user", browser)
        self.assertEqual(browser[browser.index("--user") + 1], f"{os.getuid()}:{os.getgid()}")

    def test_decoder_timeout_explicitly_removes_decoder(self):
        calls = self.invoke_timeout(decoder=True)
        self.assertTrue(
            any(
                command[1:3] == ["rm", "-f"] and "mimik-lan-0123456789ab-decoder" in command
                for command in calls
            )
        )

    def test_every_container_is_cpu_capped(self):
        """Machine schedule: no container may run uncapped (two cores on weekdays)."""
        for cpus in ("2", "6"):
            calls = self.invoke_timeout(decoder=True, cpus=cpus)
            runs = [command for command in calls if command[1] == "run"]
            self.assertEqual(len(runs), 6, runs)
            for command in runs:
                self.assertEqual(
                    [part for part in command if part.startswith("--cpus")],
                    [f"--cpus={cpus}"],
                    command,
                )
            decoder = next(command for command in runs if "qr_decode" in command)
            self.assertIn(f"CARGO_BUILD_JOBS={cpus}", decoder)

    def test_proxy_container_is_confined_and_pinned(self):
        calls = self.invoke_timeout()
        proxy = next(c for c in calls if c[1] == "run" and "mimik-lan-0123456789ab-proxy" in c)
        for flag in ("--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges"):
            self.assertIn(flag, proxy)
        self.assertIn("@sha256:", proxy[-1])
        self.assertTrue(
            any(c[1:3] == ["rm", "-f"] and "mimik-lan-0123456789ab-proxy" in c for c in calls)
        )
        self.assertTrue(
            any(c[1:3] == ["rm", "-f"] and "mimik-lan-0123456789ab-tls" in c for c in calls)
        )

    def test_loopback_stream_checks_run_inside_the_server_container(self):
        calls = self.invoke_timeout()
        loopback = next(c for c in calls if c[1] == "exec" and "--streams-only" in c)
        self.assertIn("mimik-lan-0123456789ab-server", loopback)
        self.assertIn("127.0.0.1", loopback)

    def test_proxy_config_keeps_default_buffering_and_a_short_idle_listener(self):
        config = lan_smoke.proxy_config("172.20.0.2")
        self.assertNotIn("proxy_buffering", config)
        self.assertNotIn("gzip on", config)
        self.assertIn("proxy_pass http://172.20.0.2:3000;", config)
        self.assertIn("proxy_read_timeout 60s;", config)
        self.assertIn("proxy_read_timeout 3s;", config)
        self.assertIn("listen 3445 ssl;", config)
        self.assertIn("proxy_read_timeout 20s;", config)
        self.assertIn("proxy_set_header X-Forwarded-Proto http;", config)
        self.assertIn("proxy_set_header Host $http_host;", config)

    def test_cpu_cap_must_be_a_small_positive_integer(self):
        self.assertEqual(lan_smoke.cpu_cap("2"), "2")
        for bad in ("0", "-1", "1.5", "x", "", "65"):
            with self.assertRaises(argparse.ArgumentTypeError, msg=bad):
                lan_smoke.cpu_cap(bad)

    def test_one_network_failure_does_not_skip_other_cleanup(self):
        calls = self.invoke_timeout(fail_network=True)
        removed = [command[-1] for command in calls if command[1:3] == ["network", "rm"]]
        self.assertCountEqual(
            removed, ["mimik-lan-0123456789ab-secondary", "mimik-lan-0123456789ab-primary"]
        )


if __name__ == "__main__":
    unittest.main()
