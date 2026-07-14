# Host-neutral offload API contract

Offload is an optional core capability and is **disabled by default** (`offload.enabled=false`).
It has no agent-host types, paths, authentication profiles, hooks, telemetry, or backend-client
dependency. A host opts in and translates its native events into the DTOs below.

## Aggregated operations (3)

These operations may be exposed as three HTTP endpoints under the existing API namespace,
keeping the complete service within the ten-endpoint limit. They are also usable directly from
CLI/library code. Routes are deliberately not wired in this core contract.

### `before_prompt`

Request:

```json
{"agent_id":"main","session_id":"s1","system_prompt":"...","user_prompt":"...","messages":[],"context_window":200000}
```

Response:

```json
{"messages":[],"context":{"totalTokens":1234,"messagesTokens":900},"active_mmd":null,"offload_enabled":true}
```

Semantics: load the session state and prior entries; inject an active Mermaid task summary when
one exists; reapply confirmed L3 replacements; calculate the pre-request context snapshot. With
offload disabled it returns messages unchanged and performs no writes.

### `after_tool`

Request:

```json
{"agent_id":"main","session_id":"s1","tool":{"toolName":"read","toolCallId":"call_1","params":{},"result":{},"error":null,"timestamp":"2026-07-13T00:00:00Z","durationMs":4},"messages":[],"context_window":200000}
```

Response:

```json
{"messages":[],"buffered_pairs":1,"l1_entries":[],"l2_updated":false,"context":{"totalTokens":1300},"compression":{"mode":"none","tokens_saved":0}}
```

Semantics: buffer the exact tool pair in process memory, run L1 at the configured threshold, run
L2 when its null-node/timeout condition fires, then apply mild/aggressive/emergency L3 policy. A
selected L1 flush drains its full configured selection in requests of at most five pairs. The
first two consecutive failures keep each chunk retryable; the third writes the TS-compatible
`[L1 degraded]` local fallback so the queue cannot remain stuck indefinitely.

### `llm_output`

Request:

```json
{"agent_id":"main","session_id":"s1","assistant_message":{},"usage":{"input_tokens":1234,"output_tokens":80},"finish_reason":"tool_use"}
```

Response:

```json
{"force_l1":false,"processed_entries":0,"l1_entries":[],"l2_updated":false,"state":{"lastOffloadedToolCallId":null}}
```

Semantics: this is observational only, matching the pinned TS `llm_output` hook. It does not
flush L1, run L2, or consume/persist the optional provider `usage`; pending work is deferred to
the next input or after-tool threshold. The DTO retains `usage` for forward compatibility, but
the current implementation must not claim provider-token tracking from this route.

## Persistence invariants

- The default data root is `~/.openclaw/context-offload`; an explicit `offload.dataDir` wins.
  Data is isolated at `<data_root>/<agent_id>/`; session rows use
  `offload-<session_id>.jsonl`; shared task diagrams live in `mmds/`, full results in `refs/`.
- Pending tool pairs and their L1 retry counters are memory-only. They are intentionally absent
  after process restart; no `pending-*.json` checkpoint is written or replayed.
- JSONL appends require a non-empty `tool_call_id`; tolerant reads skip corrupt/invalid rows.
- Tool IDs are indexed both verbatim and underscore-free to preserve Anthropic-ID matching.
- Reclamation is disabled below three retention days, protects the active MMD, retains at least
  15 MMD files per agent, and only removes aged orphan refs. Production schedules the first pass
  five minutes after startup and repeats every 24 hours; shutdown cancels the scheduler safely.

## Migration coverage and explicit remaining work

Implemented in `aeon_memory_core::offload`:

- [x] DTO/defaults, session paths, tolerant JSONL/state/registry storage and disk reclamation.
- [x] Exact TS local-LLM system prompts and byte-compatible L1/L1.5/L2 user-prompt builders;
  calls go exclusively through the host-neutral `LlmRunner` trait.
- [x] L1 five-pair request batching with raw refs, three-attempt degraded fallback and complete
  selected-queue draining; L1.5 task boundaries/transitions,
  and independent L2 null/timeout/wait selection, Mermaid update, and node backfill.
- [x] Injectable `Tokenizer` algorithm boundary, provider-truth snapshot arithmetic and the
  original heuristic fallback. A host supplies cl100k/o200k BPE without coupling core to a
  provider; fixtures prove filtering, JSON overhead and user-prompt deduplication.
- [x] Mild/aggressive/emergency L3 selection, normalized-ID result replacement, protected MMD
  messages, active-MMD pair-safe injection, and budgeted full/meta history-MMD reconstruction.

Gateway routes and CLI commands are intentionally outside this core contract and are wired by
the gateway crate. Host-specific auth lookup, hook registration, tracing, state
reporting, and backend transport remain adapters and must not enter core.
