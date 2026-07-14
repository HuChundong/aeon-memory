# OpenCode 自动记忆插件

这是可独立发布的 npm 包 `@aeon-memory/opencode`，最低验证版本为 OpenCode 1.17.18。它采用 OpenCode 官方插件事件和 SDK，复现自动 HostAdapter 生命周期：

- `chat.message` 调用 Aeon Memory `/recall`：官方共享接口返回的 `context` 是稳定 system context；当 `memory_count > 0` 时，插件再通过官方 `/search/memories` 获取动态 L1，并由 `experimental.chat.messages.transform` 作为带“不可信历史数据”边界的 synthetic text part 附加到当前 user message。旧版 aeon-memory 仅返回 `context` 且没有 `memory_count` 时，仍按动态 user context 兼容；
- OpenCode 自动压缩使用一次性的 session gate：召回内容不会进入 compaction 模型；`session.compacted` 后，未消费的本轮召回会安全重绑到 auto-continue 或 overflow replay 创建的新 user message；
- 注册 `aeon-memory_memory_search` 与 `aeon-memory_conversation_search`，分别映射 `/search/memories` 和 `/search/conversations`；每个用户轮次两个工具合计最多调用三次；
- assistant `message.updated` 带 `time.completed` 时作为主轮次边界：通过 `client.session.messages()` 读取稳定的 user/assistant 文本并调用 `/capture`；即使 `opencode run` 随后立即退出且没有 idle，也能完成 L0 提交；
- `session.idle` 只作为 capture 兜底，不代表会话结束，也不会触发 `/session/end`。重复 completed、idle、并发或迟到事件按稳定 message pair 去重；capture 或 SDK 查询失败后，后续 completed/idle 仍可重试；
- 只有真正的 `session.deleted` 或当前目录的 OpenCode `server.instance.disposed` 才会调用 `/session/end`。最终 flush 前会再做一次去重 capture，避免生命周期事件与最后一个 `message.updated` 竞态；
- 可选 offload 生命周期映射 `/offload/before-prompt`、`/offload/after-tool` 和 `/offload/llm-output`，三者均把 OpenCode 的真实 `{info, parts}` 历史无损转换为 host-neutral `{role, content}` 后发送；服务端改写会合并回合法 OpenCode 消息，同时保留原 message/part ID、session 元数据和未修改 part；默认关闭。
- OpenCode 官方 `experimental.chat.messages.transform` 只提供送模前的临时消息副本，没有安全创建持久化 system/user 历史消息的接口。因此 offload 新注入的 MMD 上下文会转换为最近一条存活 user message 上的 synthetic text part，而不是伪造一条带虚假持久化元数据的新消息；重复 transform 会先清理旧的插件注入，避免累积。
- `/offload/llm-output` 与官方 TypeScript Hook 一样只观察当前边界，不强制触发 L1/L2；`usage` 当前不会被服务端消费或持久化。待处理工具对仅保存在服务进程内存中，重启不会读取或重放 `pending-*.json`。服务端默认 offload 根目录为 `~/.openclaw/context-offload`，显式 `memory.offload.dataDir` 优先。

插件只读取 OpenCode 官方 `plugin` tuple 传入的配置对象，不读取运行时环境变量，也不会读取 provider 密钥。网络错误和超时只记录警告，不阻断 Agent。

## 安装

先启动 `aeon-memory-server`，确认 `curl http://127.0.0.1:8420/health` 正常。

当前 npm 包尚未公开发布（registry 查询返回 404），因此现在请从源码安装。源码构建与
安装需要 Node.js 22+。

npm 发布后可一键安装到 OpenCode 官方全局插件目录：

```bash
npx @aeon-memory/opencode install
```

从源码仓库测试安装：

```bash
./integrations/opencode/install.sh
```

