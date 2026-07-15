# Aeon Memory 原生包快速使用说明

本目录是无需 Docker、无需 Node.js 的完整原生运行包。请保持以下文件在同一目录：

- `aeon-memory`、`aeon-memory-server`（Windows 为 `.exe`）；
- `vec0.so`、`vec0.dylib` 或 `vec0.dll`；
- `aeon-memory.yaml`。

首次使用必须编辑 `aeon-memory.yaml`：设置 LLM API 和强随机 `server.apiKey`；
Embedding API 是可选增强，未配置时保留 JSONL、SQLite/FTS 关键词检索和 L0 → L3
管线，但不提供向量召回与向量去重。示例刻意省略 `data.baseDir`，因此沿用默认目录
`~/.aeon-memory/data`；也可显式指定其他持久化目录。

启动服务：

```bash
# Linux/macOS
./aeon-memory-server --config ./aeon-memory.yaml

# Windows PowerShell
.\aeon-memory-server.exe --config .\aeon-memory.yaml
```

检查状态：

```bash
curl http://127.0.0.1:8420/health
```

`/health` 表示核心已初始化和本地存储已组合，不会探测远程 LLM/Embedding 端点。

升级已有受服务管理的安装时，先停止旧进程，再替换整个原生包并启动服务；不要覆盖正在运行的
可执行文件。启动后 `./aeon-memory-server --version` 和 `/health` 返回的 `version` 必须一致。
若磁盘文件已更新但 `/health` 仍是旧版本，说明旧进程仍持有旧 inode，重启服务即可，不能只重复复制文件。

CLI 是无需服务端的替代入口。先停止使用同一数据目录的 `aeon-memory-server`，再执行：

```bash
./aeon-memory --config ./aeon-memory.yaml status
```

`aeon-memory` 与 `aeon-memory-server` 都直接打开本地数据，绝对不能同时写同一目录。

程序会自动发现与可执行文件同目录的 sqlite-vec。仅在需要放到其他目录时设置
`AEON_MEMORY_VEC0_PATH`，该变量优先级最高。不要从未知来源替换 vec0 文件；包内
`SHA256SUMS` 可用于校验解压后的文件。

完整安装、配置、CLI、HTTP、数据目录、升级和回滚说明见包内
`README_CN.md`；卸载 JSON 结构见 `OFFLOAD_API_CONTRACT.md`。

仅当原独立 TS 服务使用 SQLite 后端时，Rust 才能直接替换。OpenClaw、Hermes、
TCVDB 与 reporter 集成不在直接替换范围。从原 TS 服务切换时，先确认
`memory.storeBackend: sqlite`，停止 TS 进程并确认数据目录没有写入者，再启动 Rust。
两者可直接使用同一配置和数据目录，但绝对不能同时写入。
