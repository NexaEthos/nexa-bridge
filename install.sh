#!/bin/sh
# Install Nexa Bridge's command-line programs and link this server to a workspace. docs/wiki/server-setup.md
#   curl -fsSL https://raw.githubusercontent.com/NexaEthos/nexa-bridge/main/install.sh | sh
#   curl -fsSL …/install.sh | sh -s -- --model <exact id> --yes      (options go to `nexa-bridge setup`)
#   curl -fsSL …/install.sh | sh -s -- --uninstall                   (unlink and remove the programs)
set -eu

repo="NexaEthos/nexa-bridge"
bin_dir="${NEXA_BRIDGE_BIN_DIR:-$HOME/.local/bin}"
version="${NEXA_BRIDGE_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
fail() { printf 'nexa-bridge install: %s\n' "$*" >&2; exit 1; }

if [ "${1:-}" = "--uninstall" ]; then
  [ -x "$bin_dir/nexa-llm-connector" ] || fail "nothing to uninstall in $bin_dir"
  exec "$bin_dir/nexa-llm-connector" uninstall
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target="x86_64-unknown-linux-musl" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-musl" ;;
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  *) fail "no build for $(uname -s) $(uname -m)" ;;
esac

if [ "$version" = "latest" ]; then
  base="https://github.com/$repo/releases/latest/download"
else
  base="https://github.com/$repo/releases/download/v$version"
fi

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  else fail "neither sha256sum nor shasum is installed: the downloads cannot be verified"
  fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM

fetch() {
  if [ -n "${NEXA_BRIDGE_FROM:-}" ]; then
    cp "$NEXA_BRIDGE_FROM/$1" "$work/$1" || fail "$1 is not in $NEXA_BRIDGE_FROM"
  else
    curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$1" "$base/$1" || fail "cannot download $1 from $base"
  fi
}

fetch SHA256SUMS
for program in nexa-llm-bridge nexa-llm-connector; do
  asset="$program-$target"
  fetch "$asset"
  expected="$(awk -v name="$asset" '$2 == name { print $1 }' "$work/SHA256SUMS")"
  [ -n "$expected" ] || fail "$asset is not listed in SHA256SUMS"
  [ "$(sha256 "$work/$asset")" = "$expected" ] || fail "$asset does not match its checksum; nothing was installed"
done

mkdir -p "$bin_dir"
for program in nexa-llm-bridge nexa-llm-connector; do
  install -m 0755 "$work/$program-$target" "$bin_dir/$program"
done
printf '#!/bin/sh\nexec "%s/nexa-llm-connector" "$@"\n' "$bin_dir" > "$bin_dir/nexa-bridge"
chmod 0755 "$bin_dir/nexa-bridge"
say "✓ $("$bin_dir/nexa-llm-connector" --version) installed in $bin_dir (checksums verified)"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) say "! $bin_dir is not on your PATH: add it to run \`nexa-bridge\` by name" ;;
esac

if (: </dev/tty) 2>/dev/null; then
  exec "$bin_dir/nexa-llm-connector" setup "$@" </dev/tty
fi
exec "$bin_dir/nexa-llm-connector" setup "$@"
