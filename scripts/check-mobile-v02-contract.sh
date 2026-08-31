#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail() {
    echo "mobile v0.2 contract gate: $*" >&2
    exit 1
}

test ! -e crates/mobile-ffi/include/media_backup.h \
    || fail "the unversioned C header still exists"
test -f crates/mobile-ffi/include/media_backup_v0_2_r1.h \
    || fail "the v0.2 revision 1 C header is missing"
for required_ios_property in \
    'NSPhotoLibraryUsageDescription:' \
    'NSPhotoLibraryAddUsageDescription:' \
    'BGTaskSchedulerPermittedIdentifiers:'; do
    grep -q -F "$required_ios_property" clients/ios/project.yml \
        || fail "the generated iOS Info.plist contract is missing: $required_ios_property"
done

if grep -R -I -n -E 'extern "C" fn pb_|@_silgen_name\("pb_' \
    crates/mobile-ffi/src clients/ios/MediaBackup clients/android/app/src/main \
    | grep -v -E 'extern "C" fn mb_v0_2_r1_|@_silgen_name\("mb_v0_2_r1_'; then
    fail "an unversioned v0.1 native ABI entry point remains"
fi
if grep -R -I -n -E 'Java_com_example_mediabackup_NativeBridge_|native(Open|Close|Needs|Enqueue|Next|MarkUpload|MarkPart|MarkComplete|MarkFailed|Stats)([^[:alnum:]_]|$)' \
    crates/mobile-ffi/src clients/ios/MediaBackup clients/android/app/src/main; then
    fail "an unversioned v0.1 native ABI entry point remains"
fi

if grep -R -I -n -E '^package com\.example\.mediabackup$|applicationId = "com\.example\.mediabackup"|PRODUCT_BUNDLE_IDENTIFIER: com\.example\.mediabackup$' \
    clients/android/app/src/main clients/android/app/build.gradle.kts clients/ios/MediaBackup clients/ios/project.yml; then
    fail "a v0.1 application or bundle namespace remains"
fi

if grep -R -I -n -E 'UserDefaults\.standard|"media_backup_secure"|"agent\.sqlite"|"backup-staging"|"media-backup-now"|"media-backup-periodic"|"com\.example\.mediabackup"|com\.example\.mediabackup\.processing"|com\.example\.mediabackup\.upload"' \
    clients/android/app/src/main clients/ios/MediaBackup; then
    fail "production mobile code still names a v0.1 state namespace"
fi
if grep -n '"bearer_token"' \
    clients/android/app/src/main/java/com/example/mediabackup/SecureConfig.kt \
    clients/ios/MediaBackup/BackupCoordinator.swift clients/ios/MediaBackup/MobileContractV02.swift; then
    fail "a v0.1 persisted bearer-token key remains"
fi

temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
sed '/^#\[cfg(test)\]/,$d' crates/agent-core/src/lib.rs > "$temporary/agent-core.rs"
sed '/^#\[cfg(test)\]/,$d' crates/mobile-ffi/src/lib.rs > "$temporary/mobile-ffi.rs"
if grep -R -I -n -E 'master_key_b64|dedupe_key_b64' \
    "$temporary" clients/android/app/src/main clients/ios/MediaBackup; then
    fail "production mobile code still accepts or emits v0.1 encryption keys"
fi

for required in \
    'media-backup-mobile-v0.2-r1' \
    'agent-v0.2-r1.sqlite' \
    'backup-staging-v0.2-r1' \
    'mb_v0_2_r1_open' \
    'Java_com_example_mediabackup_v02_NativeBridgeV02_openV02R1' \
    'com.example.mediabackup.v02'; do
    # All platform clients live below clients/; keeping this gate on the
    # canonical paths makes directory drift fail visibly in CI.
    grep -R -I -q -F "$required" crates/mobile-ffi crates/agent-core clients/android clients/ios \
        || fail "required v0.2 contract marker is missing: $required"
done

echo "mobile v0.2 ABI and state-epoch static gate passed"
