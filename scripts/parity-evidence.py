#!/usr/bin/env python3
"""Create, check, and compare evidence for the pinned OpenTabletDriver baseline."""

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INVENTORY = ROOT / "docs/parity/upstream-inventory.json"
MATRIX = ROOT / "docs/parity/CAPABILITY_MATRIX.md"
BACKLOG = ROOT / "docs/parity/WORK_ITEMS.md"
LEDGER = ROOT / "docs/parity/evidence-ledger.json"
KINDS = {
    "source-reviewed",
    "unit-tested",
    "differential-tested",
    "integration-tested",
    "hardware-verified",
}
AUTOMATED = KINDS - {"source-reviewed", "hardware-verified"}
STATES = {"open", "implemented", "verified", "blocked", "not_applicable"}
HASH = re.compile(r"[0-9a-f]{64}\Z")
COMMIT = re.compile(r"[0-9a-f]{40}\Z")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"), object_pairs_hook=unique_object)


def write_new(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8", newline="\n") as output:
        json.dump(data, output, indent=2, ensure_ascii=False)
        output.write("\n")


def canonical_hash(value):
    payload = json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def capability_ids(matrix_text):
    ids = re.findall(r"^\| (CAP-\d+) \|", matrix_text, flags=re.MULTILINE)
    if len(ids) != len(set(ids)) or not ids:
        raise ValueError("capability matrix has duplicate or missing CAP IDs")
    return sorted(ids)


def task_ids(backlog_text):
    ids = re.findall(r"^### ([A-Z]\d{2}) - ", backlog_text, flags=re.MULTILINE)
    if len(ids) != len(set(ids)) or not ids:
        raise ValueError("work item backlog has duplicate or missing task IDs")
    return set(ids)


def source_keys(inventory):
    result = {
        "configurations": [item["path"] for item in inventory["configurations"]],
        "parsers": [item["type"] for item in inventory["referenced_parsers"]],
        "plugins": [item["path"] for item in inventory["catalog_entries"]],
    }
    for section, keys in result.items():
        if len(keys) != len(set(keys)) or not keys:
            raise ValueError(f"source inventory has duplicate or missing {section} keys")
    return result


def blank(state="open"):
    return {
        "state": state,
        "owner": None,
        "pull_request": None,
        "release": None,
        "evidence": [],
        "blockers": [],
    }


def bootstrap(inventory, caps):
    keys = source_keys(inventory)
    return {
        "schema_version": 1,
        "source": {
            "upstream_revision": inventory["upstream"]["revision"],
            "catalog_revision": inventory["catalog"]["revision"],
            "inventory_sha256": canonical_hash(inventory),
        },
        "capabilities": {cap: blank() for cap in caps},
        "configurations": {path: blank() for path in keys["configurations"]},
        "parsers": {name: blank() for name in keys["parsers"]},
        "plugins": {
            item["path"]: {
                **blank("open" if item["metadata_allows_baseline"] else "not_applicable"),
                "classes": [],
                "class_inventory_complete": False,
            }
            for item in inventory["catalog_entries"]
        },
        "manual_plugins": {},
        "plugin_classes": {},
    }


def check_keys(actual, expected, where, errors):
    if not isinstance(actual, dict):
        errors.append(f"{where}: expected an object")
        return False
    missing = set(expected) - set(actual)
    extra = set(actual) - set(expected)
    if missing:
        errors.append(f"{where}: missing {len(missing)} keys: {', '.join(sorted(missing)[:3])}")
    if extra:
        errors.append(f"{where}: unexpected {len(extra)} keys: {', '.join(sorted(extra)[:3])}")
    return not (missing or extra)


def valid_class_names(names):
    return (
        isinstance(names, list)
        and all(isinstance(name, str) and name.strip() for name in names)
        and len(names) == len(set(names))
    )


def valid_date(value):
    try:
        return isinstance(value, str) and datetime.fromisoformat(value).tzinfo is not None
    except ValueError:
        return False


def check_evidence(item, where, source_revision, tasks, plugin, errors):
    evidence = item.get("evidence")
    if not isinstance(evidence, list):
        errors.append(f"{where}: evidence must be a list")
        return []
    passing = []
    for index, entry in enumerate(evidence):
        location = f"{where}.evidence[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{location}: expected an object")
            continue
        required = (
            "kind", "task", "source_revision", "rust_commit", "platform", "architecture",
            "date", "tester", "reference", "link", "result",
        )
        for name in required:
            if not isinstance(entry.get(name), str) or not entry[name].strip():
                errors.append(f"{location}: missing {name}")
        if not isinstance(entry.get("kind"), str) or entry["kind"] not in KINDS:
            errors.append(f"{location}: unknown evidence kind")
        if not isinstance(entry.get("task"), str) or entry["task"] not in tasks:
            errors.append(f"{location}: unknown work item")
        if entry.get("source_revision") != source_revision:
            errors.append(f"{location}: stale source revision")
        if not COMMIT.fullmatch(str(entry.get("rust_commit", ""))):
            errors.append(f"{location}: rust_commit must be a full Git SHA")
        if not valid_date(entry.get("date")):
            errors.append(f"{location}: date needs an ISO timestamp with timezone")
        if entry.get("result") not in ("pass", "fail"):
            errors.append(f"{location}: result must be pass or fail")
        if entry.get("kind") not in ("source-reviewed", None):
            if not isinstance(entry.get("command"), str) or not entry["command"].strip():
                errors.append(f"{location}: test command or manual procedure is required")
        if entry.get("kind") == "hardware-verified":
            for name in ("device_model", "firmware", "transport"):
                if not isinstance(entry.get(name), str) or not entry[name].strip():
                    errors.append(f"{location}: hardware evidence needs {name}")
        if plugin and entry.get("kind") in ("integration-tested", "hardware-verified"):
            if not HASH.fullmatch(str(entry.get("dll_sha256", ""))):
                errors.append(f"{location}: unchanged plugin needs a DLL SHA-256")
        if "fixture_sha256" in entry and not HASH.fullmatch(str(entry["fixture_sha256"])):
            errors.append(f"{location}: fixture_sha256 must be a SHA-256")
        if entry.get("result") == "pass" and isinstance(entry.get("kind"), str):
            passing.append(entry)
    return passing


