#!/usr/bin/env python3
"""Validate the metadata-only P01 catalog corpus."""

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = ROOT / "docs/parity/plugin-corpus.json"
EXPECTED_RECORDS = 57
EXPECTED_IDENTITIES = 50
IDENTITY_FIELDS = ("name", "owner", "repository_url")


def validate(path: Path = DEFAULT_CORPUS) -> list[str]:
    data = json.loads(path.read_text(encoding="utf-8"))
    errors: list[str] = []
    if data.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    records = data.get("records")
    if not isinstance(records, list):
        return errors + ["records must be an array"]
    identities = set()
    record_keys = set()
    for index, record in enumerate(records):
        prefix = f"records[{index}]"
        identity = record.get("identity", {})
        if any(not identity.get(field) for field in IDENTITY_FIELDS):
            errors.append(f"{prefix} is missing an identity field")
        key = tuple(identity.get(field) for field in IDENTITY_FIELDS)
        identities.add(key)
        version = record.get("plugin_version")
        record_key = (*key, version)
        if record_key in record_keys:
            errors.append(f"{prefix} duplicates identity/version {record_key}")
        record_keys.add(record_key)
        if record.get("verification", {}).get("archive_hash") != "unverified":
            errors.append(f"{prefix} must not overstate archive hash verification")
        for field in ("runtime_category", "exported_classes", "referenced_assemblies",
                      "platform_requirements", "external_prerequisites", "api_dependencies"):
            if record.get("classification", {}).get(field) in (None, ""):
                errors.append(f"{prefix} must state {field} or an explicit unknown")
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
    args = parser.parse_args()
    errors = validate(args.corpus)
    if errors:
        print("\n".join(errors))
        return 1
    print(f"Valid plugin corpus: {EXPECTED_RECORDS} records, {EXPECTED_IDENTITIES} identities")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
