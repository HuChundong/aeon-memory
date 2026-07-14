# TypeScript / Rust Test-Parity Inventory

Baseline inspected on 2026-07-13:

- TypeScript: `TencentDB-Agent-Memory` at
  `4339e63650920871eb0e8888083a1779d114e3ae` (`0.3.6`).
- Rust: this repository at `ae53c32f61914e3d9a635e335730084b9b4aeea6`.

This document records evidence, not an assertion of parity.  A Rust-only unit
test proves internal consistency; it does **not** prove equivalence to the
TypeScript implementation.  A checked-in fixture proves equivalence only when
its provenance is reproducible from the pinned TypeScript baseline.

## Current executable evidence

| Suite | Result | Interpretation |
|---|---:|---|
| `cargo test --workspace --locked -- --list` | Dynamic inventory; 330-test snapshot on 2026-07-13 | The command is the source of truth and must be rerun for release evidence; the snapshot is not a stable invariant. Broad Rust testing includes executable TS-derived oracle suites, while Rust-only tests remain internal-consistency evidence rather than parity proof. |
| `npm test` in TS checkout | Not runnable before dependency install (`vitest: command not found`) | No installed baseline test runner. |
| `npm ci` in TS checkout | Fails: `package.json` and `package-lock.json` are out of sync | The TS baseline is not reproducibly installable with the lock file. |
| TS test inventory | 4 Vitest files / 67 `it(...)` cases; 2 Hermes Python files / 21 `test_*` functions | Core pipeline behavior is overwhelmingly untested by the original suite. |
| TS `.github/workflows/pr-ci.yml` | Does not invoke `npm test` | Upstream CI never gates the 67 Vitest cases. |
| Rust `.github/workflows/pr-ci.yml` | locked fmt/clippy/test, pinned-TS golden/config/search/SQLite/pipeline/L3 regeneration, real HTTP+seed differential, release build | The release gate exercises all currently checked-in differential suites. |
| `npm test` in `integrations/opencode` | 44-test snapshot on 2026-07-13 | Includes a real `aeon-memory-server` gateway E2E across mild/aggressive/MMD flows. SIGTERM and test-isolation sealing gates passed as recorded below. |

Inventory totals are observations, not compatibility criteria. Release evidence
must be regenerated from the commands above; a retained count must always carry
its snapshot date and command.

## Direct test-file mapping

| TypeScript test | Behavior | Rust evidence | Status |
|---|---|---|---|
| `src/utils/time.test.ts` | timezone/offset/DST and formatting | pinned-TS `utils_oracle` and `time_production_oracle` + Rust `TimeContext` | **Live dual-run gated.** UTC/IANA/DST/fixed-offset date, datetime, local-midnight, LLM-offset and production clock/config boundaries are covered. Process-clock-only assertions remain ordinary Rust tests. |
| `src/utils/sanitize.test.ts` | injection, L0/L1 gates, sanitizers | pinned-TS `utils_oracle` | **Live dual-run gated.** Existing injection vectors plus framework commands, short/ordinary content, XML boundaries and fenced-code byte output. |
| `src/utils/no-think-fetch.test.ts` | seven strategies and config helpers | pinned-TS `utils_oracle` | **Live dual-run gated.** All seven body transforms, existing-field merge, unknown passthrough, valid/invalid values and boolean/undefined normalization. Non-string Fetch `BodyInit` is transport-specific passthrough. |
| `src/offload/auth-profile-key.test.ts` (5 cases) | host SDK auth-profile lookup | none (host-specific feature intentionally removed) | **Approved removal.** The standalone runtime has no OpenClaw auth-profile store. |
| `hermes-plugin/.../test_gateway_shutdown_leak.py` | Hermes process shutdown/leak behavior | gateway `process_boundary.rs` | **Not equivalent.** Rust tests its independent server; Hermes adapter behavior was removed. |
| `hermes-plugin/.../test_memory_tencentdb_recovery.py` | Hermes recovery behavior | pipeline checkpoint tests | **Not equivalent.** Similar concern, different observable boundary. |

## Core behavior mapping

The following maps production modules even where the TS project has no native
test.  `Golden` means a fixture claims to have been produced by executing TS;
`Rust-only` means no TS oracle is exercised during the test.

