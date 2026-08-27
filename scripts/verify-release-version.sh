#!/usr/bin/env bash
set -euo pipefail

release_tag="${1:-${GITHUB_REF_NAME:-}}"

if [[ ! "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Release tag must use semantic version format vMAJOR.MINOR.PATCH (received: ${release_tag:-<empty>})." >&2
  exit 1
fi

release_version="${release_tag#v}"
cargo_version="$({
  awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && /^version[[:space:]]*=/ {
      gsub(/^[^"]*"|".*$/, "")
      print
      exit
    }
  ' Cargo.toml
})"
ios_version="$(sed -n 's/^[[:space:]]*MARKETING_VERSION:[[:space:]]*"\([^"]*\)"/\1/p' ios/project.yml | head -n 1)"

if [[ -z "$cargo_version" || -z "$ios_version" ]]; then
  echo "Unable to read the Cargo or iOS project version." >&2
  exit 1
fi

if ! [[ "$release_version" == "$cargo_version" && "$release_version" == "$ios_version" ]]; then
  echo "Release versions do not match:" >&2
  echo "  tag:   $release_version" >&2
  echo "  Cargo: $cargo_version" >&2
  echo "  iOS:   $ios_version" >&2
  exit 1
fi

echo "Release version $release_version is consistent."
