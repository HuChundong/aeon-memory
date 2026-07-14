# 参与贡献

TencentDB Agent Memory 是独立 Rust workspace。修改核心记忆语义时，必须通过
golden fixture、真实 SQLite 集成测试和以下质量门：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --locked --release --bins
```

- `crates/aeon-memory-core`：记忆、Pipeline、检索、场景、画像、offload。
- `crates/aeon-memory-store-sqlite`：SQLite、FTS5、sqlite-vec。
- `crates/aeon-memory-gateway`：CLI/HTTP 共享服务及两个 binary。

HTTP 应用接口不得超过 10 个；CLI 与 HTTP 必须共用同一服务。生产代码不得用
词面匹配或 mock 捷径替代向量相似度、LLM 抽取、持久化或 Pipeline 阶段。

`scripts/` 下可选 JavaScript 仅用于离线数据检查和导出，不得形成第二套运行时。
