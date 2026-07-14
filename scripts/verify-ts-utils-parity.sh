#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"; expected="4339e63650920871eb0e8888083a1779d114e3ae"; cleanup=""
if [[ -z "${AEON_MEMORY_TS_BASELINE:-}" ]];then AEON_MEMORY_TS_BASELINE="$($repo/scripts/prepare-ts-baseline.sh)";export AEON_MEMORY_TS_BASELINE;cleanup="$AEON_MEMORY_TS_BASELINE";fi
generated="$(mktemp)";trap 'rm -f "$generated"; [[ -z "$cleanup" ]] || rm -rf "$cleanup"' EXIT
[[ "$(git -C "$AEON_MEMORY_TS_BASELINE" rev-parse HEAD)" == "$expected" ]]
AEON_MEMORY_ORACLE_OUTPUT="$generated" node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-utils-oracle.mjs"
cmp "$generated" "$repo/crates/aeon-memory-core/tests/fixtures/utils_oracle.json"
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-core --test utils_oracle --locked
printf 'TypeScript utility/tool parity verified at %s\n' "$expected"
