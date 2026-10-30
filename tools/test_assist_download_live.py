"""Offline tests; run with python3 -I tools/test_assist_download_live.py."""

import hashlib
import io
import json
import re
import sys
import tempfile
import unittest
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, Mock, patch

# -I deliberately excludes the script directory. Only import this checkout's tools.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import assist_download_live as live  # noqa: E402

CSRF = "c" * 64


def form(action, **fields):
    inputs = dict(csrf=CSRF, **fields)
    return (
        f'<form action="{live.PANEL}/{action}" method="post">'
        + "".join(f'<input name="{k}" value="{v}">' for k, v in inputs.items())
        + "<button>Submit</button></form>"
    )


class ParsingTests(unittest.TestCase):
    def test_forms_choose_exact_model_and_preserve_csrf_and_token(self):
        html = (
            form("model/review", model="hf:first")
            + form("model/review", model="hf:second&amp;encoded")
            + form("download/confirm", token="private-token")
            + form("model/try", model="")
            + form("model/try", model="local:chosen.gguf")
        )
        panel = live.Panel(html)
        self.assertEqual(
            panel.form(live.PANEL + "/model/review", model="hf:second&encoded"),
            {"csrf": CSRF, "model": "hf:second&encoded"},
        )
        self.assertEqual(panel.form(live.PANEL + "/download/confirm")["token"], "private-token")
        self.assertEqual(
            panel.form(live.PANEL + "/model/try", model="local:chosen.gguf")["model"],
            "local:chosen.gguf",
        )

    def test_ambiguous_missing_and_no_csrf_refused(self):
        for html in (
            "",
            form("download/resume", id="1") * 2,
            '<form action="/admin/assistant/download/resume"><input name="id" value="1"></form>',
        ):
            with self.subTest(html=html), self.assertRaises(AssertionError):
                live.Panel(html).form(live.PANEL + "/download/resume", id=1)

    def test_identical_duplicates_require_explicit_opt_in(self):
        panel = live.Panel(form("model/try", model="local:chosen.gguf") * 2)
        with self.assertRaisesRegex(AssertionError, "exactly one"):
            panel.form(live.PANEL + "/model/try", model="local:chosen.gguf")
        self.assertEqual(
            panel.form(live.PANEL + "/model/try", identical_ok=True, model="local:chosen.gguf"),
            {"csrf": CSRF, "model": "local:chosen.gguf"},
        )

    def test_ambiguous_duplicates_refused_even_with_opt_in(self):
        panel = live.Panel(
            form("model/try", model="local:chosen.gguf", token="first")
            + form("model/try", model="local:chosen.gguf", token="second")
        )
        for identical_ok in (False, True):
            with (
                self.subTest(identical_ok=identical_ok),
                self.assertRaisesRegex(AssertionError, "exactly one"),
            ):
                panel.form(
                    live.PANEL + "/model/try", identical_ok=identical_ok, model="local:chosen.gguf"
                )

    def test_review_visible_rows_not_hidden_inputs(self):
        panel = live.Panel(
            "<table><tr><th>Licence</th><td>apache-2.0 &amp; terms</td></tr>"
            "<tr><th>Revision</th><td><code>abc</code></td></tr></table>"
            + form("download/confirm", token="secret")
        )
        self.assertEqual(panel.rows, {"Licence": "apache-2.0 & terms", "Revision": "abc"})
        self.assertNotIn("secret", str(panel.rows))

    def test_trial_text_includes_timings_excludes_forms_and_other_sections(self):
        html = (
            "<section><h2>Model</h2><h3>Trial 1 of local:test.gguf: passed</h3>"
            "<p>Passed in 50 s. Priming 2 s.</p><table><tr><td>1</td><td>3 s</td></tr></table>"
            + form("model/switch", trial="hidden-key")
            + "</section><section><h2>Device</h2><p>unrelated</p></section>"
        )
        panel = live.Panel(html)
        text = " ".join(panel.trial)
        for expected in ("Trial 1", "Priming 2 s", "3 s"):
            self.assertIn(expected, text)
        for excluded in ("hidden-key", CSRF, "Submit", "unrelated"):
            self.assertNotIn(excluded, text)

    def test_real_rust_pins_and_different_numeric_values(self):
        source = (live.ROOT / "src/assistant/model.rs").read_text()
        for chosen, size_constant in (("granite", "GRANITE_SIZE"), ("qwen", "QWEN_SIZE")):
            pin = live.parse_pin(source, chosen)
            self.assertGreater(pin.size, 1 << 30)
            self.assertEqual(len(pin.sha256), 64)
            self.assertEqual(pin.local, "local:" + pin.filename)
            self.assertIn("/revision/" + pin.repo_revision[1] + "?blobs=true", pin.api_url)
            changed = re.sub(rf"({size_constant}: u64 = )[0-9_]+;", r"\g<1>123_456;", source)
            self.assertEqual(live.parse_pin(changed, chosen).size, 123456)

    def test_constants_missing_duplicate_or_invalid_fail_closed(self):
        source = (live.ROOT / "src/assistant/model.rs").read_text()
        for changed in (
            source.replace("GRANITE_SIZE", "MISSING_SIZE"),
            source + "\nconst GRANITE_SIZE: u64 = 4;",
            source.replace("local:granite-4.2-3b-Q4_K_M.gguf", "local:../bad.gguf"),
        ):
            with self.subTest(source=changed[-40:]), self.assertRaises(AssertionError):
                live.parse_pin(changed, "granite")


