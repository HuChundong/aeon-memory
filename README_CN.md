# Aeon Memory：完整中文手册

[默认中文首页](README.md) · [English](README_EN.md) ·
[上游 TencentDB Agent Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)

> Aeon Memory 是社区独立维护的 Rust 兼容实现，并非腾讯或腾讯云官方产品。核心分层记忆
> 与上下文卸载设计源自 TencentDB Agent Memory。我们感谢上游团队与贡献者的开源工作，
> 完整说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

这是一个无需 OpenClaw、Hermes、Node.js 或 Docker 的轻量级记忆系统。默认交付目标是
跨平台原生单服务包，保留 L0 → L1 → L2 → L3、SQLite 全文与向量检索、场景、
画像和可选上下文卸载核心逻辑。

原生包提供两个程序：

- `aeon-memory-server`：独立 HTTP 服务，应用接口严格限制为 10 个；
- `aeon-memory`：直接打开同一核心与数据目录的一次性命令行程序，不是 HTTP 客户端。

## 1. 下载正确的原生包

原生发布工作流已固定配置以下平台包，但目前尚未公开发布 GitHub Release。只有成功
执行带标签的发布工作流后，以下压缩包才会出现在 Release 中：

| 系统 | 架构 | 包名后缀 | sqlite-vec |
|---|---|---|---|
| Linux | x86_64 | `x86_64-unknown-linux-gnu.tar.gz` | `vec0.so` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu.tar.gz` | `vec0.so` |
| macOS | Intel | `x86_64-apple-darwin.tar.gz` | `vec0.dylib` |
| macOS | Apple Silicon | `aarch64-apple-darwin.tar.gz` | `vec0.dylib` |
| Windows | x86_64 | `x86_64-pc-windows-msvc.zip` | `vec0.dll` |

Release 不使用 `latest` sqlite-vec。构建脚本固定下载官方 `sqlite-vec 0.1.9`，
并在打包前逐平台校验官方 SHA-256；校验不一致时立即失败。

先用 Release 同级的 `SHA256SUMS` 校验整个压缩包。解压后还可用包内
`SHA256SUMS` 校验每个文件：

```bash
# Linux
sha256sum -c SHA256SUMS

# macOS
shasum -a 256 -c SHA256SUMS
```

Windows PowerShell 可执行：

```powershell
Get-FileHash .\aeon-memory-*.zip -Algorithm SHA256
```

## 2. 解压与目录要求

解压后保持以下文件在同一目录：

```text
aeon-memory-<版本>-<平台>/
├── aeon-memory                 # Windows: aeon-memory.exe
├── aeon-memory-server          # Windows: aeon-memory-server.exe
├── vec0.so|dylib|dll
├── aeon-memory.yaml
├── 使用说明.md
├── README_CN.md
├── OFFLOAD_API_CONTRACT.md
├── SHA256SUMS
├── LICENSE
└── THIRD_PARTY_NOTICES.md
```

Linux/macOS 如需补充执行权限：

```bash
chmod +x aeon-memory aeon-memory-server
```

macOS 可能对从浏览器下载的未签名程序触发隔离提示。确认 Release 校验值正确后，
可在“系统设置 → 隐私与安全性”中允许，或由管理员按本机安全策略处理隔离属性；
不要对来源不明的二进制执行绕过操作。

### sqlite-vec 自动发现顺序

程序在启动时按以下优先级查找本平台的 `vec0.so`、`vec0.dylib` 或 `vec0.dll`：

1. `AEON_MEMORY_VEC0_PATH` 指定的文件或目录；
2. 当前运行的 `aeon-memory` / `aeon-memory-server` 可执行文件所在目录；
3. 开发测试 fixture；
4. `LD_LIBRARY_PATH`、`DYLD_LIBRARY_PATH`、`DYLD_FALLBACK_LIBRARY_PATH`、`PATH`；
5. 常见 Unix 系统库目录。

正常使用 Release 包无需设置环境变量，也不依赖启动时的当前目录。只有将扩展放在
其他位置时才设置 `AEON_MEMORY_VEC0_PATH`：

```bash
export AEON_MEMORY_VEC0_PATH=/opt/aeon-memory/vec0.so
```

```powershell
$env:AEON_MEMORY_VEC0_PATH = "C:\aeon-memory\vec0.dll"
```

扩展缺失或加载失败时，FTS 文本检索仍可初始化，但向量表和向量召回不可用；启用
Embedding/Hybrid 的生产部署应将此视为部署失败并修复文件或架构问题。

## 3. 配置 `aeon-memory.yaml`

原生包内已带完整示例配置。首次启动前至少修改：

- `server.apiKey`：强随机访问令牌；仅本机且明确不需要鉴权时才留空；
- `data.baseDir`：可选持久化目录；省略时沿用 TS 默认目录；
- `llm.*`：OpenAI-compatible `/chat/completions` 服务；
- `memory.embedding.*`：可选的 OpenAI-compatible `/embeddings` 服务；
- 仅启用 Embedding 时，`model` 和 `dimensions` 才必须配置并与已有数据库一致。

LLM 是 L0 → L3 管线的必需依赖；Embedding 是可选增强。未配置 Embedding 时，L0
JSONL、SQLite/FTS 关键词检索和 L0 → L3 管线仍可使用，但向量召回、向量去重不可用，
Hybrid/Embedding 执行会退化到关键词能力。

核心示例：

```yaml
server:
  host: "127.0.0.1"
  port: 8420
  apiKey: "替换为强随机令牌"
  corsOrigins: []

