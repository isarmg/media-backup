#!/usr/bin/env bash
set -euo pipefail
ulimit -c 0
project_dir="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_dir"
python3 scripts/check-mobile-header.py
CARGO_INCREMENTAL=0 cargo build --locked -p media-backup-mobile
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
cc -std=c17 -Wall -Wextra -Werror -I crates/mobile-ffi/include \
  crates/mobile-ffi/tests/abi.c -L target/debug -lmedia_backup_mobile \
  -Wl,-rpath,"$project_dir/target/debug" -o "$scratch/abi"
"$scratch/abi" "$scratch"
if nm -D --defined-only target/debug/libmedia_backup_mobile.so | rg 'mb_v0_2_r1|sarmg_ffi_.*_v1|panicProbe'; then
  echo "removed FFI symbols remain" >&2
  exit 1
fi
