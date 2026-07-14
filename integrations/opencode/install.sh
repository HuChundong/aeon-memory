#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ ! -f "$SCRIPT_DIR/dist/cli.js" ] || [ "$SCRIPT_DIR/src/cli.ts" -nt "$SCRIPT_DIR/dist/cli.js" ] || [ "$SCRIPT_DIR/src/aeon-memory.ts" -nt "$SCRIPT_DIR/dist/aeon-memory.js" ]; then
  npm --prefix "$SCRIPT_DIR" run build
fi
exec node "$SCRIPT_DIR/dist/cli.js" install --local "$@"
