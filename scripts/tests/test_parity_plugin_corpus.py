import json
import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "docs/parity/plugin-corpus.json"
SPEC = importlib.util.spec_from_file_location("parity_plugin_corpus", ROOT / "scripts/parity-plugin-corpus.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
validate = MODULE.validate
verify_catalog_metadata_hash = MODULE.verify_catalog_metadata_hash


class PluginCorpusTests(unittest.TestCase):
    def test_pinned_corpus_has_expected_counts_and_explicit_unknowns(self):
        self.assertEqual(validate(CORPUS), [])

    def test_duplicate_identity_version_is_rejected(self):
        data = json.loads(CORPUS.read_text(encoding="utf-8"))
        data["records"].append(data["records"][0])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "corpus.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            self.assertTrue(any("duplicates identity/version" in error for error in validate(path)))

    def test_missing_unknown_state_is_rejected(self):
        data = json.loads(CORPUS.read_text(encoding="utf-8"))
        del data["records"][0]["classification"]["runtime_category"]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "corpus.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            self.assertTrue(any("must state runtime_category" in error for error in validate(path)))

    def test_path_and_metadata_hash_drift_are_rejected(self):
        data = json.loads(CORPUS.read_text(encoding="utf-8"))
        data["records"][0]["catalog_path"] = "Repository/not-in-inventory.json"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "corpus.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            errors = validate(path)
            self.assertTrue(any("not eligible in pinned inventory" in error for error in errors))

    def test_metadata_hash_drift_is_rejected_against_catalog_bytes(self):
        payload = b'{"Name":"fixture"}\n'
        record = {
            "catalog_path": "Repository/fixture.json",
            "provenance": {"metadata_blob_sha256": hashlib.sha256(payload).hexdigest()},
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = root / record["catalog_path"]
            metadata.parent.mkdir(parents=True)
            metadata.write_bytes(payload)
            self.assertIsNone(verify_catalog_metadata_hash(record, root))
            record["provenance"]["metadata_blob_sha256"] = "0" * 64
            self.assertIn("SHA-256 differs", verify_catalog_metadata_hash(record, root))


if __name__ == "__main__":
    unittest.main()
