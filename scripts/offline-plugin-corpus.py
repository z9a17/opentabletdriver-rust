#!/usr/bin/env python3
"""Run source-reviewed real DLL fixtures without discovery or native input.

Archives must already exist in --artifacts. Network retrieval belongs to the
repository's src/download.rs path. Extraction is limited to one manifest DLL
per verified archive; third-party DLLs are never rebuilt or modified.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import zipfile


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def within(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if not path.is_relative_to(root) or path == root:
        raise ValueError(f"Artifact path escapes root: {relative}")
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("docs/parity/plugin-offline-corpus-2026-10-15.json"))
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--graphbench", type=Path, required=True, help="Already built GraphBench.dll; this script does not compile.")
    parser.add_argument("--tablet", type=Path, required=True, help="Read-only fixture tablet JSON, never a live device.")
    parser.add_argument("--output", type=Path, required=True, help="JSON evidence result, outside saved settings.")
    args = parser.parse_args()
    root = args.artifacts.resolve()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    observations = []
    for item in manifest["plugins"]:
        archive = within(root, item["archive"])
        dll = within(root, item["dll"])
        archive_hash = digest(archive.read_bytes())
        if archive_hash != item["archive_sha256"]:
            raise ValueError(f"Archive hash mismatch: {archive.name}")
        with zipfile.ZipFile(archive) as zipped:
            entries = [entry for entry in zipped.infolist() if not entry.is_dir() and Path(entry.filename).name == dll.name]
            if len(entries) != 1 or entries[0].file_size > 16 * 1024 * 1024:
                raise ValueError(f"Expected one bounded {dll.name} entry in {archive.name}")
            payload = zipped.read(entries[0])
        dll_hash = digest(payload)
        if dll_hash != item["dll_sha256"]:
            raise ValueError(f"DLL hash mismatch inside {archive.name}")
        dll.parent.mkdir(parents=True, exist_ok=True)
        if not dll.exists() or digest(dll.read_bytes()) != dll_hash:
            dll.write_bytes(payload)
        observations.append({"identity": item["identity"], "plugin_version": item["plugin_version"],
                             "archive_sha256": archive_hash, "dll_sha256": dll_hash,
                             "classes": [entry["type_name"] for entry in item["classes"]]})
    command = ["dotnet", str(args.graphbench.resolve()), "unused", str(args.tablet.resolve()), "corpus",
               str(args.manifest.resolve()), str(root)]
    result = subprocess.run(command, capture_output=True, text=True, timeout=120, check=False)
    evidence = {"schema_version": 1, "scope": manifest["scope"], "manifest_sha256": digest(args.manifest.read_bytes()),
                "command": command, "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr,
                "artifacts": observations,
                "validation_limits": ["No app, daemon, device discovery, native OS output, UI or saved settings.",
                                      "Same vendored SDK direct-interface reference; not a separate upstream daemon process.",
                                      "Bezier uses a constant zero trace and forced test-owned timer deadlines; no jitter/nonzero timing equivalence claim.",
                                      "Per-class cases do not establish whole-package or catalog compatibility."]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    print(result.stdout, end="")
    print(result.stderr, end="", file=sys.stderr)
    print(f"Evidence: {args.output}")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
