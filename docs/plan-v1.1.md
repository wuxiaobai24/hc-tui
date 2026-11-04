# HC TUI 技术方案 v1.1

状态：**可执行基线**
日期：2026-11-04
取代：[plan-v1.0.md](plan-v1.0.md)（本文与 v1.0 冲突时，以本文为准）
依据：hc 0.1.17 源码与 `hc schema`、原 Herdr TUI 源码副本、2026-11-04 的实测（见 [protocol-facts.md](protocol-facts.md)）

---

## 0. 相对 v1.0 的变更

| # | v1.0 | v1.1 | 原因 |
|---|---|---|---|
| 1 | 客户端 `hello` 声明 `automatic_input` / `resize_notifications` | 只发 `{framed:true}`，能力以返回值为准 | `hello.features` 是「拨入的另一个 core」声明用；对客户端无效（实测） |
| 2 | `AUTOMATIC_INPUT = 6` 帧 | 删除；core 自己回答终端查询，客户端只答透传过来的 | 帧 kind 只有 0–5；`relay.rs` 已代答 DA1/DSR/颜色 |
| 3 | attach 参数 `resize_notifications` | 改为读 attach **返回**的该字段（对客户端恒 false） | 参数不存在 |
| 4 | `terminal.run {size}` | `terminal.run` 后立刻 `attach{cols,rows}`；core 在首帧前 resize | `RunParams` 无 size；默认 200x120 |
| 5 | 未提事件流 credit | 所有 stream 都必须回 CREDIT | 事件流也有 256 KiB 窗口，不还会静默停住 |
| 6 | observe 视为旧 Core 降级 | 明确为当前语义：最近活动的非 observe viewer 定尺寸 | 实测 |
| 7 | kitty graphics 整体延后 | 渲染可延后，**查询应答进首版** | core 只转发不代答，程序可能等待 |
| 8 | 未提 FACTS | attach 后立即发 FACTS | 颜色/像素尺寸查询依赖它 |
| 9 | 排期 17–33 人日 | 28–45 人日（单人 6–9 周） | P1 被低估 |
| 10 | 未定义 Ghostty 门禁 | `G0/G1` 门禁 + 兜底判据 | 唯一单点风险 |

---

## 1. 目标与边界

### 1.1 职责划分

- **Core（hc）**：PTY、进程与 Holder、持久化、SSH、远程机器、OSC 7501 状态解析、终端查询代答、屏幕快照（`paint`）。
- **hc-tui**：连接与重连、VT 模拟、Workspace/Tab/Pane、布局、输入路由、渲染、本地偏好。
- **不进入 hc-tui**：任何 PTY 生命周期管理；任何「猜测终端在做什么」的判断。

### 1.2 首版范围

必须：Workspace / Tab / 横纵分屏 / 焦点 / 缩放；Sidebar（机器树 + Agent 列表）；任意 CLI 作为普通终端；全彩 ANSI、alternate screen、Unicode 宽字符、滚动、复制、粘贴；detach 后进程存活、重开恢复；远程终端发现与 attach；可配置键位/主题。

暂缓：原生语义 Chat UI、文件树/Diff/Browser、Kitty Graphics **渲染**、原版设置页/Worktree/插件、Windows/Web/Android 首发。

> 注意：Kitty Graphics 的**能力协商应答**属于首版（见 §3.7），只有像素渲染延后。

---

## 2. 架构

```
              ┌──────────────── hc-tui (前台) ────────────────┐
  Host TTY ──▶│ Input Router ─▶ App Reducer ─▶ Layout ─▶ UI   │
              │       │              │                        │
              │       ▼              ▼                        │
              │  KeyEncoder     hc-client (单 reader)          │
              │       │              │                        │
              │  TerminalView ◀── 帧分发 ──▶ Events           │
              └───────┬──────────────┬────────────────────────┘
                      │ attach DATA  │ JSON-RPC
                      ▼              ▼
                 hc connect 子进程（stdin/stdout 裸协议）
                      │
                      ▼
                 herdr-core（本地，可级联远程）
```

