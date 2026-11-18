"""Adapter contract regressions; these do not replace the real Rust suite."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "cargo_nix_driver", Path(__file__).with_name("cargo-nix-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class ExampleAdapterTests(unittest.TestCase):
    def test_existing_binary_environment_is_replaced_for_example(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target/bin").mkdir(parents=True)
            source = root / "source"
            example = source / "server/examples/node_compat_full.rs"
            example.parent.mkdir(parents=True)
            example.write_text("fn main() {}")
            compiler = root / "mock-rustc"
            compiler.write_text(
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "from pathlib import Path\n"
                "Path('target/invocation.json').write_text(json.dumps({\n"
                "  'args': sys.argv[1:], 'bin': os.environ['CARGO_BIN_NAME'],\n"
                "  'crate': os.environ['CARGO_CRATE_NAME']}))\n"
                "Path('target/bin/node_compat_full').write_bytes(b'fixture')\n")
            compiler.chmod(0o755)
            (root / "target/benchmark-example-command.json").write_text(json.dumps({
                "compiler": str(compiler), "source": str(source),
                "args": ["--crate-name", "server", "src/main.rs", "--out-dir", "target/bin",
                         "--extern", "server=target/lib/libserver.rlib"],
                "env": {"CARGO_BIN_NAME": "server", "CARGO_CRATE_NAME": "server"},
            }))
            previous = Path.cwd()
            try:
                os.chdir(root)
                driver.compile_example()
            finally:
                os.chdir(previous)
            invocation = json.loads((root / "target/invocation.json").read_text())
            self.assertEqual(invocation["bin"], "node_compat_full")
            self.assertEqual(invocation["crate"], "node_compat_full")
            self.assertEqual(invocation["args"][1], "node_compat_full")
            self.assertIn(str(example), invocation["args"])
            self.assertIn("server=target/lib/libserver.rlib", invocation["args"])
            self.assertEqual((root / "target/bin/node_compat_full").read_bytes(), b"fixture")


if __name__ == "__main__":
    unittest.main()
