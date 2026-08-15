#!/bin/sh
# mix2 installer — https://github.com/elleryfamilia/mix2
#
#   curl -fsSL https://raw.githubusercontent.com/elleryfamilia/mix2/feat/mvp/install.sh | sh
#
# Downloads the latest release for this platform, verifies its checksum,
# installs to ~/.local/share/mix2, and links ~/.local/bin/mix2.
set -eu

REPO="elleryfamilia/mix2"
INSTALL_DIR="${MIX2_INSTALL_DIR:-$HOME/.local/share/mix2}"
BIN_DIR="${MIX2_BIN_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'mix2 install: %s\n' "$*" >&2; exit 1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) os_part="macos" ;;
  Linux)  os_part="linux" ;;
  *) fail "unsupported OS: $os (macOS and Linux only for now)" ;;
esac
case "$arch" in
  arm64|aarch64) arch_part="arm64" ;;
  x86_64|amd64)  arch_part="x64" ;;
  *) fail "unsupported architecture: $arch" ;;
esac
target="${os_part}-${arch_part}"
asset="mix2-${target}.tar.gz"

base="https://github.com/${REPO}/releases/latest/download"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "→ downloading ${asset} (latest release)"
curl -fsSL -o "$tmp/$asset" "$base/$asset" \
  || fail "download failed — is the release published and the repo public?"
curl -fsSL -o "$tmp/checksums.txt" "$base/checksums.txt" \
  || fail "checksum manifest download failed"

say "→ verifying checksum"
expected="$(grep " ${asset}\$" "$tmp/checksums.txt" | awk '{print $1}')"
[ -n "$expected" ] || fail "no checksum entry for $asset"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $asset"

say "→ installing to $INSTALL_DIR"
rm -rf "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR" "$BIN_DIR"
tar -xzf "$tmp/$asset" -C "$INSTALL_DIR" --strip-components=1
ln -sf "$INSTALL_DIR/mix2" "$BIN_DIR/mix2"

if ! command -v node >/dev/null 2>&1; then
  say "⚠ mix2 needs Node.js >= 22 at runtime — install it from https://nodejs.org"
fi
say "⚠ mix2 needs both the Claude Code and Codex CLIs installed and signed in:"
say "    claude   https://claude.com/claude-code"
say "    codex    https://developers.openai.com/codex/cli"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "⚠ add $BIN_DIR to your PATH to run mix2 from anywhere" ;;
esac

say "✓ installed $("$INSTALL_DIR/mix2" --version 2>/dev/null || echo mix2). Run: mix2"
