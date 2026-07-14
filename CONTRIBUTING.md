# Contributing

TencentDB Agent Memory is a standalone Rust workspace. Changes to memory
semantics should include evidence that behavior remains compatible with the
golden fixtures and real SQLite integration tests.

## Development checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --locked --release --bins
```

The workspace contains:

- `crates/aeon-memory-core`: memory, pipeline, retrieval, persona, scene and offload logic.
- `crates/aeon-memory-store-sqlite`: SQLite, FTS5 and sqlite-vec persistence.
- `crates/aeon-memory-gateway`: shared CLI/HTTP service and the two binaries.

Keep the HTTP application surface at 10 routes or fewer. CLI and HTTP changes
must use the same service implementation. Never replace vector similarity,
LLM extraction, persistence, or pipeline stages with lexical/mock shortcuts in
production code.

Optional JavaScript under `scripts/` is limited to offline data inspection and
export. It must not become a second runtime implementation.
