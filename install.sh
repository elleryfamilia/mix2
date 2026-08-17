#!/bin/sh
# mix2 installer — https://github.com/elleryfamilia/mix2
#
#   curl -fsSL https://raw.githubusercontent.com/elleryfamilia/mix2/feat/mvp/install.sh | sh
#
# Downloads a release for this platform, verifies its checksum, installs it
# to ~/.local/share/mix2, and links ~/.local/bin/mix2. `mix2 update` runs
# this same script (pinned to a release tag) to upgrade in place.
#
# Environment:
#   MIX2_INSTALL_DIR       install location (default ~/.local/share/mix2)
#   MIX2_BIN_DIR           where the `mix2` symlink goes (default ~/.local/bin)
#   MIX2_VERSION           install this release tag (e.g. v0.4.0) instead of latest
#   MIX2_NO_LINK           if set, do not create the symlink (updates use this:
#                          the existing link keeps pointing at MIX2_INSTALL_DIR)
#   MIX2_RELEASE_BASE_URL  override the download base URL (tests only)
set -eu

say() { printf '%s\n' "$*"; }
fail() { printf 'mix2 install: %s\n' "$*" >&2; exit 1; }

: "${HOME:?HOME is not set}"
REPO="elleryfamilia/mix2"
INSTALL_DIR="${MIX2_INSTALL_DIR:-$HOME/.local/share/mix2}"
INSTALL_DIR="${INSTALL_DIR%/}" # the lock/staging dirs are siblings of it
BIN_DIR="${MIX2_BIN_DIR:-$HOME/.local/bin}"

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

want=""
if [ -n "${MIX2_VERSION:-}" ]; then
  tag="$MIX2_VERSION"
  case "$tag" in v*) ;; *) tag="v$tag" ;; esac
  want="${tag#v}"
  base="https://github.com/${REPO}/releases/download/${tag}"
  label="release ${tag}"
else
  base="https://github.com/${REPO}/releases/latest/download"
  label="latest release"
fi
if [ -n "${MIX2_RELEASE_BASE_URL:-}" ]; then base="$MIX2_RELEASE_BASE_URL"; fi

# One installer at a time per install dir. mkdir is atomic, so two
# concurrent runs cannot both take the lock; a stale lock (from a killed
# run) is reported with the path to remove.
parent="$(dirname "$INSTALL_DIR")"
mkdir -p "$parent"
[ -w "$parent" ] || fail "cannot write to $parent"
lock="${INSTALL_DIR}.lock"
if ! mkdir "$lock" 2>/dev/null; then
  fail "another mix2 install is in progress (if not, remove $lock and retry)"
fi

# From here on we own the lock: install the trap before anything else can
# fail, so the lock never goes stale. (Only after taking it — a run that
# lost the race must not delete the winner's lock.)
tmp=""
stage=""
old=""
cleanup() {
  # If we died between moving the old install aside and moving the new one
  # in, put the old one back so the user still has a working mix2.
  if [ -n "$old" ] && [ ! -e "$INSTALL_DIR" ]; then mv "$old" "$INSTALL_DIR"; fi
  if [ -n "$tmp" ]; then rm -rf "$tmp"; fi
  if [ -n "$stage" ]; then rm -rf "$stage"; fi
  # (`if`s, not `[ ] &&`: under set -e a false test as the trap's last
  # command would turn a successful install into exit status 1)
  rm -rf "$lock"
}
trap cleanup EXIT
tmp="$(mktemp -d)"

say "→ downloading ${asset} (${label})"
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
if [ -n "$want" ] && [ "$version" != "$want" ]; then
  fail "asked for ${want} but the download contains ${version}"
fi

# Transactional install: extract into a sibling staging dir (same
# filesystem, so the swap below is a rename), check it looks like a mix2
# release, then swap it into place. A failure before the swap leaves any
# existing install untouched.
say "→ installing mix2 ${version} to $INSTALL_DIR"
stage="$(mktemp -d "${INSTALL_DIR}.new.XXXXXX")"
chmod 755 "$stage" # mktemp makes it 0700; match what mkdir -p used to give
tar -xzf "$tmp/$asset" -C "$stage" --strip-components=1
for f in mix2 mix2-core mix2-consult mix2.bundle.mjs; do
  [ -f "$stage/$f" ] || fail "release archive is missing $f — not installing"
done
if [ -e "$INSTALL_DIR" ]; then
  old="${INSTALL_DIR}.old.$$"
  mv "$INSTALL_DIR" "$old"
fi
if ! mv "$stage" "$INSTALL_DIR"; then
  fail "could not move the new install into place" # cleanup restores $old
fi
stage=""
if [ -n "$old" ]; then rm -rf "$old"; fi
old=""

if [ -z "${MIX2_NO_LINK:-}" ]; then
  mkdir -p "$BIN_DIR"
  ln -sf "$INSTALL_DIR/mix2" "$BIN_DIR/mix2"
fi

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

if [ -z "${MIX2_NO_LINK:-}" ]; then
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) need "$BIN_DIR on your PATH (to run mix2 from anywhere)" ;;
  esac
fi

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
