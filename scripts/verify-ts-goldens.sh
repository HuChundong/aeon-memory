#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
cleanup=""
fixture_before="$(mktemp)"
fixture_after="$(mktemp)"

if [[ -z "${AEON_MEMORY_TS_BASELINE:-}" ]]; then
  AEON_MEMORY_TS_BASELINE="$($repo/scripts/prepare-ts-baseline.sh)"
  export AEON_MEMORY_TS_BASELINE
  cleanup="$AEON_MEMORY_TS_BASELINE"
fi
trap '[[ -z "$cleanup" ]] || rm -rf "$cleanup"; rm -f "$fixture_before" "$fixture_after"' EXIT

expected="4339e63650920871eb0e8888083a1779d114e3ae"
actual="$(git -C "$AEON_MEMORY_TS_BASELINE" rev-parse HEAD)"
[[ "$actual" == "$expected" ]] || { echo "wrong TS baseline: $actual" >&2; exit 2; }
tsx="$AEON_MEMORY_TS_BASELINE/node_modules/tsx/dist/cli.mjs"
[[ -f "$tsx" ]] || { echo "missing pinned TSX runtime: $tsx" >&2; exit 2; }

oracle_files=(
  crates/aeon-memory-core/tests/fixtures/rrf_merge.json
  crates/aeon-memory-core/tests/fixtures/bm25_rank.json
  crates/aeon-memory-core/tests/fixtures/fts_query.json
  crates/aeon-memory-core/tests/fixtures/l1_jsonl_record.json
  crates/aeon-memory-core/tests/fixtures/prompt_l1_extraction.txt
  crates/aeon-memory-core/tests/fixtures/prompt_l1_dedup.txt
  crates/aeon-memory-core/tests/fixtures/prompt_l1_extraction_dynamic.txt
  crates/aeon-memory-core/tests/fixtures/prompt_l1_dedup_dynamic.txt
  crates/aeon-memory-core/tests/fixtures/prompt_scene_system_15.txt
  crates/aeon-memory-core/tests/fixtures/prompt_scene_dynamic.txt
  crates/aeon-memory-core/tests/fixtures/prompt_persona_system.txt
  crates/aeon-memory-core/tests/fixtures/prompt_persona_dynamic.txt
  crates/aeon-memory-core/tests/fixtures/offload_prompt_legacy_replay.json
  crates/aeon-memory-core/tests/fixtures/runtime_trace.json
  crates/aeon-memory-core/tests/fixtures/pipeline_branches.json
  crates/aeon-memory-core/tests/fixtures/l0_runtime_oracle.json
  crates/aeon-memory-core/tests/fixtures/embedding_runtime_oracle.json
  crates/aeon-memory-core/tests/fixtures/dedup_runtime_oracle.json
  crates/aeon-memory-core/tests/fixtures/recall_runtime_oracle.json
  crates/aeon-memory-gateway/tests/fixtures/disabled_capture_oracle.json
  crates/aeon-memory-core/tests/fixtures/offload_parser_oracle.json
  crates/aeon-memory-core/tests/fixtures/session_filter_oracle.json
  crates/aeon-memory-core/tests/fixtures/persona_trigger_oracle.json
  crates/aeon-memory-core/tests/fixtures/cleaner_oracle.json
  crates/aeon-memory-core/tests/fixtures/degraded_l1_oracle.json
  crates/aeon-memory-core/tests/fixtures/time_production_oracle.json
  crates/aeon-memory-core/tests/fixtures/offload_state_oracle.json
  crates/aeon-memory-core/src/prompt/resources/scene_extraction_template.txt
  crates/aeon-memory-core/src/prompt/resources/persona_generation.txt
)
(cd "$repo" && shasum -a 256 "${oracle_files[@]}") > "$fixture_before"

node "$repo/crates/aeon-memory-core/tests/fixtures/gen-golden-fixtures.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-offload-legacy-replay.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-runtime-trace.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-pipeline-branches.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-l0-oracle.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-embedding-oracle.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-dedup-oracle.mjs"
node "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-recall-oracle.mjs"
node "$repo/crates/aeon-memory-gateway/tests/fixtures/gen-ts-disabled-capture-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-offload-parser-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-session-filter-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-persona-trigger-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-cleaner-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-degraded-l1-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-time-production-oracle.mjs"
node "$tsx" "$repo/crates/aeon-memory-core/tests/fixtures/gen-ts-offload-state-oracle.mjs"
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-core --test golden_compat --test offload_complete --test offload_parser_oracle --test offload_state_oracle --test session_filter_oracle --test persona_trigger_oracle --test cleaner_oracle --test time_production_oracle --test runtime_trace_compat --test pipeline_branches_compat --test l0_runtime_compat --test embedding_runtime_compat --test dedup_runtime_compat --test recall_runtime_compat
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-store-sqlite adapter::tests::degraded_jsonl_newest_fifty_preserve_ts_prompt_order -- --exact
cargo test --manifest-path "$repo/Cargo.toml" -p aeon-memory-gateway --test production_runtime disabled_extraction_persists_l0_without_notifying_pipeline -- --exact
(cd "$repo" && shasum -a 256 "${oracle_files[@]}") > "$fixture_after"
diff -u "$fixture_before" "$fixture_after"

# Regenerate the fixed-seed randomized oracle and a fresh sqlite-vec database
# through the pinned TypeScript runtime on every parity run.
AEON_MEMORY_TS_BASELINE="$AEON_MEMORY_TS_BASELINE" "$repo/scripts/verify-ts-parity.sh"
AEON_MEMORY_TS_BASELINE="$AEON_MEMORY_TS_BASELINE" "$repo/scripts/verify-ts-sqlite-parity.sh"
AEON_MEMORY_TS_BASELINE="$AEON_MEMORY_TS_BASELINE" "$repo/scripts/verify-ts-l3-parity.sh"
AEON_MEMORY_TS_BASELINE="$AEON_MEMORY_TS_BASELINE" "$repo/scripts/verify-ts-utils-parity.sh"
AEON_MEMORY_TS_BASELINE="$AEON_MEMORY_TS_BASELINE" "$repo/scripts/verify-ts-scene-persona-parity.sh"