### 2.1 硬约束

1. **单 reader 按帧序分发。** 事件与 attach 字节共用一个 outbox，帧序有保证；客户端若用多个 task 各读各的，`size` 事件与后续字节会乱序，observe viewer 会用旧尺寸解析新字节。
2. **VT 状态单线程。** 每个 `TerminalView` 的 `Terminal + RenderState` 只在一个线程内推进；跨线程只传字节和渲染请求。
3. **UI 线程不 await。** 网络/子进程在 Tokio 侧，UI 通过通道收状态。
4. **首期单 Cargo package**，`src/{app,hc,term,layout,ui}` 模块隔离；`crates/ghostty-vt` 是唯一例外（必须独立编）。

### 2.2 连接方式

首期启动 `hc connect --session <s>` 子进程，读写其 stdin/stdout。它复用 core 的 endpoint/session 解析，并在 core 未运行时自动拉起。后期再评估 Unix Socket 直连（需复现同样规则）。

---

## 3. Core 协议（实现要点）

完整实测记录见 [protocol-facts.md](protocol-facts.md)。

### 3.1 握手

1. 起 `hc connect`，发**明文** JSON-RPC：`hello {generation:1, framed:true}`。
2. 读明文回复；此后全部按帧解析。
3. 校验 `generation == 1`；缺必需 method 时报「版本不匹配」并提示 `hc update`。
4. `features` / `methods` 只做能力判断，**不要**反过来往里塞客户端能力。

### 3.2 帧

`[channel u32 LE][kind u8][len u32 LE][payload u8;len]`，HEADER=9，payload ≤ 16 MiB。
kind：`0 DATA / 1 CREDIT / 2 CLOSE / 3 RESIZE / 4 FOCUS / 5 FACTS`。

- channel 0：JSON-RPC（control 队列，不吃 credit）。
- 其他 channel：流，初始 credit 256 KiB。

### 3.3 流与 credit（必须）

- 每个流（attach、events）都要按收到的 DATA 字节数回 `CREDIT(u32 LE)`。
- 不回 credit 的症状是**静默停住**：不报错、不重连、数据就是不来。
- 积压超 4 MiB 时，attach 流被整段丢弃并由 core 在 DATA 里内联一份完整重绘；events 流保留整行后发 `missed` + 快照。
- 因此 VT 必须能处理**流中间的整屏复位**（`\x18` 开头那段），而不能假设只在 attach 时出现。

### 3.4 attach 生命周期

```
terminal.run            → 返回 record（默认 200x120）
terminal.attach{cols,rows} → {channel, resize_notifications, terminal}
  ← size 事件（若尺寸变了）
  ← 首帧完整 ANSI
  ← 实时 DATA …………………→ CREDIT
  ←…… 键盘 DATA / RESIZE / FOCUS / FACTS
CLOSE（本地 detach）
```

- 尺寸在首帧之前应用，不会出现「先按旧尺寸画一帧」。
- `observe:true` 的 viewer 不参与定尺寸，也收不到重绘；它的尺寸来源只有 `size` 事件。
- 程序结束时 core 先发完最后输出，再发 `CLOSE`（payload 为 JSON reason）。

### 3.5 尺寸语义

- leader = **最近活动的非 observe viewer**。
- 两个非 observe 客户端会互抢尺寸（最后动的赢），这是当前设计，不是 bug。
- 默认所有本地 Pane 用非 observe；「镜像 Pane」才用 observe，并在 UI 上标注只读。

### 3.6 事件

`created / restarted / lost / finished / removed / keep / status / turn / title / labels / size / foreground / doc / missed / heartbeat`

- `subscribe` 返回 `{seq, terminals[], channel}`；按 seq 顺序应用。
- `missed` = 整体重同步，不要继续应用增量。
- `doc` 只发给无过滤订阅者。
- 客户端重连后重建 seq 基线。

### 3.7 终端查询应答

