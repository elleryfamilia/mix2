#!/bin/sh
# mix2 launcher — shipped in release tarballs next to mix2.bundle.mjs,
# mix2-core, and mix2-consult. Resolves symlinks so `~/.local/bin/mix2 ->
# ~/.local/share/mix2/mix2` finds its siblings.
set -e

SOURCE="$0"
while [ -h "$SOURCE" ]; do
  DIR="$(cd "$(dirname "$SOURCE")" && pwd)"
  SOURCE="$(readlink "$SOURCE")"
  case "$SOURCE" in
    /*) ;;
    *) SOURCE="$DIR/$SOURCE" ;;
  esac
done
DIR="$(cd "$(dirname "$SOURCE")" && pwd)"

if ! command -v node >/dev/null 2>&1; then
  echo "mix2 needs Node.js >= 22 (https://nodejs.org). Install it, then run mix2 again." >&2
  exit 1
fi

NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]')"
if [ "$NODE_MAJOR" -lt 22 ]; then
  echo "mix2 needs Node.js >= 22; found $(node --version). Upgrade Node, then run mix2 again." >&2
  exit 1
fi

export MIX2_CORE_BIN="$DIR/mix2-core"
exec node "$DIR/mix2.bundle.mjs" "$@"
