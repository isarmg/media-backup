#!/usr/bin/env bash
set -euo pipefail
ulimit -c 0
project_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_dir"
command -v javac >/dev/null
command -v java >/dev/null
CARGO_INCREMENTAL=0 cargo build --locked -p media-backup-mobile --features jni-host-tests
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
javac -encoding UTF-8 -d "$scratch" crates/mobile-ffi/tests/jni/NativeBridgeV2.java
java -Djava.library.path="$project_dir/target/debug" -cp "$scratch" \
  org.sarmg.mediabackup.NativeBridgeV2 "$scratch" 2>&1 | tee "$scratch/jni.log"
if rg -q 'private-secret' "$scratch/jni.log"; then
  echo "JNI leaked a private input or panic payload" >&2
  exit 1
fi
