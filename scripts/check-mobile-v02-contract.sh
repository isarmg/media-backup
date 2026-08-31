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

if grep -R -I -n -E 'Java_org_sarmg_mediabackup_NativeBridge_|native(Open|Close|Needs|Enqueue|Next|MarkUpload|MarkPart|MarkComplete|MarkFailed|Stats)([^[:alnum:]_]|$)' \
    crates/mobile-ffi/src clients/ios/MediaBackup clients/android/app/src/main; then
    fail "a non-current unversioned native ABI entry point remains"
fi

if grep -R -I -n -E 'com[.]example|Java_com_example_' \
    crates/mobile-ffi/src clients/android/app/src clients/android/app/build.gradle.kts clients/ios; then
    fail "a development placeholder application identity remains"
fi

for required in \
    'media-backup-mobile-v0.2-r1' \
    'agent-v0.2-r1.sqlite' \
    'backup-staging-v0.2-r1' \
    'mb_v0_2_r1_open' \
    'Java_org_sarmg_mediabackup_NativeBridgeV02_openV02R1' \
    'org.sarmg.mediabackup'; do
    # All platform clients live below clients/; keeping this gate on the
    # canonical paths makes directory drift fail visibly in CI.
    grep -R -I -q -F "$required" crates/mobile-ffi crates/agent-core clients/android clients/ios \
        || fail "required v0.2 contract marker is missing: $required"
done

grep -q -F 'MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_BASE64' .github/workflows/release.yml \
    || fail "the formal Android release does not require the current PKCS#12 Secret"
grep -q -F '0cfc2811d48cdeab3e6d857029d879e001ab9531c06784b4d48d15a847771421' \
    .github/workflows/release.yml \
    || fail "the formal Android release does not pin the current certificate fingerprint"
grep -q -F 'assembleRelease' .github/workflows/release.yml \
    || fail "the formal Android release is not a signed release APK build"
if grep -q -E 'assembleDebug|app-debug[.]apk|PHOTO_ANDROID_' .github/workflows/release.yml; then
    fail "the formal release workflow still contains a debug or old Android signing path"
fi

echo "mobile v0.2 ABI and state-epoch static gate passed"
