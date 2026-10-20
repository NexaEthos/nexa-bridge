#!/usr/bin/env bash
# Wiki: docs/wiki/desktop.md
set -euo pipefail

if (( $# > 1 )); then
  echo "usage: $0 [target-triple]" >&2
  exit 2
fi
sidecar_dir="$(cd "$(dirname "$0")" && pwd)"
workspace_dir="$(cd "$sidecar_dir/../.." && pwd)"
sidecar_triple="${1:-${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}}"
case "$sidecar_triple" in
  ''|*[!a-zA-Z0-9_-]*) echo "target must be a Rust target triple" >&2; exit 2 ;;
esac
cd "$workspace_dir"
sidecar_target_dir="${CARGO_TARGET_DIR:-$workspace_dir/target}"
case "$sidecar_target_dir" in
  /*) ;;
  *) sidecar_target_dir="$workspace_dir/$sidecar_target_dir" ;;
esac
sidecar_extension=""
case "$sidecar_triple" in
  *-windows-*) sidecar_extension=".exe" ;;
esac

echo "target triple: $sidecar_triple"
cargo build --release --locked --manifest-path "$workspace_dir/Cargo.toml" \
  --target "$sidecar_triple" --target-dir "$sidecar_target_dir" \
  -p nexa-llm-bridge -p nexa-llm-connector

mkdir -p "$sidecar_dir/binaries"
for bin in nexa-llm-bridge nexa-llm-connector; do
  src="$sidecar_target_dir/$sidecar_triple/release/$bin$sidecar_extension"
  [ -f "$src" ] || { echo "missing $src" >&2; exit 1; }
done
for bin in nexa-llm-bridge nexa-llm-connector; do
  src="$sidecar_target_dir/$sidecar_triple/release/$bin$sidecar_extension"
  cp "$src" "$sidecar_dir/binaries/$bin-$sidecar_triple$sidecar_extension"
  echo "  binaries/$bin-$sidecar_triple$sidecar_extension"
done
