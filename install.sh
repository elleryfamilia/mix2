#!/bin/sh
# mix2 installer — https://github.com/elleryfamilia/mix2
#
#   curl -fsSL https://raw.githubusercontent.com/elleryfamilia/mix2/main/install.sh | sh
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
#   MIX2_VERIFY_TIMEOUT    seconds to wait for the new launcher's --version (default 10)
set -eu

say() { printf '%s\n' "$*"; }
fail() { printf 'mix2 install: %s\n' "$*" >&2; exit 1; }
# Major version of the node on PATH, or 0 when there is none.
node_major() {
  if command -v node >/dev/null 2>&1; then
    node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0
  else
    echo 0
  fi
}

: "${HOME:?HOME is not set}"
REPO="elleryfamilia/mix2"
INSTALL_DIR="${MIX2_INSTALL_DIR:-$HOME/.local/share/mix2}"
# Strip every trailing slash: the lock, staging and rollback dirs are
# formed by appending to this path and must be siblings, not children.
while [ "${INSTALL_DIR%/}" != "$INSTALL_DIR" ]; do INSTALL_DIR="${INSTALL_DIR%/}"; done
[ -n "$INSTALL_DIR" ] || fail "MIX2_INSTALL_DIR must be a directory path, not /"
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
aside=""    # where the previous install is moved while the new one is unverified
accepted="" # set once the new install has been verified
probe=""    # pid of the verification run of the new launcher, if any
cleanup() {
  # Cleanup must run to the end: no errexit in here (one failing rm must
  # not skip the restore), and a second Ctrl+C must not cut it short.
  set +e
  trap '' HUP INT TERM
  if [ -n "$probe" ]; then kill "$probe" 2>/dev/null; fi
  # The previous install sits at $aside from the moment it is renamed
  # there until the new one is accepted. Renames are atomic, so "$aside
  # exists" is the truth of that state — no variable is assigned after
  # the fact for a signal to slip in between. Exiting in that window — a
  # failure, or a signal — means whatever sits at $INSTALL_DIR is
  # unverified: drop it and put the previous install back.
  if [ -z "$accepted" ]; then
    # The staging dir was created and no longer exists ⇒ it was renamed
    # into $INSTALL_DIR (atomic), and that tree is unverified: drop it.
    if [ -n "$stage" ] && [ ! -e "$stage" ]; then rm -rf "$INSTALL_DIR"; fi
    if [ -n "$aside" ] && [ -e "$aside" ]; then
      rm -rf "$INSTALL_DIR"
      if [ ! -e "$INSTALL_DIR" ] && mv "$aside" "$INSTALL_DIR"; then
        :
      else
        printf 'mix2 install: could not restore the previous install; it is at %s\n' "$aside" >&2
      fi
    fi
  elif [ -n "$aside" ] && [ -e "$aside" ]; then
    rm -rf "$aside" # accepted; interrupted before the old copy was gone
  fi
  if [ -n "$tmp" ]; then rm -rf "$tmp"; fi
  if [ -n "$stage" ] && [ -e "$stage" ]; then rm -rf "$stage"; fi
  # (`if`s, not `[ ] &&`: a false test as the trap's last command would
  # turn a successful install into exit status 1)
  rm -rf "$lock"
}
trap cleanup EXIT
# sh does not run the EXIT trap when killed by a signal (terminal closed,
# Ctrl+C, kill): route the common ones through exit so cleanup still runs.
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
# We hold the lock, so siblings left by a run that was SIGKILLed (which
# no trap can catch) are safe to sweep now.
for d in "$INSTALL_DIR".old.* "$INSTALL_DIR".new.*; do
  if [ -e "$d" ]; then rm -rf "$d"; fi
done
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
  candidate="${INSTALL_DIR}.old.$$"
  rm -rf "$candidate" # a leftover from a crashed run with this pid must not swallow the mv
  aside="$candidate"
  mv "$INSTALL_DIR" "$aside" || fail "could not move the previous install aside"
fi
if ! mv "$stage" "$INSTALL_DIR"; then
  fail "could not move the new install into place" # cleanup restores the previous install
fi
# ($stage is kept: "created but gone" is how cleanup knows the swap happened.)

