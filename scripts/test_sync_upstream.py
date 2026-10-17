import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from sync_upstream import CodeChanged, ROOT, review, sync, validate_data


def dataset(ua="Test"):
    return gzip.compress(json.dumps([{
        "appName": "Netscape", "platform": "Test", "userAgent": ua,
        "vendor": "", "deviceCategory": "desktop", "pluginsLength": 0,
        "screenHeight": 800, "screenWidth": 1200, "viewportHeight": 700,
        "viewportWidth": 1100, "weight": 1.0,
    }]).encode(), mtime=0)


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / "upstream"
        self.root = Path(self.temp.name) / "port"
        self.repo.mkdir()
        self.root.mkdir()
        self.git("init", "-q")
        (self.repo / "src").mkdir()
        (self.repo / "src/user-agent.ts").write_text("export const sample = 1;\n")
        (self.repo / "src/user-agents.json.gz").write_bytes(dataset())
        (self.repo / "package.json").write_text(json.dumps({"version": "1.0.0", "dependencies": {}}))
        (self.repo / "README.md").write_text("Documentation\n")
        self.commit()
        review(self.repo, self.root)
        sync(self.repo, self.root)

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.STDOUT)

    def commit(self):
        self.git("add", "-A")
        self.git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "fixture")

    def snapshot(self):
        return {str(p.relative_to(self.root)): p.read_bytes()
                for p in self.root.rglob("*") if p.is_file()}

    def assert_blocked(self):
        before = self.snapshot()
        with self.assertRaises(CodeChanged):
            sync(self.repo, self.root)
        self.assertEqual(before, self.snapshot(), "code drift must never modify data or baseline")

    def test_data_update_and_version_bump_are_allowed(self):
        content = dataset("Updated")
        (self.repo / "src/user-agents.json.gz").write_bytes(content)
        (self.repo / "package.json").write_text(json.dumps({"version": "1.0.1", "dependencies": {}}))
        (self.repo / "README.md").write_text("Updated docs\n")
        self.commit()
        baseline = (self.root / "upstream.lock.json").read_bytes()
        self.assertTrue(sync(self.repo, self.root))
        self.assertEqual(content, (self.root / "data/user-agents.json.gz").read_bytes())
        metadata = json.loads((self.root / "data/upstream.json").read_bytes())
        self.assertEqual(metadata["sha256"], hashlib.sha256(content).hexdigest())
        self.assertEqual(metadata["commit"], self.git("rev-parse", "HEAD").decode().strip())
        self.assertEqual(baseline, (self.root / "upstream.lock.json").read_bytes())
        before = self.snapshot()
        self.assertFalse(sync(self.repo, self.root))
        self.assertEqual(before, self.snapshot())

    def test_modified_source_blocks_data_update(self):
        (self.repo / "src/user-agent.ts").write_text("export const sample = 2;\n")
        (self.repo / "src/user-agents.json.gz").write_bytes(dataset("Must not copy"))
        self.commit()
        self.assert_blocked()

    def test_added_source_blocks(self):
        (self.repo / "new-code.ts").write_text("export const added = true;\n")
        self.commit()
        self.assert_blocked()

    def test_deleted_source_blocks(self):
        (self.repo / "src/user-agent.ts").unlink()
        self.commit()
        self.assert_blocked()

    def test_dependency_change_blocks(self):
        (self.repo / "package.json").write_text(json.dumps({"version": "1.0.1", "dependencies": {"new": "1"}}))
        self.commit()
        self.assert_blocked()

    def test_file_mode_change_blocks(self):
        self.git("update-index", "--chmod=+x", "src/user-agent.ts")
        self.git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "mode")
        self.assert_blocked()

    def test_invalid_data_does_not_modify_files(self):
        (self.repo / "src/user-agents.json.gz").write_bytes(dataset().replace(b"\x1f\x8b", b"xx", 1))
        self.commit()
        before = self.snapshot()
        with self.assertRaises((ValueError, OSError)):
            sync(self.repo, self.root)
        self.assertEqual(before, self.snapshot())

    def test_monitored_symlink_blocks(self):
        source = self.repo / "src/user-agent.ts"
        source.unlink()
        source.symlink_to("../package.json")
        self.commit()
        self.assert_blocked()

    def test_gitlink_blocks(self):
        revision = self.git("rev-parse", "HEAD").decode().strip()
        self.git("update-index", "--add", "--cacheinfo", f"160000,{revision},vendor/tool")
        self.git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "gitlink")
        self.assert_blocked()

    def test_missing_or_symlink_dataset_does_not_modify_files(self):
        path = self.repo / "src/user-agents.json.gz"
        for symlink in (False, True):
            if path.exists() or path.is_symlink():
                path.unlink()
            if symlink:
                path.symlink_to("../package.json")
            self.commit()
            before = self.snapshot()
            with self.assertRaises(ValueError):
                sync(self.repo, self.root)
            self.assertEqual(before, self.snapshot())


class BundledDataTests(unittest.TestCase):
    def test_python_and_rust_share_gzip_acceptance_fixtures(self):
        directory = ROOT / "tests/fixtures/gzip"
        expectations = json.loads((directory / "expectations.json").read_bytes())
        for name, expected in expectations.items():
            with self.subTest(name=name):
                try:
                    validate_data((directory / name).read_bytes())
                    accepted = True
                except (ValueError, OSError, EOFError):
                    accepted = False
                self.assertEqual(accepted, expected)

    def test_invalid_json_numbers_are_rejected(self):
        for value in ("NaN", "Infinity", "-Infinity", "1e9999"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_data(gzip.compress(f'[{{"unknown": {value}}}]'.encode()))

    def test_bundled_dataset_and_provenance(self):
        content = (ROOT / "data/user-agents.json.gz").read_bytes()
        metadata = json.loads((ROOT / "data/upstream.json").read_bytes())
        self.assertEqual(metadata["sha256"], hashlib.sha256(content).hexdigest())
        self.assertEqual(metadata["profiles"], validate_data(content))
        self.assertEqual(len(metadata["commit"]), 40)


if __name__ == "__main__":
    unittest.main()
