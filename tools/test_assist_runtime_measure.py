import os
import socket
import stat
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

import assist_runtime_measure as measure
import toolloop_ops as ops

MANUAL = {"pwm1_enable": 1, "pwm1": 255, "fan1_input": 3200, "path": "/sys/class/hwmon/hwmon4"}
AUTO = dict(MANUAL, pwm1_enable=2, pwm1=60)


def sensors(junction):
    return {
        ops.GPU_EDGE: {"value": 50.0, "crit": 100.0},
        ops.GPU_JUNCTION: {"value": junction, "crit": 110.0},
    }


class StreamServer:
    """A loopback server that answers one request with `chunks` server-sent events, one per
    `every` seconds (a streamed completion that never goes quiet)."""

    def __init__(self, chunks, every):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.port = self.listener.getsockname()[1]
        self.chunks, self.every = chunks, every
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        connection, _ = self.listener.accept()
        try:
            connection.recv(65536)
            connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
            for _ in range(self.chunks):
                connection.sendall(b'data: {"choices":[{"delta":{"content":"x"}}]}\n\n')
                time.sleep(self.every)
            connection.sendall(b"data: [DONE]\n\n")
        except OSError:
            pass
        finally:
            connection.close()
            self.listener.close()


def streaming(port):
    run = measure.Server.__new__(measure.Server)
    run.port, run.key = port, "k"
    return run


class CancelTests(unittest.TestCase):
    def test_frequent_chunks_cannot_postpone_the_cancel_deadline(self):
        server = StreamServer(chunks=300, every=0.02)  # 6 s of steady chunks
        began = time.monotonic()
        result = streaming(server.port).stream_first_token("u", 8, drop_after=0.3)
        self.assertLess(time.monotonic() - began, 1.5)
        self.assertTrue(result.cancelled)
        self.assertGreater(result.tokens, 0)  # tokens had arrived: not a mid-prompt cancel

    def test_a_stream_that_ends_first_is_not_a_cancel(self):
        server = StreamServer(chunks=3, every=0.01)
        result = streaming(server.port).stream_first_token("u", 8, drop_after=5)
        self.assertFalse(result.cancelled)
        self.assertEqual(result.tokens, 3)

    def test_a_token_count_drops_after_that_many_tokens(self):
        server = StreamServer(chunks=50, every=0.01)
        result = streaming(server.port).stream_first_token("u", 8, drop_at_token=5)
        self.assertTrue(result.cancelled)
        self.assertEqual(result.tokens, 5)


def args_for(folder, gpu="Vulkan0"):
    runtime = Path(folder) / "runtime"
    (runtime / "vulkan").mkdir(parents=True)
    server = runtime / "vulkan" / "llama-server"
    server.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(60)\n")
    server.chmod(server.stat().st_mode | stat.S_IXUSR)
    return mock.Mock(
        runtime=runtime,
        models_dir=Path(folder),
        model="m.gguf",
        threads=2,
        gpu_device=gpu,
        log=None,
        cache_ram=0,
    )


