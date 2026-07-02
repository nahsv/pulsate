#!/bin/sh
# Pulsate installer — download a prebuilt release binary, verify it, install it.
#
#   curl --proto '=https' --tlsv1.2 -fsSL \
#     https://raw.githubusercontent.com/squaretick/pulsate/main/scripts/install.sh | sh
#
# Environment overrides:
#   PULSATE_VERSION  pin a release tag (e.g. v0.1.0); default = latest release
#   INSTALL_DIR      install location; default = $XDG_BIN_HOME or ~/.local/bin
#   PULSATE_TARGET   force a Rust target triple (e.g. x86_64-unknown-linux-musl)
#   NO_COLOR         set to any value to disable colored output
#
# Installs: pulsate and p8 (short alias, when present in the release).
set -eu

REPO="squaretick/pulsate"
PRIMARY_BIN="pulsate"
EXTRA_BINS="p8"
INSTALL_DIR="${INSTALL_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}"

# ---------------------------------------------------------------------------
# Branding / output
# ---------------------------------------------------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  TEAL="$(printf '\033[38;2;61;163;140m')"
  BOLD="$(printf '\033[1m')"
  DIM="$(printf '\033[2m')"
  RESET="$(printf '\033[0m')"
else
  TEAL=""; BOLD=""; DIM=""; RESET=""
fi

wordmark() {
  [ -n "$TEAL" ] && printf '%s' "$TEAL"
  cat <<'BANNER'
  ####  #   # #      ####  ###  ##### #####
  #   # #   # #     #     #   #   #   #
  ####  #   # #      ###  #####   #   ####
  #     #   # #         # #   #   #   #
  #      ###  ##### ####  #   #   #   #####
BANNER
  [ -n "$RESET" ] && printf '%s' "$RESET"
  printf '%sA reverse-proxy gateway in one binary.%s\n\n' "$DIM" "$RESET"
}

step() { printf '%s▸%s %s\n' "$TEAL" "$RESET" "$1"; }
ok()   { printf '%s✓%s %s\n' "$TEAL" "$RESET" "$1"; }
err()  { printf '%sinstall error:%s %s\n' "$BOLD" "$RESET" "$1" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ---------------------------------------------------------------------------
# Detect platform -> Rust target triple + archive extension
# ---------------------------------------------------------------------------
detect_target() {
  if [ -n "${PULSATE_TARGET:-}" ]; then
    target="$PULSATE_TARGET"
    case "$target" in *windows*) ext="zip" ;; *) ext="tar.gz" ;; esac
    return
  fi
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$arch" in
    x86_64 | amd64) cpu="x86_64" ;;
    arm64 | aarch64) cpu="aarch64" ;;
    *) err "unsupported architecture: $arch (try: cargo install pulsate)" ;;
  esac
  case "$os" in
    Linux)
      target="${cpu}-unknown-linux-gnu"; ext="tar.gz" ;;
    Darwin)
      target="${cpu}-apple-darwin"; ext="tar.gz" ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT)
      target="${cpu}-pc-windows-msvc"; ext="zip" ;;
    *)
      err "unsupported OS: $os (try: cargo install pulsate, or Docker)" ;;
  esac
}

# ---------------------------------------------------------------------------
# Resolve release tag
# ---------------------------------------------------------------------------
resolve_version() {
  version="${PULSATE_VERSION:-}"
  [ -n "$version" ] && return
  have curl || err "curl is required to discover the latest release"
  version="$(curl --proto '=https' --tlsv1.2 -fsSL \
    "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)"
  [ -n "$version" ] || err "could not determine the latest release; set PULSATE_VERSION"
}

# ---------------------------------------------------------------------------
# SHA256 verification (fail closed)
# ---------------------------------------------------------------------------
sha256_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  elif have openssl; then openssl dgst -sha256 "$1" | awk '{print $NF}'
  else err "no SHA256 tool found (need sha256sum, shasum, or openssl)"
  fi
}

verify_checksum() {
  # $1 = archive file, $2 = checksum file (format: "<hash>  <name>")
  expected="$(cut -d' ' -f1 < "$2")"
  [ -n "$expected" ] || err "checksum file is empty or malformed"
  actual="$(sha256_of "$1")"
  if [ "$expected" != "$actual" ]; then
    err "checksum mismatch (expected $expected, got $actual) — aborting"
  fi
}

# ---------------------------------------------------------------------------
# Install one binary from the extracted archive
# ---------------------------------------------------------------------------
install_bin() {
  name="$1"; required="$2"
  src="$workdir/$name"
  [ -f "$src" ] || src="$(find "$workdir" -name "$name" -type f 2>/dev/null | head -1)"
  if [ -z "$src" ] || [ ! -f "$src" ]; then
    [ "$required" = "yes" ] && err "$name not found in archive"
    printf '%s-%s skipped %s (not in this release)\n' "$DIM" "$RESET" "$name"
    return
  fi
  install -m 755 "$src" "$INSTALL_DIR/$name" 2>/dev/null \
    || { mkdir -p "$INSTALL_DIR" && install -m 755 "$src" "$INSTALL_DIR/$name"; }
  chmod +x "$INSTALL_DIR/$name"
  ok "installed ${BOLD}${name}${RESET} → $INSTALL_DIR/$name"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
wordmark
detect_target
resolve_version

base="${PRIMARY_BIN}-${version}-${target}"
archive="${base}.${ext}"
url="https://github.com/${REPO}/releases/download/${version}/${archive}"
# The checksum asset is named "<base>.sha256" (no archive extension) — this is
# how upload-rust-binary-action names it. Do NOT append ".sha256" to $archive.
checksum_url="https://github.com/${REPO}/releases/download/${version}/${base}.sha256"

have curl || err "curl is required"
case "$ext" in
  tar.gz) have tar || err "tar is required" ;;
  zip) have unzip || err "unzip is required to install on Windows" ;;
esac

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

step "downloading ${BOLD}pulsate ${version}${RESET} (${target})"
curl --proto '=https' --tlsv1.2 -fSL "$url" -o "$workdir/$archive" \
  || err "no release asset for ${target} at ${url}"

step "verifying checksum"
if curl --proto '=https' --tlsv1.2 -fsSL "$checksum_url" -o "$workdir/$archive.sha256"; then
  verify_checksum "$workdir/$archive" "$workdir/$archive.sha256"
  ok "checksum verified"
else
  err "checksum file missing ($checksum_url) — refusing to install unverified binary"
fi

step "extracting"
case "$ext" in
  tar.gz) tar -xzf "$workdir/$archive" -C "$workdir" ;;
  zip) unzip -q "$workdir/$archive" -d "$workdir" ;;
esac

mkdir -p "$INSTALL_DIR"
install_bin "$PRIMARY_BIN" yes
for b in $EXTRA_BINS; do install_bin "$b" no; done

printf '\n'
ok "$("$INSTALL_DIR/$PRIMARY_BIN" --version 2>/dev/null || echo "pulsate $version")"

# PATH hint
case ":$PATH:" in
  *":$INSTALL_DIR:"*) : ;;
  *)
    printf '\n%s!%s %s is not on your PATH. Add it:\n' "$BOLD" "$RESET" "$INSTALL_DIR"
    # shellcheck disable=SC2016  # literal $PATH is intentional in the hint
    printf '    %sexport PATH="%s:$PATH"%s\n' "$DIM" "$INSTALL_DIR" "$RESET"
    ;;
esac

printf '\nTry: %spulsate info%s\n' "$BOLD" "$RESET"
