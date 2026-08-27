#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run this setup script as root (or with sudo)." >&2
  exit 1
fi

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

service postgresql start

if ! runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_roles WHERE rolname = 'photo_backup'" | grep -q 1; then
  runuser -u postgres -- psql -c "CREATE ROLE photo_backup LOGIN PASSWORD 'photo_backup'"
fi

if ! runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_database WHERE datname = 'photo_backup'" | grep -q 1; then
  runuser -u postgres -- createdb -O photo_backup photo_backup
fi

if [[ ! -f "$project_dir/.env" ]]; then
  cp "$project_dir/.env.example" "$project_dir/.env"
fi

sed -i '/^SETUP_TOKEN=/d;/^ADMIN_TOKEN=/d' "$project_dir/.env"
if ! grep -q '^ADMIN_USERNAME=' "$project_dir/.env"; then
  printf '\nADMIN_USERNAME=admin\n' >> "$project_dir/.env"
fi
if ! grep -q '^ADMIN_PASSWORD=' "$project_dir/.env"; then
  admin_password="$(openssl rand -base64 36 | tr -d '\n')"
  printf 'ADMIN_PASSWORD=%s\n' "$admin_password" >> "$project_dir/.env"
  echo "Created a random ADMIN_PASSWORD in $project_dir/.env."
fi
chmod 600 "$project_dir/.env"

chmod +x "$project_dir/scripts/run-server-wsl.sh" "$project_dir/scripts/start-server-wsl.sh"
install -m 0644 "$project_dir/scripts/photo-backup.service" /etc/systemd/system/photo-backup.service
systemctl daemon-reload
echo "WSL setup is ready."
