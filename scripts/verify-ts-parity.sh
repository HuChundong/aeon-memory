#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
baseline_commit="4339e63650920871eb0e8888083a1779d114e3ae"
cleanup=""

if [[ -n "${AEON_MEMORY_TS_BASELINE:-}" ]]; then
  baseline="$AEON_MEMORY_TS_BASELINE"
else
  cleanup="$(mktemp -d)"
  baseline="$cleanup/TencentDB-Agent-Memory"
  git clone --quiet https://github.com/TencentCloud/TencentDB-Agent-Memory.git "$baseline"
  git -C "$baseline" checkout --quiet "$baseline_commit"
fi
trap '[[ -z "$cleanup" ]] || rm -rf "$cleanup"' EXIT

actual_commit="$(git -C "$baseline" rev-parse HEAD)"
if [[ "$actual_commit" != "$baseline_commit" ]]; then
  printf 'TypeScript baseline must be %s, got %s\n' "$baseline_commit" "$actual_commit" >&2
  exit 1
fi

[[ -x "$baseline/node_modules/.bin/tsx" ]] || { echo "baseline is not bootstrapped; use prepare-ts-baseline.sh" >&2; exit 2; }

generated="$(mktemp)"
trap 'rm -f "$generated"; [[ -z "$cleanup" ]] || rm -rf "$cleanup"' EXIT
AEON_MEMORY_TS_BASELINE="$baseline" \
AEON_MEMORY_TSX_CLI="$baseline/node_modules/tsx/dist/cli.mjs" \
AEON_MEMORY_ORACLE_OUTPUT="$generated" \
node "$repo_root/crates/aeon-memory-core/tests/fixtures/gen-ts-config-search-oracle.mjs"

cmp "$generated" "$repo_root/crates/aeon-memory-core/tests/fixtures/config_search_oracle.json"
cargo test --manifest-path "$repo_root/Cargo.toml" -p aeon-memory-core --test config_search_oracle
printf 'TypeScript parity verified at %s\n' "$baseline_commit"
