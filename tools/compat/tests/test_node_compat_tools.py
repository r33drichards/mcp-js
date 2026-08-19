import hashlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[3]
COMMON_PATH = REPO / "tools/compat/node_compat_common.py"


def load_common():
    spec = importlib.util.spec_from_file_location("node_compat_common", COMMON_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class CorpusMetadataTests(unittest.TestCase):
    def test_load_versions_requires_source_fields(self):
        common = load_common()
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "versions.json"
            path.write_text(json.dumps({"source": {"repository": "https://example.test"}}))
            with self.assertRaisesRegex(ValueError, "missing required fields"):
                common.load_versions(path)

    def test_validate_source_rejects_bad_sha256(self):
        common = load_common()
        source = {
            "repository": "https://example.test/repo",
            "commit": "abc123",
            "archive_url": "https://example.test/archive.tar.gz",
            "sha256": "z" * 64,
        }
        with self.assertRaisesRegex(ValueError, "sha256"):
            common.validate_source("source", source)

    def test_download_verifies_and_extracts_archive(self):
        common = load_common()
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            archive = root / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                payload = b"hello"
                info = tarfile.TarInfo("repo-root/test/file.js")
                info.size = len(payload)
                tar.addfile(info, io.BytesIO(payload))
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            source = {
                "repository": "https://example.test/repo",
                "commit": "abc123",
                "archive_url": archive.as_uri(),
                "sha256": digest,
            }
            extracted = common.download_and_verify("source", source, root / "cache")
            self.assertEqual((extracted / "test/file.js").read_text(), "hello")
            self.assertEqual(extracted, common.download_and_verify("source", source, root / "cache", offline=True))

    def test_download_rejects_checksum_mismatch(self):
        common = load_common()
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            archive = root / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                payload = b"hello"
                info = tarfile.TarInfo("repo-root/file.js")
                info.size = len(payload)
                tar.addfile(info, io.BytesIO(payload))
            source = {
                "repository": "https://example.test/repo",
                "commit": "abc123",
                "archive_url": archive.as_uri(),
                "sha256": "0" * 64,
            }
            with self.assertRaisesRegex(ValueError, "checksum"):
                common.download_and_verify("source", source, root / "cache")

    def test_safe_extract_rejects_parent_path(self):
        common = load_common()
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            archive = root / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                payload = b"escape"
                info = tarfile.TarInfo("repo-root/../../escape")
                info.size = len(payload)
                tar.addfile(info, io.BytesIO(payload))
            with self.assertRaisesRegex(ValueError, "unsafe archive member"):
                common.safe_extract_tar(archive, root / "out")


if __name__ == "__main__":
    unittest.main()

class InventoryAndReportTests(unittest.TestCase):
    def run_tool(self, script, *args):
        return subprocess.run(
            [sys.executable, str(REPO / f"tools/compat/{script}"), *map(str, args)],
            cwd=REPO,
            check=True,
            capture_output=True,
            text=True,
        )

    def test_inventory_classifies_module_families_and_profiles(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            corpus = root / "corpus"
            files = [
                "test/parallel/test-stream-readable.js",
                "test/parallel/test-fs-read-file.js",
                "test/parallel/test-child-process-exec.js",
                "test/parallel/test-net-server.js",
                "test/parallel/test-worker-message-port.js",
                "test/parallel/test-addon-loading.js",
                "test/sequential/test-inspector-session.js",
            ]
            for relative in files:
                path = corpus / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("// fixture\n")
            output = root / "inventory.json"
            self.run_tool(
                "gen-node-compat-inventory.py",
                "--corpus",
                corpus,
                "--output",
                output,
            )
            inventory = json.loads(output.read_text())
            by_path = {entry["path"]: entry for entry in inventory["tests"]}
            self.assertEqual(by_path[files[0]]["family"], "streams")
            self.assertEqual(by_path[files[0]]["profile"], "pure")
            self.assertEqual(by_path[files[1]]["family"], "filesystem")
            self.assertEqual(by_path[files[1]]["profile"], "filesystem")
            self.assertEqual(by_path[files[2]]["family"], "subprocess")
            self.assertEqual(by_path[files[2]]["profile"], "subprocess")
            self.assertEqual(by_path[files[3]]["family"], "networking")
            self.assertEqual(by_path[files[3]]["profile"], "network-server")
            self.assertEqual(by_path[files[4]]["family"], "workers")
            self.assertEqual(by_path[files[4]]["profile"], "workers")
            self.assertEqual(by_path[files[5]]["status"], "unsupported")
            self.assertIn("native", by_path[files[5]]["reason"])
            self.assertEqual(by_path[files[6]]["profile"], "inspector")
            self.assertEqual([entry["path"] for entry in inventory["tests"]], sorted(files))

    def test_report_aggregates_without_combining_corpus_versions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            expectations = root / "expectations.json"
            inventory = root / "inventory.json"
            versions = root / "versions.json"
            json_output = root / "report.json"
            markdown_output = root / "report.md"
            expectations.write_text(json.dumps({
                "test/parallel/test-events.js": {
                    "status": "pass",
                    "family": "events",
                    "profile": "pure",
                    "compatibility": "exact",
                },
                "test/parallel/test-fs.js": {
                    "status": "policy_required",
                    "family": "filesystem",
                    "profile": "filesystem",
                    "compatibility": "adapted",
                    "reason": "requires filesystem policy",
                },
            }))
            inventory.write_text(json.dumps({
                "schema_version": 1,
                "source": {"name": "deno_node_test", "commit": "abc", "node_version": "26.5.1"},
                "tests": [
                    {"path": "test/parallel/test-events.js", "family": "events", "profile": "pure", "status": "untriaged", "compatibility": "unsupported", "reason": "not selected"},
                    {"path": "test/parallel/test-net.js", "family": "networking", "profile": "network-client", "status": "untriaged", "compatibility": "unsupported", "reason": "not selected"},
                ],
            }))
            versions.write_text(json.dumps({
                "node": {"tag": "v22.14.0", "repository": "https://example.test/node", "vendored_by": "script"},
                "deno_node_test": {"repository": "https://example.test/deno", "commit": "abc", "node_version": "26.5.1", "archive_url": "https://example.test/a", "sha256": "0" * 64},
                "citgm": {"repository": "https://example.test/citgm", "commit": "def", "archive_url": "https://example.test/b", "sha256": "1" * 64},
            }))
            self.run_tool(
                "gen-node-compat-report.py",
                "--expectations", expectations,
                "--inventory", inventory,
                "--versions", versions,
                "--json-output", json_output,
                "--markdown-output", markdown_output,
            )
            report = json.loads(json_output.read_text())
            self.assertEqual(report["fast_suite"]["status"]["pass"], 1)
            self.assertEqual(report["full_corpus"]["total"], 2)
            self.assertEqual(report["versions"]["fast_suite_node"], "v22.14.0")
            self.assertEqual(report["versions"]["deno_corpus_node"], "26.5.1")
            markdown = markdown_output.read_text()
            self.assertIn("Node `v22.14.0`", markdown)
            self.assertIn("Node `26.5.1`", markdown)
            self.assertNotIn("combined compatibility", markdown.lower())

class CommandContractTests(unittest.TestCase):
    def run_command(self, *args, env=None, check=True):
        command_env = dict(**__import__("os").environ)
        if env:
            command_env.update(env)
        return subprocess.run(
            [str(REPO / "tools/compat/node-compat.sh"), *args],
            cwd=REPO,
            env=command_env,
            check=check,
            capture_output=True,
            text=True,
        )

    def test_help_lists_supported_commands(self):
        result = self.run_command("--help")
        for command in ("fetch", "inventory", "report", "fast", "family <name>", "profile <name>", "check"):
            self.assertIn(command, result.stdout)

    def test_fast_and_filtered_commands_set_expected_cargo_environment(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            log = root / "commands.log"
            cargo = fake_bin / "cargo"
            cargo.write_text(
                "#!/usr/bin/env bash\n"
                "printf 'family=%s profile=%s args=%s\\n' "
                '"${NODE_COMPAT_FAMILY-}" "${NODE_COMPAT_PROFILE-}" "$*" >> "$NODE_COMPAT_COMMAND_LOG"\n'
            )
            cargo.chmod(0o755)
            env = {
                "PATH": f"{fake_bin}:{__import__('os').environ['PATH']}",
                "NODE_COMPAT_COMMAND_LOG": str(log),
            }
            self.run_command("fast", env=env)
            self.run_command("family", "events", env=env)
            self.run_command("profile", "pure", env=env)
            lines = log.read_text().splitlines()
            self.assertIn("family= profile= args=test --test node_compat node_core_subset_matches_expectations -- --nocapture", lines[0])
            self.assertIn("family=events profile=", lines[1])
            self.assertIn("family= profile=pure", lines[2])

    def test_filtered_commands_require_values(self):
        self.assertNotEqual(self.run_command("family", check=False).returncode, 0)
        self.assertNotEqual(self.run_command("profile", check=False).returncode, 0)

class FullWorkflowTests(unittest.TestCase):
    def test_full_workflow_is_required_railway_matrix(self):
        text=(REPO / ".github/workflows/node-compat-full.yml").read_text()
        self.assertIn("runs-on: [self-hosted, railway]",text)
        self.assertIn("fail-fast: false",text)
        self.assertIn("name: Node Compatibility Full",text)
        self.assertIn("if: always()",text)
