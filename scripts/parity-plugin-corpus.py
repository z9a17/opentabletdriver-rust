#!/usr/bin/env python3
"""Validate the metadata-only P01 catalog corpus."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = ROOT / "docs/parity/plugin-corpus.json"
DEFAULT_INVENTORY = ROOT / "docs/parity/upstream-inventory.json"
DEFAULT_ARCHIVE_AUDIT = ROOT / "docs/parity/plugin-archive-audit.json"
EXPECTED_RECORDS = 57
EXPECTED_IDENTITIES = 50
IDENTITY_FIELDS = ("name", "owner", "repository_url")


def verify_catalog_metadata_hash(record: dict, catalog_root: Path) -> str | None:
    """Return a hash error when a corpus row does not match catalog bytes."""
    catalog_path = catalog_root / record["catalog_path"]
    try:
        actual_hash = hashlib.sha256(catalog_path.read_bytes()).hexdigest()
    except OSError as error:
        return f"cannot read catalog metadata {record['catalog_path']}: {error}"
    if actual_hash != record.get("provenance", {}).get("metadata_blob_sha256"):
        return f"metadata blob SHA-256 differs from catalog bytes for {record['catalog_path']}"
    return None


def validate(path: Path = DEFAULT_CORPUS, inventory_path: Path = DEFAULT_INVENTORY,
             catalog_root: Path | None = None,
             archive_audit_path: Path = DEFAULT_ARCHIVE_AUDIT) -> list[str]:
    data = json.loads(path.read_text(encoding="utf-8"))
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    errors: list[str] = []
    if data.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    records = data.get("records")
    if not isinstance(records, list):
        return errors + ["records must be an array"]
    identities = set()
    record_keys = set()
    expected = {
        entry["path"]: entry
        for entry in inventory.get("catalog_entries", [])
        if entry.get("metadata_allows_baseline")
    }
    actual_paths = set()
    source_root = catalog_root.resolve() if catalog_root else None
    for index, record in enumerate(records):
        prefix = f"records[{index}]"
        catalog_path = record.get("catalog_path")
        if catalog_path in actual_paths:
            errors.append(f"{prefix} duplicates catalog path {catalog_path}")
        actual_paths.add(catalog_path)
        source = expected.get(catalog_path)
        if source is None:
            errors.append(f"{prefix} catalog path is not eligible in pinned inventory: {catalog_path}")
            continue
        source_hash = record.get("provenance", {}).get("metadata_blob_sha256")
        if source_root:
            hash_error = verify_catalog_metadata_hash(record, source_root)
            if hash_error:
                errors.append(f"{prefix} {hash_error}")
        identity = record.get("identity", {})
        if any(not identity.get(field) for field in IDENTITY_FIELDS):
            errors.append(f"{prefix} is missing an identity field")
        key = tuple(identity.get(field) for field in IDENTITY_FIELDS)
        identities.add(key)
        version = record.get("plugin_version")
        inventory_fields = {
            "name": identity.get("name"),
            "owner": identity.get("owner"),
            "repository_url": identity.get("repository_url"),
            "plugin_version": version,
            "minimum_driver_version": record.get("supported_driver", {}).get("minimum"),
            "maximum_driver_version": record.get("supported_driver", {}).get("maximum"),
            "download_url": record.get("metadata", {}).get("download_url"),
            "archive_sha256": record.get("metadata", {}).get("archive_sha256_declared"),
            "license": record.get("metadata", {}).get("license_identifier_declared"),
        }
        for field, value in inventory_fields.items():
            if source.get(field) != value:
                errors.append(f"{prefix} {field} differs from pinned inventory for {catalog_path}")
        record_key = (*key, version)
        if record_key in record_keys:
            errors.append(f"{prefix} duplicates identity/version {record_key}")
        record_keys.add(record_key)
        if record.get("verification", {}).get("archive_hash") not in ("unverified", "verified"):
            errors.append(f"{prefix} must state whether the archive hash is verified")
        for field in ("runtime_category", "exported_classes", "referenced_assemblies",
                      "platform_requirements", "external_prerequisites", "api_dependencies"):
            if record.get("classification", {}).get(field) in (None, ""):
                errors.append(f"{prefix} must state {field} or an explicit unknown")
    missing_paths = set(expected) - actual_paths
    if missing_paths:
        errors.append(f"corpus is missing {len(missing_paths)} eligible inventory paths")
    if data.get("catalog", {}).get("revision") != inventory.get("catalog", {}).get("revision"):
        errors.append("catalog revision differs from pinned inventory")
    audited_paths = set()
    if archive_audit_path.exists():
        archive_audit = json.loads(archive_audit_path.read_text(encoding="utf-8"))
        if archive_audit.get("catalog", {}).get("revision") != data.get("catalog", {}).get("revision"):
            errors.append("archive audit catalog revision differs from corpus")
        corpus_by_path = {record.get("catalog_path"): record for record in records}
        for index, audited in enumerate(archive_audit.get("records", [])):
            prefix = f"archive_audit.records[{index}]"
            catalog_path = audited.get("catalog_path")
            if catalog_path in audited_paths:
                errors.append(f"{prefix} duplicates catalog path {catalog_path}")
            audited_paths.add(catalog_path)
            source = corpus_by_path.get(catalog_path)
            if source is None:
                errors.append(f"{prefix} catalog path is absent from corpus: {catalog_path}")
                continue
            audit = audited.get("audit", {})
            if audited.get("identity") != source.get("identity") or audited.get("plugin_version") != source.get("plugin_version"):
                errors.append(f"{prefix} identity/version differs from corpus")
            if audit.get("catalog_download_url") != source.get("metadata", {}).get("download_url"):
                errors.append(f"{prefix} download URL differs from corpus")
            if audit.get("archive_sha256_declared") != source.get("metadata", {}).get("archive_sha256_declared"):
                errors.append(f"{prefix} declared archive hash differs from corpus")
            observed = audit.get("archive_sha256_observed")
            if audit.get("archive_sha256_verified") is not True or observed != audit.get("archive_sha256_declared"):
                errors.append(f"{prefix} observed archive hash is not verified against catalog")
            if source.get("verification", {}).get("archive_hash") != "verified":
                errors.append(f"{prefix} archive verification is not reflected in corpus")
            metadata = audit.get("metadata_inspection", {})
            if metadata.get("plugin_code_loaded_or_executed") is not False:
                errors.append(f"{prefix} must explicitly state plugin code was not loaded or executed")
            if not audit.get("remaining_unknowns"):
                errors.append(f"{prefix} must list remaining unknowns")
            candidate_names = [item.get("name") for item in metadata.get("plugin_candidate_types", [])]
            if candidate_names != source.get("classification", {}).get("exported_classes"):
                errors.append(f"{prefix} plugin candidate types differ from corpus")
            reference_names = [f"{item.get('name')} {item.get('version')}" for item in metadata.get("references", [])]
            if reference_names != source.get("classification", {}).get("referenced_assemblies"):
                errors.append(f"{prefix} assembly references differ from corpus")
        for record in records:
            verification = record.get("verification", {})
            if (verification.get("archive_hash") == "verified"
                    or verification.get("binary_inspection") == "managed_metadata_only") and record.get("catalog_path") not in audited_paths:
                errors.append(f"verified archive or binary-inspection claim lacks audit evidence: {record.get('catalog_path')}")
    else:
        for record in records:
            verification = record.get("verification", {})
            if verification.get("archive_hash") == "verified" or verification.get("binary_inspection") == "managed_metadata_only":
                errors.append(f"archive audit file is missing for verified claim: {archive_audit_path}")
                break
    if len(records) != EXPECTED_RECORDS:
        errors.append(f"expected {EXPECTED_RECORDS} eligible records; found {len(records)}")
    if len(identities) != EXPECTED_IDENTITIES:
        errors.append(f"expected {EXPECTED_IDENTITIES} identities; found {len(identities)}")
    declared = data.get("counts", {})
    if declared.get("records") != len(records) or declared.get("identities") != len(identities):
        errors.append("declared counts do not match corpus contents")
    if declared.get("eligible_records_expected") != EXPECTED_RECORDS:
        errors.append("eligible_records_expected must be 57 for the pinned baseline")
    if declared.get("eligible_identities_expected") != EXPECTED_IDENTITIES:
        errors.append("eligible_identities_expected must be 50 for the pinned baseline")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", nargs="?", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--catalog-root", type=Path,
                        help="clean Plugin-Repository checkout at the exact pinned revision; verifies metadata bytes")
    args = parser.parse_args()
    if args.catalog_root:
        revision = subprocess.run(
            ["git", "-C", str(args.catalog_root), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        expected_revision = json.loads(DEFAULT_CORPUS.read_text(encoding="utf-8"))["catalog"]["revision"]
        if revision != expected_revision:
            print(f"catalog checkout revision must be {expected_revision}; found {revision}")
            return 1
        status = subprocess.run(
            ["git", "-C", str(args.catalog_root), "status", "--porcelain", "--untracked-files=normal"],
            check=True, capture_output=True, text=True,
        ).stdout
        if status.strip():
            print("catalog checkout must be clean")
            return 1
    errors = validate(args.corpus, catalog_root=args.catalog_root)
    if errors:
        print("\n".join(errors))
        return 1
    print(f"Valid plugin corpus: {EXPECTED_RECORDS} records, {EXPECTED_IDENTITIES} identities")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