- **core 已代答**：DA1、DSR（光标位置）、默认前景/背景/光标色等。这些序列不会出现在 attach 流里。
- **core 不答、透传给 viewer**：kitty graphics 的 `\x1b_G…a=q`、部分窗口查询。
- 客户端必须对透传查询给出应答（不支持也要回否定），否则程序可能一直等。应答通过 attach 的 DATA 回写。

### 3.8 输入

- 逐键：attach DATA（原始 VT 字节，由本地 VT 的当前模式决定编码）。
- `terminal.send` = 粘贴 + Enter，用于「给 Agent 发一条消息」，**不用于逐键**。
- `terminal.keys` = 命名键序列，供 Agent/脚本使用。
- 鼠标：FACTS 提供 cell 像素尺寸；由本地 VT 的鼠标模式决定编码格式。

### 3.9 文档（布局持久化）

- `doc.get` → `{value, version}`；`doc.set{name, value, version}` → `{version}`。
- 冲突错误 `kind = conflict`；`version = 0` 表示从未写过。
- name 规则同终端名（1–32，`a-z0-9-_.`）；value ≤ 1 MiB。

---

## 4. 终端视图

### 4.1 结构

```rust
struct TerminalView {
    terminal: ghostty_vt::Terminal,   // 本地 VT
    render_state: ghostty_vt::RenderState,
    attach: Option<AttachStream>,     // channel + credit 记账
    scroll_offset: usize,
    size: (u16, u16),
    modes: ViewModes,                 // 由 VT 状态派生，供输入编码
}
```

### 4.2 数据流

```
attach DATA ─▶ Terminal::write ─▶ RenderState::update ─▶ dirty rows ─▶ Buffer
                     │
                     └─▶ take_responses() ─▶ attach DATA（仅透传查询的应答）
```

- 解析与刷新解耦：字节到达即写 VT，渲染按 30–60 FPS 合并。
- 只画 `content` 区，绝不越过 Pane 边框/Sidebar。
- `SpacerTail` 单元格必须跳过（宽字符占两列），否则中文会错位一列。

### 4.3 输入

`HostKey → 前缀/导航/复制模式 → 焦点 TerminalView → KeyEncoder → attach DATA`。
Bracketed paste 依据 VT 当前模式决定是否包裹；大段粘贴一次写入，不逐字符。

### 4.4 降级

- VT 初始化或渲染失败：显示错误文本 + 「用 `hc read` 查看」提示，不留空白 Pane。
- 未知序列：忽略，不 panic。

---

## 5. 布局与状态

```rust
enum SplitNode {
    Leaf { pane_id: PaneId },
    Split { axis: Axis, ratio: f32, first: Box<SplitNode>, second: Box<SplitNode> },
}
```

- `PaneId` 与 `TerminalAddress` 独立：一个 Core 终端可以出现在多个布局视图（镜像需 observe）。
- 默认一个 Workspace 绑定一台机器 + 一个根目录；Tab 承载任务视图。
- 最小内容区 20x5；空间不足时自动折叠 Sidebar 或进 zoom。
- **Close Pane 只移除视图；Kill Terminal 是独立且需确认的操作。**

状态模型（权威字段全部来自 Core）：`blocked > error > done > working > idle > unknown`。
`done` 用 `turn` 增量 + 本地 `seen_turn`；重连不得把旧完成当新完成。`null` 不得推断为 idle。

---

## 6. 持久化

- 共享：`doc` 名 `hctui-layout-v1`，`{schema_version, revision, workspaces[]}`。**不存屏幕内容与对话**。
- 本地：`~/.config/hc-tui/config.toml`、`~/.local/state/hc-tui/state.json`（焦点、侧栏宽度、scroll、seen_turn、主题）。
- CAS 事务：本地立即应用 → debounce 200–500 ms → `doc.set{version}` → `conflict` 时拉最新 + 重放意图 → 仍失败则保留未提交操作并提示。
- 远程机器的布局不写进远端 doc；机器别名可变，引用需校验 + orphan 修复界面。

---

## 7. 可靠性与安全

