# Real-process TypeScript/Rust differential gateway suite

This suite treats the pinned TypeScript gateway as the executable oracle. It
starts two isolated gateway processes, gives each its own data root and port,
and drives both through HTTP. It never imports either implementation's core and
never modifies the TypeScript checkout.

## Run

Prepare the read-only baseline exactly as documented in
`TS_DIFFERENTIAL_BASELINE.md`, then run:

```sh
AEON_MEMORY_TS_BASELINE=/absolute/path/to/pinned-installed-baseline \
  ./scripts/differential-gateway.sh
```

Use `--keep` to retain all logs, YAML files, SQLite databases, JSONL files,
checkpoints, mock requests and `DIFFERENTIAL_REPORT.json` after a passing run.
A failed run is always retained and prints its temporary directory.

## What is compared

- Shared semantics of the seven official routes: health, recall, capture,
  memory/conversation search, session end and seed. The three Rust-only offload
  routes remain covered by their TS-derived oracle tests.
- Authentication, CORS preflight, malformed JSON, missing fields, unknown
  routes, Unicode and a 256-KiB turn.
- Capture latency with a deterministic three-second LLM delay. Capture must
  return in under one second, proving that L1 is not awaited by the request.
- Warm-up thresholds 1, 2, 4 and steady-state 5, including observable L1/L2/L3
  request ordering.
- A one-second idle debounce, session-end flush, two interleaved sessions,
  deterministic LLM failures/retries and process restart recovery.
- Search results and durable SQLite, JSON/JSONL, checkpoint, scene and persona
  state after shutdown.

`scripts/mock-openai.mjs` is an OpenAI-compatible chat/embedding server. Its
control endpoints are private to the test process:

- `POST /__control` sets `delayMs`, `failNextPerKey` and `failStatus`.
- `POST /__reset` clears control state and the event log.
- `GET /__log` returns request type, authorization identity, sequence,
  monotonic start/end time, status and exact body.

## Equality policy

The suite is fail-closed. HTTP statuses and business JSON are exact. Search
presentation timestamps, health uptime, seed duration/output directory and
generated L0 IDs are normalized only where `APPROVED_DIFFERENCES.md` explicitly
permits it. LLM request type/count/order/status and persisted record contents
have no tolerance. Any new normalization must first be justified and added to
that allow-list.

The suite intentionally reports all scenarios it can finish instead of
stopping at the first mismatch. This makes a single run useful for fixing a
cluster of parity regressions while still exiting non-zero if any mismatch is
present.

Scheduler assertions use causal polling with hard deadlines, not a fixed sleep.
The warm-up scenario sets `l2MaxIntervalSeconds` to 60 so the periodic L2 poller
cannot race the one-second delay-after-L1 path. It requires L1 batches at
cumulative turns 1, 3, 7 and 12, three corresponding dedup calls, and exactly
four L2/L3 pairs. This scenario sets `l2DelayAfterL1Seconds` and
`l2MinIntervalSeconds` to zero: each completed L1 deterministically makes its
L2 due, while `l2MaxIntervalSeconds=60` keeps the periodic poller out of the
test window. Sampling continues until the fourth L1's downstream dedup and all
four L2/L3 pairs complete, or a hard timeout records a failure. Unknown task
types, dedup-before-corresponding-L1, L2-before-first-L1 and
L3-before-corresponding-L2 all fail the run.

Durable JSON and JSONL are parsed and recursively emitted with canonical key
ordering; JSONL records are canonically sorted because cross-session worker
completion order is not deterministic. Random message/L0/L1 IDs are mapped to
tokens derived from their business identity, so source-ID relationships and
multiplicity remain checkable even when concurrent sessions finish in another
order. Named wall-clock fields are replaced by scoped tokens; counts, state
flags, content, scene data and input timestamps remain exact.
`last_l1_cursor` is not discarded: the snapshot also records its derived L0
boundary per session (processed/remaining row counts, exact-boundary membership
and maximum processed input timestamp).
The idle/retry scenario filters its comparison to L1/dedup plus HTTP status;
L2/L3 causality is asserted independently by the warm-up scenario.
