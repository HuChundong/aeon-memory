# TencentDB Agent Memory — standalone Rust distribution

This workspace is the standalone Rust implementation of TencentDB Agent Memory.
It preserves the L0 → L1 → L2 → L3 memory model and optional context offload
without any agent-host runtime dependency.

## Current delivery status

The core, SQLite store, production composition, CLI/HTTP transports and
their contracts are linked. Both binaries parse the same YAML/JSON config and
compose the SQLite store, external LLM, embedding service, pipeline and
optional offload engine. Run the completion gates at the end of this document
on the intended data and platform before production deployment.

## Default delivery: native single-service archives

Docker is not required. The native release workflow is configured to build
archives for Linux x86_64/ARM64, macOS Intel/Apple Silicon and Windows x86_64,
but no public GitHub Release is currently published. After a successful tagged
release, every archive contains
`aeon-memory`, `aeon-memory-server`, the platform's `vec0` loadable extension,
`aeon-memory.yaml`,
checksums, licenses and a Chinese quick-start guide.

The release workflow pins sqlite-vec to `0.1.9`, downloads explicit upstream
asset names (never `latest`), and verifies each asset against its upstream
SHA-256 before packaging. Keep vec0 next to the executable and it is discovered
automatically. See [README_CN.md](README_CN.md) for end-user installation,
configuration, API, backup and upgrade instructions.

## Build

The workspace currently targets Rust 1.88+ (edition 2024):

```bash
cargo test --workspace
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --locked --release --bins
ls -lh target/release/aeon-memory target/release/aeon-memory-server
```

The release profile enables thin LTO, one codegen unit, symbol stripping and
abort-on-panic. Source builds do not download sqlite-vec. Place the verified
platform extension next to the binaries or set `AEON_MEMORY_VEC0_PATH`.

### Optional Docker image

The native archive is the default delivery. If an existing deployment requires
containers, build the optional multi-stage image with:

```bash
docker build -f Dockerfile.rust -t aeon-memory-rust:local .
```

The image runs as UID/GID 10001. Mount a real config and persistent data:

```bash
docker run --rm -p 127.0.0.1:8420:8420 \
  -v "$PWD/aeon-memory.yaml:/etc/aeon-memory/aeon-memory.yaml:ro" \
  -v "$HOME/.aeon-memory/data:/var/lib/aeon-memory/data" \
  -v "/absolute/path/vec0.so:/usr/local/lib/vec0.so:ro" \
  -e AEON_MEMORY_VEC0_PATH=/usr/local/lib/vec0.so \
  aeon-memory-rust:local
```

For the container example, set `server.host: "0.0.0.0"` in the mounted config;
the example file intentionally defaults to host-only `127.0.0.1` for safer
non-container use. Keep the published Docker port bound to host loopback as
shown unless network exposure is intentional and authenticated.

`Dockerfile.rust` deliberately does not download an unpinned native extension.
Use an official `sqlite-vec` binary matching the container architecture and
mount it as shown.

## Configuration

Copy [`config/aeon-memory.example.yaml`](config/aeon-memory.example.yaml) to
`aeon-memory.yaml`. Fields use the existing camelCase schema.

- `server.host`, `server.port`, `server.apiKey`, `server.corsOrigins` define the
  HTTP listener. Keep loopback binding unless Bearer authentication is set.
- `data.baseDir` is the root for all persistent memory data.
- `llm` configures an OpenAI-compatible `/chat/completions` API.
- `memory.embedding` optionally configures an OpenAI-compatible `/embeddings`
  API. When enabled, `dimensions` must match the model and existing vector
  metadata. Without it, JSONL, SQLite/FTS keyword retrieval and the L0 → L3
  pipeline remain available; vector recall/deduplication is unavailable and
  hybrid/embedding execution degrades to keyword capability.
- `memory.storeBackend` must remain `sqlite` for this standalone build. The
  config type still parses `tcvdb` for source compatibility, but this Rust
  workspace does not currently contain a wired TCVDB store crate.
- `memory.offload.enabled` defaults to `false`. Its three host-neutral routes
  return an invalid-input response while offload is disabled.

