# herdr core 兼容 Tern TSP 的可行性

日期：2026-11-06
对照对象：herdr-core **0.1.18**（`hc`）与 Tern Surface Protocol v1（<https://docs.stencil.so/tern/protocol/>）
性质：评估，不含实现；文中的「实测」都在隔离 session（`HC_SESSION=tsp-probe`，独立 `XDG_*`）里跑过。

---

## 1. 结论

**协议层可兼容，语义层要改模型，渲染层是真成本。**

| 层 | 结论 | 依据 |
|---|---|---|
| 传输（APC、分块、JSON body） | **今天已经兼容**：core 原样透传，不吞不改 | §2.1 |
| 握手（`q` hello / DA1 顺序） | **需要 core 动手**，但改动集中：core 是唯一能写 pty 输入的人 | §2.2 |
| 文档与流控（frames、credits、retention、快照） | **需要新状态**：core 从「字符网格 + scrollback」升级成「网格 + 表面文档」 | §2.3、§4 |
| 渲染（74 个 kinds） | **主要成本**，且按 viewer 重复收费 | §3、§5 |

一句话：**核心的架构和 Tern 是同一个形状，所以能接；但"兼容"意味着 herdr core 要变成一台会排版的终端，而不只是会转发字节的 mux。**

---

## 2. 实测到的现状

### 2.1 APC 原样透传到 viewer，且流内有序