def check_row(item, where, revision, tasks, kind, errors, plugin=False, configuration=None):
    if not isinstance(item, dict):
        errors.append(f"{where}: expected an object")
        return []
    state = item.get("state")
    if not isinstance(state, str) or state not in STATES:
        errors.append(f"{where}: invalid state {state!r}")
    for field in ("owner", "pull_request", "release"):
        value = item.get(field)
        if value is not None and (not isinstance(value, str) or not value.strip()):
            errors.append(f"{where}: {field} must be null or a nonempty string")
    if state in ("implemented", "verified") and not item.get("pull_request"):
        errors.append(f"{where}: {state} needs a merged pull request")
    blockers = item.get("blockers")
    if not isinstance(blockers, list) or any(not isinstance(b, str) or not b.strip() for b in blockers):
        errors.append(f"{where}: blockers must be nonempty strings in a list")
    elif state == "blocked" and not blockers:
        errors.append(f"{where}: blocked state needs a reason")
    passing = check_evidence(item, where, revision, tasks, plugin, errors)
    kinds = {entry.get("kind") for entry in passing}
    if state == "not_applicable" and kind != "plugins":
        errors.append(f"{where}: only catalog metadata can establish not_applicable")
    if state in ("implemented", "verified") and not kinds.intersection(AUTOMATED):
        errors.append(f"{where}: {state} needs a passing automated test")
    if state == "verified":
        if kind == "configurations":
            valid_hardware = [
                entry for entry in passing if entry.get("kind") == "hardware-verified"
                and entry.get("device_model") == configuration["name"]
            ]
            if not valid_hardware:
                errors.append(f"{where}: verified configuration needs matching model hardware evidence")
        elif kind == "parsers" and not {"differential-tested", "hardware-verified"}.issubset(kinds):
            errors.append(f"{where}: verified parser needs differential and hardware evidence")
        elif kind == "plugin_classes" and "integration-tested" not in kinds:
            errors.append(f"{where}: verified class needs unchanged-DLL integration evidence")
        elif kind == "plugins" and "integration-tested" not in kinds:
            errors.append(f"{where}: verified package needs unchanged-DLL integration evidence")
        elif kind == "capabilities" and not kinds.intersection({"integration-tested", "hardware-verified"}):
            errors.append(f"{where}: verified capability needs integration or hardware evidence")
    return passing


