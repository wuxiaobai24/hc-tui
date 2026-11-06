# v1.0 方案评审记录

> 历史资料：保留当时的设计与评审，不代表当前功能。当前入口见 [README](../README.md)、[使用指南](usage.md) 和 [对齐契约](herdr-app-alignment.md)。

日期：2026-11-04
对象：[plan-v1.0.md](plan-v1.0.md)
结论：**方向成立，可作为基线；5 处协议事实需更正，4 项约束需补充，1 个单点风险已量化并验证。**

---

## 1. 评审方法

不是只读文档，做了三件事：

1. **读源码**：hc 0.1.17（以安装版 `hc schema` 为准，本机 checkout 偏旧）、原 Herdr TUI 副本（Ghostty 封装、Pane 渲染、BSP 布局）。
2. **实测协议**：在隔离 session（`/tmp` 下的 XDG 路径）里起了真实 core，跑通
   `hello → terminal.run → events.subscribe → terminal.attach → 双 viewer resize`，
   抓取首帧、事件与帧序。
3. **编译验证**：把原 TUI 的 `crates/ghostty-vt` 单独编译，并用真实首帧做还原测试。

> 所有测试都在 `HC_SESSION` + 独立 XDG 路径下进行，未触碰本机真实 core；结束后已 `hc server stop` 清理。

---

## 2. 成立的部分

| 判断 | 证据 |
|---|---|
| 客户端必须自己跑 VT | attach 只发原始字节 + 首帧 ANSI；`terminal.read` 只返回纯文本；无「带样式 cell」RPC |
| 首帧可用于重建整屏 | 抓到 392 B 首帧，含软复位、模式全关、清屏、tabstop 重建、内容、光标定位；喂本地 VT 后逐 cell 还原正确 |
| framed 协议可用 | 9 B header、channel 0 JSON-RPC、事件/attach 分 channel、256 KiB credit、4 MiB 重绘阈值 |
| `hc connect` 子进程方案可行 | 实测连通，且 core 未运行时自动拉起（`connect_or_start`） |
| doc 是真 CAS | `doc.get` 返回 version；`doc.set{version}` 冲突 kind = `conflict`；value ≤ 1 MiB |
| 逐键不能用 `terminal.send` | `send` = 粘贴 + Enter |

---

## 3. 事实更正

| # | v1.0 原文 | 实际 | 依据 |
|---|---|---|---|
| 1 | §5.1 客户端 `hello` 声明 `automatic_input` / `resize_notifications` | `hello.features` 是「另一个 core 拨入」时的能力声明，不是客户端协商；带上也不会改变 attach 的返回值 | schema 字段说明；实测 attach 仍返回 `resize_notifications:false` |
| 2 | §5.2 `AUTOMATIC_INPUT = 6` 帧 kind | 只有 0–5。且 core 的 Screen 已代答 DA1/DSR/颜色查询，这些序列不转发 | `wire.rs`；`relay.rs::take_responses` 与其单测 |
| 3 | §5.4 attach 参数 `resize_notifications` | 该字段在 **返回** 里，对普通客户端恒 false | 实测两次 |
| 4 | §7 `terminal.run {size:{cols,rows}}` | `RunParams` 无 size；新终端默认 200x120（`headless` 可配）；尺寸由 attach 决定，且在首帧前应用 | `api/types.rs`、`config.rs`、实测 run 返回 200x120 |
| 5 | §5.3 事件清单 | 少 `keep`；`foreground` 源码会发但官方 subscribe 文档未列 | `events.rs` 的 emit 点 |

---

## 4. 遗漏项（会导致「偶发但难查」的 bug）