The Rust config parser supports the same YAML/JSON values and discovery names
as the original standalone service: `AEON_MEMORY_GATEWAY_CONFIG`, then
`aeon-memory.yaml`/`aeon-memory.json` in the current directory, then those
names under the default data directory. An explicit `--config <path>` remains
supported by both binaries; a pure-environment configuration also remains
valid when none of those files exists. The data-directory precedence is
`AEON_MEMORY_DATA_DIR`, explicit `data.baseDir`, then the canonical
`~/.aeon-memory/data` default. `~` uses `HOME` on Unix and `USERPROFILE` on Windows.
`AEON_MEMORY_VEC0_PATH` is independently consumed by the SQLite store.
The original `AEON_MEMORY_GATEWAY_PORT/HOST/API_KEY`, `AEON_MEMORY_CORS_ORIGINS`, and
`AEON_MEMORY_LLM_*` field overrides remain supported, as do whole-value `${VAR}`
placeholders in YAML/JSON. Relative paths resolve from the process working
directory. Startup writes the resolved config and data paths to stderr.

### sqlite-vec behavior

Extension discovery uses this priority: `AEON_MEMORY_VEC0_PATH`, the directory of the
running `aeon-memory` or `aeon-memory-server` executable, crate test fixtures,
dynamic-library paths (including Windows `PATH`), then Unix system library
paths. The executable-directory lookup is independent of the process working
directory and recognizes `vec0.so`, `vec0.dylib` and `vec0.dll`. When
the extension cannot load, FTS can still initialize but vector tables/search
are unavailable. Treat that degradation as a failed deployment when hybrid or
embedding recall is required.

Never change an existing store's embedding model or dimensions in place. Back
up the entire data directory first and reindex through a verified migration
tool; no standalone Rust reindex command is currently exposed.

## HTTP API: 7 memory routes + 3 offload routes = 10

`GET /health` is public. The nine POST routes require
`Authorization: Bearer <server.apiKey>` when an API key is configured. All
request and response bodies are JSON.

| Method | Route | Minimum request |
|---|---|---|
| `GET` | `/health` | none |
| `POST` | `/recall` | `{"query":"...","session_key":"s1"}` |
| `POST` | `/capture` | `{"user_content":"...","assistant_content":"...","session_key":"s1"}` |
| `POST` | `/search/memories` | `{"query":"..."}` |
| `POST` | `/search/conversations` | `{"query":"..."}` |
| `POST` | `/session/end` | `{"session_key":"s1"}` |
| `POST` | `/seed` | `{"data":{...}}` |
| `POST` | `/offload/before-prompt` | `{"agent_id":"main","session_id":"s1","system_prompt":"","user_prompt":"...","messages":[],"context_window":200000}` |
| `POST` | `/offload/after-tool` | see `OFFLOAD_API_CONTRACT.md` |
| `POST` | `/offload/llm-output` | see `OFFLOAD_API_CONTRACT.md` |

Example:

```bash
curl -sS http://127.0.0.1:8420/recall \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $AEON_MEMORY_API_KEY" \
  -d '{"query":"What does the user prefer?","session_key":"s1"}'
```

There are no `/stats`, `/memories` or `/reindex` HTTP routes. `OPTIONS` is CORS
protocol handling, not an application route.

`/health` reports core initialization and local store composition. Its
`stores.vectorStore` and `stores.embeddingService` fields do not probe remote
LLM or embedding endpoints. `/recall` returns stable persona/scene context and
`memory_count`; dynamic L1 memories are obtained from `/search/memories` when
that count is positive. `/capture` commits L0 and schedules the background
pipeline; `/session/end` flushes pending work. The `memory.capture.enabled` and
`memory.recall.enabled` settings gate host auto-hooks, not explicit HTTP or CLI
operations.

## CLI contract

CLI and HTTP share the same concrete service, but are alternative transports.
`aeon-memory` composes the core and opens the configured data directory directly; it
does not call `aeon-memory-server`, so no server needs to be running. Never run both
against the same data directory concurrently. CLI commands drain work they
schedule before exit. Every CLI invocation requires a config with an external
LLM; pass it globally as shown:

```bash
aeon-memory --config aeon-memory.yaml seed --input history.json --session-key import-1
aeon-memory --config aeon-memory.yaml capture --user 'hello' --assistant 'hi' --session-key s1
aeon-memory --config aeon-memory.yaml recall --query 'preferences' --session-key s1
aeon-memory --config aeon-memory.yaml search memories --query 'database' --limit 5 --type episodic
aeon-memory --config aeon-memory.yaml search conversations --query 'migration' --limit 10 --session-key s1
aeon-memory --config aeon-memory.yaml session end --session-key s1
aeon-memory --config aeon-memory.yaml status
aeon-memory --config aeon-memory.yaml show persona
aeon-memory --config aeon-memory.yaml show scenes
aeon-memory --config aeon-memory.yaml offload before-prompt --input before-prompt.json
aeon-memory --config aeon-memory.yaml offload after-tool --input after-tool.json
aeon-memory --config aeon-memory.yaml offload llm-output --input llm-output.json
```

`seed` accepts `--strict-round-role` and
`--auto-fill-timestamps=<true|false>`. The three offload input files follow
[`OFFLOAD_API_CONTRACT.md`](OFFLOAD_API_CONTRACT.md).

The package does not install a system service automatically. Administrators
must supply and maintain their own systemd, launchd, Windows Service Wrapper,
or equivalent unit with fixed configuration and data paths.

## Data compatibility

`data.baseDir` contains the complete portable L0 → L3 layout, including:

- `conversations/*.jsonl` for L0 conversation shards;
- L1 JSONL records and `vectors.db` (SQLite, FTS5 and optional vec0 tables);
- `scene_blocks/*.md` and scene indexes for L2;
- `persona.md` for L3;
- checkpoint and manifest files used by pipeline scheduling and store identity.

The default directory is `~/.aeon-memory/data`. Never run two
writers against the same directory concurrently.

Offload state is separate: it defaults to `~/.openclaw/context-offload`, and an
explicit `memory.offload.dataDir` wins. Back up that root separately whenever
offload is enabled.



## Production completion gates

Only checked items below have current execution evidence. The first five are
the native Rust CLI/HTTP delivery gates; the final container item is optional
and does not block the default native standalone deliverable.

- [x] `aeon-memory` and `aeon-memory-server` instantiate the concrete store, LLM, embedding,
  pipeline and offload service. `production_runtime` exercises the real
  composition; release binary help and missing-config failure paths were also
  executed.
- [x] The current workspace test suite, strict workspace Clippy, and
  `cargo build --workspace --locked --release --bins` pass on macOS arm64.
  Binary size is build-toolchain dependent and is not a compatibility
  guarantee.
- [x] One real compiled server-process run exercised all 10 routes with the concrete
  service, including Bearer rejection, the configured no-CORS-header policy,
  graceful Ctrl-C shutdown and persisted conversation search after restart.
  Transport tests separately cover the explicit CORS allow-list behavior.
- [x] TS/Rust compatibility fixtures cover JSONL, SQLite schema/data, FTS/RRF/
  prompt semantics and loaded vec0 rows. A focused test also proves an explicit
  `AEON_MEMORY_VEC0_PATH` file has highest discovery priority, executable-sibling
  discovery works, and missing/invalid extension files fail soft.
- [x] Every documented CLI command is covered by transport-to-service mapping
  tests. A real `aeon-memory capture` child-process test verifies L1/L2 drain before
  exit. Other commands are not claimed as latest-tree real release-process
  coverage.
- [ ] **Optional container publication:** the multi-stage image still
  needs a successful build and runtime smoke on each advertised architecture.
  The first Docker Hub pulls ended in
  EOF; a cached official linux/amd64 Rust-base fallback under arm64 QEMU first
  exhausted compiler resources and then remained impractically slow with one
  build job. This does not substitute for the default slim image and no image
  size or container runtime claim is made.

Repository evidence for the non-container gates:

- `crates/aeon-memory-core/tests/golden_compat.rs` and
  `offload_prompt_legacy_replay.json` contain outputs generated by executing
  the legacy baseline and assert byte-/score-/order-exact Rust parity.
- `crates/aeon-memory-store-sqlite/tests/compat.rs` opens the legacy SQLite fixture and
  verifies schemas, metadata, FTS/vector rows, and Rust round trips.
- `crates/aeon-memory-gateway/tests/process_boundary.rs` starts the real
  `aeon-memory-server`, exercises all 10 routes, Bearer and CORS policy, graceful
  shutdown, and restart persistence against a real temporary SQLite store.
- `crates/aeon-memory-gateway/tests/interface.rs` verifies every documented CLI
  command maps to the shared production service contract.