def validate(ledger, inventory, caps, tasks):
    errors = []
    if not isinstance(ledger, dict):
        return ["ledger must be an object"]
    check_keys(ledger, (
        "schema_version", "source", "capabilities", "configurations", "parsers",
        "plugins", "manual_plugins", "plugin_classes",
    ), "ledger", errors)
    if ledger.get("schema_version") != 1:
        errors.append("unsupported ledger schema version")
    source = ledger.get("source")
    if isinstance(source, dict):
        check_keys(source, ("upstream_revision", "catalog_revision", "inventory_sha256"), "source", errors)
        if source.get("upstream_revision") != inventory["upstream"]["revision"]:
            errors.append("source: upstream revision changed; review a baseline diff")
        if source.get("catalog_revision") != inventory["catalog"]["revision"]:
            errors.append("source: catalog revision changed; review a baseline diff")
        if source.get("inventory_sha256") != canonical_hash(inventory):
            errors.append("source: inventory contents changed; review a baseline diff")
    else:
        errors.append("source: expected an object")

    source_ids = source_keys(inventory)
    expected = {"capabilities": caps, **source_ids}
    for kind, ids in expected.items():
        section = ledger.get(kind)
        if not isinstance(section, dict):
            errors.append(f"{kind}: expected an object")
            continue
        check_keys(section, ids, kind, errors)
        config_lookup = {row["path"]: row for row in inventory["configurations"]} if kind == "configurations" else {}
        catalog_lookup = {row["path"]: row for row in inventory["catalog_entries"]} if kind == "plugins" else {}
        revision = inventory["catalog"]["revision"] if kind == "plugins" else inventory["upstream"]["revision"]
        for key in ids:
            if key not in section:
                continue
            item = section[key]
            where = f"{kind}[{key}]"
            check_row(
                item, where, revision, tasks, kind, errors,
                plugin=kind == "plugins", configuration=config_lookup.get(key),
            )
            if kind == "plugins" and isinstance(item, dict):
                expected_eligibility = catalog_lookup[key]["metadata_allows_baseline"]
                if item.get("state") == "not_applicable" and expected_eligibility:
                    errors.append(f"{where}: eligible plugin cannot be not_applicable")
                if not expected_eligibility and item.get("state") != "not_applicable":
                    errors.append(f"{where}: metadata is not eligible for the pinned baseline")
                names = item.get("classes")
                if not valid_class_names(names):
                    errors.append(f"{where}: classes must be a unique list")
                if not isinstance(item.get("class_inventory_complete"), bool):
                    errors.append(f"{where}: class_inventory_complete must be boolean")

    manual = ledger.get("manual_plugins")
    classes = ledger.get("plugin_classes")
    if not isinstance(manual, dict) or not isinstance(classes, dict):
        errors.append("manual_plugins and plugin_classes must be objects")
        return errors
    catalog = ledger.get("plugins")
    if not isinstance(catalog, dict):
        catalog = {}
    for key, item in manual.items():
        where = f"manual_plugins[{key}]"
        if not isinstance(item, dict) or not key.startswith("manual:"):
            errors.append(f"{where}: expected a manual: identity and object")
            continue
        for field in ("identity", "version", "source_url", "classes", "class_inventory_complete"):
            if field not in item:
                errors.append(f"{where}: missing {field}")
        for field in ("identity", "version", "source_url"):
            if not isinstance(item.get(field), str) or not item[field].strip():
                errors.append(f"{where}: {field} must be a nonempty string")
        names = item.get("classes")
        if not valid_class_names(names):
            errors.append(f"{where}: classes must be a unique list")
        if not isinstance(item.get("class_inventory_complete"), bool):
            errors.append(f"{where}: class_inventory_complete must be boolean")
        check_row(item, where, inventory["catalog"]["revision"], tasks, "plugins", errors, plugin=True)
        if item.get("state") == "not_applicable":
            errors.append(f"{where}: manually installed plugin needs an explicit open or blocked state")
    for key, item in classes.items():
        where = f"plugin_classes[{key}]"
        if not isinstance(item, dict):
            errors.append(f"{where}: expected an object")
            continue
        owner = item.get("plugin_key")
        name = item.get("type_name")
        if not isinstance(owner, str) or not isinstance(name, str) or key != f"{owner}::{name}" or not name.strip():
            errors.append(f"{where}: class key does not match plugin_key and type_name")
        if isinstance(owner, str) and owner.startswith("catalog:"):
            package = catalog.get(owner.removeprefix("catalog:"))
        else:
            package = manual.get(owner)
        package_classes = package.get("classes") if isinstance(package, dict) else None
        if not isinstance(package_classes, list) or name not in package_classes:
            errors.append(f"{where}: class missing from its package inventory")
        check_row(item, where, inventory["catalog"]["revision"], tasks, "plugin_classes", errors, plugin=True)
    for key, package in {**{f"catalog:{key}": value for key, value in catalog.items()}, **manual}.items():
        if not isinstance(package, dict):
            continue
        names = package.get("classes", [])
        if not isinstance(names, list):
            continue
        for name in names:
            if not isinstance(name, str) or not name.strip() or f"{key}::{name}" not in classes:
                errors.append(f"{key}: declared class {name!r} lacks a class record")
        if package.get("state") == "verified":
            if not package.get("class_inventory_complete") or not names:
                errors.append(f"{key}: verified package needs a complete, nonempty class inventory")
            verified = all(
                isinstance(classes.get(f"{key}::{name}"), dict)
                and classes[f"{key}::{name}"].get("state") == "verified"
                for name in names if isinstance(name, str)
            )
            if not verified:
                errors.append(f"{key}: all declared classes must be verified")
    return errors