class GpuGuardTests(unittest.TestCase):
    def patched(self, folder, fan, readings):
        calls = iter(readings)
        last = readings[-1]
        return (
            mock.patch.object(ops, "heavy_allowed", return_value=True),
            mock.patch.object(ops, "hwmon_root", return_value=Path(folder)),
            mock.patch.object(ops, "read_fan", return_value=fan),
            mock.patch.object(ops, "read_sensors", side_effect=lambda root: next(calls, last)),
        )

    def test_an_automatic_fan_is_refused_before_the_spawn(self):
        with tempfile.TemporaryDirectory() as folder, mock.patch("subprocess.Popen") as popen:
            patches = self.patched(folder, AUTO, [sensors(60.0)])
            with patches[0], patches[1], patches[2], patches[3]:
                with self.assertRaises(measure.RefusedError):
                    measure.Server(args_for(folder))
            popen.assert_not_called()

    def test_a_hot_junction_stops_the_child_during_the_health_wait(self):
        with tempfile.TemporaryDirectory() as folder:
            spawned = []
            real = measure.subprocess.Popen

            def spy(*args, **kwargs):
                process = real(*args, **kwargs)
                spawned.append(process)
                return process

            patches = self.patched(folder, MANUAL, [sensors(60.0), sensors(90.0)])
            with patches[0], patches[1], patches[2], patches[3]:
                with mock.patch.object(measure.subprocess, "Popen", spy):
                    with self.assertRaisesRegex(RuntimeError, "thermal"):
                        measure.Server(args_for(folder))
            self.assertIsNotNone(spawned[0].poll())  # killed and reaped

    def test_an_interrupt_while_waiting_kills_the_child(self):
        with tempfile.TemporaryDirectory() as folder:
            spawned = []
            real = measure.subprocess.Popen

            def spy(*args, **kwargs):
                process = real(*args, **kwargs)
                spawned.append(process)
                return process

            patches = self.patched(folder, MANUAL, [sensors(60.0)])
            with patches[0], patches[1], patches[2], patches[3]:
                with (
                    mock.patch.object(measure.subprocess, "Popen", spy),
                    mock.patch.object(
                        measure.Server, "wait_healthy", side_effect=KeyboardInterrupt
                    ),
                ):
                    with self.assertRaises(KeyboardInterrupt):
                        measure.Server(args_for(folder))
            self.assertIsNotNone(spawned[0].poll())
            with self.assertRaises(ProcessLookupError):
                os.killpg(spawned[0].pid, 0)


class CacheMeasureTests(unittest.TestCase):
    def test_reuse_fraction_and_the_75_percent_rule(self):
        self.assertEqual(measure.reuse_fraction(0, 1946), 0.0)
        self.assertEqual(measure.reuse_fraction(3000, 1946), 1.0)
        self.assertEqual(measure.reuse_fraction(5, 0), 0.0)
        self.assertTrue(measure.passes(1460, 1946))
        self.assertFalse(measure.passes(1459, 1946))

    def test_stream_result_reads_first_token_and_last_timings(self):
        lines = [
            b'data: {"choices":[{"delta":{"role":"assistant"}}]}\n',
            b"\n",
            b'data: {"choices":[{"delta":{"content":""}}]}\n',
            b'data: {"choices":[{"delta":{"content":"{"}}]}\n',
            b'data: {"choices":[],"timings":{"prompt_n":5,"cache_n":1900}}\n',
            b"data: [DONE]\n",
        ]
        ticks = iter([2.5])
        first, timings = measure.stream_result(lines, 1.0, clock=lambda: next(ticks))
        self.assertEqual(first, 1.5)
        self.assertEqual(timings, {"prompt_n": 5, "cache_n": 1900})
        self.assertEqual(measure.stream_result([b"data: [DONE]\n"], 0.0), (None, None))

    def test_reminder_prompt_has_its_size_and_shares_no_prefix(self):
        system = measure.reminder_system(7000)
        self.assertEqual(len(system), 7000)
        self.assertNotEqual(system[:40], measure.PREFIX[:40])
        self.assertIn("<data>", measure.reminder_user(1))
        self.assertNotEqual(measure.reminder_user(1), measure.reminder_user(2))

    def test_the_window_allows_two_threads_only_in_the_weekday_day(self):
        from datetime import datetime

        monday_noon = datetime(2026, 11, 16, 12, 0)
        monday_night = datetime(2026, 11, 16, 3, 0)
        sunday_noon = datetime(2026, 11, 15, 12, 0)
        self.assertTrue(measure.window_allows(2, monday_noon))
        self.assertFalse(measure.window_allows(3, monday_noon))
        self.assertTrue(measure.window_allows(5, monday_night))
        self.assertTrue(measure.window_allows(5, sunday_noon))


if __name__ == "__main__":
    unittest.main()
