import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "ocr_server.sh"
MODEL = "1f18abe17b1ed8b4e47ee9b1ad0e274c93daf5efbb6b29a04ff1712e37051e05"
MMPROJ = "98c0563673b1657ff6d021d1e5f04af06cbf61bb40c63ac613e8bb71b42fb2c0"


class OcrServerScriptTests(unittest.TestCase):
    """Runs the script with fake docker, date and sha256sum; docker calls are logged."""

    def run_script(self, *, day="4", hour="20", internal="true", exists=True, device="cpu"):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bin_dir, models, log = root / "bin", root / "models", root / "docker.log"
            bin_dir.mkdir()
            models.mkdir()
            (models / "surya-2.gguf").write_text("model")
            (models / "surya-2-mmproj.gguf").write_text("mmproj")
            fakes = {
                "docker": f"""#!/bin/sh
echo "$*" >> {log}
if [ "$1 $2" = "network inspect" ]; then
    [ {str(exists).lower()} = true ] || exit 1
    case "$*" in *Internal*) echo {internal};; esac
fi
exit 0
""",
                "date": f"""#!/bin/sh
case "$1" in +%u) echo {day};; +%H) echo {hour};; esac
""",
                "sha256sum": f"""#!/bin/sh
case "$1" in *mmproj*) echo "{MMPROJ}  $1";; *) echo "{MODEL}  $1";; esac
""",
                "getent": "#!/bin/sh\necho video:x:27:\n",
            }
            for name, body in fakes.items():
                path = bin_dir / name
                path.write_text(body)
                path.chmod(0o755)
            env = dict(
                os.environ,
                PATH=f"{bin_dir}:{os.environ['PATH']}",
                LLAMA_DIR=str(root / "llama"),
                MODEL_DIR=str(models),
                DEVICE=device,
            )
            result = subprocess.run(
                ["sh", str(SCRIPT), "start"], env=env, capture_output=True, text=True, check=False
            )
            calls = log.read_text().splitlines() if log.exists() else []
            return result, calls

    def started(self, calls):
        return [call for call in calls if call.startswith("run ")]

    def test_an_existing_network_that_is_not_internal_is_refused(self):
        result, calls = self.run_script(internal="false")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not internal", result.stderr)
        self.assertEqual(self.started(calls), [])

    def test_a_missing_network_is_created_internal(self):
        result, calls = self.run_script(exists=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("network create --internal mimik-ocr-net", calls)

    def test_the_gpu_is_refused_in_weekday_working_hours(self):
        result, calls = self.run_script(day="2", hour="10", device="gpu")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.started(calls), [])

    def test_weekday_cpu_runs_are_capped_at_two_cores_with_limits(self):
        result, calls = self.run_script(day="2", hour="10")
        self.assertEqual(result.returncode, 0, result.stderr)
        (run,) = self.started(calls)
        for flag in ("--cpus 2", "--memory 8g", "--pids-limit 256", "-t 2", "--cap-drop=ALL"):
            self.assertIn(flag, run)

    def test_evening_gpu_runs_keep_the_image_encoder_on_the_cpu(self):
        result, calls = self.run_script(day="4", hour="20", device="gpu")
        self.assertEqual(result.returncode, 0, result.stderr)
        (run,) = self.started(calls)
        self.assertIn("--no-mmproj-offload", run)
        self.assertIn("--device /dev/dri", run)


if __name__ == "__main__":
    unittest.main()
