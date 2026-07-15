# Aeon Memory

[![CI](https://github.com/HuChundong/aeon-memory/actions/workflows/pr-ci.yml/badge.svg)](https://github.com/HuChundong/aeon-memory/actions/workflows/pr-ci.yml)
[![Release](https://github.com/HuChundong/aeon-memory/actions/workflows/release.yml/badge.svg)](https://github.com/HuChundong/aeon-memory/actions/workflows/release.yml)
[![npm](https://img.shields.io/npm/v/%40aeon-memory%2Fopencode)](https://www.npmjs.com/package/@aeon-memory/opencode)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[简体中文](README.md) · **English**

Aeon Memory is an independent, lightweight, cross-platform memory service for
AI agents. It turns ongoing conversations into layered facts, scenarios, and
user profiles while preserving raw evidence for drill-down. It provides a
native CLI, a standalone HTTP server with exactly ten application routes, and
an npm-installable OpenCode integration. The runtime does not require Node.js
or Docker.

## Highlights

- Layered memory: L0 raw conversations, L1 structured atoms, L2 scenarios, and
  L3 persona, with evidence preserved for drill-down.
- One Rust core, two alternative interfaces: a direct local CLI and a stable
  HTTP service.
- Local-first JSONL, Markdown, SQLite, FTS5, and sqlite-vec storage.
- Native Linux, macOS, and Windows release artifacts; Docker is optional.
- Keyword retrieval without embeddings, and hybrid/vector retrieval when an
  OpenAI-compatible embedding endpoint is configured.
- Automated regression coverage for the core state machine, storage,
  retrieval, HTTP API, CLI, and host lifecycle.

## Quick start

Download the archive for your platform from
[GitHub Releases](https://github.com/HuChundong/aeon-memory/releases), verify it
against the release `SHA256SUMS`, extract it, and keep the two binaries and the
bundled `vec0` extension together.

Copy `aeon-memory.example.yaml` to `aeon-memory.yaml` and configure at least an
OpenAI-compatible chat-completions endpoint:

```yaml
server:
  host: "127.0.0.1"
  port: 8420
  apiKey: "replace-with-a-strong-random-token"
  corsOrigins: []

llm:
  baseUrl: "https://your-provider.example/v1"
  apiKey: "your-llm-key"
  model: "your-chat-model"
  maxTokens: 4096
  timeoutMs: 120000

memory:
  recall:
    enabled: true
    strategy: "keyword"
  embedding:
    enabled: false
  offload:
    enabled: false
```

Start the service:

```bash
./aeon-memory-server --config ./aeon-memory.yaml
curl http://127.0.0.1:8420/health
```

Or build from source with Rust 1.85+:

```bash
git clone https://github.com/HuChundong/aeon-memory.git
cd aeon-memory
cargo build --locked --release --bins
```

## CLI without the service

The CLI opens the same local core and data directory directly; it is not an
HTTP client. Stop the server before running the CLI against the same data.

```bash
aeon-memory --config aeon-memory.yaml status
aeon-memory --config aeon-memory.yaml capture --user 'Prefer English' --assistant 'Noted' --session-key demo
aeon-memory --config aeon-memory.yaml recall --query 'What is my language preference?' --session-key demo
aeon-memory --config aeon-memory.yaml search memories --query 'English' --limit 5
aeon-memory --config aeon-memory.yaml session end --session-key demo
```

## OpenCode integration

Start `aeon-memory-server`, then install and restart OpenCode:

```bash
npx @aeon-memory/opencode@latest install
npx @aeon-memory/opencode@latest status
```

The installer adds the exact published npm version to OpenCode's global npm
directory and writes a standard plugin tuple to the existing
`~/.config/opencode/opencode.jsonc` or `opencode.json`. If both files exist it
stops and requires an explicit selection. Set `gatewayUrl` and, when server
authentication is enabled, `apiKey` in the selected file. The plugin recalls
before each user turn, captures completed user/assistant pairs, and flushes
only on a true session or instance lifecycle boundary. See
[the plugin documentation](integrations/opencode/README.md) for every option.

## HTTP API

The application surface is intentionally limited to:

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | Local core health |
| POST | `/recall` | Stable context plus strategy-selected dynamic L1 context |
| POST | `/capture` | Commit an L0 turn |
| POST | `/search/memories` | Search L1 atoms |
| POST | `/search/conversations` | Search L0 conversations |
| POST | `/session/end` | Flush pending session work |
| POST | `/seed` | Import history |
| POST | `/offload/before-prompt` | Before-prompt offload |
| POST | `/offload/after-tool` | After-tool offload |
| POST | `/offload/llm-output` | LLM-output boundary |

`/health` is public. All other routes require `Authorization: Bearer <token>`
when `server.apiKey` is set.

## Data and security

The default data directory is `~/.aeon-memory/data`. The precedence is
`AEON_MEMORY_DATA_DIR`, explicit `data.baseDir`, then the default. Stop the
service and back up the complete directory before upgrades.

For a service-managed native upgrade, stop the old process before switching to
the complete new package, then restart it. Verify both
`aeon-memory-server --version` and `/health` report the target version; a new
file on disk with an old `/health` version means the old process is still
running and must be restarted.

Keep the service on `127.0.0.1` unless remote access is required. For any
non-loopback binding, use a strong API key, firewall rules, TLS termination,
and a minimal CORS allowlist. Never commit credentials or production memory.
Report vulnerabilities privately according to [SECURITY.md](SECURITY.md).

## Quality and development

Executable fixtures, behavior oracles, and real SQLite integration tests cover
the memory pipeline, persistence, retrieval, CLI, HTTP API, and OpenCode
lifecycle. Public behavior changes must include regression tests and review.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

cd integrations/opencode
npm ci
npm run typecheck
npm test
npm run pack:check
```

Version tags matching `vX.Y.Z` trigger a release workflow that validates Cargo
and npm version alignment, builds five native archives, publishes a GitHub
Release with checksums, and publishes `@aeon-memory/opencode` through npm
Trusted Publishing.

See [CONTRIBUTING.md](CONTRIBUTING.md), [RELEASING.md](RELEASING.md), and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before contributing.

## Source, license, and acknowledgements

Aeon Memory's layered-memory and context-offload core is derived from the
open-source
[TencentDB Agent Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)
project and continues under its MIT license. We sincerely thank its maintainers
and contributors for creating and sharing the original design and code. Aeon
Memory is independently maintained and is not an official distribution of the
original project or its affiliated company.

Aeon Memory is available under the [MIT License](LICENSE). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for complete notices. Our
thanks also go to the sqlite-vec, SQLite, Rust, OpenCode, and dependency
communities.