# Before discarding the previous install, prove the new one runs and is
# the version we think it is. Needs a usable Node (the launcher's own
# requirement); without one there is nothing mix2 could do anyway, and the
# checklist below says so.
if [ "$(node_major)" -ge 22 ]; then
  # Run the probe in the background with a watchdog (10s; tests shorten it
  # via MIX2_VERIFY_TIMEOUT): a launcher that hangs must not hang the
  # update, and there is no portable `timeout`.
  "$INSTALL_DIR/mix2" --version > "$tmp/version.txt" 2>/dev/null &
  probe=$!
  i=0
  limit=$(( ${MIX2_VERIFY_TIMEOUT:-10} * 10 ))
  while kill -0 "$probe" 2>/dev/null && [ "$i" -lt "$limit" ]; do
    sleep 0.1
    i=$((i + 1))
  done
  timed_out=""
  if kill -0 "$probe" 2>/dev/null; then
    kill "$probe" 2>/dev/null || true
    timed_out=1
  fi
  probe_status=0
  wait "$probe" 2>/dev/null || probe_status=$?
  probe=""
  reported="$(cat "$tmp/version.txt" 2>/dev/null || true)"
  # Accept only a clean run: exited 0, within the deadline, right output.
  problem=""
  if [ -n "$timed_out" ]; then
    problem="did not answer --version within the deadline"
  elif [ "$probe_status" -ne 0 ]; then
    problem="--version exited with status ${probe_status}"
  elif [ "$reported" != "mix2 ${version}" ]; then
    problem="reported '${reported}', expected 'mix2 ${version}'"
  fi
  if [ -n "$problem" ]; then
    if [ -n "$aside" ] && [ -e "$aside" ]; then
      # cleanup (EXIT trap) drops the new tree and restores the old one
      fail "the new install does not run (${problem}); the previous version was restored"
    fi
    rm -rf "$INSTALL_DIR"
    fail "the new install does not run (${problem})"
  fi
fi
accepted=1
if [ -n "$aside" ] && [ -e "$aside" ]; then
  # A leftover we cannot delete (root-owned files from a sudo install, an
  # immutable flag) is a warning, not a failed update: the new install is in.
  rm -rf "$aside" || say "⚠ could not remove the previous install at $aside — delete it by hand"
fi

if [ -z "${MIX2_NO_LINK:-}" ]; then
  mkdir -p "$BIN_DIR"
  ln -sf "$INSTALL_DIR/mix2" "$BIN_DIR/mix2"
fi

# mix2 refuses to start without Node >= 22 and the agent CLIs backing its
# two team slots. A team is any two of the supported CLIs (the same CLI
# twice counts, as two independent sessions), picked in config or the
# startup picker, so require at least one here and recommend two. Check
# now so the install ends with honest next steps instead of a success
# line on a machine that can't run it.
missing=""
need() {
  if [ -z "$missing" ]; then missing="  • $*"; else missing="${missing}
  • $*"; fi
}
if command -v node >/dev/null 2>&1; then
  [ "$(node_major)" -ge 22 ] \
    || need "Node.js >= 22 (found $(node --version 2>/dev/null || echo 'an unknown version')) — https://nodejs.org"
else
  need "Node.js >= 22 — https://nodejs.org"
fi
# Keep this list in sync with the core's harness registry
# (crates/mix2-core/src/agents/registry.rs).
agents_found=""
agent_count=0
for cli in claude codex cursor-agent opencode copilot; do
  command -v "$cli" >/dev/null 2>&1 || continue
  agent_count=$((agent_count + 1))
  agents_found="${agents_found:+$agents_found, }$cli"
done
[ "$agent_count" -ge 1 ] \
  || need "an agent CLI — mix2 teams any two of claude, codex, cursor-agent, opencode, copilot (links: https://github.com/elleryfamilia/mix2#requirements)"

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
  say "The agent CLIs you use must also be signed in — mix2 checks at startup"
  say "and names the exact fix per agent."
  say "Fix the above, then run: mix2"
elif [ "$agent_count" -eq 1 ]; then
  say "✓ installed mix2 ${version} — found ${agents_found}, which can fill both"
  say "  team slots as two independent sessions. A second supported CLI (claude,"
  say "  codex, cursor-agent, opencode, copilot) gives the team a second model."
  say "  Sign in, then run: mix2"
else
  say "✓ installed mix2 ${version} — found ${agents_found}. Make sure the two"
  say "  you use are signed in, then run: mix2"
fi
