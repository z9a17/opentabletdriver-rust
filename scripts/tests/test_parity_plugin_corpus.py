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

    def test_archive_audit_is_bound_to_corpus_and_verified_hash(self):
        audit = json.loads((ROOT / "docs/parity/plugin-archive-audit.json").read_text(encoding="utf-8"))
        audit["records"][0]["audit"]["archive_sha256_observed"] = "0" * 64
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "audit.json"
            path.write_text(json.dumps(audit), encoding="utf-8")
            errors = validate(archive_audit_path=path)
            self.assertTrue(any("observed archive hash is not verified" in error for error in errors))

    def test_archive_audit_candidate_types_match_corpus(self):
        audit = json.loads((ROOT / "docs/parity/plugin-archive-audit.json").read_text(encoding="utf-8"))
        audit["records"][0]["audit"]["metadata_inspection"]["plugin_candidate_types"][0]["name"] = "Changed.Type"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "audit.json"
            path.write_text(json.dumps(audit), encoding="utf-8")
            errors = validate(archive_audit_path=path)
            self.assertTrue(any("plugin candidate types differ from corpus" in error for error in errors))

    def test_missing_archive_audit_rejects_verified_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "missing-audit.json"
            errors = validate(archive_audit_path=missing)
            self.assertTrue(any("archive audit file is missing for verified claim" in error for error in errors))

    def test_metadata_inspection_claim_requires_matching_audit_record(self):
        data = json.loads(CORPUS.read_text(encoding="utf-8"))
        data["records"][2]["verification"]["binary_inspection"] = "managed_metadata_only"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "corpus.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            errors = validate(path)
            self.assertTrue(any("verified archive or binary-inspection claim lacks audit evidence" in error
                                and data["records"][2]["catalog_path"] in error for error in errors))


if __name__ == "__main__":
    unittest.main()
