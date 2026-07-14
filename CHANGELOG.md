# Changelog

Aeon Memory 的所有重要公开变更记录在此。格式遵循
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本遵循
[Semantic Versioning](https://semver.org/)。

## [Unreleased]

## [0.6.1] - 2026-07-14

### 新增

- 面向正式开源维护的中英文文档、社区健康文件、上游归属和标准 MIT 元数据。
- 由 `vX.Y.Z` 标签触发的跨平台 GitHub Release 与 npm Trusted Publishing 工作流。
- npm 包默认 `README.md`、源码地址、问题反馈地址和公开发布元数据。

### 变更

- Rust workspace、数据工具与 OpenCode npm 插件统一使用 `0.6.1` 版本。
- GitHub Actions 更新到 Node.js 24 运行时。

## [0.6.0] - 2026-07-14

### 新增

- 独立 Rust 核心、CLI 与严格 10 个应用接口的 HTTP 服务。
- SQLite、FTS5、sqlite-vec 与 L0 → L1 → L2 → L3 分层记忆管线。
- OpenCode 自动记忆插件 `@aeon-memory/opencode` 首次公开发布。
- TypeScript 上游基线差分测试与跨平台原生打包流程。

[Unreleased]: https://github.com/HuChundong/aeon-memory/compare/v0.6.1...HEAD
[0.6.1]: https://github.com/HuChundong/aeon-memory/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/HuChundong/aeon-memory/releases/tag/v0.6.0
