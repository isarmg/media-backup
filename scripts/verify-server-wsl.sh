#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$project_dir/.env"
set +a

cookie_file="$(mktemp)"
trap 'rm -f "$cookie_file"' EXIT

login_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --cookie-jar "$cookie_file" \
  --header 'Content-Type: application/json' \
  --data "{\"username\":\"$ADMIN_USERNAME\",\"password\":\"$ADMIN_PASSWORD\"}" \
  http://127.0.0.1:8080/admin/api/login)"

overview_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --cookie "$cookie_file" http://127.0.0.1:8080/admin/api/overview)"

page_fields="$(curl --silent http://127.0.0.1:8080/admin \
  | grep -Eo 'id="admin(Username|Password)"' \
  | wc -l)"

client_rejection="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header 'Content-Type: application/json' \
  --data '{"username":"missing-user","password":"incorrect-password","device_name":"verification","platform":"test"}' \
  http://127.0.0.1:8080/v1/auth/bootstrap)"

printf 'admin_login=%s overview=%s account_fields=%s invalid_client_login=%s\n' \
  "$login_status" "$overview_status" "$page_fields" "$client_rejection"

test "$login_status" = "204"
test "$overview_status" = "200"
test "$page_fields" = "2"
test "$client_rejection" = "401"
