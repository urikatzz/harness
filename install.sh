#!/bin/sh
# Installs uriTheKing:  curl -fsSL https://raw.githubusercontent.com/urikatzz/harness/main/install.sh | sh
# Options (env): URITHEKING_VERSION=v0.1.0 (default: latest), URITHEKING_INSTALL_DIR=~/.local/bin
set -eu

REPO="urikatzz/harness"
BIN="uritheking"
INSTALL_DIR="${URITHEKING_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${URITHEKING_VERSION:-latest}"

fail() { echo "uritheking install: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "'$1' is required"; }
need curl; need tar; need uname

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-gnu" ;;
  *)      fail "unsupported OS: $(uname -s) (macOS and Linux only)" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x86_64" ;;
  *)             fail "unsupported CPU: $(uname -m)" ;;
esac
target="$arch-$os"
asset="$BIN-$target.tar.gz"

if [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset ($VERSION)..."
curl -fsSL "$base/$asset" -o "$tmp/$asset" || fail "download failed: $base/$asset"
curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256" || fail "checksum download failed"

expected="$(cut -d ' ' -f 1 < "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)"
else
  actual="$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $asset"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/$BIN" "$INSTALL_DIR/$BIN"
echo "Installed $BIN to $INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) echo "Run: $BIN" ;;
  *) echo "Add it to your PATH, then run $BIN:"
     echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.$(basename "${SHELL:-sh}")rc" ;;
esac
