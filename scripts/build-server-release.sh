#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly project_dir
readonly binary="${1:-}"
readonly source_revision="${2:-}"
readonly output_arg="${3:-}"
readonly version="0.2.0"
readonly target="x86_64-unknown-linux-gnu"
readonly package="photo-backup-server-$version-$target"
readonly release_contract_sha256="3a65b1a129118beeafe552c42d27812320d08bdde7966cedc9ac1e5476e995e9"

staging_root=""
archive_staging=""
output_dir=""

fail() {
  printf 'release build error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [[ -n "$staging_root" && -d "$staging_root" && ! -L "$staging_root" ]]; then
    case "$staging_root" in
      "$output_dir"/.server-release-*)
        [[ -n "$output_dir" ]] && rm -rf -- "$staging_root"
        ;;
      *) printf 'release build error: refusing unexpected staging cleanup\n' >&2 ;;
    esac
  fi
  if [[ -n "$archive_staging" && -f "$archive_staging" && ! -L "$archive_staging" ]]; then
    case "$archive_staging" in
      "$output_dir"/.photo-backup-server-*.tmp)
        [[ -n "$output_dir" ]] && rm -f -- "$archive_staging"
        ;;
      *) printf 'release build error: refusing unexpected archive cleanup\n' >&2 ;;
    esac
  fi
}
trap cleanup EXIT

[[ -n "$binary" && -n "$source_revision" && -n "$output_arg" ]] ||
  fail "usage: build-server-release.sh BINARY SOURCE_REVISION OUTPUT_DIRECTORY"
[[ "$binary" = /* && -f "$binary" && -x "$binary" && ! -L "$binary" ]] ||
  fail "BINARY must be an absolute executable regular file"
[[ "$source_revision" =~ ^[0-9a-f]{40}$ ]] ||
  fail "SOURCE_REVISION must be 40 lowercase hexadecimal characters"
[[ "$output_arg" = /* ]] || fail "OUTPUT_DIRECTORY must be absolute"
if [[ -e "$output_arg" || -L "$output_arg" ]]; then
  [[ -d "$output_arg" && ! -L "$output_arg" ]] || fail "output path must be a real directory"
else
  mkdir -m 0755 -- "$output_arg"
fi
output_dir="$(cd "$output_arg" && pwd -P)"
archive="$output_dir/$package.tar.gz"
[[ ! -e "$archive" && ! -L "$archive" ]] || fail "release archive already exists: $archive"

staging_root="$(mktemp -d -- "$output_dir/.server-release-XXXXXX")"
release_root="$staging_root/$package"
mkdir -m 0755 -- \
  "$release_root" \
  "$release_root/bin" \
  "$release_root/config" \
  "$release_root/docs" \
  "$release_root/include" \
  "$release_root/scripts" \
  "$release_root/share" \
  "$release_root/share/web" \
  "$release_root/systemd"

install -m 0755 -- "$binary" "$release_root/bin/photo-backup-server"
install -m 0755 -- \
  "$project_dir/scripts/setup-wsl.sh" \
  "$project_dir/scripts/start-server-wsl.sh" \
  "$project_dir/scripts/run-server-wsl.sh" \
  "$project_dir/scripts/verify-server-wsl.sh" \
  "$release_root/scripts/"
install -m 0644 -- "$project_dir/scripts/photo-backup.service" \
  "$release_root/systemd/photo-backup.service"
install -m 0644 -- "$project_dir/.env.example" \
  "$release_root/config/photo-backup.env.example"
install -m 0644 -- "$project_dir/docs/SERVER_RELEASE.md" "$release_root/README.md"
install -m 0644 -- "$project_dir/IMMICH_COMPARISON.md" \
  "$release_root/docs/IMMICH_COMPARISON.md"
install -m 0644 -- "$project_dir/LICENSE" "$release_root/LICENSE"
install -m 0644 -- "$project_dir/crates/mobile-ffi/include/photo_backup_v0_2_r1.h" \
  "$release_root/include/photo_backup_v0_2_r1.h"
install -m 0644 -- "$project_dir/crates/server/src/admin.html" \
  "$release_root/share/web/admin.html"
install -m 0644 -- "$project_dir/crates/server/src/admin.css" \
  "$release_root/share/web/admin.css"
install -m 0644 -- "$project_dir/vendor/sarmg-design/bundle.css" \
  "$release_root/share/web/sarmg-design.css"

python3 "$project_dir/scripts/write-release-manifest.py" "$release_root" "$source_revision"
verification="$("$release_root/bin/photo-backup-server" release-verify "$release_root")" ||
  fail "the real server binary rejected the assembled release"
[[ "$verification" == PHOTO_BACKUP_RELEASE_VERIFIED_V1$'\tphoto-backup-server\t0.2.0\t'"$source_revision"$'\tx86_64-unknown-linux-gnu\t'"$release_contract_sha256" ]] ||
  fail "release verification returned an unexpected identity"

archive_staging="$output_dir/.$package.tar.gz.$BASHPID.tmp"
(
  umask 022
  set -o noclobber
  tar -C "$staging_root" --sort=name --mtime='@0' --owner=0 --group=0 \
    --numeric-owner --format=posix --pax-option=delete=atime,delete=ctime \
    -cf - "$package" | gzip -n >"$archive_staging"
)
chmod 0644 "$archive_staging"
mv -T -n -- "$archive_staging" "$archive"
[[ ! -e "$archive_staging" ]] || fail "release archive path appeared concurrently"
archive_staging=""
[[ -f "$archive" && ! -L "$archive" && "$(stat -c '%a:%h' -- "$archive")" == "644:1" ]] ||
  fail "failed to install a single-link immutable release archive"

printf '%s\n' "$archive"
