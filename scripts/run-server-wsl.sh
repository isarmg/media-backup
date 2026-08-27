#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"

if [[ ! -f .env ]]; then
  echo "Missing $project_dir/.env; run scripts/setup-wsl.sh first." >&2
  exit 1
fi

set -a
# shellcheck disable=SC1091
source .env
set +a

exec cargo run --release -p photo-backup-server