| Behavior / TS source | Rust implementation and tests | Parity evidence | Gap / required differential oracle |
|---|---|---|---|
| Config (`src/config.ts`, gateway config) | `aeon-memory-core/config`, `aeon-memory-gateway/config`, `config_search_oracle` | Fixed-seed pinned-TS normalization oracle | Defaults, full/minimal configs, aliases, invalid-value fallback, cleanup/offload coercion and randomized representative inputs are live-gated. |
| L0 extraction/recording (`l0-recorder.ts`) | `record/l0_recorder.rs` plus `l0_runtime_compat.rs` | Pinned TS runtime oracle | Cursor, position slicing, original-user replacement, structured content/base64 removal, assistant code stripping, returned records and persisted JSONL fields are exact. Generated IDs/recording wall clock are intentionally excluded. |
| Auto-capture (`auto-capture.ts`) | unit tests plus real gateway differential | Real TS/Rust capture and persistence | Successful turn count/notification, SQLite/JSONL persistence, restart, common per-turn `recorded_at`, error and empty-input boundaries are exercised. Remote embedding wire behavior is independently pinned by the runtime embedding oracle. |
| Auto-recall (`auto-recall.ts`) | runtime oracle, unit tests and real gateway differential | Fixed-commit populated TS runtime transcript + exact Rust keyword output | Actual TS mocks pin populated keyword final text/metadata, Unicode-aware per-line and total budget, FTS query/limit, hybrid RRF ordering, vector ordering, embedding timeout forwarding, and overall timeout returning no injection. Rust exact-compares the populated keyword contract and separately executes hybrid, fallback and non-blocking timeout paths. |
| L1 extraction (`l1-extraction.ts`) | prompt, extractor and production pipeline | Byte-exact stable/dynamic prompts, deterministic mock LLM unit paths and real gateway/seed transcripts | Parsed/fenced output, model request messages, storage calls, empty/failure behavior and persisted production effects are covered; LLM content quality is intentionally outside deterministic parity. |
| Degraded L1 fallback | `l1-extraction`, `offload_complete`; `degraded_l1_oracle` | Pinned-TS newest-50 prompt ordering and fallback fixture plus Rust production-path replay | The bounded history selection, ordering and degraded write behavior are deterministic gates; live model output quality is outside parity. |
| L1 dedup (`l1-dedup.ts`) | prompt and `record/l1_dedup.rs` | Fixed-commit TS decision/call transcript + Rust state tests | Actual TS mocks pin batch embedding, timeout forwarding, STORE/SKIP/UPDATE, target/merge payload, no-recall and LLM-error fallback. Rust now batches embeddings, degrades vector failure to FTS, uses the exact LLM task/timeout contract and stores all on LLM failure. Writer persistence remains covered by extractor/writer store tests. |
| L1 writer/reader | `l1_writer.rs`, SQLite store, golden/runtime/store suites | TS byte-exact JSONL plus SQLite schema/data roundtrip | Optional fields/defaults, Unicode/escaping, append/upsert/delete/read and ordering are exercised through writer and cross-runtime store tests. |
| Search utils / SQLite FTS | `search.rs`, `fts_query.rs`, store; `config_search_oracle`, SQLite parity | Fixed-seed randomized TS oracle plus TS-created database | Dense BM25 numeric corpus, randomized RRF ties/duplicates/order/payloads, Chinese/English FTS tokenization and exact common-DB search behavior are live-gated. |
| Embedding / vector search | `embedding/openai.rs`, SQLite adapter | Fixed-commit TS provider runtime transcript + Rust local HTTP protocol tests + TS-created DB fixture | The actual TS provider calls a local mock and pins authorization/body, UTF-16 truncation, index sorting, Float32 normalization, empty data, missing data, malformed values, dimension mismatch, and zero-HTTP empty batch. Rust exercises the same protocol and exact transcript contract; common-DB KNN order remains covered separately. |
| Model/provider request semantics | `llm/openai`, no-think transforms, gateway/offload model config tests | Local OpenAI-compatible mock transcripts and pinned configuration/request fixtures | Model name, temperature, timeout, request shape and disable-thinking behavior are deterministic gates. Live supplier authentication, availability, rate limits and output quality remain deployment smoke boundaries, not parity proof. |
| Scene extraction/format/index/navigation | `scene.rs`, `profile.rs`; `runtime_trace_compat`, `scene_persona_fs_oracle` | Pinned-TS parsing, formatting, filename normalization, navigation, success merge/delete/index cleanup, persona signal, failure restoration, backup and full filesystem snapshots | Within the pinned baseline and `APPROVED_DIFFERENCES.md` scope, covered deterministic branches have no identified mismatch; broader model-behavior quality remains non-deterministic. |
| Persona generation/trigger | `persona.rs`, `profile.rs`; `persona_trigger_oracle`, `scene_persona_fs_oracle` | Pinned-TS oracle for all five trigger priorities and recovery conditions; first/incremental/skip/failure runner branches, checkpoint advance, XML sanitation, navigation, backup and full filesystem snapshots | Within the pinned baseline and `APPROVED_DIFFERENCES.md` scope, covered deterministic branches have no identified mismatch. |
| Pipeline manager/checkpoint | `pipeline/manager.rs`, `checkpoint.rs`; `runtime_trace_compat` and `pipeline_branches_compat` | TS-executed deterministic traces plus Rust branch tests | Main flow, retry success/exhaustion, checkpoint recovery, idle reset, multi-session ordering, shutdown drain and L3 suppression compare event-for-event. This is representative branch coverage, not proof over every possible concurrent interleaving. |
| Seed input/runtime | `seed/*` plus real-gateway differential | Real TS/Rust HTTP seed with deterministic mock LLM | Success response fields/path convention, exact normalized LLM messages, isolated SQLite L0 rows and live-store isolation match. Nested `config_override`, invalid-value fallback and non-mutation of live config are replayed. Only measured `duration_ms` is normalized as runtime timing. |
| Memory/conversation tools | `tools/*` | live formatter oracle plus Rust store/embedding execution tests | Empty/message/non-empty response formatting is byte-exact against TS; hybrid ordering, filters, limits and capability errors are covered by deterministic Rust fakes sharing already dual-run RRF/FTS primitives. Remaining external-store failures are adapter fault tests, not formatter parity. |
| `AeonMemoryCore` facade | `aeon-memory_core.rs`, real gateway differential, production runtime tests | Shared public operations execute through the real TS and Rust facades | Recall, capture, both searches, session end and seed are compared at the transport boundary with SQLite/filesystem persistence and restart; Rust-only lifecycle composition separately verifies initialization, shutdown and profiles. Host plugin registration is an approved adapter removal. |
| L1/L1.5/L2 offload hooks/state/prompts/parsers | `offload/*`, `offload_parser_oracle`, `offload_state_oracle`, `offload_complete`, `offload_runtime_oracle.json` | Pinned-TS prompt/parser replay plus an actually executed `registerOffload` fixture covering `before_tool_call`, `after_tool_call`, `llm_output`, pending state and filesystem output | Pending pairs/retry counters are process-only; `llm_output` is observational and does not consume usage or run L1/L2. Rust tests cover restart clearing, no `pending-*.json`, three-failure degraded fallback, 12-pair draining in ≤5-pair calls, L1.5/L2 state and filesystem effects. Provider usage remains accepted-but-unused exactly as in the pinned TS hook. |
| L3 reclaim + token tracking + compression | `offload/reclaim.rs`, `offload/token.rs`, `offload/l3.rs`; pinned-TS generated `reclaim_oracle.json`, `l3_token_oracle.json`, `l3_compression_oracle.json`, runtime scheduler test | **Live dual-run gated.** Exact `o200k_base` counts over fixed boundary + 64-case seeded Unicode corpus, including TS's rejected-special fallback with JS UTF-16 length semantics. Compression compares byte-exact mild cascade; aggressive threshold/last-user behavior; emergency target/min-two, stalled-head tail tool-pair deletion and oversized truncation; and history/injection/active MMD extraction/reinsertion ordering. Reclaim compares every stat and persisted tree; a real short-delay injected scheduler test verifies execution and shutdown cancellation while production uses the TS 5-minute/24-hour cadence. | Product scope fixes encoding to `o200k_base`; alternate encodings are unsupported. Remaining limitations are non-functional fault/performance domains: permission/race injection and cache identity/performance. Mermaid remains separately Rust-tested. |
| HTTP gateway | `aeon-memory-gateway` integration tests plus `scripts/differential-gateway.sh` | Both real gateways: six shared POST routes, health/404, malformed and missing input, capture/search, restart persistence, deterministic seed success and normalized LLM requests | Shared tested behavior is gated. Dynamic seed duration/output directory names are explicitly normalized; Rust-only offload routes are new contracts rather than parity claims. |
| OpenCode adapter | `integrations/opencode` test suite | 44-test snapshot plus real Rust gateway E2E | Recall/capture integration and mild/aggressive/MMD flows execute against a real `aeon-memory-server`. SIGTERM exits cleanly with status 0, and the cross-process isolation gate passes in three independent Cargo processes. |
| CLI | `aeon-memory-gateway` interface/process tests | Rust-only; surface differs from upstream TS plugin/seed CLI | Treat as new API contract. Core effects must be compared through shared operation transcripts. |
| Session filter/env | `utils/session_filter.rs`, standard env access | pinned-TS `session_filter_oracle` plus `utils_oracle` | **Live dual-run gated.** Built-ins, user globs, trigger/key detection, missing/session-internal contexts and set/missing env reads match. |
| Daily memory cleaner | `utils/memory_cleaner.rs`, SQLite expiration methods, standalone runtime scheduler; `cleaner_oracle` | Pinned-TS filesystem retention oracle plus real SQLite threshold/deletion test | Date-shard recognition/deletion, non-shard handling, L0=50/L1=20 guards, cutoff deletion and production daily scheduling are implemented. Permission failures remain non-functional fault injection. |
| Reporter | intentionally omitted standalone telemetry adapter | TS source inspection: local reporter only emits fail-soft structured logger messages; its instance ID file is host installation metadata, not memory state | Approved host presentation/installation-metadata removal; no capture, recall, search, pipeline, SQLite or profile state is changed by report calls. |
| Persona/scene backup and restore | private helpers in `persona.rs` and `scene.rs`; `scene_persona_fs_oracle` | Pinned-TS category layout/naming, shallow-copy content, max-keep pruning/unlimited retention, persona pre-run backup and scene failure restoration snapshots | Within the pinned baseline and `APPROVED_DIFFERENCES.md` scope, covered file/directory backup behavior has no identified mismatch. |
| TCVDB/BM25 remote backends | deliberately absent/lightweight SQLite target | none | Must be an approved scope difference; cannot be described as language variance. |

