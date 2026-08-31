#!/usr/bin/env bash
set -euo pipefail

base_url="${MEDIA_BACKUP_VERIFY_URL:-http://127.0.0.1:8080}"
forwarded_proto="${MEDIA_BACKUP_VERIFY_FORWARDED_PROTO:-https}"

health_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  "$base_url/health")"
admin_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "X-Forwarded-Proto: $forwarded_proto" "$base_url/admin")"

printf 'health=%s admin_page=%s\n' "$health_status" "$admin_status"
test "$health_status" = "200"
test "$admin_status" = "200"
