# @aeon-memory/opencode

[![npm](https://img.shields.io/npm/v/%40aeon-memory%2Fopencode)](https://www.npmjs.com/package/@aeon-memory/opencode)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Aeon Memory 的 OpenCode 自动集成插件。它使用 OpenCode 官方插件事件和配置格式，将
OpenCode 会话连接到独立运行的 `aeon-memory-server`，自动完成召回、记录与会话排空。

主项目与服务端安装说明：<https://github.com/HuChundong/aeon-memory>

## 前置条件

- OpenCode `1.17.18` 或更高版本（当前测试至 `1.17.20`；更高版本会提示 experimental hook 尚待验证）；
- Node.js 22 或更高版本（用于安装器）；
- 已启动且 `/health` 正常的 Aeon Memory HTTP 服务。

```bash
curl http://127.0.0.1:8420/health
```

## 安装

```bash
npx @aeon-memory/opencode@latest install
npx @aeon-memory/opencode@latest status
```

重启 OpenCode 后生效。发布包安装器会把当前发布版本作为精确的 npm registry 依赖写入
`~/.config/opencode/package.json`、`package-lock.json` 与 `node_modules`，并注册标准 OpenCode
`plugin` tuple。它会自动选择已有的 `opencode.jsonc` 或 `opencode.json`；两者同时存在时会安全
停止并要求用 `--config FILE` 明确选择。没有配置文件时默认创建 `opencode.jsonc`。已有的其他插件、
配置和 JSONC 注释会被保留。

卸载：

```bash
npx @aeon-memory/opencode@latest uninstall
```

可用的安装器选项：

```text
--target DIR  指定 OpenCode 配置目录，适合隔离测试
--config FILE 指定确切的 OpenCode JSON/JSONC 配置文件
--local       从当前源码目录安装，仅用于开发测试
--dry-run     仅显示将执行的操作
--force       在低于最低验证版本时强制安装
```

## 配置

安装器会生成以下配置；请按服务端设置修改 `gatewayUrl` 和 `apiKey`：

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": [
    [
      "@aeon-memory/opencode",
      {
        "enabled": true,
        "recallEnabled": true,
        "captureEnabled": true,
        "toolsEnabled": true,
        "gatewayUrl": "http://127.0.0.1:8420",
        "apiKey": "",
        "userId": "",
        "recallTimeoutMs": 5000,
        "captureTimeoutMs": 10000,
        "sessionEndTimeoutMs": 120000,
        "offloadTimeoutMs": 30000,
        "recallMaxChars": 12000,
        "captureMaxChars": 40000,
        "offloadEnabled": false,
        "contextWindow": 200000,
        "systemContextUserTextModelPatterns": ["*qwen3.6*"]
      }
    ]
  ]
}
```

| 配置项 | 默认值 | 说明 |
|---|---:|---|
| `enabled` | `true` | 完全启用或停用插件 |
| `recallEnabled` | `true` | 自动召回并注入本轮上下文 |
| `captureEnabled` | `true` | 自动记录完成轮次并在会话结束时排空 |
| `toolsEnabled` | `true` | 注册两个显式记忆搜索工具 |
| `gatewayUrl` | `http://127.0.0.1:8420` | Aeon Memory 服务地址 |
| `apiKey` | 空 | 与服务端 `server.apiKey` 相同的 Bearer token |
| `userId` | 空 | 转发给兼容网关的元数据；当前本地存储不按此字段隔离 |
| `recallTimeoutMs` | `5000` | 召回与搜索请求超时 |
| `captureTimeoutMs` | `10000` | 对话记录请求超时 |
| `sessionEndTimeoutMs` | `120000` | 最终排空管线超时 |
| `offloadTimeoutMs` | `30000` | 上下文卸载请求超时 |
| `recallMaxChars` | `12000` | 召回查询与注入字符上限 |
| `captureMaxChars` | `40000` | 单条捕获文本字符上限 |
| `offloadEnabled` | `false` | 启用三类上下文卸载 Hook |
| `contextWindow` | `200000` | Offload DTO 的上下文窗口 |
| `systemContextUserTextModelPatterns` | `["*qwen3.6*"]` | 大小写不敏感的模型 ID glob；命中时将稳定 system context 改为 synthetic user text，以兼容只接受单条 system message 的模型网关；设为 `[]` 可关闭 |

插件只读取上述配置对象，不读取 Provider 密钥或运行时环境变量。未知字段、错误类型或越界
值会在加载时直接报错。网络错误采用 fail-open 策略：记录警告但不会阻断 Agent。

## 生命周期

- 用户消息开始时调用 `/recall`，直接使用同一次召回返回的 `prepend_context`；仅旧服务端需要兼容调用 `/search/memories`；
- 召回内容只写入送模前的临时消息副本，不污染持久化 OpenCode 历史；
- 助手消息完成时读取稳定的 user/assistant 文本并调用 `/capture`；
- `session.idle` 只作为 capture 兜底，不会错误结束会话；
- 只有 `session.deleted` 或当前实例 `server.instance.disposed` 才调用 `/session/end`；
- 自动压缩、标题请求和重复 transform 均有隔离与去重处理；
- `session_key` 包含工作区哈希与 OpenCode session ID，防止项目之间串线；
- reasoning、tool、synthetic、ignored part 不会被当作用户对话采集；常见凭据会被过滤；
- 召回内容始终标注为不可信历史数据，不能作为命令执行。

插件注册 `aeon-memory_memory_search` 与 `aeon-memory_conversation_search` 两个工具，允许
Agent 在需要证据时主动下钻 L1 和 L0。

## 源码开发

```bash
git clone https://github.com/HuChundong/aeon-memory.git
cd aeon-memory/integrations/opencode
npm ci
npm run typecheck
npm test
npm run pack:check
```

源码安装：

```bash
cd ../..
./integrations/opencode/install.sh
```

问题反馈请前往 <https://github.com/HuChundong/aeon-memory/issues>；安全问题请勿公开提交，
请遵循主仓库的 [安全策略](https://github.com/HuChundong/aeon-memory/security/policy)。

本插件和 Aeon Memory 采用 MIT 许可证。项目代码来源与完整致谢见主仓库 README。
