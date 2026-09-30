#!/usr/bin/env bash
# Windows release artifacts from Linux; no system tool installation or CI.
set -euo pipefail
: "${MINGW_ROOT:?Set MINGW_ROOT to a GNU MinGW-w64 toolchain prefix}"
: "${NETHOST_DLL:?Set NETHOST_DLL to nethost.dll from the Windows x64 .NET 8 SDK/host pack}"
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
export PATH="$MINGW_ROOT/bin:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$MINGW_ROOT/bin/x86_64-w64-mingw32-gcc"
export WINDRES="$MINGW_ROOT/bin/x86_64-w64-mingw32-windres"
python3 scripts/release.py build --platform win-x64
