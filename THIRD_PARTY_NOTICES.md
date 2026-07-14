# Third-party notices / 第三方声明

## TencentDB Agent Memory

Aeon Memory is an independent Rust reimplementation derived from and designed
for behavioral compatibility with
[TencentDB Agent Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory).
The upstream project introduced the L0 Conversation -> L1 Atom -> L2 Scenario
-> L3 Persona hierarchy and the context-offload design that this project
preserves.

Upstream copyright: Copyright (C) 2026 Tencent. All rights reserved.
Upstream license: MIT.

Aeon Memory is not an official Tencent or Tencent Cloud product, is not
affiliated with Tencent, and is maintained independently. We sincerely thank
the TencentDB Agent Memory maintainers and contributors for publishing their
design and implementation as open source.

Aeon Memory 是基于腾讯开源项目
[TencentDB Agent Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)
设计并进行行为兼容的独立 Rust 重实现。L0 到 L3 分层记忆体系与上下文卸载设计来自
上游项目。本项目并非腾讯或腾讯云官方产品，与腾讯无隶属或背书关系，由社区独立维护。
衷心感谢上游维护者和所有贡献者以开源方式分享这些工作。

## sqlite-vec

Native release archives bundle the official
[sqlite-vec](https://github.com/asg017/sqlite-vec) loadable extension:

- Fixed release version: `0.1.9`
- License: MIT OR Apache-2.0
- Distribution: unmodified upstream binaries
- Integrity: every asset is verified against the SHA-256 values published in
  upstream release `v0.1.9` before packaging

All third-party components remain subject to their respective licenses.
