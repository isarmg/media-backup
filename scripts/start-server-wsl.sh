#!/usr/bin/env bash
set -euo pipefail

if [[ ! -x /mnt/sarmg.org/photo-backup/target/release/photo-backup-server ]]; then
  echo "Missing release binary; run: cargo build --release -p photo-backup-server" >&2
  exit 1
fi

systemctl enable --now photo-backup.service
systemctl --no-pager --full status photo-backup.service
