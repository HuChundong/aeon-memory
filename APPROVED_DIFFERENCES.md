# Approved TypeScript / Rust differences

This is the authoritative allow-list for differences from the pinned
TypeScript baseline `4339e63650920871eb0e8888083a1779d114e3ae`. A reachable
business-path mismatch not listed here is a Rust defect.

## Removed host and storage adapters

- OpenClaw plugin registration, hook-policy mutation, auth-profile lookup and
  host lifecycle wiring are removed. The host-neutral operations and offload
  state machines they invoked remain implemented behind Rust traits, CLI and
  HTTP.
- The Hermes Python adapter, watchdog and child-process recovery wrapper are
  removed. The standalone Rust server owns its process lifecycle directly.
- Fail-soft reporter telemetry and its installation-instance metadata file are
  removed. Reporter calls do not mutate memory, checkpoint, search or pipeline
  state in the baseline.
- The remote TCVDB/BM25 backend is removed from the lightweight distribution.
  SQLite/FTS/sqlite-vec is the supported runtime store. Offline export tools
  remain available, but selecting `tcvdb` is not a supported runtime mode.

## New standalone surfaces

- The Rust CLI is a new transport over the shared core contract.
- The three `/offload/*` HTTP routes are new host-neutral transports for the
  baseline hook operations. Together with the seven memory/health routes, the
  complete HTTP application surface remains exactly ten routes.

These additions do not authorize different core effects; their state changes
are covered by Rust integration tests and the underlying TS-derived oracles.

## Allowed comparison normalization

- Pipeline `last_active_time`: absolute difference at most **10 ms**, solely
  because the fixed TS `Date.now()` oracle advances one millisecond per call
  while Rust injects one stable instant.
- RRF and BM25 floating-point results: absolute difference at most **1e-12**.
- Values produced from the real process clock are compared structurally rather
  than literally: capture/search presentation timestamps, persona checkpoint
  generation time, scene/persona backup timestamp, and seed output-directory
  timestamp. Their formats, surrounding content and persisted business fields
  are still exact-compared.
- Seed `duration_ms` is runtime timing and is excluded from value equality.
- Generated seed L0 IDs are normalized only when comparing the otherwise exact
  outgoing LLM prompt. Persisted row counts, roles, content, timestamps and
  isolation are compared exactly.

No LLM-output, prompt, HTTP status/error, record count, search order, file
content, checkpoint flag, retry decision or persistence effect has a tolerance.

## Residual verification boundaries

Permission failures, concurrent filesystem races beyond the deterministic
traces, cache/performance identity, arbitrary model-output quality and the
complete mathematical input domain are not claimed as exhaustively proven.
They are verification boundaries, not approved semantic differences.
