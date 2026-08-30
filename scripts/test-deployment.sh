#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
setup_script="$project_dir/scripts/setup-wsl.sh"
unit_source="$project_dir/scripts/photo-backup.service"
test_root="$(mktemp -d)"

cleanup() {
  chmod -R u+rwX "$test_root" 2>/dev/null || true
  rm -rf -- "$test_root"
}
trap cleanup EXIT

fail() {
  printf 'deployment test failed: %s\n' "$*" >&2
  exit 1
}

assert_unit_setting() {
  grep -Fqx "$1" "$unit_source" || fail "missing unit setting: $1"
}

cargo_version="$({
  awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && /^version[[:space:]]*=/ {
      gsub(/^[^"]*"|".*$/, "")
      print
      exit
    }
  ' "$project_dir/Cargo.toml"
})"
[[ "$cargo_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "invalid Cargo version"

fake_binary="$test_root/photo-backup-server"
printf '#!/usr/bin/env bash\nexit 0\n' >"$fake_binary"
chmod 0755 "$fake_binary"

if PHOTO_BACKUP_SETUP_ROOT="$test_root/alternate-without-test-mode" \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >/dev/null 2>&1; then
  fail "alternate root was accepted outside test mode"
fi
if PHOTO_BACKUP_SETUP_ROOT=/ PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >/dev/null 2>&1; then
  fail "test mode was allowed to target the real root"
fi

install_root="$test_root/install-root"
mkdir "$install_root"
first_output="$test_root/first-output"
PHOTO_BACKUP_SETUP_ROOT="$install_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >"$first_output" 2>&1

release_dir="$install_root/opt/isarmg/photo-backup/releases/$cargo_version"
installed_binary="$release_dir/bin/photo-backup-server"
current_link="$install_root/opt/isarmg/photo-backup/current"
config="$install_root/etc/isarmg/photo-backup.env"
installed_unit="$install_root/etc/systemd/system/photo-backup.service"

[[ -x "$installed_binary" && ! -L "$installed_binary" ]] || fail "release binary was not installed"
[[ "$(stat -c '%a' "$installed_binary")" == "755" ]] || fail "release binary mode is not 0755"
[[ -L "$current_link" ]] || fail "current is not a symbolic link"
[[ "$(readlink "$current_link")" == "releases/$cargo_version" ]] ||
  fail "current does not use the managed relative release target"
[[ "$(readlink -f "$current_link")" == "$release_dir" ]] || fail "current resolves outside releases"
cmp --silent "$fake_binary" "$installed_binary" || fail "installed binary differs"
cmp --silent "$unit_source" "$installed_unit" || fail "installed unit differs"

[[ -f "$config" && ! -L "$config" ]] || fail "configuration is not a regular file"
[[ "$(stat -c '%a' "$config")" == "600" ]] || fail "configuration mode is not 0600"
grep -Fqx '# INITIAL-SECRETS-MUST-BE-REPLACED' "$config" || fail "initial-secret marker is missing"
grep -Fqx 'DATABASE_URL=sqlite:///var/lib/isarmg/photo-backup/db/app.db' "$config" ||
  fail "SQLite database path is incorrect"
grep -Fqx 'DATA_DIR=/var/lib/isarmg/photo-backup/data' "$config" || fail "data path is incorrect"
[[ -d "$install_root/var/lib/isarmg/photo-backup/db" ]] || fail "database directory is missing"
[[ -d "$install_root/var/lib/isarmg/photo-backup/data" ]] || fail "data directory is missing"

admin_secret="$(awk -F= '/^ADMIN_PASSWORD=/ { print $2 }' "$config")"
metrics_secret="$(awk -F= '/^METRICS_TOKEN=/ { print $2 }' "$config")"
[[ "$admin_secret" =~ ^[[:xdigit:]]{64}$ ]] || fail "admin secret is not 256-bit random hex"
[[ "$metrics_secret" =~ ^[[:xdigit:]]{64}$ ]] || fail "metrics secret is not 256-bit random hex"
[[ "$admin_secret" != "$metrics_secret" ]] || fail "independent secrets are equal"
if grep -Fq "$admin_secret" "$first_output" || grep -Fq "$metrics_secret" "$first_output"; then
  fail "setup output disclosed a generated secret"
fi

config_digest="$(sha256sum "$config" | awk '{ print $1 }')"
chmod 0644 "$config"
second_output="$test_root/second-output"
PHOTO_BACKUP_SETUP_ROOT="$install_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >"$second_output" 2>&1
[[ "$(sha256sum "$config" | awk '{ print $1 }')" == "$config_digest" ]] ||
  fail "idempotent setup overwrote configuration"
[[ "$(stat -c '%a' "$config")" == "600" ]] || fail "idempotent setup did not restore config mode"
grep -Fq 'Preserved existing /etc/isarmg/photo-backup.env' "$second_output" ||
  fail "idempotent setup did not report preservation"

different_binary="$test_root/different-photo-backup-server"
printf '#!/usr/bin/env bash\nexit 7\n' >"$different_binary"
chmod 0755 "$different_binary"
if PHOTO_BACKUP_SETUP_ROOT="$install_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$different_binary" "$setup_script" >/dev/null 2>&1; then
  fail "same Cargo version silently accepted different artifact content"
fi
cmp --silent "$fake_binary" "$installed_binary" || fail "immutable release was modified"

symlink_release_root="$test_root/symlink-release-root"
release_escape="$test_root/release-escape"
mkdir -p "$symlink_release_root/opt/isarmg/photo-backup" "$release_escape"
ln -s "$release_escape" "$symlink_release_root/opt/isarmg/photo-backup/releases"
if PHOTO_BACKUP_SETUP_ROOT="$symlink_release_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >/dev/null 2>&1; then
  fail "symlinked releases directory was accepted"
fi
[[ -z "$(find "$release_escape" -mindepth 1 -print -quit)" ]] || fail "release symlink escaped test root"

symlink_config_root="$test_root/symlink-config-root"
config_escape="$test_root/config-escape"
mkdir -p "$symlink_config_root/etc" "$config_escape"
ln -s "$config_escape" "$symlink_config_root/etc/isarmg"
if PHOTO_BACKUP_SETUP_ROOT="$symlink_config_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >/dev/null 2>&1; then
  fail "symlinked configuration directory was accepted"
fi
[[ -z "$(find "$config_escape" -mindepth 1 -print -quit)" ]] || fail "config symlink escaped test root"

malicious_current_root="$test_root/malicious-current-root"
mkdir -p "$malicious_current_root/opt/isarmg/photo-backup"
ln -s /tmp "$malicious_current_root/opt/isarmg/photo-backup/current"
if PHOTO_BACKUP_SETUP_ROOT="$malicious_current_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >/dev/null 2>&1; then
  fail "an unmanaged current symlink was accepted"
fi

special_config_root="$test_root/special-config-root"
mkdir -p "$special_config_root/etc/isarmg"
mkfifo "$special_config_root/etc/isarmg/photo-backup.env"
if PHOTO_BACKUP_SETUP_ROOT="$special_config_root" PHOTO_BACKUP_SETUP_TEST=1 \
  PHOTO_BACKUP_BINARY="$fake_binary" "$setup_script" >/dev/null 2>&1; then
  fail "a special configuration target was accepted"
fi

for setting in \
  'User=isarmg-photo' \
  'Group=isarmg-photo' \
  'UMask=0077' \
  'StateDirectory=isarmg/photo-backup' \
  'RuntimeDirectory=isarmg/photo-backup' \
  'EnvironmentFile=/etc/isarmg/photo-backup.env' \
  'ExecStart=/opt/isarmg/photo-backup/current/bin/photo-backup-server serve' \
  'ReadWritePaths=/var/lib/isarmg/photo-backup /run/isarmg/photo-backup' \
  'ProtectSystem=strict' \
  'ProtectHome=true' \
  'NoNewPrivileges=true' \
  'PrivateTmp=true' \
  'PrivateDevices=true' \
  'ProtectKernelTunables=true' \
  'ProtectKernelModules=true' \
  'ProtectKernelLogs=true' \
  'ProtectControlGroups=true' \
  'RestrictSUIDSGID=true' \
  'LockPersonality=true'; do
  assert_unit_setting "$setting"
done

if grep -Eqi 'postgres|/mnt/|User=root' "$unit_source" "$setup_script" \
  "$project_dir/scripts/run-server-wsl.sh" "$project_dir/scripts/start-server-wsl.sh"; then
  fail "deployment files retain PostgreSQL, source-mount, or root-service configuration"
fi
if grep -Fq 'project_dir/.env' "$setup_script" "$project_dir/scripts/run-server-wsl.sh" \
  "$project_dir/scripts/start-server-wsl.sh" "$project_dir/scripts/verify-server-wsl.sh"; then
  fail "deployment scripts still depend on a source-tree .env"
fi
if grep -Eq 'sed[[:space:]]+-i|systemctl[[:space:]]+(enable|start|restart)' "$setup_script"; then
  fail "setup mutates secrets with sed or starts the service"
fi

bash -n "$setup_script" "$project_dir/scripts/run-server-wsl.sh" \
  "$project_dir/scripts/start-server-wsl.sh" "$project_dir/scripts/verify-server-wsl.sh"
if command -v shellcheck >/dev/null; then
  shellcheck "$setup_script" "$project_dir/scripts/run-server-wsl.sh" \
    "$project_dir/scripts/start-server-wsl.sh" "$project_dir/scripts/verify-server-wsl.sh" "$0"
fi
if [[ "${PHOTO_BACKUP_VERIFY_SYSTEMD:-0}" == "1" ]]; then
  command -v systemd-analyze >/dev/null || fail "systemd-analyze is required for unit verification"
  if ! systemd-analyze --root="$install_root" --recursive-errors=no verify "$installed_unit" \
    >"$test_root/systemd-verify" 2>&1; then
    cat "$test_root/systemd-verify" >&2
    fail "systemd unit verification failed"
  fi
fi

printf 'deployment scripts passed static and temporary-root tests\n'
