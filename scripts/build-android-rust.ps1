$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$output = Join-Path $root "android\app\src\main\jniLibs"
Push-Location $root
try {
    cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o $output build -p photo-backup-mobile --release
} finally {
    Pop-Location
}