class HashTests(unittest.TestCase):
    def test_actual_file_multiple_chunks_and_tail(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "model"
            data = b"a" * live.HASH_CHUNK + b"tail"
            path.write_bytes(data)
            self.assertEqual(live.streaming_sha256(path), hashlib.sha256(data).hexdigest())

    def test_empty_and_read_bound(self):
        stream = Mock()
        stream.read.side_effect = [b"abc", b""]
        stream.__enter__ = Mock(return_value=stream)
        stream.__exit__ = Mock(return_value=False)
        path = Mock()
        path.open.return_value = stream
        self.assertEqual(live.streaming_sha256(path), hashlib.sha256(b"abc").hexdigest())
        self.assertEqual(stream.read.call_args_list, [((8 * 1024 * 1024,),), ((8 * 1024 * 1024,),)])
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "empty"
            path.touch()
            self.assertEqual(live.streaming_sha256(path), hashlib.sha256(b"").hexdigest())


class ResumeTests(unittest.TestCase):
    def test_kill_exact_threshold_once_while_receiving(self):
        status = {"phase": "running", "received": 100, "total": 1000}
        self.assertTrue(live.should_kill(status, 100, False))
        for threshold, killed in ((101, False), (None, False), (100, True)):
            self.assertFalse(live.should_kill(status, threshold, killed))
        for phase in ("verifying", "done", "idle", "failed"):
            self.assertFalse(live.should_kill(dict(status, phase=phase), 100, False))
        self.assertFalse(live.should_kill(dict(status, received=1000), 100, False))

    def test_rollback_is_rounded_down_then_one_chunk(self):
        self.assertEqual(live.resume_floor(10 * live.MIB + 123), 9 * live.MIB)
        self.assertEqual(live.resume_floor(live.MIB - 1), 0)
        live.check_resume_sample(9 * live.MIB, 10 * live.MIB + 123)
        with self.assertRaises(AssertionError):
            live.check_resume_sample(9 * live.MIB - 1, 10 * live.MIB + 123)
        with self.assertRaises(AssertionError):
            live.check_resume_sample(0, live.MIB)

    def test_machine_window(self):
        for hour in (9, 12, 17):
            now = datetime(2026, 10, 29, hour)
            self.assertFalse(live.trial_allowed("qwen", now))
            self.assertTrue(live.trial_allowed("granite", now))
        for now in (
            datetime(2026, 10, 29, 18),
            datetime(2026, 10, 29, 8),
            datetime(2026, 10, 31, 12),
        ):
            self.assertTrue(live.trial_allowed("qwen", now))

    def test_thread_cap_at_weekday_boundaries_and_weekend(self):
        for now, heavy_allowed in (
            (datetime(2026, 10, 29, 8, 59), True),
            (datetime(2026, 10, 29, 9), False),
            (datetime(2026, 10, 29, 17, 59), False),
            (datetime(2026, 10, 29, 18), True),
            (datetime(2026, 10, 31, 12), True),
        ):
            with self.subTest(now=now):
                self.assertTrue(live.threads_allowed(1, now))
                self.assertTrue(live.threads_allowed(2, now))
                self.assertEqual(live.threads_allowed(5, now), heavy_allowed)
                self.assertFalse(live.threads_allowed(0, now))
                self.assertFalse(live.threads_allowed(-1, now))


class FixtureTests(unittest.TestCase):
    def setUp(self):
        self.pin = live.parse_pin((live.ROOT / "src/assistant/model.rs").read_text(), "granite")
        self.value = {
            "sha": self.pin.repo_revision[1],
            "private": False,
            "gated": False,
            "cardData": {"license": "apache-2.0"},
            "siblings": [
                {
                    "rfilename": "model.gguf",
                    "lfs": {"sha256": self.pin.sha256, "size": self.pin.size},
                },
            ],
        }

    def opener(self, value=None, body=None):
        response = io.BytesIO(
            body if body is not None else json.dumps(value or self.value).encode()
        )
        response.status = 200
        response.headers = {"Set-Cookie": "must-not-be-written", "Content-Encoding": "identity"}
        opener = Mock()
        opener.open.return_value = response
        return opener

    def test_preserves_shape_public_values_and_uses_correct_url_timeout(self):
        opener = self.opener()
        with tempfile.TemporaryDirectory() as temp:
            path = live.write_fixture(self.pin, "granite", Path(temp), opener)
            self.assertEqual(path.name, "hf-model-info-granite.json")
            self.assertEqual(json.loads(path.read_text()), self.value)
            self.assertNotIn("must-not-be-written", path.read_text())
        args, kwargs = opener.open.call_args
        self.assertEqual(args[0].full_url, self.pin.api_url)
        self.assertEqual(kwargs, {"timeout": 20})
        self.assertNotIn("Cookie", args[0].headers)
        self.assertNotIn("Authorization", args[0].headers)

    def test_invalid_metadata_does_not_write(self):
        for body in (
            b"{",
            b"x" * (live.MIB + 1),
            b"[]",
            json.dumps(dict(self.value, private=True)).encode(),
            json.dumps(dict(self.value, sha="wrong")).encode(),
        ):
            with tempfile.TemporaryDirectory() as temp:
                with self.assertRaises((AssertionError, ValueError)):
                    live.write_fixture(self.pin, "granite", Path(temp), self.opener(body=body))
                self.assertEqual(list(Path(temp).iterdir()), [])

    def test_redirect_refused_before_request_to_other_host(self):
        with self.assertRaises(AssertionError):
            live.NoRedirect().redirect_request(
                None, None, 302, "Found", {}, "https://other.invalid/a?signature=secret"
            )


class OutputTests(unittest.TestCase):
    def test_queries_fragments_and_userinfo_removed(self):
        text = (
            "error https://user:password@cdn.example/a?X-Amz-Signature=secret&x=1 "
            "https://huggingface.co/file#token https://host/path"
        )
        self.assertEqual(
            live.redact_urls(text),
            "error https://cdn.example/a https://huggingface.co/file https://host/path",
        )
        with patch("builtins.print") as output:
            live.say(text)
        self.assertNotIn("secret", output.call_args.args[0])


class WorkflowTests(unittest.TestCase):
    def runner(self):
        pin = live.parse_pin((live.ROOT / "src/assistant/model.rs").read_text(), "granite")
        args = SimpleNamespace(
            timeout=10, kill_at_bytes=None, pinned="granite", threads=2, device="cpu"
        )
        return live.Live(args, pin, Path("unused"), b"dummy")

    def test_main_launch_options_and_models_permissions(self):
        for mode in (0o700, 0o755, 0o750, 0o770, 0o1700):
            with self.subTest(mode=oct(mode)), tempfile.TemporaryDirectory() as temp:
                models = Path(temp) / "models"
                models.mkdir(mode=0o700)
                models.chmod(mode)
                runtime = Path(temp) / "llama"
                runner = Mock()
                runner.launch.side_effect = AssertionError("stop after startup validation")
                with (
                    patch.object(live, "auth_request", side_effect=ConnectionRefusedError),
                    patch.object(live, "create_accounts"),
                    patch.object(live, "Live", return_value=runner) as factory,
                    patch.object(live, "say") as say,
                    patch.object(live, "datetime") as clock,
                ):
                    clock.now.return_value = datetime(2026, 10, 29, 20).astimezone()
                    self.assertEqual(
                        live.main(
                            [
                                __file__,
                                "--models",
                                str(models),
                                "--pinned",
                                "granite",
                                "--runtime",
                                str(runtime),
                                "--threads",
                                "5",
                            ]
                        ),
                        1,
                    )
                if mode == 0o700:
                    args = factory.call_args.args[0]
                    self.assertEqual(args.runtime, runtime)
                    self.assertEqual(args.threads, 5)
                    runner.launch.assert_called_once_with()
                else:
                    factory.assert_not_called()
                    self.assertIn("must have mode 0700", say.call_args.args[0])

    def test_main_refuses_invalid_or_weekday_excess_threads_before_launch(self):
        with tempfile.TemporaryDirectory() as temp:
            for threads in ("0", "-1", "3", "5"):
                with (
                    self.subTest(threads=threads),
                    patch.object(live, "datetime") as clock,
                    patch.object(live, "Live") as factory,
                    patch.object(live, "auth_request") as request,
                    patch.object(live, "say") as say,
                ):
                    clock.now.return_value = datetime(2026, 10, 29, 12).astimezone()
                    self.assertEqual(
                        live.main(
                            [
                                __file__,
                                "--models",
                                temp,
                                "--pinned",
                                "granite",
                                "--threads",
                                threads,
                            ]
                        ),
                        1,
                    )
                    factory.assert_not_called()
                    request.assert_not_called()
                    self.assertIn("threads", say.call_args.args[0])

    def test_launch_passes_runtime_and_threads_including_defaults(self):
        for runtime, threads in ((None, 2), (Path("verified/llama"), 5)):
            with self.subTest(runtime=runtime, threads=threads):
                runner = self.runner()
                runner.args.binary = Path("mimik")
                runner.args.models = Path("models")
                runner.args.runtime = runtime
                runner.args.threads = threads
                with (
                    patch.object(live, "threads_allowed", return_value=True),
                    patch.object(live, "start", return_value=(Mock(), Mock())) as start,
                    patch.object(
                        live, "auth_request", return_value=(303, {"location": "/auth/session"}, "")
                    ),
                    patch.object(live, "auth_cookie", return_value="cookie"),
                    patch.object(live, "auth_fields", return_value={}),
                ):
                    runner.launch()
                extra = start.call_args.kwargs["extra"]
                expected = ("--assistant", "--models-dir", "models")
                if runtime is not None:
                    expected += ("--assistant-runtime", str(runtime))
                expected += ("--assistant-device", "cpu", "--assistant-threads", str(threads))
                self.assertEqual(extra, expected)

    def test_gpu_device_is_passed_and_refused_in_weekday_hours(self):
        thursday = datetime(2026, 10, 29, 10, 0).astimezone()
        evening = datetime(2026, 10, 29, 19, 0).astimezone()
        self.assertFalse(live.trial_allowed("qwen", thursday))
        runner = self.runner()
        runner.args.binary = Path("mimik")
        runner.args.models = Path("models")
        runner.args.runtime = None
        runner.args.device = "auto"
        with patch.object(live, "trial_allowed", return_value=False):
            with patch.object(live, "start") as start:
                with self.assertRaisesRegex(AssertionError, "GPU"):
                    runner.launch()
                start.assert_not_called()
        self.assertTrue(live.trial_allowed("qwen", evening))
        with (
            patch.object(live, "trial_allowed", return_value=True),
            patch.object(live, "start", return_value=(Mock(), Mock())) as start,
            patch.object(
                live, "auth_request", return_value=(303, {"location": "/auth/session"}, "")
            ),
            patch.object(live, "auth_cookie", return_value="cookie"),
            patch.object(live, "auth_fields", return_value={}),
        ):
            runner.launch()
        extra = start.call_args.kwargs["extra"]
        self.assertEqual(extra[extra.index("--assistant-device") + 1], "auto")

    def test_trial_stops_when_thread_window_closes(self):
        runner = self.runner()
        runner.args.threads = 5
        runner.panel = Mock(return_value=live.Panel(form("model/try", model=runner.pin.local)))
        runner.post = Mock()
        with patch.object(live, "threads_allowed", side_effect=[True, False]):
            with self.assertRaisesRegex(AssertionError, "threads"):
                runner.trial()
        runner.post.assert_called_once()
        self.assertEqual(runner.panel.call_count, 1)

    def test_launch_and_trial_refuse_excess_threads_before_starting_work(self):
        for stage in ("launch", "trial"):
            with self.subTest(stage=stage):
                runner = self.runner()
                runner.args.threads = 5
                runner.post = Mock()
                with (
                    patch.object(live, "datetime") as clock,
                    patch.object(live, "start") as start,
                ):
                    clock.now.return_value = datetime(2026, 10, 29, 9).astimezone()
                    with self.assertRaisesRegex(AssertionError, "threads"):
                        getattr(runner, stage)()
                start.assert_not_called()
                runner.post.assert_not_called()

    def test_post_refusal_is_not_retried(self):
        runner = self.runner()
        panel = live.Panel(form("download/resume", id="7"))
        with patch.object(
            live, "auth_request", return_value=(303, {"location": "wrong"}, "")
        ) as request:
            with self.assertRaises(AssertionError):
                runner.post(panel, live.PANEL + "/download/resume", "download-resuming", id=7)
        self.assertEqual(request.call_count, 1)

    def test_poll_deadline_and_failed_transfer(self):
        runner = self.runner()
        runner.deadline = 0
        with self.assertRaises(AssertionError):
            runner.remaining()
        runner = self.runner()
        row = {
            "id": 7,
            "spec": runner.pin.hf,
            "file": runner.pin.filename,
            "size": runner.pin.size,
            "state": "confirmed",
        }
        runner.state = Mock(return_value={"rows": [row], "download": {"id": 7, "phase": "failed"}})
        with self.assertRaises(AssertionError):
            runner.transfer()
        self.assertEqual(runner.state.call_count, 1)

    def test_completion_without_requested_crash_fails(self):
        runner = self.runner()
        runner.args.kill_at_bytes = 2 * live.MIB
        row = {
            "id": 7,
            "spec": runner.pin.hf,
            "file": runner.pin.filename,
            "size": runner.pin.size,
            "state": "complete",
        }
        runner.state = Mock(return_value={"rows": [row], "download": {"phase": "done"}})
        with self.assertRaises(AssertionError):
            runner.transfer()

    def test_resume_requires_observed_retained_offset_then_new_progress(self):
        for observe_offset in (True, False):
            with self.subTest(observe_offset=observe_offset):
                runner = self.runner()
                runner.args.kill_at_bytes = 3 * live.MIB
                baseline = 4 * live.MIB
                row = {
                    "id": 7,
                    "spec": runner.pin.hf,
                    "file": runner.pin.filename,
                    "size": runner.pin.size,
                    "state": "confirmed",
                }

                def state(received, complete=False, row=row, pin=runner.pin):
                    return {
                        "rows": [dict(row, state="complete" if complete else "confirmed")],
                        "download": {
                            "id": 7,
                            "phase": "done" if complete else "running",
                            "received": received,
                            "total": pin.size,
                        },
                    }

                states = [state(baseline)]
                if observe_offset:
                    states.append(state(live.resume_floor(baseline)))
                states.extend([state(baseline + 100), state(0, complete=True)])
                runner.state = Mock(side_effect=states)
                runner.crash_resume = Mock(return_value=baseline)
                runner.args.models = MagicMock()
                partial = runner.args.models / ".partial" / runner.pin.sha256
                # Publication racing a disk probe is allowed; JSON must supply evidence.
                partial.stat.side_effect = FileNotFoundError
                with patch.object(live.time, "sleep"), patch.object(live, "say"):
                    if observe_offset:
                        runner.transfer()
                    else:
                        with self.assertRaisesRegex(AssertionError, "offset was not observed"):
                            runner.transfer()
                runner.crash_resume.assert_called_once_with(row)

    def test_close_uses_shared_stop_helper(self):
        runner = self.runner()
        runner.process, runner.log = Mock(), Mock()
        process, log = runner.process, runner.log
        with patch.object(live, "stop") as stop:
            runner.close()
            runner.close()
        stop.assert_called_once_with(process, log)

    def test_trial_posts_downloaded_file_and_reports_pass_without_switch(self):
        runner = self.runner()
        initial = live.Panel(
            form("model/try", model="") + form("model/try", model=runner.pin.local) * 2
        )
        running = live.Panel(f"<section><h3>Trial 1 of {runner.pin.local}: running</h3></section>")
        passed = live.Panel(
            f"<section><h3>Trial 1 of {runner.pin.local}: passed</h3>"
            "<p>Passed on CPU in 40 s. Priming 5 s.</p>"
            + form("model/switch", trial="not-to-be-posted")
            + "</section>"
        )
        runner.panel = Mock(side_effect=[initial, running, passed])
        with (
            patch.object(
                live,
                "auth_request",
                return_value=(303, {"location": live.PANEL + "?notice=trying-model"}, ""),
            ) as request,
            patch.object(live.time, "sleep"),
            patch.object(live, "say") as say,
        ):
            runner.trial()
        request.assert_called_once_with(
            live.PANEL + "/model/try", cookie=None, form={"csrf": CSRF, "model": runner.pin.local}
        )
        self.assertIn("Priming 5 s", say.call_args.args[0])
        self.assertNotIn("not-to-be-posted", say.call_args.args[0])

    def test_main_stops_server_on_each_workflow_failure(self):
        # No binary or network is used: exercise main's actual try/finally boundary.
        for failing_stage in ("launch", "review", "transfer", "trial"):
            with self.subTest(stage=failing_stage), tempfile.TemporaryDirectory() as temp:
                models = Path(temp) / "models"
                models.mkdir(mode=0o700)
                runner = Mock()
                getattr(runner, failing_stage).side_effect = AssertionError("synthetic failure")
                pin = live.Pin(
                    "hf:o/r/model.gguf@" + "a" * 40,
                    "local:model.gguf",
                    hashlib.sha256(b"abc").hexdigest(),
                    3,
                )

                def complete(models=models, pin=pin):
                    (models / pin.filename).write_bytes(b"abc")

                if failing_stage == "trial":
                    runner.transfer.side_effect = complete
                with (
                    patch.object(live, "auth_request", side_effect=ConnectionRefusedError),
                    patch.object(live, "create_accounts"),
                    patch.object(live, "parse_pin", return_value=pin),
                    patch.object(live, "Live", return_value=runner) as factory,
                    patch.object(live, "write_fixture", return_value=Path("fixture.json")),
                    patch.object(live, "say"),
                ):
                    result = live.main(
                        [__file__, "--models", str(models), "--pinned", "granite", "--try"]
                    )
                self.assertEqual(result, 1)
                self.assertEqual(factory.call_args.args[0].threads, 2)
                self.assertIsNone(factory.call_args.args[0].runtime)
                runner.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
