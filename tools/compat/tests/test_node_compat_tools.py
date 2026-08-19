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
