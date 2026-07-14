#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
expected="4339e63650920871eb0e8888083a1779d114e3ae"
cleanup=""

if [[ -z "${AEON_MEMORY_TS_BASELINE:-}" ]]; then
  AEON_MEMORY_TS_BASELINE="$($repo/scripts/prepare-ts-baseline.sh)"
  export AEON_MEMORY_TS_BASELINE
  cleanup="$AEON_MEMORY_TS_BASELINE"
fi
trap '[[ -z "$cleanup" ]] || rm -rf "$cleanup"; rm -rf "$temp"' EXIT

actual="$(git -C "$AEON_MEMORY_TS_BASELINE" rev-parse HEAD)"
[[ "$actual" == "$expected" ]] || { echo "wrong TS baseline: $actual" >&2; exit 2; }
expected_lock="${AEON_MEMORY_TS_BASELINE_LOCK_SHA256:-4ffbb116aa56fb46c9af791ff166486142cf884271c99a767ff145429afa2539}"
actual_lock="$(shasum -a 256 "$AEON_MEMORY_TS_BASELINE/package-lock.json" | cut -d' ' -f1)"
[[ "$actual_lock" == "$expected_lock" ]] || {
  echo "TS dependency lock drift: $actual_lock" >&2
  exit 3
}

temp="$(mktemp -d "${TMPDIR:-/tmp}/aeon-memory-ts-sqlite.XXXXXX")"
db="$temp/vectors-ts.db"
AEON_MEMORY_TS_DB_OUTPUT="$db" \
AEON_MEMORY_TSX_CLI="$AEON_MEMORY_TS_BASELINE/node_modules/tsx/dist/cli.mjs" \
node "$repo/crates/aeon-memory-store-sqlite/tests/fixtures/gen-ts-db.mjs"

# These cases intentionally reopen the same generated database. Serialize
# them so schema-idempotence checks cannot race another connection for DDL.
AEON_MEMORY_TS_DB_FIXTURE="$db" \
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-store-sqlite --test compat -- --test-threads=1

oracle="$temp/embedding-reindex-oracle.json"
AEON_MEMORY_TS_REINDEX_ORACLE_OUTPUT="$oracle" \
AEON_MEMORY_TSX_CLI="$AEON_MEMORY_TS_BASELINE/node_modules/tsx/dist/cli.mjs" \
node "$repo/crates/aeon-memory-store-sqlite/tests/fixtures/gen-ts-embedding-reindex-oracle.mjs"
cmp "$repo/crates/aeon-memory-store-sqlite/tests/fixtures/embedding_reindex_oracle.json" "$oracle"
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-store-sqlite \
  --test embedding_reindex_differential
printf 'TypeScript SQLite parity verified at %s\n' "$expected"
