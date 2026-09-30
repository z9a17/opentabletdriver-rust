"""Copies OpenTabletDriver's tablet configurations into crates/otd-core/tablets.

Usage: python scripts/update-tablet-database.py [UPSTREAM_CHECKOUT]

Each file is the exact blob at the pinned revision recorded in
docs/parity/device-catalog.json; the working copy of the checkout is not
read, so line-ending conversion cannot change the bytes. The directory's
SOURCE file records where the files came from. Existing JSON files are
replaced; review the diff afterwards.
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PREFIX = "OpenTabletDriver.Configurations/Configurations/"
TARGET = ROOT / "crates/otd-core/tablets"


def git(checkout, *args):
    return subprocess.run(["git", "-C", str(checkout), *args], check=True, capture_output=True).stdout


def main():
    checkout = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / "target/upstream/OpenTabletDriver")
    inventory = json.loads((ROOT / "docs/parity/device-catalog.json").read_text(encoding="utf-8"))
    revision = inventory["upstream"]["revision"]
    paths = [record["path"] for record in inventory["configurations"]]
    listed = git(checkout, "ls-tree", "-r", "--name-only", revision, PREFIX).decode("utf-8").splitlines()
    listed = [path for path in listed if path.endswith(".json")]
    if sorted(listed) != sorted(paths):
        sys.exit("the checkout's configuration list differs from the inventory; update the inventory first")

    # Read and authenticate every blob before changing the embedded catalog.
    blobs = {}
    for record in inventory["configurations"]:
        path = record["path"]
        if not path.startswith(PREFIX) or ".." in Path(path).parts:
            sys.exit(f"invalid configuration path: {path}")
        actual = git(checkout, "rev-parse", f"{revision}:{path}").decode().strip()
        if actual != record["git_blob"]:
            sys.exit(f"configuration blob differs from the inventory: {path}")
        blobs[path] = git(checkout, "cat-file", "blob", actual)
    for old in TARGET.rglob("*.json"):
        old.unlink()
    for path in paths:
        destination = TARGET / path[len(PREFIX):]
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(blobs[path])
    (TARGET / "SOURCE").write_text(
        "OpenTabletDriver tablet configurations, LGPL-3.0-or-later.\n"
        f"Repository: https://github.com/OpenTabletDriver/OpenTabletDriver\n"
        f"Revision: {revision}\n"
        f"Path: {PREFIX}\n"
        "Copied unchanged by scripts/update-tablet-database.py.\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"copied {len(paths)} configurations at {revision}")


if __name__ == "__main__":
    main()