llm:
  baseUrl: "https://你的服务/v1"
  apiKey: "替换为LLM密钥"
  model: "你的聊天模型"
  maxTokens: 4096
  timeoutMs: 120000

memory:
  timezone: "system"
  storeBackend: "sqlite"
  recall:
    enabled: true
    strategy: "hybrid"
    maxResults: 5
    scoreThreshold: 0.3
  embedding:
    enabled: true
    provider: "openai"
    baseUrl: "https://你的服务/v1"
    apiKey: "替换为Embedding密钥"
    model: "你的Embedding模型"
    dimensions: 1536
    sendDimensions: true
  offload:
    enabled: false
```

安全要求：监听非回环地址前必须设置 `server.apiKey`，并通过防火墙或反向代理限制
来源。配置文件包含密钥，应限制文件读取权限。数据目录优先级为：`AEON_MEMORY_DATA_DIR` >
显式 `data.baseDir` > `~/.aeon-memory/data`。`HOME`/Windows `USERPROFILE` 与 `~`
展开规则一致。`aeon-memory` 与 `aeon-memory-server` 都是直接写库的进程，不得同时
打开同一数据目录。

兼容环境变量还包括 `AEON_MEMORY_GATEWAY_PORT/HOST/API_KEY`、`AEON_MEMORY_CORS_ORIGINS` 和
`AEON_MEMORY_LLM_BASE_URL/API_KEY/MODEL/MAX_TOKENS/TIMEOUT_MS/DISABLE_THINKING`。配置中的
整值占位符（如 `apiKey: "${LLM_API_KEY}"`）也会按原行为展开。显式相对路径以进程
当前工作目录为基准；启动日志会打印最终配置路径与数据目录，便于确认没有写错库。

## 4. 启动与健康检查

Linux/macOS：

```bash
./aeon-memory-server --config ./aeon-memory.yaml
curl http://127.0.0.1:8420/health
```

Windows PowerShell：

```powershell
.\aeon-memory-server.exe --config .\aeon-memory.yaml
Invoke-RestMethod http://127.0.0.1:8420/health
```

`/health` 返回 HTTP 200、`status: "ok"` 表示核心已初始化；`stores.vectorStore`
表示本地存储可用，`stores.embeddingService` 表示当前组合/配置状态，两者都不是远程
LLM 或 Embedding 端点探活。

服务使用 Ctrl-C 正常关闭并排空待处理队列。原生包不会自动注册系统服务；生产中需由
管理员自行创建 systemd、launchd、Windows Service Wrapper 或其他进程管理配置，
并固定工作目录、配置路径和数据目录。

## 5. CLI 完整用法

所有命令都可通过 `--config` 使用同一服务配置；也可沿用 TS 的
`AEON_MEMORY_GATEWAY_CONFIG` 或自动发现 `aeon-memory.yaml`/`aeon-memory.json`：
找不到配置文件时，也可完全使用上述 `AEON_MEMORY_*` 环境变量启动。

CLI 直接组合本地核心并打开数据目录，无需先启动 `aeon-memory-server`。它与服务端是两种
替代入口：运行 CLI 前应先停止使用同一数据目录的服务。CLI 命令退出前会排空本次命令
调度的待处理管线工作。

```bash
aeon-memory --config aeon-memory.yaml status
aeon-memory --config aeon-memory.yaml capture --user '用户消息' --assistant '助手回复' --session-key s1
aeon-memory --config aeon-memory.yaml recall --query '用户偏好是什么' --session-key s1
aeon-memory --config aeon-memory.yaml search memories --query '数据库' --limit 5 --type episodic
aeon-memory --config aeon-memory.yaml search conversations --query '迁移' --limit 10 --session-key s1
aeon-memory --config aeon-memory.yaml session end --session-key s1
aeon-memory --config aeon-memory.yaml seed --input history.json --session-key import-1
aeon-memory --config aeon-memory.yaml show persona
aeon-memory --config aeon-memory.yaml show scenes
aeon-memory --config aeon-memory.yaml offload before-prompt --input before-prompt.json
aeon-memory --config aeon-memory.yaml offload after-tool --input after-tool.json
aeon-memory --config aeon-memory.yaml offload llm-output --input llm-output.json
```

`seed` 还支持 `--strict-round-role` 和 `--auto-fill-timestamps=<true|false>`。
卸载输入结构见 [OFFLOAD_API_CONTRACT.md](OFFLOAD_API_CONTRACT.md)。

## 6. HTTP API：严格 10 个

`GET /health` 公开；配置 `server.apiKey` 后，其余九个 POST 请求必须带：

```text
Authorization: Bearer <server.apiKey>
Content-Type: application/json
```

| 方法 | 路径 | 最小请求体 |
|---|---|---|
| GET | `/health` | 无 |
| POST | `/recall` | `{"query":"...","session_key":"s1"}` |
| POST | `/capture` | `{"user_content":"...","assistant_content":"...","session_key":"s1"}` |
| POST | `/search/memories` | `{"query":"..."}` |
| POST | `/search/conversations` | `{"query":"..."}` |
| POST | `/session/end` | `{"session_key":"s1"}` |
| POST | `/seed` | `{"data": [...]}` |
| POST | `/offload/before-prompt` | host-neutral offload 请求 |
| POST | `/offload/after-tool` | 见卸载协议 |
| POST | `/offload/llm-output` | 见卸载协议 |

示例：

```bash
curl -sS http://127.0.0.1:8420/recall \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $AEON_MEMORY_API_KEY" \
  -d '{"query":"用户偏好是什么？","session_key":"s1"}'
