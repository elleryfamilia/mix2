#!/bin/sh
# mix2 launcher — shipped in release tarballs next to mix2.bundle.mjs,
# mix2-core, and mix2-consult.
set -e

DIR="$(cd "$(dirname "$0")" && pwd)"

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
