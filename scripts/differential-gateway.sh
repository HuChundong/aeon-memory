#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
baseline="${AEON_MEMORY_TS_BASELINE:?set AEON_MEMORY_TS_BASELINE to the pinned, installed TypeScript checkout}"
expected="4339e63650920871eb0e8888083a1779d114e3ae"

[[ "$(git -C "$baseline" rev-parse HEAD)" == "$expected" ]] || {
  echo "wrong TS baseline: expected $expected" >&2
  exit 2
}
[[ -d "$baseline/node_modules" ]] || {
  echo "TS baseline dependencies are missing; see TS_DIFFERENTIAL_BASELINE.md" >&2
  exit 2
}

cargo build --manifest-path "$repo/Cargo.toml" -p aeon-memory-gateway --bins >/dev/null
exec python3 "$repo/scripts/differential-gateway-e2e.py" \
  --repo "$repo" --baseline "$baseline" "$@"
