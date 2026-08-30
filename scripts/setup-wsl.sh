#!/usr/bin/env bash
set -euo pipefail

readonly service_user="isarmg-photo"
readonly service_group="isarmg-photo"
readonly app_dir="/opt/isarmg/photo-backup"
readonly releases_dir="$app_dir/releases"
readonly current_link="$app_dir/current"
readonly state_dir="/var/lib/isarmg/photo-backup"
readonly config_file="/etc/isarmg/photo-backup.env"
readonly unit_file="/etc/systemd/system/photo-backup.service"
readonly initial_secret_marker="# INITIAL-SECRETS-MUST-BE-REPLACED"

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
source_binary="${PHOTO_BACKUP_BINARY:-$project_dir/target/release/photo-backup-server}"
setup_root="${PHOTO_BACKUP_SETUP_ROOT:-/}"
test_mode="${PHOTO_BACKUP_SETUP_TEST:-0}"
release_staging=""
current_staging=""

die() {
  printf 'setup error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [[ -n "$current_staging" && -L "$current_staging" ]]; then
    rm -f -- "$current_staging"
  fi
  if [[ -n "$release_staging" && -d "$release_staging" && ! -L "$release_staging" ]]; then
    case "$release_staging" in
      "$(rooted "$releases_dir")"/.install-*) rm -rf -- "$release_staging" ;;
      *) printf 'setup error: refusing to clean unexpected staging path\n' >&2 ;;
    esac
  fi
}
trap cleanup EXIT

if [[ "$test_mode" == "1" ]]; then
  [[ "$setup_root" != "/" ]] || die "test mode refuses the real filesystem root"
elif [[ "$test_mode" == "0" ]]; then
  [[ "$setup_root" == "/" ]] || die "an alternate root is allowed only in test mode"
  [[ "$EUID" -eq 0 ]] || die "run this setup script as root (or with sudo)"
else
  die "PHOTO_BACKUP_SETUP_TEST must be 0 or 1"
fi