def snapshot_diff(old, new):
    sections = (
        ("configurations", "path"),
        ("referenced_parsers", "type"),
        ("catalog_entries", "path"),
        ("plugin_contract_sources", None),
    )
    result = {
        "old_upstream_revision": old["upstream"]["revision"],
        "new_upstream_revision": new["upstream"]["revision"],
        "old_catalog_revision": old["catalog"]["revision"],
        "new_catalog_revision": new["catalog"]["revision"],
        "sections": {},
    }
    for section, key in sections:
        left = {row[key]: row for row in old[section]} if key else {row: row for row in old[section]}
        right = {row[key]: row for row in new[section]} if key else {row: row for row in new[section]}
        result["sections"][section] = {
            "added": sorted(right.keys() - left.keys()),
            "removed": sorted(left.keys() - right.keys()),
            "changed": sorted(name for name in left.keys() & right.keys() if left[name] != right[name]),
        }
    return result


def prepare_advance(old_inventory, old_ledger, old_matrix, new_inventory, new_matrix, tasks):
    old_caps = capability_ids(old_matrix)
    errors = validate(old_ledger, old_inventory, old_caps, tasks)
    if errors:
        raise ValueError("old ledger is invalid:\n" + "\n".join(errors))
    if canonical_hash(old_inventory) == canonical_hash(new_inventory):
        raise ValueError("source inventory is unchanged; keep the existing ledger")
    next_ledger = bootstrap(new_inventory, capability_ids(new_matrix))
    archive = {
        "schema_version": 1,
        "inventory": old_inventory,
        "capability_matrix": old_matrix,
        "ledger": old_ledger,
        "next_source": next_ledger["source"],
        "source_diff": snapshot_diff(old_inventory, new_inventory),
    }
    return next_ledger, archive


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("bootstrap", help="create a new, open evidence ledger")
    create.add_argument("--inventory", type=Path, default=INVENTORY)
    create.add_argument("--matrix", type=Path, default=MATRIX)
    create.add_argument("--output", type=Path, default=LEDGER)
    check = commands.add_parser("validate", help="reject missing/stale IDs and unsupported claims")
    check.add_argument("--inventory", type=Path, default=INVENTORY)
    check.add_argument("--matrix", type=Path, default=MATRIX)
    check.add_argument("--backlog", type=Path, default=BACKLOG)
    check.add_argument("--ledger", type=Path, default=LEDGER)
    compare = commands.add_parser("diff", help="list source records changed between two inventories")
    compare.add_argument("--old", type=Path, required=True)
    compare.add_argument("--new", type=Path, required=True)
    advance = commands.add_parser("advance", help="archive old evidence and open a new baseline ledger")
    advance.add_argument("--old-inventory", type=Path, required=True)
    advance.add_argument("--new-inventory", type=Path, required=True)
    advance.add_argument("--old-ledger", type=Path, required=True)
    advance.add_argument("--old-matrix", type=Path, default=MATRIX)
    advance.add_argument("--new-matrix", type=Path, default=MATRIX)
    advance.add_argument("--backlog", type=Path, default=BACKLOG)
    advance.add_argument("--output", type=Path, required=True)
    advance.add_argument("--archive", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "bootstrap":
            inventory = read_json(args.inventory)
            ledger = bootstrap(inventory, capability_ids(args.matrix.read_text(encoding="utf-8-sig")))
            write_new(args.output, ledger)
            print(
                f"Created {args.output}: {len(ledger['capabilities'])} capabilities, "
                f"{len(ledger['configurations'])} configurations, {len(ledger['parsers'])} parsers, "
                f"{len(ledger['plugins'])} catalog records; all eligible records open."
            )
        elif args.command == "validate":
            inventory = read_json(args.inventory)
            ledger = read_json(args.ledger)
            errors = validate(
                ledger, inventory,
                capability_ids(args.matrix.read_text(encoding="utf-8-sig")),
                task_ids(args.backlog.read_text(encoding="utf-8-sig")),
            )
            if errors:
                for error in errors:
                    print(error, file=sys.stderr)
                return 1
            print(
                f"Validated {len(ledger['capabilities'])} capabilities, "
                f"{len(ledger['configurations'])} configurations, {len(ledger['parsers'])} parsers, "
                f"{len(ledger['plugins'])} catalog records, and "
                f"{len(ledger['plugin_classes'])} plugin classes against the pinned baseline."
            )
        elif args.command == "diff":
            print(json.dumps(snapshot_diff(read_json(args.old), read_json(args.new)), indent=2, ensure_ascii=False))
        else:
            inputs = {
                path.resolve() for path in (
                    args.old_inventory, args.new_inventory, args.old_ledger,
                    args.old_matrix, args.new_matrix, args.backlog,
                )
            }
            outputs = {args.output.resolve(), args.archive.resolve()}
            if len(outputs) != 2 or inputs & outputs:
                raise ValueError("advance outputs must be distinct new paths, separate from its inputs")
            if args.output.exists() or args.archive.exists():
                raise FileExistsError("advance refuses to replace an existing output or archive")
            next_ledger, archive = prepare_advance(
                read_json(args.old_inventory), read_json(args.old_ledger),
                args.old_matrix.read_text(encoding="utf-8-sig"),
                read_json(args.new_inventory), args.new_matrix.read_text(encoding="utf-8-sig"),
                task_ids(args.backlog.read_text(encoding="utf-8-sig")),
            )
            write_new(args.archive, archive)
            write_new(args.output, next_ledger)
            print(json.dumps(archive["source_diff"], indent=2, ensure_ascii=False))
            print(f"Archived prior evidence at {args.archive} and opened a new ledger at {args.output}.")
    except (ValueError, KeyError, TypeError, FileExistsError, FileNotFoundError, json.JSONDecodeError) as error:
        print(f"parity evidence: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
