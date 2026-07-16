# Changelog

Aeon Memory 的所有重要公开变更记录在此。格式遵循
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本遵循
[Semantic Versioning](https://semver.org/)。

## [Unreleased]

## [0.7.2] - 2026-07-16

### 修复

- OpenCode 插件默认对 Qwen 3.6 将稳定记忆上下文作为 synthetic user text 注入，避免严格模型网关拒绝多条 system message，同时保持其他模型的 system prompt 缓存设计不变。
- 新增可配置的 `systemContextUserTextModelPatterns` 大小写不敏感 glob 列表，可扩展同类模型兼容规则或通过空列表关闭默认规则。

## [0.7.1] - 2026-07-15

### 修复

- 原生服务进程边界测试现在断言 `/health.version` 与 Cargo 编译期版本一致；`aeon-memory-server --version` 提供同一版本的独立验证，防止发布升级后误把旧进程当作新二进制。
- 原生包升级说明明确要求先停止受服务管理的旧进程、切换完整包并重启，再比对磁盘二进制与 `/health` 版本。

## [0.7.0] - 2026-07-14

### 新增

- `/recall` 以向后兼容的 `prepend_context` 字段返回同一次召回按策略、阈值和预算选定的动态 L1 上下文。
- OpenCode 插件新增独立的 `recallEnabled`、`captureEnabled` 与 `toolsEnabled` 开关，并报告 experimental hook 的已测试版本范围。

### 修复

- OpenCode 不再用语义不同的 `/search/memories` 二次查询替代动态自动召回；连接旧服务端时仍保留兼容回退。
- assistant 完成事件按事件中的 message ID 精确捕获对应轮次，避免快速连续完成时错过较早回复。
- OpenCode 注入的场景导航与记忆工具指引使用宿主实际的 `read` 工具名。
- 明确 `userId` 当前仅为兼容元数据，不构成存储隔离边界。

## [0.6.3] - 2026-07-14

### 修复

- OpenCode 安装器现在检测并安全编辑现有 `opencode.jsonc` 或 `opencode.json`；双配置文件时要求显式选择，并保留无关 JSONC 注释与配置。
- 已发布的 OpenCode 安装器现在安装精确的 npm registry 版本，并把旧的 `file://` 插件入口迁移为标准包名 tuple，避免工作区链接和重复加载。

## [0.6.2] - 2026-07-14

### 修复

- OpenCode 发布包安装测试同时兼容 npm 11 的数组输出和 npm 12 的对象输出。
- 完成 npm Trusted Publishing 与 provenance 发布链路。

## [0.6.1] - 2026-07-14

### 新增

- 面向正式开源维护的中英文文档、社区健康文件和标准 MIT 元数据。
- 由 `vX.Y.Z` 标签触发的跨平台 GitHub Release 与 npm Trusted Publishing 工作流。
- npm 包默认 `README.md`、源码地址、问题反馈地址和公开发布元数据。

### 变更

- Rust workspace、数据工具与 OpenCode npm 插件统一使用 `0.6.1` 版本。
- GitHub Actions 更新到 Node.js 24 运行时。

## [0.6.0] - 2026-07-14

### 新增

- Aeon Memory 核心、CLI 与严格 10 个应用接口的 HTTP 服务。
- SQLite、FTS5、sqlite-vec 与 L0 → L1 → L2 → L3 分层记忆管线。
- OpenCode 自动记忆插件 `@aeon-memory/opencode` 首次公开发布。
- 行为回归测试与跨平台原生打包流程。

[Unreleased]: https://github.com/HuChundong/aeon-memory/compare/v0.7.2...HEAD
[0.7.2]: https://github.com/HuChundong/aeon-memory/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/HuChundong/aeon-memory/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/HuChundong/aeon-memory/compare/v0.6.3...v0.7.0
[0.6.3]: https://github.com/HuChundong/aeon-memory/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/HuChundong/aeon-memory/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/HuChundong/aeon-memory/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/HuChundong/aeon-memory/releases/tag/v0.6.0