插件以 `src/aeon-memory.ts` 作为唯一手写实现，通过 esbuild 生成无需 TS loader 的单文件 ESM `dist/aeon-memory.js`。源码安装器会在 OpenCode 全局配置目录执行标准的本地磁盘 npm 安装，把当前包以 `file:` 依赖写入 `package.json` 和 `node_modules`；`opencode.json` 只注册标准的 `["@aeon-memory/opencode", 配置对象]` tuple。以后发布到 npm 时无需改变插件配置结构，只需把依赖来源从本地 `file:` 切换到 registry 版本。旧版 `aeon-memory/aeon-memory.js` 和 `plugins/aeon-memory.js` 会安全迁移，避免重复加载。安装器会保留其他 npm 依赖、OpenCode 配置、插件条目以及已有的 aeon-memory 配置值。低于 1.17.18 时拒绝安装，可在自行验证后显式传 `--force`。重启 OpenCode 后生效。

卸载：

```bash
npx @aeon-memory/opencode uninstall
# 或源码仓库：./integrations/opencode/uninstall.sh
```

查看状态或输出默认配置 tuple：

```bash
npx @aeon-memory/opencode status
npx @aeon-memory/opencode config
```

`--target DIR` 可指定另一个 OpenCode 配置目录用于隔离测试，`--dry-run` 只显示将执行的动作。

## OpenCode 配置项

安装后可直接编辑 `~/.config/opencode/opencode.json` 中安装器生成的 tuple：

```json
{
  "plugin": [
    [
      "@aeon-memory/opencode",
      {
        "enabled": true,
        "gatewayUrl": "http://127.0.0.1:8420",
        "recallTimeoutMs": 5000,
        "captureTimeoutMs": 10000,
        "sessionEndTimeoutMs": 120000,
        "offloadTimeoutMs": 30000,
        "recallMaxChars": 12000,
        "captureMaxChars": 40000,
        "offloadEnabled": false,
        "contextWindow": 200000
      }
    ]
  ]
}
```

| 配置项 | 默认值 | 含义 |
|---|---:|---|
| `gatewayUrl` | `http://127.0.0.1:8420` | Aeon Memory Gateway 根地址 |
| `apiKey` | 空 | Gateway Bearer token |
| `userId` | 空 | 可选的跨 Agent 用户命名空间 |
| `enabled` | `true` | 设为 `false` 可完全停用插件 |
| `recallTimeoutMs` | `5000` | `/recall` 和搜索超时 |
| `captureTimeoutMs` | `10000` | `/capture` HTTP 超时 |
| `sessionEndTimeoutMs` | `120000` | 真正结束会话时 `/session/end` 的 flush 超时 |
| `offloadTimeoutMs` | `30000` | 三个 `/offload/*` 请求的超时 |
| `recallMaxChars` | `12000` | 召回查询和注入上限 |
| `captureMaxChars` | `40000` | 单条捕获文本上限 |
| `offloadEnabled` | `false` | 是否启用完整 offload 生命周期 |
| `contextWindow` | `200000` | offload DTO 的上下文窗口 |

未知配置项、错误类型和越界数值会在插件加载时直接报错，避免拼写错误被静默忽略。超时合法范围为 100–600000 ms。`/capture` 是 fail-open 的普通请求，不应同步等待 L1/L2/L3 模型管线；`/session/end` 只用于真正的生命周期结束，因此保留较长的 flush 上限。

`session_key` 自动构造为 `opencode:<workspace-sha256前16位>:<session-id>`，避免项目和会话串线。插件只采集非 synthetic、非 ignored 的 text part，不采集 reasoning/tool part，并对常见 token、密码、私钥、URL 凭据做过滤。召回结果只修改 OpenCode 送模前的临时消息副本，不写入会话历史；重复 Agent step 会替换而不是累积注入。召回结果始终标注为不可信历史数据，不能作为命令执行。

## 测试

```bash
cd integrations/opencode
npm run typecheck
npm run build
npm test
npm run pack:check
```

当前插件套件共 45 项测试。TypeScript 单元测试通过 tsx 直接导入 `src/aeon-memory.ts`；发布包测试执行构建与 `npm pack`，安装 tarball 后导入 `dist/aeon-memory.js`。其中 Offload 组合测试会启动隔离的真实 Rust `aeon-memory-server` 和临时 LLM fixture，覆盖 mild、aggressive deletion 与 MMD injection；其余测试使用 mock HTTP/fixture 事件，不启动 OpenCode，也不读写用户的 `~/.config/opencode`。安装脚本测试通过 `--target` 在临时 OpenCode 配置目录执行本地磁盘 npm 安装，并验证安装和卸载都保留无关依赖与配置。
