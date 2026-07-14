#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"; expected="4339e63650920871eb0e8888083a1779d114e3ae"; cleanup=""
if [[ -z "${AEON_MEMORY_TS_BASELINE:-}" ]]; then AEON_MEMORY_TS_BASELINE="$($repo/scripts/prepare-ts-baseline.sh)"; export AEON_MEMORY_TS_BASELINE; cleanup="$AEON_MEMORY_TS_BASELINE"; fi
trap '[[ -z "$cleanup" ]] || rm -rf "$cleanup"; rm -f "${token:-}" "${reclaim:-}" "${compression:-}" "${fast:-}"' EXIT
[[ "$(git -C "$AEON_MEMORY_TS_BASELINE" rev-parse HEAD)" == "$expected" ]] || { echo "wrong TS baseline" >&2; exit 2; }
token="$(mktemp)"; reclaim="$(mktemp)"; compression="$(mktemp)"; fast="$(mktemp)"
AEON_MEMORY_ORACLE_OUTPUT="$token" node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-l3-token-oracle.mjs"
AEON_MEMORY_ORACLE_OUTPUT="$reclaim" node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-reclaim-oracle.mjs"
AEON_MEMORY_ORACLE_OUTPUT="$compression" node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-l3-compression-oracle.mjs"
AEON_MEMORY_TS_OFFLOAD_FAST_PATH_OUTPUT="$fast" node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-offload-fast-path-oracle.mjs"
cmp "$token" "$repo/crates/aeon-memory-core/tests/fixtures/l3_token_oracle.json"
cmp "$reclaim" "$repo/crates/aeon-memory-core/tests/fixtures/reclaim_oracle.json"
cmp "$compression" "$repo/crates/aeon-memory-core/tests/fixtures/l3_compression_oracle.json"
cmp "$fast" "$repo/crates/aeon-memory-core/tests/fixtures/offload_fast_path_oracle.json"
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-core --test l3_token_oracle --test reclaim_oracle --test l3_compression_oracle --test offload_fast_path_oracle
printf 'TypeScript L3/token/reclaim parity verified at %s\n' "$expected"
