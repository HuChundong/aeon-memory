# TencentDB Agent Memory

A lightweight, standalone memory system implemented in Rust. It preserves the
L0 → L1 → L2 → L3 memory pipeline, SQLite/FTS/vector retrieval, persona and
scene generation, and optional context offload without depending on any agent
host or plugin runtime.

## Build and verify

The native release workflow is configured to build archives for Linux x86_64/
ARM64, macOS Intel/Apple Silicon, and Windows x86_64. No public GitHub Release
is currently published; archives become available only after a successful
tagged release. Each archive contains both binaries, a checksum-verified
sqlite-vec extension, config and instructions; Docker is optional. See
[README_CN.md](README_CN.md) for installation and upgrade details.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --locked --release --bins
```

The build produces two binaries:

- `aeon-memory`: CLI for capture, recall, search, session flush, seed and offload.
- `aeon-memory-server`: standalone HTTP server exposing exactly 10 routes.

These are alternative transports over the same local core. `aeon-memory` opens the
configured data directory directly; it is not an HTTP client for
`aeon-memory-server`. Do not run both against the same data directory at the same
time.

Copy `config/aeon-memory.example.yaml`, configure an OpenAI-compatible LLM
and optional embedding endpoint, then run:

```bash
target/release/aeon-memory-server --config aeon-memory.yaml
curl http://127.0.0.1:8420/health
```

Alternatively, stop the server and use `aeon-memory --config aeon-memory.yaml status`
directly against the local data directory.

## HTTP API

The application surface is limited to:

`GET /health`, `POST /recall`, `POST /capture`,
`POST /search/memories`, `POST /search/conversations`,
`POST /session/end`, `POST /seed`, and the three generic offload operations
under `/offload/*`.

`/health` confirms core initialization and local store composition; it does not
probe remote LLM or embedding endpoints. `/recall` returns stable persona/scene
context and `memory_count`; retrieve dynamic L1 memories through
`/search/memories` when that count is positive. `/capture` commits L0 and
schedules the background pipeline, while `/session/end` flushes pending work.
The LLM is required; embedding is optional, with keyword retrieval retained
when vector retrieval is unavailable.

See [README_RUST.md](README_RUST.md) for configuration, CLI examples, storage
compatibility and deployment details. See
[OFFLOAD_API_CONTRACT.md](OFFLOAD_API_CONTRACT.md) for the host-neutral offload
protocol.

## Optional data tools

The `bin/` and `scripts/` JavaScript utilities are offline migration/export
tools only. They are not part of the Rust runtime and do not provide an adapter.

## Legacy history

`CHANGELOG.md` and `RUST_MIGRATION_PLAN.md` retain historical references to the
former plugin implementations for provenance only. Those integrations are no
longer shipped or supported by the runtime.
