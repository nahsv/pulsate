#!/bin/sh
# Install Pulsate (the `pulsate` binary and its `p8` alias) from GitHub releases.
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/nahsv/pulsate/main/scripts/install.sh | sh
#
# Env overrides: PULSATE_VERSION (e.g. v0.1.0), PULSATE_BIN_DIR (default /usr/local/bin).
set -eu

REPO="nahsv/pulsate"
BIN_DIR="${PULSATE_BIN_DIR:-/usr/local/bin}"

err() { echo "install: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Map uname to a Rust target triple.
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  suffix="unknown-linux-gnu" ;;
  Darwin) suffix="apple-darwin" ;;
  *) err "unsupported OS: $os (use cargo install pulsate, or Docker)" ;;
esac
case "$arch" in
  x86_64|amd64)  cpu="x86_64" ;;
  arm64|aarch64) cpu="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac
target="${cpu}-${suffix}"

# Resolve the version (latest release unless pinned).
version="${PULSATE_VERSION:-}"
if [ -z "$version" ]; then
  have curl || err "curl is required"
  version="$(curl --proto '=https' --tlsv1.2 -fsSL \
    "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)"
  [ -n "$version" ] || err "could not determine the latest version; set PULSATE_VERSION"
fi

url="https://github.com/${REPO}/releases/download/${version}/pulsate-${version}-${target}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading Pulsate ${version} for ${target} ..."
curl --proto '=https' --tlsv1.2 -fSL "$url" -o "$tmp/pulsate.tar.gz" \
  || err "download failed: $url"
tar -xzf "$tmp/pulsate.tar.gz" -C "$tmp"

# The archive contains both `pulsate` and `p8`.
install_one() {
  src="$tmp/$1"
  [ -f "$src" ] || src="$(find "$tmp" -name "$1" -type f | head -1)"
  [ -n "$src" ] && [ -f "$src" ] || err "$1 not found in archive"
  if [ -w "$BIN_DIR" ]; then
    install -m 755 "$src" "$BIN_DIR/$1"
  else
    echo "Installing to $BIN_DIR (needs sudo) ..."
    sudo install -m 755 "$src" "$BIN_DIR/$1"
  fi
}
install_one pulsate
install_one p8

echo "Installed: $("$BIN_DIR/pulsate" --version)"
echo "Try: pulsate info"
