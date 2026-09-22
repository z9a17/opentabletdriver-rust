"""Regression tests for parity evidence claims and baseline transitions."""

import copy
import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "parity-evidence.py"
SPEC = importlib.util.spec_from_file_location("parity_evidence", SCRIPT)
evidence = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evidence)


class ParityEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.inventory = evidence.read_json(evidence.INVENTORY)
        cls.capabilities = evidence.capability_ids(evidence.MATRIX.read_text(encoding="utf-8"))
        cls.tasks = evidence.task_ids(evidence.BACKLOG.read_text(encoding="utf-8"))

    def setUp(self):
        self.ledger = evidence.bootstrap(self.inventory, self.capabilities)

    def errors(self, inventory=None):
        return evidence.validate(
            self.ledger, inventory or self.inventory, self.capabilities, self.tasks
        )

    def record(self, kind, revision=None, **extra):
        return {
            "kind": kind,
            "task": "F03",
            "source_revision": revision or self.inventory["upstream"]["revision"],
            "rust_commit": "a" * 40,
            "platform": "Windows 11",
            "architecture": "x86_64",
            "date": "2026-09-22T12:00:00+00:00",
            "tester": "test fixture",
            "reference": "UP-DEVICE",
            "link": "https://example.invalid/result",
            "result": "pass",
            "command": "cargo test --locked fixture_case",
            **extra,
        }

    def test_bootstrap_covers_all_pinned_records_without_support_claims(self):
        self.assertEqual(len(self.ledger["capabilities"]), 52)
        self.assertEqual(len(self.ledger["configurations"]), 339)
        self.assertEqual(len(self.ledger["parsers"]), 52)
        self.assertEqual(len(self.ledger["plugins"]), 98)
        self.assertEqual(
            sum(row["state"] == "open" for row in self.ledger["plugins"].values()),
            57,
        )
        self.assertFalse(self.errors())

    def test_missing_configuration_is_rejected(self):
        key = next(iter(self.ledger["configurations"]))
        del self.ledger["configurations"][key]
        self.assertTrue(any("configurations: missing 1 keys" in row for row in self.errors()))

    def test_content_change_at_same_revision_is_rejected(self):
        changed = copy.deepcopy(self.inventory)
        changed["configurations"][0]["name"] += " changed"
        self.assertTrue(any("inventory contents changed" in row for row in self.errors(changed)))

    def test_imported_configuration_cannot_claim_hardware_verification(self):
        key = next(iter(self.ledger["configurations"]))
        row = self.ledger["configurations"][key]
        row.update(state="verified", pull_request="https://github.com/example/pull/1")
        row["evidence"] = [
            self.record("unit-tested"),
            self.record("hardware-verified", device_model="a different model", firmware="1", transport="USB"),
        ]
        self.assertTrue(any("matching model hardware evidence" in error for error in self.errors()))

    def test_matching_hardware_record_can_verify_one_configuration(self):
        key = next(iter(self.ledger["configurations"]))
        row = self.ledger["configurations"][key]
        row.update(state="verified", pull_request="https://github.com/example/pull/1")
        row["evidence"] = [
            self.record("unit-tested"),
            self.record(
                "hardware-verified",
                device_model=self.inventory["configurations"][0]["name"],
                firmware="1",
                transport="USB",
            ),
        ]
        self.assertFalse(self.errors())

    def test_parser_requires_differential_and_hardware_evidence(self):
        row = next(iter(self.ledger["parsers"].values()))
        row.update(state="verified", pull_request="https://github.com/example/pull/1")
        row["evidence"] = [self.record("unit-tested")]
        self.assertTrue(any("differential and hardware evidence" in error for error in self.errors()))

    def test_verified_package_needs_complete_classes_and_unchanged_dll(self):
        key = next(key for key, row in self.ledger["plugins"].items() if row["state"] == "open")
        package = self.ledger["plugins"][key]
        package.update(state="verified", pull_request="https://github.com/example/pull/1")
        package["evidence"] = [self.record(
            "integration-tested", self.inventory["catalog"]["revision"], dll_sha256="0" * 64,
        )]
        errors = self.errors()
        self.assertTrue(any("complete, nonempty class inventory" in row for row in errors))
        package.update(classes=["Example.Filter"], class_inventory_complete=True)
        self.ledger["plugin_classes"][f"catalog:{key}::Example.Filter"] = {
            **evidence.blank("verified"),
            "plugin_key": f"catalog:{key}",
            "type_name": "Example.Filter",
            "pull_request": "https://github.com/example/pull/1",
            "evidence": [self.record("integration-tested", self.inventory["catalog"]["revision"], dll_sha256="bad")],
        }
        self.assertTrue(any("unchanged plugin needs a DLL SHA-256" in row for row in self.errors()))

    def test_unknown_work_item_and_not_applicable_configuration_are_rejected(self):
        row = next(iter(self.ledger["configurations"].values()))
        row["state"] = "not_applicable"
        row["evidence"] = [self.record("source-reviewed", task="Z99")]
        errors = self.errors()
        self.assertTrue(any("unknown work item" in error for error in errors))
        self.assertTrue(any("only catalog metadata" in error for error in errors))

    def test_malformed_class_record_returns_errors_instead_of_crashing(self):
        key = next(key for key, row in self.ledger["plugins"].items() if row["state"] == "open")
        self.ledger["plugins"][key]["classes"] = ["Example.Filter"]
        self.ledger["plugin_classes"][f"catalog:{key}::Example.Filter"] = None
        self.assertTrue(any("expected an object" in error for error in self.errors()))

    def test_source_diff_and_advance_preserve_old_evidence(self):
        old_key = next(iter(self.ledger["capabilities"]))
        self.ledger["capabilities"][old_key]["evidence"] = [self.record("source-reviewed")]
        new_inventory = copy.deepcopy(self.inventory)
        changed_key = new_inventory["configurations"][0]["path"]
        new_inventory["configurations"][0]["git_blob"] = "b" * 40
        removed_key = new_inventory["referenced_parsers"].pop()["type"]
        added = copy.deepcopy(new_inventory["catalog_entries"][0])
        added["path"] += ".copy"
        new_inventory["catalog_entries"].append(added)
        new_inventory["upstream"]["revision"] = "b" * 40
        source_diff = evidence.snapshot_diff(self.inventory, new_inventory)
        self.assertIn(changed_key, source_diff["sections"]["configurations"]["changed"])
        self.assertIn(removed_key, source_diff["sections"]["referenced_parsers"]["removed"])
        self.assertIn(added["path"], source_diff["sections"]["catalog_entries"]["added"])
        next_ledger, archive = evidence.prepare_advance(
            self.inventory,
            self.ledger,
            evidence.MATRIX.read_text(encoding="utf-8"),
            new_inventory,
            evidence.MATRIX.read_text(encoding="utf-8"),
            self.tasks,
        )
        self.assertEqual(
            archive["ledger"]["capabilities"][old_key]["evidence"],
            self.ledger["capabilities"][old_key]["evidence"],
        )
        self.assertEqual(next_ledger["capabilities"][old_key]["state"], "open")
        self.assertEqual(next_ledger["capabilities"][old_key]["evidence"], [])
        self.assertFalse(evidence.validate(next_ledger, new_inventory, self.capabilities, self.tasks))

    def test_advance_cli_archives_before_opening_and_refuses_overwrite(self):
        new_inventory = copy.deepcopy(self.inventory)
        new_inventory["configurations"][0]["git_blob"] = "b" * 40
        with tempfile.TemporaryDirectory(prefix="otd-evidence-test-") as temporary:
            folder = Path(temporary)
            old_inventory_path = folder / "old-inventory.json"
            old_ledger_path = folder / "old-ledger.json"
            new_inventory_path = folder / "new-inventory.json"
            output_path = folder / "next-ledger.json"
            archive_path = folder / "archive.json"
            evidence.write_new(old_inventory_path, self.inventory)
            evidence.write_new(old_ledger_path, self.ledger)
            evidence.write_new(new_inventory_path, new_inventory)
            command = [
                "advance",
                "--old-inventory", str(old_inventory_path),
                "--old-ledger", str(old_ledger_path),
                "--new-inventory", str(new_inventory_path),
                "--output", str(output_path),
                "--archive", str(archive_path),
            ]
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(evidence.main(command), 0)
            archived = evidence.read_json(archive_path)
            self.assertEqual(archived["ledger"], self.ledger)
            self.assertEqual(
                evidence.read_json(output_path)["source"]["inventory_sha256"],
                evidence.canonical_hash(new_inventory),
            )
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(evidence.main(command), 1)
            self.assertEqual(evidence.read_json(archive_path), archived)


if __name__ == "__main__":
    unittest.main()
