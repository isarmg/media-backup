#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$ROOT/clients/ios/Vendor"
rm -rf "$VENDOR/MediaBackupRust.xcframework"
mkdir -p "$VENDOR/device/headers" "$VENDOR/simulator/headers"

cd "$ROOT"
cargo build -p media-backup-mobile --release --target aarch64-apple-ios
cargo build -p media-backup-mobile --release --target aarch64-apple-ios-sim
cp crates/mobile-ffi/include/media_backup_ffi_v2.h crates/mobile-ffi/include/module.modulemap "$VENDOR/device/headers/"
cp crates/mobile-ffi/include/media_backup_ffi_v2.h crates/mobile-ffi/include/module.modulemap "$VENDOR/simulator/headers/"

xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libmedia_backup_mobile.a \
  -headers "$VENDOR/device/headers" \
  -library target/aarch64-apple-ios-sim/release/libmedia_backup_mobile.a \
  -headers "$VENDOR/simulator/headers" \
  -output "$VENDOR/MediaBackupRust.xcframework"