程序每 300 ms 打印一条 `ESC _ tsp;f;{...} ESC \`，同时 `terminal.attach` 抓原始字节：

```
\x1b_tsp;f;{"sf":"s1","s":119,"ops":[["set","a",{"text":"hi"}]]}\x1b\\
```

- APC **完整出现在 attach 流里**，和 VT 字节保持同序。
- core 自己的 Screen **丢弃** APC（屏幕上只有程序另外打印的 `.`），`hc read` 看到的也是丢弃后的结果。
- 结论：viewer 侧今天就能"听见"完整的 TSP 会话，core 不需要为见到字节而改任何东西。

### 2.2 core 自答 DA1，程序据此优雅降级

程序打印 hello 查询 + `ESC [ c`，回显里出现：

```
^[[?62;22c
```

- core 把 DA1 答复写进 pty 输入（和我们自己在 `docs/protocol-facts.md` §7 记的一致）。
- 前一条 `tsp;q` **没有** `tsp;r` 回复，所以按 TSP 规范程序判为「没有 TSP」，继续用自己的 ANSI 渲染器。
- 结论：**今天的不兼容是安全的**——不会乱码、不会卡住，只是拿不到 native surface。

### 2.3 快照只有网格，文档不重连

同一条 attach 流里，首帧是标准的整屏复位 + 网格重放（`\x18`、软复位、模式全关、清屏、tabstop、逐行内容），**没有**之前已经发出的 `tsp;f` 帧。

- 结论：TSP 的文档状态今天**不能跨 attach**。谁把文档存在 viewer 里，谁就丢重连。

---

## 3. 架构同构：这就是"可以"的理由

| Tern | herdr |
|---|---|
| 程序在 pane 里写 pty | 一样 |
| session daemon 的 **screen 持有 surface 文档**，多个 window attach 拿到同样的字节 | core 持有 screen，多个 viewer attach 拿到同样的字节 |
| window 负责排版与绘制，`ack` 由「在画的那个窗口」发；没有窗口时程序按 credits 挂住 | viewer 负责绘制；detach 后程序本来就得等 |
| 后台 / 重连：daemon 用 `o`/`s`/`f`/`settle`/`x` 合成重放给新窗口 | core 现在已经会合成整屏复位；缺的就是表面文档这一段 |
| 窗口尺寸决定 `cols`，程序据此只做 `rows` 回退和 ANSI 宽度 | 尺寸归「最近活跃的非 observe viewer」 |

也就是说 Tern 已经把自己的 daemon/window 切分做成了 herdr core/viewer 的形状。这不是类比，是同一套进程拓扑。

---

## 4. 三条候选路线

### A. core 自己排版，viewer 不变（推荐方向）

core 解析 TSP → 保持文档 → 用**单元格**把表面渲染成网格行 → 照常往下游发 VT 字节。

- viewer（hc-tui / herdr-app / `hc attach` / 手机 / `hc read`）**零改动**，`hc read`、`hc ls` 预览、截图也都还有内容。
- 重连自动成立：表面已经在网格里，首帧重放天然包含它。
- 代价：core 里要有一套排版引擎（flex 一维、`ch` 单位、折行、行数回退）、点击命中映射、retention。
- 动画（spinner/elapsed）没有 GPU 也能做，就是 core 定时重绘那几行。

### B. core 只存文档，viewer 排版

core 存文档树（默认 64 MiB/表面预算）、握手、attach 时重放合成快照；viewer 画。

- core 改动比 A 小，画质上限高（herdr-app 是 wgpu，98 MB 的 GPU GUI，理论上画得出 sans / 渐变）。
- 代价：每个 viewer 都要重新实现 74 个 kinds + CSS 子集 + markdown + 表格 + 树 + 图表 + 命中测试；hc-tui 是单元格 TUI，**结构上做不到**（无比例字体、无渐变、无图混排）。多 viewer 时 ack 归属、尺寸归属都要仲裁。
- `hc read`、手机、detach 后的可读性全部变空白。

### C. 不接（今天的现状）

程序探测失败 → 用自己的渲染器。零成本、零风险，也是「herdr 作为 mux 被 Tern 包着」时唯一正确的行为（见 §6）。

---

## 5. 硬约束清单（接的时候会踩到的）

| # | 约束 | 影响 |
|---|---|---|
| 1 | **DA1 前必须已有 `tsp;r`** | core 自答 DA1，所以只能由 core 回 hello；core 若想委托 viewer，就得把 DA1 答复扣住等 viewer（或超时），否则程序永远判无 TSP |
| 2 | **`kinds` 必须诚实** | 回复里列了不会画的就是 bug；没列的 kinds 允许程序改用 `rows` 回退——所以「窄 kinds」是合法形态 |
| 3 | **多 viewer 抢尺寸** | 已经是「最后活动者赢」；TSP 的排版吃 `cols`，两人抢会抖动（同 `docs/protocol-facts.md` §5 的老问题，这里会变成可见的布局跳变） |
| 4 | **ack 只由画的那个窗口发** | 多 viewer 时谁算「那个窗口」要定规则；detach 期间程序按 credits 挂住，是规范行为，不是 bug |
| 5 | **`hc read` / 手机 / 截图** | 文档不在网格里的方案（B）会让这些入口看到空白 pane |
| 6 | **scrollback 锚点** | 关闭的 `flow` 表面要一直留到锚点行离开 scrollback；core 的 scrollback 淘汰点正好是挂载处 |
| 7 | **clean pane / alternate screen** | `inline` 表面活着时要盖掉 shell 行与更老的表面；`screen` 表面等效于 alt screen。core 现在只会往下游倒字节，没有「这块视口归谁」的概念 |
| 8 | **单条 APC ≤ 262 144 字节**，body 分块 `c=`/`m=`，帧可到 400 MB | core 的解析器必须能跨 chunk 拼、能在被打断时丢掉并回 `error` |
| 9 | **blob 是 SHA-256 内容寻址缓存**，`TERN_BLOB_DIR` 可走文件 | 要接图片就得有磁盘缓存与 LRU，不能只当字节转发 |
| 10 | **`hour12` / `cell` 像素 / `reduceMotion`** | `cell` 我们已经有 `FACTS` 帧；`hour12`、`dark`、`reduceMotion` 要 core 从宿主拿并随 `theme`/`motion` 事件更新 |
| 11 | **`ESC ] 7501 ; state=…`** | 与 Tern 的 Program Status **是同一个序列、同一套状态词**（idle/working/done/blocked/error）。core 已经在记它，这块白送 |

---

## 6. 嵌套风险：herdr 自己是个 mux

Tern 文档 §Inside tmux 明确写了这个坑：tmux 会抢答 DA1、还会把 APC body 当 pane 标题，所以 Tern 只能靠 `@tern-tsp` 会话选项 + `allow-set-title off` 走旁路。

herdr core 处在**完全相同的位置**，而且更难：它的下游是 viewer，上游是程序，DA1 由它自己答。

现实组合已经存在：本机 `~/.config/tern/plugins/herdr`（tern-herdr 0.4.0）就是**在 Tern 的 pane 里 `herdr terminal attach`**。也就是说 `程序 → herdr core → Tern` 三层，其中 Tern 和 herdr core 都想当"那台 TSP 终端"。

若 core 实现 TSP，必须：

1. 提供与 `@tern-tsp` 等价的显式开关（session 级或 env），默认在外层已是 TSP 终端时**闭麦**；
2. 外层声明自己讲 TSP 时，core 不吞、不答、不改 APC，纯透传（今天的行为）；
3. 拦掉内层已消费的 body，别让它落到屏幕上变成文本（Tern 在 tmux 里就是被 APC→标题坑过）。

没有这一条，herdr 接 TSP 反而会打断已经在用的 tern-herdr 工作流。

---

## 7. 建议路线

| 阶段 | 做什么 | 成本 | 交付 |
|---|---|---|---|
| P0 | 明确"不接"，把探测行为与本文写进文档；确认 core 不在任何路径上改写 APC | 0 | 今天就是安全的 |
| P1 | core 侧最小握手：识别 `ESC _ tsp;`、在 DA1 前回 hello，`kinds` 只列单元格画得出来的子集（`col`/`row`/`card`/`rule`/`spacer`/`text`/`ansi`/`rows`），`features` 只列 `flow` | 小 | 程序会切到 native surface，且只发这几类 |
| P2 | core 侧表面存储：文档树、retention、`x keep`、scrollback 锚点、attach 时重放、click 命中回写 pty | 中 | 重连可用、`hc read` 有内容 |
| P3 | 扩 kinds（`table`/`tree`/`kv`/`stats`/`badge`/`progress`/`md`…）+ 动画时钟 | 大 | 覆盖 omp 这类真实程序 |
| P4 | GPU/富渲染让给 herdr-app 或独立 crate；core 只保证窄子集永远能画 | 很大 | 可选 |

**不要做**：让 hc-tui 去实现完整 TSP。它是单元格 TUI，74 个 kinds 里的比例字体、渐变、图表、图片混排都没有落点；窄 kinds + `rows` 回退是它能站的位置。

---

## 8. 值得单独记的巧合

`ESC ] 7501 ; state=<idle|working|done|blocked|error>` 在两个系统里是同一个序列、同一套词表：Tern 用它决定「要不要在关闭 pane 前确认」，herdr core 用它做 agent 状态与 `turn` 通知。

这不是协议重合，是同一层语义被两边各自发现了一遍。将来接 TSP 时，herdr 的 agent 状态可以直接当 Tern 的程序状态用，反之亦然——这一点比 74 个 kinds 更值钱。

---

## 9. 场景 B：把 hc-tui 跑在 Tern 里，不改 core

链路：`程序 → herdr core → hc-tui（ghostty-vt + ratatui）→ Tern 的格子`。

**结论：不会渲染 TSP，但也不会坏——看到的是程序自己的 ANSI 界面。**

### 9.1 为什么结构性做不到

- TSP 握手要求 `tsp;r` **先于** DA1 答复到达程序输入。pane 的 pty 归 core，DA1 是 core 在同一段解析里立刻答的（实测 `?62;22c`），hc-tui 最早也要等 core 把字节转过来才看见查询。
- hc-tui 即使事后往 attach 流写一条 `tsp;r`，程序已经读完 DA1 判为「无 TSP」，规范里写明后到的回复不生效。
- 所以「谁拥有 pty，谁才能当 TSP 终端」。不改 core，这条路关死。

### 9.2 不会显示成乱码

`vendor/libghostty-vt/src/terminal/apc.zig`：APC 按标识符分流，只有 kitty `G` 与 glyph 被识别；其余走 `.ignore`（`unknown_max_bytes = 0`，不缓存不报错）。`tsp;` 落在这里，既不上屏也不应答。

### 9.3 Tern 也帮不上

Tern 只回答**自己 pane 的 pty** 上的查询。hc-tui 里程序的查询早被 core 答掉，hc-tui 自己又只输出格子、不把 APC 转发出去，所以 Tern 眼里的 hc-tui 就是个普通 TUI 程序。

真想在 Tern 里看 native surface，只有两条路：程序直接跑在 Tern 的 pane 里，或者 core 侧实现（§4 的 A）。

### 9.4 一个真实隐患：`TERM_PROGRAM`

程序会看 `TERM_PROGRAM=tern` 走「不等回复、直接发 `o`/`f`」的快路径（`handshake.md#starting-without-waiting`）。那样发出去的帧没人接，pane 先是空白，规范说 1 秒后自己回退——但不是每个程序都实现了回退。

实测 core 与客户端的归属：

| 来源 | pane 里的 `TERM_PROGRAM` |
|---|---|
| core daemon 的环境（`TERM_PROGRAM=zzz` / `=tern`） | **`ghostty`**：core 自己改写，环境漏不进去 |
| `terminal.run` 的 `env` 参数 | 生效；`hc` CLI 会把自己的终端身份塞进去 |
| hc-tui 的 `terminal.run` | 不带 `env`，所以 pane 拿到 core 的 `ghostty` |

所以 hc-tui 本身是安全的；**危险的是在 Tern 的 pane 里直接 `hc run <agent>`**，那时 CLI 会把自己的 `TERM_PROGRAM=tern` 传下去。

规避与可选小修（都不碰 core）：

1. `env -u TERM_PROGRAM hc run …`；
2. 或者给 hc-tui 的 `terminal.run` 补上 `env: {TERM_PROGRAM: "hc-tui", COLORTERM: ...}`，把身份说清楚，别让下游继承模糊值（`terminal.run` 的 `env` 是现成参数：*Set on top of the core's environment*）。
