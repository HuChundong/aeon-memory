# Contributing / 参与贡献

Thank you for helping Aeon Memory. By participating, you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md). Security reports belong in the private
channel described by [SECURITY.md](SECURITY.md), not in public issues.

## Before starting

1. Search existing issues and pull requests.
2. Open an issue before a large feature, new public interface, storage format
   change, or intentional upstream behavior difference.
3. Keep changes focused. Do not mix unrelated cleanup with behavior changes.
4. Never include API keys, `.env` files, production memory, or user data.

## Architecture invariants

- `crates/aeon-memory-core`: memory pipeline, retrieval, persona, scenes, and offload.
- `crates/aeon-memory-store-sqlite`: SQLite, FTS5, and sqlite-vec persistence.
- `crates/aeon-memory-gateway`: shared CLI/HTTP service and both native binaries.
- `integrations/opencode`: the official Aeon Memory OpenCode npm integration.
- `scripts/`: offline migration, parity, and release helpers only; never a second runtime.

The HTTP application surface must remain at ten routes or fewer. CLI and HTTP
features must share the same service implementation. Production code must not
replace vector similarity, LLM extraction, persistence, or pipeline stages
with lexical or mocked shortcuts.

Changes to memory semantics require evidence against preserved TypeScript
goldens and real SQLite integration tests. Record unavoidable language or host
differences explicitly in the pull request and cover them with regression tests.

## Local checks

Rust 1.85+, Node.js 22+, and the platform sqlite-vec extension are required.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked --release --bins

npm ci
npm run build

cd integrations/opencode
npm ci
npm run typecheck
npm test
npm run pack:check
```

Use the targeted differential scripts under `scripts/` for core semantic
changes. Tests must be deterministic and must not read or write the
developer's real `~/.aeon-memory` or OpenCode configuration.

## Pull requests

- Explain the user-visible outcome, compatibility impact, and validation run.
- Add tests for behavior changes and update both Chinese and English docs when
  the public interface changes.
- Link the related issue where one exists.
- Do not update package versions or create tags in an ordinary pull request;
  maintainers follow [RELEASING.md](RELEASING.md).

---

感谢参与 Aeon Memory。开始较大的功能、公开接口、存储格式或上游语义差异前，请先提交
Issue 讨论。修改必须保持聚焦，不得包含密钥、生产记忆或用户数据。核心语义修改必须补充
TypeScript 基线差分证据与真实 SQLite 测试；不可避免的差异必须在 PR 中明确说明并由回归
测试覆盖。提交 PR 时请说明用户可见结果、兼容性影响和实际执行的验证，
公开接口变化应同步更新中英文文档。