## Final sealing gates

- **SIGTERM shutdown/restart: passed.** The real process-boundary test exits
  with status 0 after clean termination and state flushing for the current
  server and OpenCode adapter build.
- **Test isolation: passed.** Three independent Cargo processes each completed
  39/39 tests, covering environment, user-directory, port, process and database
  isolation across cases and reruns.

These results are release-sealing evidence for the inspected snapshot and must
be rerun when the server, adapter or test harness changes.

## Golden provenance harness

The original generators depended on deleted source/Git objects in `aeon-memory`.
This is now replaced by an explicit pinned baseline:

1. `scripts/prepare-ts-baseline.sh` clones commit `4339e636...` into a
   disposable directory and resolves dependencies with pinned npm 11.11.0,
   `--ignore-scripts`, and a required generated-lock SHA-256. It verifies the
   business-source tree hash before and after installation.
2. All generators require `AEON_MEMORY_TS_BASELINE`; verification wrappers reject any
   checkout not at the pinned commit.
3. `scripts/verify-ts-goldens.sh` executes TS, runs the Rust compatibility
   suites, and fails if generated outputs drift.
4. CI prepares the pinned checkout and runs both golden verification and the
   real-gateway differential test.

This flow was replayed successfully against a fresh temporary clone on
2026-07-13: 7 golden, 8 offload, 2 runtime-trace, 4 randomized config/search,
and 15 live SQLite compatibility tests passed, with no generated-output drift.

