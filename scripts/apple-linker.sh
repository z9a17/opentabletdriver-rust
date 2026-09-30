#!/usr/bin/env bash
set -euo pipefail
: "${SDKROOT:?Set SDKROOT to an extracted macOS SDK}"
: "${OTD_APPLE_TARGET:?Set OTD_APPLE_TARGET to x86_64-apple-darwin or aarch64-apple-darwin}"
rustc_path="${RUSTC:-rustc}"
linker_root="$($rustc_path --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld"
sdk_version=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["Version"])' "$SDKROOT/SDKSettings.json")
deployment="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
exec clang --target="$OTD_APPLE_TARGET" -Wl,-platform_version,macos,"$deployment","$sdk_version" -isysroot "$SDKROOT" \
  -fuse-ld="$linker_root/ld64.lld" "$@"
