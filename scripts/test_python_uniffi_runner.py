"""Unit tests for the local runner; no V8 build required."""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "runner", Path(__file__).with_name("test-python-uniffi.py")
)
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class RunnerTests(unittest.TestCase):
    def test_missing_bindgen(self):
        with (
            patch.object(sys, "argv", ["runner"]),
            patch.object(runner.shutil, "which", return_value=None),
        ):
            self.assertEqual(runner.main(), 1)

    def test_wrong_bindgen_version(self):
        with (
            patch.object(sys, "argv", ["runner"]),
            patch.object(runner, "require_tool", return_value="bindgen"),
            patch.object(
                subprocess, "check_output", return_value="uniffi-bindgen 0.31.0"
            ),
        ):
            self.assertEqual(runner.main(), 1)

    def test_existing_library_uses_current_python_and_cleans_up(self):
        with tempfile.TemporaryDirectory() as directory:
            library = Path(directory) / "input.so"
            library.write_bytes(b"test library")
            calls = []

            def run(command, **kwargs):
                calls.append(command)
                if command[0] == sys.executable:
                    out = Path(kwargs["cwd"])
                    self.assertTrue((out / "libmcp_v8_uniffi.so").exists())
                    self.assertEqual(kwargs["env"]["PYTHONPATH"], str(out))
                    self.output_directory = out

            with (
                patch.object(sys, "platform", "linux"),
                patch.object(sys, "argv", ["runner", "--library", str(library)]),
                patch.object(runner, "require_tool", return_value="bindgen"),
                patch.object(
                    subprocess, "check_output", return_value="uniffi-bindgen 0.32.0\n"
                ),
                patch.object(subprocess, "run", side_effect=run),
            ):
                self.assertEqual(runner.main(), 0)
            self.assertEqual(len(calls), 2)
            self.assertEqual(calls[1][0], sys.executable)
            self.assertFalse(self.output_directory.exists())

    def test_subprocess_failure_is_reported(self):
        with (
            patch.object(sys, "argv", ["runner"]),
            patch.object(runner, "require_tool", return_value="bindgen"),
            patch.object(
                subprocess,
                "check_output",
                side_effect=subprocess.CalledProcessError(1, "bindgen"),
            ),
        ):
            self.assertEqual(runner.main(), 1)

    def test_build_uses_shared_v8_flags(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            calls = []

            def run(command, **kwargs):
                calls.append(command)
                if command[0] == "cargo":
                    env = kwargs["env"]
                    self.assertNotIn("RUSTY_V8_ARCHIVE", env)
                    self.assertEqual(env["V8_FROM_SOURCE"], "1")
                    self.assertIn(
                        "v8_monolithic_for_shared_library=true", env["GN_ARGS"]
                    )
                    artifact = (
                        Path(env["CARGO_TARGET_DIR"])
                        / "release"
                        / "libmcp_v8_uniffi.so"
                    )
                    artifact.parent.mkdir(parents=True)
                    artifact.write_bytes(b"test library")

            with (
                patch.object(runner, "ROOT", root),
                patch.object(sys, "platform", "linux"),
                patch.object(sys, "argv", ["runner"]),
                patch.dict(
                    os.environ,
                    {"RUSTY_V8_ARCHIVE": "wrong.a", "CARGO_BUILD_TARGET": ""},
                ),
                patch.object(runner, "require_tool", side_effect=lambda name: name),
                patch.object(
                    subprocess, "check_output", return_value="uniffi-bindgen 0.32.0"
                ),
                patch.object(subprocess, "run", side_effect=run),
            ):
                self.assertEqual(runner.main(), 0)
            self.assertEqual(
                calls[0][:4], ["cargo", "build", "-p", "mcp-v8-uniffi-python"]
            )
            self.assertEqual(len(calls), 3)


if __name__ == "__main__":
    unittest.main()
