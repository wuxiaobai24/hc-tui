# hc-tui

基于 [herdr-core](https://github.com/herdrdev/herdr-core)（`hc`）的 Rust 原生终端工作台。
Core 拥有终端与进程，本仓库拥有 Workspace / Tab / Pane / 渲染。

当前状态：**骨架阶段**。可编译、可运行、有测试；尚未连接 core 的 attach。

## 快速开始

```sh
# 需要 Zig 0.16.0（libghostty-vt 硬性要求）与 Rust stable
mise install                 # 或自行安装 zig 0.16
cargo build
cargo run -- probe           # 连上 core，打印握手结果
cargo run                    # 打开 TUI（演示 VT 缓冲）
cargo test
```

`probe` 在 core 未运行时会由 `hc connect` 自动拉起。

## 已经有什么

| 模块 | 状态 |
|---|---|
| `hc::codec` | framed 编解码（9 B header、DATA/CREDIT/CLOSE/RESIZE/FOCUS/FACTS），有单测 |
| `hc::client` | `hc connect` 子进程、明文 hello、generation 校验、单条 RPC、帧读写、credit |
| `term::view` | 本地 VT → Ratatui Buffer 的桥：样式、宽字符、真彩、反显、光标 |
| `layout::split` | BSP 分裂与矩形计算，有单测 |
| `app` / `ui` | 事件循环、侧栏、状态栏、前缀键（`ctrl+b`） |
| `tui` | raw mode / alt screen / 鼠标的进入与 RAII 恢复 |

测试里有一条**真实 core 首帧**（`tests/fixtures/core-first-paint.bin`），
用来锁住「字节 → cell」的还原行为。

## 尚未做（G1）

- attach 生命周期：首帧、实时 DATA、`size` 事件、observe 语义、重连
- 事件订阅与 `missed` 重同步
- FACTS 上报、透传查询应答（kitty 探测）
- Pane 绑定真实终端、BSP 多 Pane、Tab / Workspace 持久化

## 文档

| 文档 | 作用 |
|---|---|
| [docs/plan-v1.1.md](docs/plan-v1.1.md) | 权威实施方案，冲突时以本文为准 |
| [docs/review-v1.0.md](docs/review-v1.0.md) | 对 v1.0 的评审、证据与更正理由 |
| [docs/protocol-facts.md](docs/protocol-facts.md) | hc 协议实测事实，实现的直接依据 |
| [docs/adr/0001-ghostty-vt.md](docs/adr/0001-ghostty-vt.md) | Ghostty 选型、风险与升级路径 |
| [docs/plan-v1.0.md](docs/plan-v1.0.md) | v1.0 原文归档 |

## 环境基线

- `hc` 0.1.17（以 `hc schema` 为准；源码 checkout 可能更旧）
- **Zig 0.16.0**（0.17 编译失败，见 ADR-0001）
- Rust stable（骨架验证于 1.99.1）

许可：Apache-2.0。第三方组件见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。
