#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$ROOT/ios/Vendor"
rm -rf "$VENDOR/PhotoBackupRust.xcframework"
mkdir -p "$VENDOR/device/headers" "$VENDOR/simulator/headers"

cd "$ROOT"
cargo build -p photo-backup-mobile --release --target aarch64-apple-ios
cargo build -p photo-backup-mobile --release --target aarch64-apple-ios-sim
cp crates/mobile-ffi/include/photo_backup_v0_2_r1.h "$VENDOR/device/headers/"
cp crates/mobile-ffi/include/photo_backup_v0_2_r1.h "$VENDOR/simulator/headers/"

xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libphoto_backup_mobile.a \
  -headers "$VENDOR/device/headers" \
  -library target/aarch64-apple-ios-sim/release/libphoto_backup_mobile.a \
  -headers "$VENDOR/simulator/headers" \
  -output "$VENDOR/PhotoBackupRust.xcframework"
