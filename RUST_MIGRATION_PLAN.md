# TencentDB Agent Memory → Rust 迁移冻结基线

> 历史迁移记录：文中 TypeScript 与旧插件路径仅用于说明 Rust 等价实现的证据来源，
> 不属于当前发布的运行时。

> 生效日期: 2026-07-13 | 状态: **冻结** | 前置: 仅允许读分析，禁止改任何非本文文件的代码

---

## 1. 第一轮分析复盘与纠偏

### 1.1 已证实的结论（无需修改）

| 条目 | 源码证据 |
|------|----------|
| 项目是 TS npm 包，三种部署形态 | `package.json:2` (`@tencentdb-agent-memory/memory-tencentdb`), `index.ts:132` (OpenClaw plugin), `src/gateway/server.ts:114` (Gateway), `src/cli/index.ts:54` (CLI) |
| L0→L1→L2→L3 分层构成长期记忆核心 | `src/core/aeon-memory-core.ts:75`, README_CN.md:75-77 |
| L0 JSONL 每日分片 + cursor | `src/core/conversation/l0-recorder.ts:1-16`, `src/utils/checkpoint.ts` |
| L1 三种记忆类型 (persona/episodic/instruction) | `src/core/record/l1-writer.ts:32` |
| L2 scene_blocks/*.md | `src/core/scene/scene-extractor.ts` |
| L3 persona.md | `src/core/persona/persona-generator.ts` |
| RRF 融合 (K=60) | `src/core/hooks/auto-recall.ts:601` |
| Hybrid 搜索策略 + 自动降级 | `src/core/hooks/auto-recall.ts:345-388` |
| 延迟嵌入 (sqlite path) | `src/core/hooks/auto-capture.ts:150-291` |
| Pipeline 调度语义 | `src/utils/pipeline-manager.ts:1-77` |
| Warmup 指数递增 | `src/utils/pipeline-manager.ts:346-371` |
| 召回预算控制 | `src/core/hooks/auto-recall.ts:708-777` |
| prependContext / appendSystemContext 分离 | `src/core/hooks/auto-recall.ts:196-218` |
| Hermes 仅通过 HTTP 7 个端点消费 | `hermes-plugin/memory/memory_tencentdb/client.py:111-196` |
| OpenClaw adapter 在 `src/adapters/openclaw/` | `src/adapters/openclaw/host-adapter.ts`, `llm-runner.ts` |
| Standalone adapter 在 `src/adapters/standalone/` | `src/adapters/standalone/host-adapter.ts`, `llm-runner.ts` |
| CLI 当前只有 seed 子命令 | `src/cli/index.ts:56` |
| AGENTS.md 不存在 | 无此文件 |

### 1.2 需要纠正的结论

| 原分析 | 纠正 | 证据 |
|--------|------|------|
| "offload 属于可移除/阶段 2" | **offload 是记忆系统核心组成部分，必须迁移，但因其独立性和 OpenClaw 深度耦合放入阶段 2** | README_CN.md:75-76 将短期记忆分层列为三大分层之一，作为核心架构支柱；但 `src/offload/index.ts:268` 完全依赖 `api.on()` OpenClaw 钩子，且 `offload.enabled: false` 默认关闭 (`src/config.ts:478`) |
| "新增 HTTP 接口 /stats, /memories, /reindex" | **删除**。Hermes Python 客户端只使用 7 个 (`client.py:111-196`)，无任何生产者消费额外端点 | 新增管理端点无现有核心用例证明 |
| "CLI 只需 seed + 可选的 stats" | CLI 必须覆盖 `AeonMemoryCore` 全部核心方法：seed, capture, recall, search, session-end, status | `src/core/tdai-core.ts` (upstream) 公开 `handleBeforeRecall`, `handleTurnCommitted`, `searchMemories`, `searchConversations`, `handleSessionEnd` |
| "上层的几个 prompt 文案不变" | prompt 文本**必须逐字逐句匹配**，因为影响提取质量 | 提示词位于 `src/core/prompts/l1-extraction.ts`, `src/core/prompts/l1-dedup.ts`, `src/core/prompts/scene-extraction.ts`, `src/core/prompts/persona-generation.ts` |

---

## 2. offload 模块是否属于核心——最终判定

### 2.1 证据链

```
README_CN.md:75-76
  短期记忆（上下文卸载/任务）的分层：
  底层 → refs/*.md
  中层 → jsonl (step-level summaries)
  高层 → Mermaid 任务画布
  ↑ 这是 offload 模块

README_CN.md:80-81
  长期个性化（用户理解）的分层：
  L0 → L1 → L2 → L3
  ↑ 这是 L0-L3 模块

README: Roadmap
  [x] Short-term context compression (Context Offload + Mermaid canvas)
  [x] Long-term personalized memory (L0 → L3)
  ↑ 两者都是已完成的核心特性
```

### 2.2 判定

**offload 属于记忆系统核心**，但：

| 维度 | offload | L0-L3 |
|------|---------|-------|
| 默认启用 | ❌ (`offload.enabled: false`) | ✅ (全部默认 true) |
| 数据目录 | `~/.openclaw/context-offload/` (独立) | `~/.openclaw/aeon-memory/` (独立) |
| 存储格式 | offload-{sessionId}.jsonl + refs/*.md + mmds/*.mmd | conversations/*.jsonl + records/*.jsonl + scene_blocks/*.md + vectors.db |
| LLM 管线 | L1 工具摘要 / L1.5 任务边界 / L2 Mermaid / L3 上下文压缩 / L4 Skill | L1 记忆提取 / L2 场景归纳 / L3 画像生成 |
| OpenClaw 耦合 | 极深 (`api.on` for after_tool_call / before_tool_call / llm_input / llm_output / before_prompt_build / registerContextEngine) | 中 (仅通过 HostAdapter trait + LLMRunner trait) |
| 文件数 | ~40 文件 (~40% 代码量) | ~45 文件 |

**结论：L0-L3 长期记忆为 Phase 1，offload 为 Phase 2。offload 不纳入 Phase 1 的原因是：** 数据模型完全独立、默认关闭、OpenClaw 钩子深度耦合需要单独解耦设计，纳入 Phase 1 将显著推迟 L0-L3 核心交付。但 Phase 2 完成前不可删除任何 offload 源文件。

---

## 3. 最终 HTTP 接口清单（7 个，≤10）

| # | 方法 | 路径 | 请求体 | 响应体 | 核心用例证明 |
|---|------|------|--------|--------|------------|
| 1 | GET | `/health` | — | `{status, version, uptime, stores}` | `client.py:111` `supervisor.py:88` |
| 2 | POST | `/recall` | `{query, session_key}` | `{context, strategy, memory_count}` | `client.py:115` |
| 3 | POST | `/capture` | `{user_content, assistant_content, session_key, session_id?}` | `{l0_recorded, scheduler_notified}` | `client.py:122` |
| 4 | POST | `/search/memories` | `{query, limit?, type?, scene?}` | `{results, total, strategy}` | `client.py:142` |
| 5 | POST | `/search/conversations` | `{query, limit?, session_key?}` | `{results, total}` | `client.py:151` |
| 6 | POST | `/session/end` | `{session_key}` | `{flushed: true}` | `client.py:158` |
| 7 | POST | `/seed` | `{data, session_key?, strict_round_role?, auto_fill_timestamps?, config_override?}` | `{sessions_processed, rounds_processed, ...}` | `client.py:165` |

**不新增** `/stats`, `/memories`, `/reindex`——无现有 HTTP 客户端消费，管理能力由 CLI 覆盖。

---

## 4. CLI 命令与 AeonMemoryCore API 的映射

| 命令 | 映射的 Core API | 功能 | 现有证明 |
|------|----------------|------|----------|
| `aeon-memory seed --input <file> [...]` | `executeSeed()` → `performAutoCapture()` × N + `MemoryPipelineManager` | 批量灌入历史对话 | `src/cli/commands/seed.ts:25` |
| `aeon-memory capture --user <text> --assistant <text> --session-key <key>` | `handleTurnCommitted()` | 单轮对话捕获 | 对应 `src/gateway/server.ts:393` |
| `aeon-memory recall --query <text> --session-key <key>` | `handleBeforeRecall()` | 手动测试召回 | 对应 `src/gateway/server.ts:371` |
| `aeon-memory search memories --query <text> [--limit N] [--type TYPE] [--scene SCENE]` | `searchMemories()` | L1 结构记忆搜索 | 对应 `src/gateway/server.ts:423` |
| `aeon-memory search conversations --query <text> [--limit N] [--session-key KEY]` | `searchConversations()` | L0 原始对话搜索 | 对应 `src/gateway/server.ts:446` |
| `aeon-memory session end --session-key <key>` | `handleSessionEnd()` | 结束会话刷新 | 对应 `src/gateway/server.ts:467` |
| `aeon-memory status` | 直接读 Store (countL0, countL1, 文件扫描) | 系统统计 (session 数/L0/L1/L2/L3 存储量) | 新命令，无现有等价 |
| `aeon-memory show persona` | 读 `persona.md` | 显示当前 persona | 新命令，补充白盒可调试性 |
| `aeon-memory show scenes` | 读 `scene_blocks/*.md` 索引 | 显示场景列表 | 新命令，补充白盒可调试性 |

---

## 5. Rust Crate/Module 结构（冻结）

```
aeon-memory/
├── Cargo.toml                         # workspace root
├── src/
│   ├── main.rs                        # CLI (clap): 所有 aeon-memory 子命令
│   └── server.rs                      # HTTP (axum): 7 endpoints
│
├── crates/
│   ├── aeon-memory-core/                     # 核心逻辑, 无框架依赖
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                 # barrel export
│   │       ├── types.rs               # 所有核心类型 + HostAdapter/LLMRunner trait
│   │       ├── error.rs               # thiserror 枚举
│   │       │
│   │       ├── store/
│   │       │   ├── mod.rs
│   │       │   ├── traits.rs          # IMemoryStore trait
│   │       │   └── types.rs           # L1SearchResult, L0Record, ProfileRecord 等
│   │       │
│   │       ├── record/
│   │       │   ├── mod.rs
│   │       │   ├── l0_recorder.rs     # JSONL + cursor (port of l0-recorder.ts)
│   │       │   ├── l1_extractor.rs    # LLM extraction (port of l1-extractor.ts)
│   │       │   ├── l1_writer.rs       # JSONL + store write (port of l1-writer.ts)
│   │       │   ├── l1_dedup.rs        # conflict detection (port of l1-dedup.ts)
│   │       │   └── l1_reader.rs       # read (port of l1-reader.ts)
│   │       │
│   │       ├── scene/
│   │       │   ├── mod.rs
│   │       │   ├── scene_extractor.rs # LLM (port of scene-extractor.ts)
│   │       │   ├── scene_index.rs     # index read/write (port of scene-index.ts)
│   │       │   ├── scene_navigation.rs# nav generation (port of scene-navigation.ts)
│   │       │   └── scene_format.rs    # (port of scene-format.ts, filename-normalizer.ts)
│   │       │
│   │       ├── persona/
│   │       │   ├── mod.rs
│   │       │   ├── persona_generator.rs (port of persona-generator.ts)
│   │       │   └── persona_trigger.rs   (port of persona-trigger.ts)
│   │       │
│   │       ├── hooks/
│   │       │   ├── mod.rs
│   │       │   ├── auto_recall.rs     # port of auto-recall.ts
│   │       │   └── auto_capture.rs    # port of auto-capture.ts
│   │       │
│   │       ├── tools/
│   │       │   ├── mod.rs
│   │       │   ├── memory_search.rs   # port of memory-search.ts
│   │       │   └── conversation_search.rs (port of conversation-search.ts)
│   │       │
│   │       ├── llm/
│   │       │   ├── mod.rs
│   │       │   ├── traits.rs          # LLMRunner trait
│   │       │   └── openai.rs          # OpenAI-compatible HTTP impl
│   │       │
│   │       ├── embedding/
│   │       │   ├── mod.rs
│   │       │   ├── traits.rs          # EmbeddingService trait
│   │       │   ├── openai.rs          # remote impl
│   │       │   └── noop.rs            # server-side (TCVDB mode)
│   │       │
│   │       ├── prompt/                # const &str only, NO serde
│   │       │   ├── mod.rs
│   │       │   ├── l1_extraction.rs   # EXACT copy of TS prompt
│   │       │   ├── l1_dedup.rs        # EXACT copy
│   │       │   ├── scene_extraction.rs# EXACT copy
│   │       │   └── persona_generation.rs (EXACT copy)
│   │       │
│   │       ├── search/
│   │       │   ├── mod.rs
│   │       │   ├── rrf.rs             # RRF fusion (port of auto-recall.ts RRF)
│   │       │   └── budget.rs          # recall char budget (port of auto-recall.ts)
│   │       │
│   │       ├── pipeline/
│   │       │   ├── mod.rs
│   │       │   ├── manager.rs         # MemoryPipelineManager (port)
│   │       │   ├── timer.rs           # ManagedTimer (port of managed-timer.ts)
│   │       │   ├── serial_queue.rs    # (port of serial-queue.ts)
│   │       │   └── checkpoint.rs      # (port of checkpoint.ts)
│   │       │
│   │       ├── profile/
│   │       │   └── profile_sync.rs    # (port of profile-sync.ts)
│   │       │
│   │       ├── seed/
│   │       │   ├── mod.rs
│   │       │   ├── input.rs           # validation, normalization (port of seed/input.ts)
│   │       │   ├── runtime.rs         # orchestrator (port of seed/seed-runtime.ts)
│   │       │   └── types.rs           # NormalizedInput, SeedProgress 等
│   │       │
│   │       ├── config/
│   │       │   └── mod.rs             # serde + envy (port of src/config.ts + src/gateway/config.ts)
│   │       │
│   │       └── utils/
│   │           ├── mod.rs
│   │           ├── time.rs            # chrono (port of utils/time.ts)
│   │           ├── sanitize.rs        # (port of utils/sanitize.ts)
│   │           └── manifest.rs        # (port of utils/manifest.ts)
│   │
│   ├── aeon-memory-store-sqlite/             # rusqlite + sqlite-vec
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── connection.rs
│   │       ├── schema.rs              # DDL + migration
│   │       ├── l1.rs                  # L1 tables + vec0 + FTS5
│   │       ├── l0.rs                  # L0 tables + vec0 + FTS5
│   │       └── fts.rs                 # buildFtsQuery (port of sqlite.ts)
│   │
│   └── aeon-memory-bm25/                     # BM25 local encoder
│       ├── Cargo.toml
│       └── src/lib.rs                 # port of bm25-local.ts + bm25-client.ts
│
├── tests/                             # 集成测试
│   ├── e2e_capture_recall.rs
│   ├── e2e_seed.rs
│   ├── compat_jsonl.rs                # 逐字节比 TS/Rust JSONL
│   ├── compat_search.rs               # 比 TS/Rust 搜索排序
│   └── compat_pipeline.rs             # 比 TS/Rust pipeline 调度
│
└── benches/                           # 基准测试
    └── search_bench.rs
```

---

## 6. 分阶段实现顺序（每阶段 `cargo test` 通过）

### Phase 1a — 核心框架 + 存储（2 周）

```
实现顺序（严格按此序，每步可提交且 cargo test 通过）:

Step 1: Cargo workspace +  crate 骨架
  - Cargo.toml (workspace)
  - crates/aeon-memory-core/src/{lib,types,error}.rs
  - cargo check

Step 2: 配置解析
  - crates/aeon-memory-core/src/config/mod.rs (serde + envy)
  - 测试: AeonMemoryConfig::from_file() 读现有 aeon-memory.yaml
  
Step 3: 存储抽象 + SQLite 实现
  - crates/aeon-memory-core/src/store/{traits,types}.rs
  - crates/aeon-memory-store-sqlite/src/{lib,connection,schema,l1,l0,fts}.rs
  - 测试: create L1/L0 表, upsert, query, delete, FTS5 search
  - ⚠️ 风险验证: sqlite-vec loadable .dylib/.so 在目标平台可加载

Step 4: LLM 调用 + Embedding
  - crates/aeon-memory-core/src/llm/{traits,openai}.rs
  - crates/aeon-memory-core/src/embedding/{traits,openai,noop}.rs
  - 测试: mock HTTP 服务器验证请求格式

Step 5: 时间 + 清理工具
  - crates/aeon-memory-core/src/utils/{time,sanitize,manifest}.rs
  - 测试: 时间格式与 TS 一致、清理逻辑
```

### Phase 1b — L0+L1 写入 + 捕获（1.5 周）

```
Step 6: L0 recorder
  - crates/aeon-memory-core/src/record/l0_recorder.rs
  - 测试: 写一行 JSONL → 逐字节与 TS 输出一致
  
Step 7: L1 writer
  - crates/aeon-memory-core/src/record/l1_writer.rs
  - 测试: 写 L1 JSONL + VectorStore upsert

Step 8: L1 extractor + dedup
  - crates/aeon-memory-core/src/record/{l1_extractor,l1_dedup}.rs
  - crates/aeon-memory-core/src/prompt/l1_extraction.rs
  - crates/aeon-memory-core/src/prompt/l1_dedup.rs
  - 测试: LLM 提取 → 格式检查 → dedup 向量判断

Step 9: auto-capture hook
  - crates/aeon-memory-core/src/hooks/auto_capture.rs
  - 测试: captureAtomically → L0 写 → L1 调度通知
```

### Phase 1c — L2+L3 + Pipeline 调度（2 周）

```
Step 10: Checkpoint + SerialQueue + Timer
  - crates/aeon-memory-core/src/pipeline/{checkpoint,serial_queue,timer}.rs
  - 测试: checkpoint 读写与 TS 格式一致

Step 11: Pipeline manager
  - crates/aeon-memory-core/src/pipeline/manager.rs
  - 测试:
    11a: notifyConversation → 计数递增
    11b: warmup 1→2→4→N 递增
    11c: L1 空闲超时触发
    11d: L2 delay-after-L1 + minInterval + maxInterval
    11e: L3 全局互斥 + pending 去重
    11f: session GC
    11g: flushSession + destroy

Step 12: Scene extractor (L2)
  - crates/aeon-memory-core/src/scene/{scene_extractor,scene_index,scene_navigation,scene_format}.rs
  - crates/aeon-memory-core/src/prompt/scene_extraction.rs
  - 测试: LLM 场景归纳 → scene_blocks/*.md

Step 13: Persona generator (L3)
  - crates/aeon-memory-core/src/persona/{persona_generator,persona_trigger}.rs
  - crates/aeon-memory-core/src/prompt/persona_generation.rs
  - 测试: LLM 画像 → persona.md

Step 14: RRF search + budget
  - crates/aeon-memory-core/src/search/{rrf,budget}.rs
  - 测试: RRF 融合排序与 TS 一致、预算精确截断
```

### Phase 1d — Recall + Tools + Seed（1.5 周）

```
Step 15: auto-recall hook
  - crates/aeon-memory-core/src/hooks/auto_recall.rs
  - 测试:
    15a: keyword/embedding/hybrid 召回
    15b: prependContext + appendSystemContext 分割
    15c: 内存工具指南注入
    15d: 超时降级

Step 16: Search tools
  - crates/aeon-memory-core/src/tools/{memory_search,conversation_search}.rs
  - 测试: Agent 搜索结果格式与 TS 一致

Step 17: Seed pipeline
  - crates/aeon-memory-core/src/seed/{input,runtime,types}.rs
  - 测试: JSON input → L0 → L1 → L2 → L3 端到端
  
Step 18: BM25 encoder
  - crates/aeon-memory-bm25/src/lib.rs
  - 测试: BM25 编码与 TS tcvdb-text 一致
```

### Phase 1e — HTTP + CLI 入口（1 周）

```
Step 19: HTTP server (axum)
  - src/server.rs
  - 测试: 7 endpoints, auth (Bearer), CORS
  - 兼容测试: 请求/响应 JSON 与 TS Gateway 逐字段一致

Step 20: CLI (clap)
  - src/main.rs
  - 测试: 每个子命令的执行路径
```

### Phase 2 — offload 短期记忆系统（暂不定时，锁定此处）

```
- crates/aeon-memory-core/src/offload/ 模块
  - OffloadStateManager: 状态机 (port of offload/state-manager.ts)
  - L1 工具摘要: 调用 LLM 为 tool call+result 生成摘要
  - L1.5 任务边界: 判断是否任务切换/继续
  - L2 Mermaid: 构建 Mermaid 流程图
  - L3 上下文压缩: 根据 token 占比触发压缩
  - storage: offload-*.jsonl, refs/*.md, mmds/*.mmd
  - hooks: after-tool-call, before-prompt-build
```

### Phase 3 — Skill 自动生成 (roadmap)

```
- 对应的 seed 数据 + LLM pipeline
```

---

## 7. 兼容基线（可自动验证的行为契约）

### 7.1 数据格式

| 文件 | 验证方法 |
|------|----------|
| `conversations/YYYY-MM-DD.jsonl` | Rust 写入一行 → 与 TS 写入的逐字节 diff = 0 |
| `records/YYYY-MM-DD.jsonl` | Rust 写入一行 → 与 TS 写入的逐字节 diff = 0 (字段顺序、转义、缩进) |
| `scene_blocks/*.md` | Rust 写入 → 与 TS 写入的逐字符 diff = 0 |
| `persona.md` | Rust 写入 → 与 TS 写入的逐字符 diff = 0 |
| `checkpoint.json` | Rust 写入 → 与 TS 写入的逐字段 diff = 0 (排序、精度) |
| `manifest.json` | Rust 写入 → 与 TS 写入的逐字段 diff = 0 |
| `vectors.db` | Rust 打开 TS 创建的 DB → query L1 表行数 = TS 报告的行数 |

### 7.2 排序与阈值

| 行为 | 验证方法 |
|------|----------|
| RRF K=60 | 相同 ranked list + RRF → 最终排序一致 |
| FTS5 BM25 分数 | 相同查询+文档 → 分数与 TS 一致 (允许 1e-6 浮点误差) |
| 向量余弦相似度 | 相同 embedding → 分数差异 < 1e-6 |
| 混合搜索阈值 0.3 | 相同候选 → 过滤后结果集一致 |
| 召回预算 maxCharsPerMemory | 相同 lines + budget → 截断后 lines 一致(含截断后缀 "…（已截断..." ) |
| 召回预算 maxTotalRecallChars | 相同 lines + budget → 丢弃/截断一致 |

### 7.3 Pipeline 调度

| 行为 | 验证方法 |
|------|----------|
| Warmup 1→2→4→N | 注入 N 轮 `notifyConversation`, 检查 L1 触发时机 |
| L1 空闲超时 | 注入 1 轮, 等待 timeout, 检查 L1 触发 |
| L2 delay-after-L1 | L1 触发后, 检查 L2 触发时间 ≥ now + delay |
| L2 minInterval | L2 完成后立即再触发 L1, 检查 L2 不触发 < minInterval |
| L2 maxInterval | L2 完成后, 等待 maxInterval, 检查 L2 触发 |
| L3 全局去重 | 并发触发两次 L2 → L3 只运行一次 |
| session GC | 模拟不活跃 session, 检查 GC 后状态为空 |

### 7.4 Prompt 文本

| Prompt | 位置 (TS) | 验证方法 |
|--------|-----------|----------|
| L1 extraction | `src/core/prompts/l1-extraction.ts` | Rust `const SYSTEM_PROMPT: &str` 与 TS 字符串 `diff` = 0 |
| L1 dedup | `src/core/prompts/l1-dedup.ts` | 同上 |
| Scene extraction | `src/core/prompts/scene-extraction.ts` | 同上 |
| Persona generation | `src/core/prompts/persona-generation.ts` | 同上 |
| 记忆工具指南 | `src/core/hooks/auto-recall.ts:35` | 同上 |

### 7.5 错误行为

| 场景 | 行为 |
|------|------|
| Embedding API 超时/失败 | 降级到 keyword-only，不抛出，不阻塞 |
| LLM API 失败 | L1 runner 返回 0 条, L2/L3 跳过, checkpoint 不变 |
| VectorStore init 失败 | 降级到 JSONL fallback, `isDegraded()` = true |
| 无效 query/空的 session_key | 返回 400 HTTP / CLI 友好错误 |
| SQLite WAL 并发写 | `busy_handler` / `busy_timeout` 防死锁 |

### 7.6 HTTP 响应

| 端点 | TS 响应字段 (src/gateway/types.ts) |
|------|------------------------------------|
| `GET /health` | `{status, version, uptime, stores: {vectorStore, embeddingService}}` |
| `POST /recall` | `{context, strategy?, memory_count?}` |
| `POST /capture` | `{l0_recorded, scheduler_notified}` |
| `POST /search/memories` | `{results, total, strategy}` |
| `POST /search/conversations` | `{results, total}` |
| `POST /session/end` | `{flushed: true}` |
| `POST /seed` | `{sessions_processed, rounds_processed, messages_processed, l0_recorded, duration_ms, output_dir}` |

---

## 8. 删除清单（删前条件：Rust 等价能力 + 兼容测试完成）

| 待删除 | 删前条件 | 优先级 |
|--------|----------|--------|
| `index.ts` | Phase 1e 完成, Rust HTTP + CLI 已全覆盖 | Phase 1e 后 |
| `src/adapters/openclaw/` | Phase 1e 完成, Rust 无 OpenClaw 依赖 | Phase 1e 后 |
| `src/adapters/standalone/` | Phase 1e 完成, Rust HTTP server + CLI 覆盖 | Phase 1e 后 |
| `src/gateway/` | Phase 1e 完成, `compat_http_response.rs` 逐字段通过 | Phase 1e 后 |
| `src/cli/` | Phase 1e 完成, Rust `aeon-memory` CLI 子命令全覆盖 | Phase 1e 后 |
| `openclaw.plugin.json` | Rust 不再需要 | Phase 1e 后 |
| `tsdown.config.ts` | Rust 不再需要 | Phase 1e 后 |
| `package.json` (openclaw peer deps) | Rust 不再需要 | Phase 1e 后 |
| `hermes-plugin/` | Phase 1e 完成, `client.py` 指向 Rust HTTP | Phase 1e 后 |
| `scripts/*.patch.sh` | OpenClaw 不再需要 | Phase 1e 后 |
| `src/offload/` | **不删** — Phase 2 迁移 | Phase 2 完成后 |
| `src/utils/pipeline-factory.ts` | Phase 1b 完成 | Phase 1b 后 |
| `src/utils/pipeline-manager.ts` | Phase 1c 完成, `compat_pipeline.rs` 通过 | Phase 1c 后 |
| `src/utils/checkpoint.ts` | Phase 1c 完成 | Phase 1c 后 |
| `src/utils/manifest.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/utils/time.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/utils/sanitize.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/config.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/core/store/sqlite.ts` | Phase 1a 完成, `compat_sqlite.rs` 通过 | Phase 1a 后 |
| `src/core/store/factory.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/core/store/embedding.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/core/conversation/l0-recorder.ts` | Phase 1b 完成, `compat_jsonl.rs` 通过 | Phase 1b 后 |
| `src/core/record/` | Phase 1b 完成 | Phase 1b 后 |
| `src/core/scene/` | Phase 1c 完成 | Phase 1c 后 |
| `src/core/persona/` | Phase 1c 完成 | Phase 1c 后 |
| `src/core/hooks/auto-recall.ts` | Phase 1d 完成 | Phase 1d 后 |
| `src/core/hooks/auto-capture.ts` | Phase 1d 完成 | Phase 1d 后 |
| `src/core/tools/` | Phase 1d 完成 | Phase 1d 后 |
| `src/core/prompts/` | Phase 1d 完成 (逐字匹配) | Phase 1d 后 |
| `src/core/seed/` | Phase 1d 完成 | Phase 1d 后 |
| `src/core/profile/` | Phase 1c 完成 | Phase 1c 后 |
| `src/core/report/` | 可选移除 (Rust 另实现 tokio-metrics) | Phase 1e 后 |
| `src/core/aeon-memory-core.ts` | Phase 1d 完成 | Phase 1d 后 |
| `src/core/index.ts` | Phase 1d 完成 | Phase 1d 后 |
| `src/core/types.ts` | Phase 1a 完成 | Phase 1a 后 |
| `src/utils/` (其余) | 各 Phase 对应完成后 | 各 Phase 后 |

---

## 9. Definition of Done

当以下所有条件满足时，迁移视为完成：

### 9.1 功能完整

- [ ] Rust `aeon-memory-core` 实现 `AeonMemoryCore` 全部公开方法，签名为 `async fn`，无 `panic`
- [ ] `cargo test --workspace` 全部通过（含 compat 测试）
- [ ] 7 个 HTTP 端点响应与 TS Gateway 逐字段一致
- [ ] CLI `aeon-memory` 全部子命令可用，JSON 输出与 TS 一致
- [ ] Rust 直接打开 TS 版本创建的 `vectors.db` + `*.jsonl` + `*.md` 无错误
- [ ] 混合搜索在相同输入下返回相同 top-5 排序
- [ ] pipeline warmup / idle / L2 timer / L3 dedup 调度行为与 TS 一致

### 9.2 性能

- [ ] 单轮 capture (含 L0 写入) < 50ms (p99)
- [ ] 单轮 recall (混合搜索, ≤5 结果) < 500ms (p99, 排除 LLM 和 embedding HTTP)
- [ ] seed 100 轮对话 < 与 TS 版本同等时间 (+/- 20%)
- [ ] SQLite WAL 模式, 支持并发读写

### 9.3 兼容

- [ ] 每个删除项对应的 Rust 等价能力已提交并通过兼容测试
- [ ] Hermes Python `client.py` 通过设置 `MEMORY_TENCENTDB_GATEWAY_CMD` 指向 Rust binary 后无感知
- [ ] 所有 prompt 文本与 TS 版本逐字匹配

### 9.4 工程

- [ ] `cargo clippy --all-targets -- -D warnings` 通过
- [ ] `cargo build --release` 产物 < 20 MB (静态链接)
- [ ] Dockerfile (multi-stage, distroless base) 构建成功
- [ ] 代码注释包含 `// port of <TS file path>:<line>` 便于未来回溯

---

## 10. 下一步唯一建议 Prompt

```
冻结基线已锁定在 RUST_MIGRATION_PLAN.md。当前阶段：实现 Phase 1a Step 1–3。
1. 在仓库根目录创建 Cargo workspace（members: aeon-memory-core, aeon-memory-store-sqlite）
2. 在 crates/aeon-memory-core/src/ 创建 lib.rs + types.rs + error.rs + config/mod.rs
3. types.rs 从 src/core/types.ts + src/core/store/types.ts 翻译，接口转为 Rust trait
4. config/mod.rs 从 src/config.ts + src/gateway/config.ts 翻译，serde + envy
5. 在 crates/aeon-memory-store-sqlite/src/ 创建 lib.rs + connection.rs + schema.rs + l0.rs + l1.rs
6. schema.rs 从 src/core/store/sqlite.ts 翻译 DDL + migration
7. 兼容测试: Rust 打开 TS 创建的 vectors.db 验证 schema 一致
8. cargo test --workspace 全部通过
禁止修改仓库中任何 .ts / .json / .yaml 文件。
```
