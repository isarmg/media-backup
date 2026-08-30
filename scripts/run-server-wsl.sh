#!/usr/bin/env bash
set -euo pipefail

readonly binary="/opt/isarmg/photo-backup/current/bin/photo-backup-server"
readonly config="/etc/isarmg/photo-backup.env"
readonly marker="# INITIAL-SECRETS-MUST-BE-REPLACED"

[[ "$EUID" -eq 0 ]] || {
  echo "Run this script as root (or with sudo)." >&2
  exit 1
}
[[ -x "$binary" ]] || {
  echo "Missing installed release binary; run scripts/setup-wsl.sh after building it." >&2
  exit 1
}
[[ -f "$config" && ! -L "$config" ]] || {
  echo "Missing regular production configuration: $config" >&2
  exit 1
}
[[ "$(stat -c '%a:%u:%g:%h' "$config")" == "600:0:0:1" ]] || {
  echo "Production configuration must be root-owned, mode 0600, and have one hard link." >&2
  exit 1
}
if grep -Fqx "$marker" "$config"; then
  echo "Replace the generated secrets in $config and remove the initial-secret marker first." >&2
  exit 1
fi

systemctl start photo-backup.service
exec journalctl --unit photo-backup.service --follow