| 场景 | 处理 |
|---|---|
| TUI 退出 / detach | 发 CLOSE，core 内终端继续运行 |
| 连接中断 | 冻结画面标 Reconnecting，指数退避重连，重连后重订阅 + 重新 attach |
| Core 重启 | 重建 seq / snapshot / docs，重新 attach |
| 远端失联 | 标记 Unreachable，其他机器不受影响 |
| `keep` + `restarted` | 新 shell 已恢复，原任务上下文不自动恢复（UI 必须说清） |
| 布局 CAS 冲突 | 操作级重放，不静默覆盖 |
| 外层终端异常 | RAII Guard 恢复 raw/mouse/alt screen（含 panic hook、SIGTERM） |

安全：

- OSC 52 剪贴板、OSC 8 超链接、title 都要过滤；不把终端字节写进日志（debug 原始帧需显式 opt-in + 脱敏）。
- 不从旧 wrapper 启用 kitty 图像落盘路径（`pane_graphics_files`），除非明确需要。
- Core socket 仅本账号；不做无认证公网转发。

---

## 8. 排期与门禁

| 阶段 | 内容 | 估算 |
|---|---|---|
| **G0** | 骨架：workspace、Zig 0.16、ghostty-vt 迁入、CI 构建矩阵、`hc-debug` 文本诊断 | 3–5 d |
| **G1** | 端到端单 Pane：framed codec → attach → 首帧渲染 → 键盘/鼠标 → CREDIT → CLOSE → 重连 | 8–12 d |
| P2 | BSP、Tab、Workspace、Sidebar、鼠标、快捷键 | 6–9 d |
| P3 | Agent 状态队列、父子 Agent、多机与远程 attach、断线恢复 | 4–6 d |
| P4 | Docs CAS、Copy Mode、压力/长稳、musl 与 macOS 发行 | 5–8 d |

单人合计 **26–40 人日 ≈ 6–9 周**（v1.0 的 17–33 人日偏乐观）。

**门禁（必须按序）**

- `G0 门禁`：`cargo build` 在 linux-gnu / linux-musl / macos-aarch64 三处通过；`hc-debug` 能把一段录制的首帧渲染出样式。
- `G1 门禁`：sh / nvim / fzf / htop / pi 五个程序可交互；detach 后进程存活；重连首帧正确；4 MiB/s 连续输出下控制通道不被饿死、内存有界。
- **未过 G1 不启动 P2 的 UI 工作。**

**Ghostty 兜底判据**（详见 [ADR-0001](adr/0001-ghostty-vt.md)）：

- G1 期间 5 个工作日内仍不能在上述 5 个程序上稳定；或
- 连续 3 个渲染 bug 的根因都是「解析不了 core 的新序列」。

任一成立 → 切换为「fork hc 的 ghostty-vt 并补 cell API」，不要两条腿走路。

---

## 9. 验收

**功能**：多 Pane 独立输入不串；关闭视图不杀进程；Tab/Workspace 切换保留终端；中文与宽字符不错位；alternate screen 进出正确；复制粘贴可用；布局双客户端并发修改可恢复。

**性能**：4 Pane 键到屏 p95 < 50 ms（指定机器）；空闲 CPU 低；内存随 Pane 数线性有界。

**语料**：`tests/fixtures/` 存真实 hc 录制的首帧与连续流（sh/nvim/fzf/htop/pi 各一份），任何 ghostty 升级或桥接改动都要跑绿。

**发行**：Linux musl 静态 + macOS aarch64；checksum；`THIRD-PARTY-NOTICES`（Apache-2.0 + MIT + Zig 依赖）。

---

## 10. 未决项

1. Kitty Graphics 渲染的优先级（应答已在首版）。
2. 输入法（IME）在 crossterm 下的一致性——需要单独验证。
3. 多客户端同时操作同一 Terminal 的 UI 提示策略（目前只有「最后活动者定尺寸」）。
4. 是否把 `term-view` 抽成独立 crate（等第二个调用方出现再说）。
5. 远程机器的布局同步模型（本地 doc vs 远端 doc）。
