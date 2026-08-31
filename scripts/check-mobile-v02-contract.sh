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

if grep -R -I -n -E 'Java_com_example_mediabackup_NativeBridge_|native(Open|Close|Needs|Enqueue|Next|MarkUpload|MarkPart|MarkComplete|MarkFailed|Stats)([^[:alnum:]_]|$)' \
    crates/mobile-ffi/src clients/ios/MediaBackup clients/android/app/src/main; then
    fail "a non-current unversioned native ABI entry point remains"
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