## Acceptance rules

- Exact equality: JSON values (after explicitly documented field-order
  normalization), strings/prompts, IDs under fixed clock/entropy, DB rows,
  filesystem content, event order, HTTP status/body, search order.
- Numeric tolerance: only where runtime floating-point implementation differs.
  RRF/BM25 use the `1e-12` absolute bound exercised by the fixed-seed dense
  corpus. No broader blanket tolerance is allowed.
- LLM nondeterminism is not acceptable as a tolerance.  Both implementations
  must receive the same stubbed LLM/embedding responses and their outgoing
  requests must be compared.
- Removed host adapters/backends are intentional scope differences, not parity.
  [`APPROVED_DIFFERENCES.md`](APPROVED_DIFFERENCES.md) is the authoritative
  allow-list; an unlisted reachable mismatch is a Rust defect.
- A mismatch outside that file is a Rust defect until evidence proves the TS
  oracle or harness is wrong.

## Approved scope differences and residual boundaries

The complete authoritative list is in
[`APPROVED_DIFFERENCES.md`](APPROVED_DIFFERENCES.md). Approved removals are the
OpenClaw/Hermes host adapters, fail-soft reporter telemetry/installation
metadata, and the remote TCVDB backend. The standalone Rust CLI and three
offload HTTP routes are new contracts covered by Rust integration tests; they
have no TypeScript transport equivalent.

Within the pinned baseline and the authoritative `APPROVED_DIFFERENCES.md`
allow-list, current deterministic gates have not identified a reachable
business-path mismatch in the standalone SQLite scope. This is a bounded
evidence statement, not a universal equivalence claim. Residual boundaries are
non-functional or inherently unbounded: permission and race fault injection,
cache/performance identity, arbitrary model-output quality, the complete
mathematical input domain, and concurrency interleavings beyond the
deterministic main/retry/recovery/timer traces.

Real-supplier smoke is a separate release boundary. Local mocks and recorded
wire contracts prove request compatibility, but do not prove current
credentials, regional reachability, model availability, rate-limit behavior or
live output quality for a configured DeepSeek/OpenAI-compatible chat or
embedding provider. Those checks must be rerun in each target deployment and
must not be presented as deterministic TS/Rust parity evidence.
