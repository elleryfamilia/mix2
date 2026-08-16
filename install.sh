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

# The tarball's top-level dir is mix2-<version>-<target>; read the version
# from it rather than running the binary (which needs Node we may not have).
topdir="$(tar -tzf "$tmp/$asset" | head -n 1)"
topdir="${topdir%%/*}"
version="${topdir#mix2-}"
version="${version%"-$target"}"

say "→ installing mix2 ${version} to $INSTALL_DIR"
rm -rf "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR" "$BIN_DIR"
tar -xzf "$tmp/$asset" -C "$INSTALL_DIR" --strip-components=1
ln -sf "$INSTALL_DIR/mix2" "$BIN_DIR/mix2"

# mix2 refuses to start unless Node >= 22 and BOTH agent CLIs are present
# and signed in — check now so the install ends with honest next steps
# instead of a success line on a machine that can't run it.
missing=""
need() {
  if [ -z "$missing" ]; then missing="  • $*"; else missing="${missing}
  • $*"; fi
}
if command -v node >/dev/null 2>&1; then
  node_major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
  [ "${node_major:-0}" -ge 22 ] \
    || need "Node.js >= 22 (found $(node --version 2>/dev/null || echo 'an unknown version')) — https://nodejs.org"
else
  need "Node.js >= 22 — https://nodejs.org"
fi
command -v claude >/dev/null 2>&1 \
  || need "Claude Code CLI — https://claude.com/claude-code"
command -v codex >/dev/null 2>&1 \
  || need "Codex CLI — https://developers.openai.com/codex/cli (npm i -g @openai/codex)"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) need "$BIN_DIR on your PATH (to run mix2 from anywhere)" ;;
esac

if [ -n "$missing" ]; then
  say ""
  say "⚠ mix2 ${version} is installed, but this machine is missing:"
  say "$missing"
  say ""
  say "Both CLIs must also be signed in (run \`claude\` once; run \`codex login\`)."
  say "Fix the above, then run: mix2"
else
  say "✓ installed mix2 ${version} — make sure both CLIs are signed in, then run: mix2"
fi