| # | 遗漏 | 后果 | 补救 |
|---|---|---|---|
| 1 | 事件流也要回 CREDIT | 长跑后事件静默停止，不报错不重连 | 所有 stream 按字节回 credit |
| 2 | observe 的尺寸来源 | observe viewer 收不到重绘，也收不到尺寸通知 | 只信 `size` 事件，先 resize 本地 VT 再处理后续字节 |
| 3 | 跨 channel 顺序 | 两个 task 各读各的 → 用旧尺寸解析新字节，宽字符错位 | 单 reader 按帧序分发 |
| 4 | kitty 查询应答 | core 不代答，程序可能一直等 | 首版给最小应答，渲染可延后 |
| 5 | attach 后未发 FACTS | 颜色/像素查询拿到默认值，鼠标坐标系不可靠 | attach → FACTS |
| 6 | 旧 wrapper 的图像落盘模块 | 新增文件写面 | 不启用或移植时裁掉 |

---

## 5. 复用边界（v1.0 表述过宽）

v1.0 §1 写「选择性复用原有 UI 代码」，实际要拆开：

- **可直接搬**：`crates/ghostty-vt`（整包）；`layout.rs` 的 BSP 算法；`pane/terminal.rs::render` 的 VT→Buffer 桥（约 700 行，含样式映射、宽字符、palette override、光标）；快捷键/主题/状态机**设计**。
- **不可直接搬**：`ui/panes.rs`、`ui/sidebar.rs`、`client/shell/*` —— 绑在旧 `AppState` 与旧服务端帧（`PaneSurfaceFrame`）上。本地 UI 虽为进程内直画，但接口与 `TerminalView` 不同，等于重写。

---

## 6. Ghostty 风险（详见 ADR-0001）

**已复现**：本机默认 `zig` 0.17.0，`build.zig` 硬性要求 0.16.0 且 0.17 的 `Build` API 已变，编译直接失败。换 mise 的 0.16.0 后 **1m38s 编过**。

**已验证**：把真实首帧（392 B）喂进旧封装，还原出粗体红、下划线绿、真彩 RGB(12,34,56)、反显、宽字符、光标 (0,4)，全部正确。

结论：可行，但 Zig 必须钉死，且版本错配（core `e5cdb86` vs 封装 `44f2a44`）要靠语料回归覆盖。

---

## 7. 排期修正

| 阶段 | v1.0 | v1.1 |
|---|---|---|
| P0 | 2–4 d | 3–5 d（含 Ghostty 门禁） |
| P1 | 4–8 d | 8–12 d |
| P2 | 4–7 d | 6–9 d |
| P3 | 3–6 d | 4–6 d |
| P4 | 4–8 d | 5–8 d |
| 合计 | 17–33 d | **26–40 d（单人 6–9 周）** |

---

## 8. 实测证据附录

### 8.1 握手与能力

```
features: ['framed', 'attach', 'events', 'status', 'shell', 'resize_notifications']
```

### 8.2 run 与 attach

```
run size 200x120
attach -> {"channel": 1, "resize_notifications": false}
```

### 8.3 首帧（节选）

```
\x18\x1b[!p\x1b[?2026l...\x1b[?1049l\x1b[H\x1b[2J\x1b[3g\x1b[9G\x1bH...
\x1b[0m\x1b[1m\x1b[38;5;1mbold red\x1b[0m \x1b[0m\x1b[4m\x1b[38;5;2munderline green
\x1b[0m\r\n宽字符：漢字テスト\r\n
\x1b[0m\x1b[38;2;12;34;56mtruecolor\x1b[0m\r\n\x1b[0m\x1b[7mreverse\x1b[0m\x1b[5;1H\x1b[0m\x1b[?9001l
```

还原结果：调色板索引色、真彩、粗体、下划线、反显、宽字符列宽、光标位置全部正确。

### 8.4 多 viewer 尺寸

```
A(non-observe) RESIZE 90x20  → 事件流: size{"cols":90,"rows":20}
B(observe)     RESIZE 120x40 → 被忽略；B 的 attach 流无通知、无重绘
```

### 8.5 编译

```
zig 0.17.0 → error: no field named 'build_root' in struct 'Build'
            error: Your Zig version v0.17.0 does not meet the required build version of v0.16.0
zig 0.16.0 → Finished `release` profile in 1m 38s
```
