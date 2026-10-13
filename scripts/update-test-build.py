"""Build and install the current branch's Windows apps for manual pre-merge testing."""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path, PurePosixPath
import tempfile
from types import SimpleNamespace

import release


def preflight_targets(targets):
    """Reject already-mapped/locked Windows payloads before replacing any file.

    Close every probe handle before copying: MoveFileEx replacement can fail
    while a destination handle is retained even with delete sharing enabled.
    This is a preflight, not a multi-file atomic transaction: later I/O failures
    or a new process can still interrupt installation. A separate --destination
    is the safe option while using the existing test build.
    """
    handles = []
    close = None
    try:
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes

            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            create = kernel.CreateFileW
            create.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                               wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD,
                               wintypes.HANDLE]
            create.restype = wintypes.HANDLE
            close = kernel.CloseHandle
            close.argtypes = [wintypes.HANDLE]
            close.restype = wintypes.BOOL
            invalid = ctypes.c_void_p(-1).value
            for target in targets:
                if not target.exists():
                    continue
                # GENERIC_WRITE | DELETE, sharing read/write/delete. Executable
                # image mappings reject write access even when ordinary reads
                # would succeed. No content or attributes are changed here.
                handle = create(str(target), 0x40000000 | 0x00010000, 0x7,
                                None, 3, 0x80, None)
                if handle == invalid:
                    error = ctypes.get_last_error()
                    raise PermissionError(
                        error,
                        f"Test build file is in use or cannot be replaced: {target}. "
                        "Choose a separate --destination while the existing build is running.",
                        str(target),
                    )
                handles.append(handle)
    finally:
        if close is not None:
            for handle in reversed(handles):
                close(handle)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, default=Path("E:/OTD RUST TEST"))
    args = parser.parse_args()
    destination = args.destination.resolve()
    if destination == Path(destination.anchor) or destination == release.ROOT:
        raise ValueError("choose a dedicated test build folder")
    output = release.ROOT / "target/test-build"
    output.mkdir(parents=True, exist_ok=True)
    # Keep package staging on the workspace drive instead of the system temp drive.
    scratch = output / "scratch"
    scratch.mkdir(exist_ok=True)
    tempfile.tempdir = str(scratch)
    release.build(SimpleNamespace(platform="win-x64", rust_target="x86_64-pc-windows-msvc", output=output))
    archive = release.archive_path(output, "win-x64")
    checksum = release.digest(archive.read_bytes())
    if Path(str(archive) + ".sha256").read_text().strip() != f"{checksum}  {archive.name}":
        raise ValueError("test package checksum mismatch")
    files = release.read_archive(archive)
    prefix = release.package_name("win-x64") + "/"
    if any(not name.startswith(prefix) for name in files):
        raise ValueError("unexpected test package root")
    payload = {name.removeprefix(prefix): data for name, data in files.items()}
    metadata = json.loads(payload["data/BUILD-INFO.json"])
    if any(metadata.get(key) != value for key, value in release.source_state().items()):
        raise ValueError("test package does not match the current source")
    for name, expected in metadata["binaries"].items():
        data = payload[name]
        release.check_binary("win-x64", name, data)
        if release.digest(data) != expected:
            raise ValueError(f"test binary hash mismatch: {name}")
    for group in ("compat", "licenses"):
        for name, expected in metadata[group].items():
            if release.digest(payload[f"data/{group}/{name}"]) != expected:
                raise ValueError(f"test dependency hash mismatch: {name}")
    release.check_binary("win-x64", "nethost.dll", payload["data/compat/nethost.dll"])
    build = {
        "version": metadata["version"],
        "branch": release.command("git", "branch", "--show-current"),
        "source_commit": metadata["source_commit"],
        "source_dirty": metadata["source_dirty"],
        "built_at_utc": datetime.now(timezone.utc).isoformat(),
        "archive_sha256": checksum,
        "purpose": "Unmerged test build; no release published",
    }
    payload["data/TEST-BUILD.json"] = (json.dumps(build, indent=2) + "\n").encode()
    payload["TEST-BUILD.txt"] = (
        f"Unmerged test build, not a release.\n\n"
        f"Version: {build['version']}\n"
        f"Branch: {build['branch']}\n"
        f"Commit: {build['source_commit']}\n"
        f"Built: {build['built_at_utc']}\n\n"
        "See data/TEST-BUILD.json for build provenance.\n"
    ).encode()
    # Resolve every destination before writing. Preserve files the package does not own.
    targets = {}
    for name in payload:
        target = destination.joinpath(*PurePosixPath(name).parts)
        if not target.resolve().is_relative_to(destination):
            raise ValueError(f"test file escapes destination through a link: {name}")
        targets[name] = target
    preflight_targets(targets.values())
    for name, target in targets.items():
        target.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".otd-update-", delete=False) as staged:
            staged.write(payload[name])
            staged_path = Path(staged.name)
        try:
            os.replace(staged_path, target)
        finally:
            staged_path.unlink(missing_ok=True)
        if release.digest(target.read_bytes()) != release.digest(payload[name]):
            raise ValueError(f"installed test file hash mismatch: {name}")
    print(f"Updated {destination}: v{build['version']}, {build['branch']}, {build['source_commit']}")
    print("No driver was started or stopped. Saved settings were not edited.")


if __name__ == "__main__":
    try:
        main()
    except PermissionError as error:
        raise SystemExit(f"Cannot replace a test build file. Choose a separate --destination or close the test app before retrying: {error}") from error