```

不存在 `/stats`、`/memories`、`/reindex` 等额外 HTTP 应用接口；`OPTIONS` 仅为
CORS 协议处理。

`/recall` 的 `context` 只包含稳定的 persona/scene 上下文，并返回 `memory_count`；
当其大于 0 时，动态 L1 记忆应通过 `/search/memories` 获取。`/capture` 同步提交 L0
后调度后台管线，真正结束会话时用 `/session/end` 排空。配置中的
`memory.capture.enabled` 与 `memory.recall.enabled` 是宿主自动 hook 开关，不会禁用
显式 HTTP 或 CLI 调用。

## 7. 数据目录与备份

`data.baseDir` 是完整可迁移的 L0 → L3 状态，主要包含：

```text
data/
├── conversations/                 # L0 JSONL
├── vectors.db                     # SQLite / FTS5 / vec0
├── scene_blocks/                  # L2 场景 Markdown
├── persona.md                     # L3 用户画像
├── .metadata/                     # checkpoint、scene index 等
└── .backup/                       # persona/scene 自动备份
```

备份前先正常停止服务，再复制整个目录；不能只复制 `vectors.db` 而遗漏 JSONL、场景、
画像和 checkpoint。SQLite 正在运行时可能存在 `-wal`、`-shm` 文件，冷备份必须在
进程退出后执行。

Offload 不属于 `data.baseDir`。其默认根目录独立为
`~/.openclaw/context-offload`，显式 `memory.offload.dataDir` 优先。启用 Offload 时
必须单独停止进程并备份该目录。

## 8. 升级与备份

1. 记录当前程序版本、Embedding provider/model/dimensions 和配置。
2. 正常停止服务，确认没有进程打开数据目录。
3. 完整备份 `data.baseDir` 和 `aeon-memory.yaml`/JSON。
4. 下载目标版本对应平台包并验证 Release SHA-256。
5. 将新包解压到新目录；不要只替换程序而保留旧版 vec0。

不要直接更换已有库的 Embedding model 或 dimensions；当前原生包没有自动 reindex
命令。失败回滚时先停止新服务，再恢复旧程序、旧 vec0、旧配置和完整数据备份。

## 9. 从源码构建

需要 Rust 1.88+：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked --release --bins
```

源码构建不会自动下载 sqlite-vec。可从固定官方版本取得正确架构的 loadable asset，
校验后将 `vec0` 放到两个可执行文件同目录，或设置 `AEON_MEMORY_VEC0_PATH`。

## 10. Docker（可选，不是默认交付）

原生包是默认且推荐的交付方式。只有已有容器编排需求时才使用 `Dockerfile.rust`；
镜像构建不会下载未固定的 native extension，需要自行挂载已验证的 vec0。Docker
示例和更底层的兼容性说明见 [README_RUST.md](README_RUST.md)。

离线迁移/导出工具位于 `bin/` 与 `scripts/`，不属于运行时适配层。历史
`CHANGELOG.md`、`RUST_MIGRATION_PLAN.md` 中的宿主名称仅作为迁移证据保留。