[[ "$setup_root" = /* ]] || die "PHOTO_BACKUP_SETUP_ROOT must be absolute"
[[ -d "$setup_root" && ! -L "$setup_root" ]] || die "setup root must be a real directory"
setup_root="$(cd "$setup_root" && pwd -P)"
[[ -f "$source_binary" && -x "$source_binary" && ! -L "$source_binary" ]] ||
  die "missing regular release binary: $source_binary"
[[ -f "$project_dir/scripts/photo-backup.service" &&
  ! -L "$project_dir/scripts/photo-backup.service" ]] || die "service unit source is invalid"

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
[[ "$cargo_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  die "Cargo workspace version must be a validated MAJOR.MINOR.PATCH value"

rooted() {
  if [[ "$setup_root" == "/" ]]; then
    printf '%s\n' "$1"
  else
    printf '%s%s\n' "$setup_root" "$1"
  fi
}

ensure_directory_chain() {
  local logical_path="$1"
  local prefix=""
  local actual
  local component
  local -a components
  [[ "$logical_path" = /* && "$logical_path" != *".."* ]] ||
    die "invalid installation directory path"
  IFS='/' read -r -a components <<<"${logical_path#/}"
  for component in "${components[@]}"; do
    [[ -n "$component" && "$component" != "." && "$component" != ".." ]] ||
      die "invalid installation directory component"
    prefix="$prefix/$component"
    actual="$(rooted "$prefix")"
    if [[ -e "$actual" || -L "$actual" ]]; then
      [[ -d "$actual" && ! -L "$actual" ]] ||
        die "installation directory chain contains a symlink or special entry: $prefix"
    else
      mkdir -m 0755 -- "$actual"
    fi
  done
}

ensure_single_link_regular_file() {
  local path="$1"
  local label="$2"
  [[ -f "$path" && ! -L "$path" ]] || die "$label must be a regular file"
  [[ "$(stat -c '%h' -- "$path")" == "1" ]] || die "$label must not have hard-link aliases"
}

ensure_root_owned_directory() {
  local path="$1"
  local label="$2"
  local mode
  [[ "$(stat -c '%u:%g' -- "$path")" == "0:0" ]] || die "$label must be owned by root"
  mode="$(stat -c '%a' -- "$path")"
  (( (8#$mode & 0022) == 0 )) || die "$label must not be writable by group or other"
}

validate_release_layout() {
  local release_path="$1"
  local release_bin="$release_path/bin"
  local installed_binary="$release_bin/photo-backup-server"
  local -a release_entries
  local -a bin_entries
  [[ -d "$release_path" && ! -L "$release_path" ]] ||
    die "release target is not a real directory"
  [[ -d "$release_bin" && ! -L "$release_bin" ]] || die "release bin is not a real directory"
  ensure_single_link_regular_file "$installed_binary" "installed release binary"
  [[ -x "$installed_binary" ]] || die "installed release binary is not executable"
  release_entries=("$release_path"/*)
  bin_entries=("$release_bin"/*)
  [[ "${#release_entries[@]}" == "1" && "${release_entries[0]}" == "$release_bin" ]] ||
    die "release directory contains unexpected entries"
  [[ "${#bin_entries[@]}" == "1" && "${bin_entries[0]}" == "$installed_binary" ]] ||
    die "release bin contains unexpected entries"
  [[ "$(stat -c '%a' -- "$release_path")" == "755" &&
    "$(stat -c '%a' -- "$release_bin")" == "755" &&
    "$(stat -c '%a' -- "$installed_binary")" == "755" ]] ||
    die "release permissions differ from the immutable layout"
  if [[ "$test_mode" == "0" ]]; then
    [[ "$(stat -c '%u:%g' -- "$release_path")" == "0:0" &&
      "$(stat -c '%u:%g' -- "$release_bin")" == "0:0" &&
      "$(stat -c '%u:%g' -- "$installed_binary")" == "0:0" ]] ||
      die "release layout must be owned by root"
  fi
}

validate_release() {
  local release_path="$1"
  local installed_binary="$release_path/bin/photo-backup-server"
  validate_release_layout "$release_path"
  cmp --silent -- "$source_binary" "$installed_binary" ||
    die "release $cargo_version already exists with different binary content"
}

validate_current_link() {
  local current_path="$1"
  local target
  local target_version
  if [[ ! -e "$current_path" && ! -L "$current_path" ]]; then
    return
  fi
  [[ -L "$current_path" ]] || die "current must be an installer-managed symbolic link"
  target="$(readlink -- "$current_path")"
  [[ "$target" =~ ^releases/([0-9]+\.[0-9]+\.[0-9]+)$ ]] ||
    die "current points outside the managed releases directory"
  target_version="${BASH_REMATCH[1]}"
  validate_release_layout "$(rooted "$releases_dir/$target_version")"
}

random_hex_256() {
  local value
  value="$(LC_ALL=C od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  [[ "$value" =~ ^[[:xdigit:]]{64}$ ]] || die "failed to generate a 256-bit secret"
  printf '%s\n' "$value"
}

shopt -s nullglob dotglob

ensure_directory_chain "$releases_dir"
ensure_directory_chain "/etc/isarmg"
ensure_directory_chain "/etc/systemd/system"
ensure_directory_chain "$state_dir/db"
ensure_directory_chain "$state_dir/data"

releases_path="$(rooted "$releases_dir")"
release_path="$(rooted "$releases_dir/$cargo_version")"
current_path="$(rooted "$current_link")"
state_path="$(rooted "$state_dir")"
config_path="$(rooted "$config_file")"
unit_path="$(rooted "$unit_file")"

if [[ "$test_mode" == "0" ]]; then
  ensure_root_owned_directory /opt "/opt"
  ensure_root_owned_directory /opt/isarmg "/opt/isarmg"
  ensure_root_owned_directory "$(rooted "$app_dir")" "$app_dir"
  ensure_root_owned_directory "$releases_path" "$releases_dir"
  ensure_root_owned_directory /etc "/etc"
  ensure_root_owned_directory /etc/isarmg "/etc/isarmg"
  ensure_root_owned_directory /etc/systemd/system "/etc/systemd/system"
  ensure_root_owned_directory /var/lib/isarmg "/var/lib/isarmg"
fi

validate_current_link "$current_path"
if [[ -e "$release_path" || -L "$release_path" ]]; then
  validate_release "$release_path"
fi
if [[ -e "$config_path" || -L "$config_path" ]]; then
  ensure_single_link_regular_file "$config_path" "configuration"
fi
if [[ -e "$unit_path" || -L "$unit_path" ]]; then
  ensure_single_link_regular_file "$unit_path" "systemd unit"
fi

if [[ "$test_mode" == "0" ]]; then
  service_home=""
  service_shell=""
  if ! getent group "$service_group" >/dev/null; then
    groupadd --system "$service_group"
  fi
  if ! id -u "$service_user" >/dev/null 2>&1; then
    useradd --system \
      --gid "$service_group" \
      --home-dir "$state_dir" \
      --no-create-home \
      --shell /usr/sbin/nologin \
      "$service_user"
  fi
  [[ "$(id -u "$service_user")" != "0" ]] || die "service account must not be root"
  [[ "$(id -g "$service_user")" == "$(getent group "$service_group" | cut -d: -f3)" ]] ||
    die "existing service account does not use the dedicated group"
  IFS=: read -r _ _ _ _ _ service_home service_shell < <(getent passwd "$service_user")
  [[ "$service_home" == "$state_dir" ]] || die "existing service account has the wrong home"
  [[ "$service_shell" == "/usr/sbin/nologin" || "$service_shell" == "/sbin/nologin" ]] ||
    die "existing service account must use a nologin shell"
fi

chmod 0755 "$(rooted "$app_dir")" "$releases_path"
if [[ ! -e "$release_path" ]]; then
  release_staging="$(mktemp -d -- "$releases_path/.install-$cargo_version.XXXXXX")"
  chmod 0755 "$release_staging"
  mkdir -m 0755 -- "$release_staging/bin"
  install -m 0755 "$source_binary" "$release_staging/bin/photo-backup-server"
  if mv -T -n -- "$release_staging" "$release_path"; then
    if [[ ! -e "$release_staging" ]]; then
      release_staging=""
    fi
  fi
fi
validate_release "$release_path"

chmod 0750 "$state_path" "$state_path/db" "$state_path/data"
if [[ "$test_mode" == "0" ]]; then
  chown "$service_user:$service_group" "$state_path" "$state_path/db" "$state_path/data"
fi

config_created=0
if [[ -e "$config_path" || -L "$config_path" ]]; then
  ensure_single_link_regular_file "$config_path" "configuration"
else
  admin_password="$(random_hex_256)"
  metrics_token="$(random_hex_256)"
  if (
    umask 077
    set -o noclobber
    printf '%s\n' \
      "$initial_secret_marker" \
      '# Replace both generated secrets, then remove the marker above before first start.' \
      'DATABASE_URL=sqlite:///var/lib/isarmg/photo-backup/db/app.db' \
      'DATA_DIR=/var/lib/isarmg/photo-backup/data' \
      'BIND=127.0.0.1:8080' \
      'ADMIN_USERNAME=admin' \
      "ADMIN_PASSWORD=$admin_password" \
      'MAX_PART_BYTES=67108864' \
      'REQUIRE_HTTPS=true' \
      'DEVELOPMENT=false' \
      'TRUSTED_PROXY_CIDRS=127.0.0.1/32,::1/128' \
      'ADMIN_SESSION_IDLE_SECONDS=1800' \
      'ADMIN_SESSION_ABSOLUTE_SECONDS=43200' \
      "METRICS_TOKEN=$metrics_token" \
      'RUST_LOG=photo_backup_server=info,tower_http=info' >"$config_path"
  ) 2>/dev/null; then
    config_created=1
  elif [[ ! -f "$config_path" || -L "$config_path" ]]; then
    die "could not create configuration without overwriting an existing path"
  fi
  unset admin_password metrics_token
  ensure_single_link_regular_file "$config_path" "configuration"
fi
chmod 0600 "$config_path"
if [[ "$test_mode" == "0" ]]; then
  chown root:root "$config_path"
fi

install -m 0644 "$project_dir/scripts/photo-backup.service" "$unit_path"
ensure_single_link_regular_file "$unit_path" "systemd unit"

desired_target="releases/$cargo_version"
if [[ ! -L "$current_path" || "$(readlink -- "$current_path")" != "$desired_target" ]]; then
  for attempt in {1..20}; do
    candidate="$(rooted "$app_dir/.current-$BASHPID-$RANDOM-$attempt")"
    if ln -s -- "$desired_target" "$candidate" 2>/dev/null; then
      current_staging="$candidate"
      break
    fi
  done
  [[ -n "$current_staging" ]] || die "could not create an atomic current-link candidate"
  mv -Tf -- "$current_staging" "$current_path"
  current_staging=""
fi

if [[ "$test_mode" == "0" ]]; then
  systemctl daemon-reload
fi

if [[ "$config_created" == "1" ]]; then
  printf 'Created %s with private random initial secrets; values were not printed.\n' "$config_file"
else
  printf 'Preserved existing %s without changing its contents.\n' "$config_file"
fi
printf 'Before first start, use sudoedit %s to replace both generated secrets and remove %s.\n' \
  "$config_file" "$initial_secret_marker"
printf 'Installed Photo Backup %s; the service was not started.\n' "$cargo_version"
