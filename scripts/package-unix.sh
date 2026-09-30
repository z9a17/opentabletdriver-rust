#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
case "${1:-}" in
  linux-x64|macos-x64|macos-arm64) ;;
  *) echo 'Usage: scripts/package-unix.sh linux-x64|macos-x64|macos-arm64' >&2; exit 2 ;;
esac
python3 scripts/release.py build --platform "$1"
