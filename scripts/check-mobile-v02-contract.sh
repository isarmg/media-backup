#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail() {
    echo "mobile v0.2 contract gate: $*" >&2
    exit 1
}

test ! -e crates/mobile-ffi/include/photo_backup.h \
    || fail "the unversioned C header still exists"
test -f crates/mobile-ffi/include/photo_backup_v0_2_r1.h \
    || fail "the v0.2 revision 1 C header is missing"

if rg -n --pcre2 'extern "C" fn pb_(?!v0_2_r1_)|@_silgen_name\("pb_(?!v0_2_r1_)|Java_com_example_photobackup_NativeBridge_|\bnative(Open|Close|Needs|Enqueue|Next|MarkUpload|MarkPart|MarkComplete|MarkFailed|Stats)\b' \
    crates/mobile-ffi/src ios/PhotoBackup android/app/src/main; then
    fail "an unversioned v0.1 native ABI entry point remains"
fi

if rg -n --pcre2 '^package com\.example\.photobackup$|applicationId = "com\.example\.photobackup"|PRODUCT_BUNDLE_IDENTIFIER: com\.example\.photobackup$' \
    android/app/src/main android/app/build.gradle.kts ios/PhotoBackup ios/project.yml; then
    fail "a v0.1 application or bundle namespace remains"
fi

if rg -n 'UserDefaults\.standard|"photo_backup_secure"|"agent\.sqlite"|"backup-staging"|"photo-backup-now"|"photo-backup-periodic"|"com\.example\.photobackup"|com\.example\.photobackup\.processing"|com\.example\.photobackup\.upload"' \
    android/app/src/main ios/PhotoBackup; then
    fail "production mobile code still names a v0.1 state namespace"
fi
if rg -n '"bearer_token"' \
    android/app/src/main/java/com/example/photobackup/SecureConfig.kt \
    ios/PhotoBackup/BackupCoordinator.swift ios/PhotoBackup/MobileContractV02.swift; then
    fail "a v0.1 persisted bearer-token key remains"
fi

temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
sed '/^#\[cfg(test)\]/,$d' crates/agent-core/src/lib.rs > "$temporary/agent-core.rs"
sed '/^#\[cfg(test)\]/,$d' crates/mobile-ffi/src/lib.rs > "$temporary/mobile-ffi.rs"
if rg -n 'master_key_b64|dedupe_key_b64' \
    "$temporary" android/app/src/main ios/PhotoBackup; then
    fail "production mobile code still accepts or emits v0.1 encryption keys"
fi

for required in \
    'photo-backup-mobile-v0.2-r1' \
    'agent-v0.2-r1.sqlite' \
    'backup-staging-v0.2-r1' \
    'pb_v0_2_r1_open' \
    'Java_com_example_photobackup_v02_NativeBridgeV02_openV02R1' \
    'com.example.photobackup.v02'; do
    rg -q --fixed-strings "$required" crates/mobile-ffi crates/agent-core android ios \
        || fail "required v0.2 contract marker is missing: $required"
done

echo "mobile v0.2 ABI and state-epoch static gate passed"
