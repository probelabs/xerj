import hashlib
import json
import pathlib
import stat
import subprocess
import sys
import tempfile
import unittest


HERE = pathlib.Path(__file__).resolve().parent


def capture_command(output, *extra, program=None):
    if program is None:
        program = (
            "import os,pathlib;"
            "root=pathlib.Path(os.environ['XERJ_DEBUG_PROFILE_DIR']);"
            "(root/'cpu.pb').write_bytes(b'cpu-profile')"
        )
    return [
        sys.executable,
        str(HERE / "capture.py"),
        "--output",
        str(output),
        "--cpu-seconds",
        "1",
        "--workload",
        "attachment-smoke",
        "--corpus",
        "none",
        "--concurrency",
        "1",
        "--cache-state",
        "cold",
        "--build-features",
        "debug-profiling",
        "--build-profile",
        "debug",
        *extra,
        "--",
        sys.executable,
        "-c",
        program,
    ]


class CaptureToolingTests(unittest.TestCase):
    def test_missing_requested_profile_is_machine_readable_failure(self):
        with tempfile.TemporaryDirectory() as parent:
            output = pathlib.Path(parent) / "capture"
            result = subprocess.run(
                [
                    sys.executable,
                    str(HERE / "capture.py"),
                    "--output",
                    str(output),
                    "--cpu-seconds",
                    "1",
                    "--workload",
                    "negative-smoke",
                    "--corpus",
                    "none",
                    "--concurrency",
                    "1",
                    "--cache-state",
                    "cold",
                    "--build-features",
                    "debug-profiling",
                    "--build-profile",
                    "debug",
                    "--",
                    "/usr/bin/sleep",
                    "0.01",
                ],
                check=False,
            )
            self.assertEqual(result.returncode, 2)
            manifest_path = output / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            self.assertEqual(manifest["status"], "profile_failed")
            self.assertEqual(manifest["missing_requested_artifacts"], ["cpu.pb"])
            self.assertIn("server.log", manifest["artifacts"])
            self.assertIn("git_head", manifest["source"])
            self.assertEqual(
                stat.S_IMODE(manifest_path.stat().st_mode),
                0o600,
            )

            inspect = subprocess.run(
                [sys.executable, str(HERE / "inspect.py"), str(output)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(inspect.returncode, 1)
            self.assertFalse(json.loads(inspect.stdout)["valid"])

    def test_stop_after_requires_publication_grace(self):
        result = subprocess.run(
            [
                sys.executable,
                str(HERE / "capture.py"),
                "--output",
                "/unused",
                "--cpu-seconds",
                "10",
                "--delay-seconds",
                "5",
                "--stop-after",
                "15",
                "--workload",
                "unused",
                "--corpus",
                "unused",
                "--concurrency",
                "1",
                "--cache-state",
                "cold",
                "--build-features",
                "debug-profiling",
                "--build-profile",
                "profiling",
                "--",
                "/usr/bin/true",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("after capture", result.stderr)

    def test_post_run_attachment_is_copied_and_hashed_after_process_exit(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            output = parent / "capture"
            evidence = parent / "telemetry.ndjson"
            program = (
                "import os,pathlib;"
                "root=pathlib.Path(os.environ['XERJ_DEBUG_PROFILE_DIR']);"
                "(root/'cpu.pb').write_bytes(b'cpu-profile');"
                f"pathlib.Path({str(evidence)!r}).write_text('complete\\n')"
            )
            result = subprocess.run(
                capture_command(
                    output,
                    "--attach-after",
                    f"telemetry={evidence}",
                    program=program,
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads((output / "manifest.json").read_text())
            metadata = manifest["attachments"]["telemetry"]
            attached = output / metadata["file"]
            self.assertEqual(manifest["status"], "complete")
            self.assertEqual(metadata["collection"], "post_process")
            self.assertEqual(metadata["process_exit_code_at_collection"], 0)
            self.assertEqual(attached.read_text(), "complete\n")
            self.assertEqual(
                metadata["sha256"], hashlib.sha256(b"complete\n").hexdigest()
            )
            self.assertEqual(manifest["attachment_failures"], [])

    def test_missing_post_run_attachment_is_manifest_failure(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            output = parent / "capture"
            missing = parent / "late-but-missing.json"
            result = subprocess.run(
                capture_command(
                    output, "--attach-after", f"correctness={missing}"
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 4)
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(manifest["status"], "attachment_failed")
            self.assertNotIn("correctness", manifest["attachments"])
            self.assertEqual(
                manifest["attachment_failures"][0]["label"], "correctness"
            )
            inspect = subprocess.run(
                [sys.executable, str(HERE / "inspect.py"), str(output)],
                check=False,
                capture_output=True,
                text=True,
            )
            summary = json.loads(inspect.stdout)
            self.assertEqual(inspect.returncode, 1)
            self.assertFalse(summary["valid"])
            self.assertFalse(summary["comparison_ready"])
            self.assertIn(
                "attachment collection failed: correctness",
                summary["errors"][0],
            )

    def test_existing_attach_remains_pre_launch_compatible(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            output = parent / "capture"
            evidence = parent / "baseline.json"
            evidence.write_text('{"correct":true}\n')
            result = subprocess.run(
                capture_command(
                    output, "--attach", f"correctness={evidence}"
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads((output / "manifest.json").read_text())
            metadata = manifest["attachments"]["correctness"]
            self.assertEqual(metadata["collection"], "pre_launch")
            self.assertNotIn("process_exit_code_at_collection", metadata)
            self.assertEqual(
                (output / metadata["file"]).read_text(), '{"correct":true}\n'
            )

    def test_post_run_attachment_is_collected_on_process_failure(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            output = parent / "capture"
            evidence = parent / "failure-context.txt"
            program = (
                "import os,pathlib,sys;"
                "root=pathlib.Path(os.environ['XERJ_DEBUG_PROFILE_DIR']);"
                "(root/'cpu.pb').write_bytes(b'cpu-profile');"
                f"pathlib.Path({str(evidence)!r}).write_text('failed after evidence\\n');"
                "sys.exit(7)"
            )
            result = subprocess.run(
                capture_command(
                    output,
                    "--attach-after",
                    f"telemetry={evidence}",
                    program=program,
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 3)
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(manifest["status"], "process_failed")
            self.assertEqual(manifest["exit_code"], 7)
            self.assertEqual(
                manifest["attachments"]["telemetry"][
                    "process_exit_code_at_collection"
                ],
                7,
            )

    def test_profile_failure_precedes_attachment_failure_without_hiding_either(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            output = parent / "capture"
            result = subprocess.run(
                capture_command(
                    output,
                    "--attach-after",
                    f"telemetry={parent / 'missing.ndjson'}",
                    program="pass",
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 2)
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(manifest["status"], "profile_failed")
            self.assertEqual(manifest["missing_requested_artifacts"], ["cpu.pb"])
            self.assertEqual(
                manifest["attachment_failures"][0]["label"], "telemetry"
            )

    def test_post_run_attachment_never_overwrites_existing_destination(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            output = parent / "capture"
            evidence = parent / "telemetry.txt"
            program = (
                "import os,pathlib;"
                "root=pathlib.Path(os.environ['XERJ_DEBUG_PROFILE_DIR']);"
                "(root/'cpu.pb').write_bytes(b'cpu-profile');"
                "(root/'attachment-telemetry.txt').write_text('existing\\n');"
                f"pathlib.Path({str(evidence)!r}).write_text('new\\n')"
            )
            result = subprocess.run(
                capture_command(
                    output,
                    "--attach-after",
                    f"telemetry={evidence}",
                    program=program,
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 4)
            self.assertEqual(
                (output / "attachment-telemetry.txt").read_text(), "existing\n"
            )
            self.assertFalse(
                (output / ".attachment-telemetry.txt.tmp").exists()
            )
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(manifest["status"], "attachment_failed")

    def test_duplicate_attachment_labels_are_rejected_before_launch(self):
        with tempfile.TemporaryDirectory() as parent:
            parent = pathlib.Path(parent)
            existing = parent / "existing.txt"
            existing.write_text("before\n")
            result = subprocess.run(
                capture_command(
                    parent / "capture",
                    "--attach",
                    f"evidence={existing}",
                    "--attach-after",
                    f"evidence={parent / 'after.txt'}",
                ),
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("duplicate attachment label", result.stderr)
            self.assertFalse((parent / "capture").exists())


if __name__ == "__main__":
    unittest.main()
