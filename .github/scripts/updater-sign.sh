#!/usr/bin/env bash
# updater-sign.sh <version> <asset> <file>: the signature lands in signed/updater/<asset>.sig (release.md).
set -euo pipefail

[ "$#" -eq 3 ] && [ -f "$3" ] || { echo "::error::expected one file for ${2:-the asset}, got: ${*:3}"; exit 1; }
version=$1
asset=$2
file=$3
for name in TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD; do
  [ -n "${!name:-}" ] || { echo "::error::${name} is missing from the release environment"; exit 1; }
done
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$file" "${work}/${asset}"
npx --yes "@tauri-apps/cli@${TAURI_CLI}" signer sign --app-version "$version" "${work}/${asset}" > /dev/null
mkdir -p signed/updater
mv "${work}/${asset}.sig" "signed/updater/${asset}.sig"
