# M9 基线：八区域帧快照、单帧耗时与 builtin↔mod 对拍

基线 commit `fc29a85`。这一份只固定可以被量化的三件事，不改任何界面行为：
`src/app.rs` 只多了一个 `#[cfg(test)]` 的离屏构造，`src/ui/mod.rs` 只多了
`#[cfg(test)] mod m9;`，其余全部在 `src/ui/m9.rs`（仅测试构建编译）、
`tests/snapshots/m9/` 与 `scripts/` 下。

八个默认区域：sidebar / tabs / status / picker / dialogs / settings / help / toast，
对应 `docs/mods.md` §3 规划里的八个常驻与浮层模块。

## 1. 帧快照

每个区域在 `80×24` 与 `120×40` 下各一份，共 16 个文件，落在
`tests/snapshots/m9/<区域>-<宽>x<高>.txt`。格式沿用 M1 的
`tests/snapshot_dump.rs`：首行是 `# region=… size=… rect=x,y,w,h`，其后每格一行
`x,y|符号|前景|背景|修饰`。

区域矩形与 `ui::draw` 一致：sidebar / tabs / status 用外框布局给出的轨道，
picker / dialogs / settings / help 用内容区，toast 用它最后落下的那个三行框。

快照状态是「有数据」场景：四个 Space（hc-tui / api / web / db）、三个 Tab、
七个带 agent 状态的 Pane；picker 有三项、prompt 里已输入 `platform`、
settings 停在第一段、help 未过滤、toast 有一条模块授权提示。

```console
$ HC_TUI_GRAPHICS=kitty cargo test -p hc-tui --lib m9_region_snapshots
running 1 test
test ui::m9::m9_region_snapshots ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 369 filtered out; finished in 0.06s
```

改动了界面并对齐预期之后重新生成：

```console
$ M9_UPDATE_SNAPSHOTS=1 HC_TUI_GRAPHICS=kitty cargo test -p hc-tui --lib m9_region_snapshots
```

快照里不含主机名、仓库路径、时间与随机身份：Space 的 `origin`、`label`、
`folder` 在测试里被钉住，`tab_bar_right` 保持默认空列表；同一份代码连跑两次
生成的 16 个文件逐字节相同。

## 2. 单帧耗时

`src/ui/m9.rs` 的 `m9_region_timings` 把一个区域单独画进 `TestBackend`：每个样本
连画 20 次取平均（摊掉 per-draw 固定开销），每轮 40 个样本（800 次渲染），跑三轮，
取 p50 最小的那一轮。合计 2400 次渲染，远高于 brief 要求的 500。百分位是
nearest-rank，计时用 `std::time::Instant`，未引入 criterion。

```console
$ HC_TUI_GRAPHICS=kitty cargo test --release -p hc-tui --lib m9_region_timings -- --ignored --nocapture
region	size	state	n	p50_ms	p90_ms	max_ms
sidebar	80x24	data	2400	0.111	0.130	0.158
sidebar	80x24	empty	2400	0.057	0.066	0.075
sidebar	120x40	data	2400	0.169	0.196	0.323
sidebar	120x40	empty	2400	0.112	0.121	0.141
tabs	80x24	data	2400	0.037	0.041	0.045
tabs	80x24	empty	2400	0.036	0.040	0.047
tabs	120x40	data	2400	0.064	0.087	0.094
tabs	120x40	empty	2400	0.059	0.086	0.090
status	80x24	data	2400	0.024	0.032	0.039
status	80x24	empty	2400	0.034	0.039	0.085
status	120x40	data	2400	0.059	0.082	0.090
status	120x40	empty	2400	0.074	0.090	0.158
picker	80x24	data	2400	0.047	0.059	0.167
picker	80x24	empty	2400	0.044	0.046	0.125
picker	120x40	data	2400	0.091	0.109	0.129
picker	120x40	empty	2400	0.076	0.105	0.122
dialogs	80x24	data	2400	0.037	0.058	0.067
dialogs	80x24	empty	2400	0.050	0.060	0.079
dialogs	120x40	data	2400	0.081	0.109	0.113
dialogs	120x40	empty	2400	0.090	0.112	0.119
settings	80x24	data	2400	0.128	0.147	0.172
settings	80x24	empty	2400	0.121	0.133	0.154
settings	120x40	data	2400	0.129	0.159	0.191
settings	120x40	empty	2400	0.135	0.171	0.182
help	80x24	data	2400	0.135	0.177	0.195
help	80x24	empty	2400	0.078	0.109	0.189
help	120x40	data	2400	0.354	0.455	0.556
help	120x40	empty	2400	0.156	0.203	0.222
toast	80x24	data	2400	0.036	0.043	0.052
toast	80x24	empty	2400	0.023	0.036	0.038
toast	120x40	data	2400	0.090	0.093	0.097
toast	120x40	empty	2400	0.075	0.084	0.088
worst_p90_ms=0.455 region=help size=120x40 state=data p50_ms=0.354 max_ms=0.556
```

表是一次实测的样例（当次负载约 20，`help` 仍是唯一的重格）：

| 项 | 该次实测 | 8 ms 单表面预算余量 |
|---|---|---|
| 全部区域最大 p90（help，120×40，有数据） | 0.455 ms | 7.55 ms |
| 同一格 p50 | 0.354 ms | 7.65 ms |
| 同一格 `max` | 0.556 ms | 7.44 ms |

`worst_p90_ms` 是全部 32 个格子里的最大 p90，也是唯一需要盯的数。它随整机负载整体
漂移：本机负载 20–24（其它会话在编译）时，连跑九次得到 0.423 / 0.455 / 0.482 /
0.485 / 0.523 / 0.540 / 0.577 / 0.683 / 0.800 ms。可复现的读法是同一命令跑几次取
最快的一次；保守值取最坏 0.800 ms，余量仍有 7.2 ms。

单个格子的绝对值同样随负载缩放（负载高时可慢 2–4 倍），`<` 0.1 ms 的格子复跑
抖动约 ±0.05 ms，结论不依赖这些格子；`max` 列最不稳定，只作参考。

## 3. 空闲 CPU

`scripts/m9-idle-cpu.sh` 在隔离的 XDG 根与 `HC_SESSION=hcp-m9a` 下用 tmux 起
`target/release/hc-tui`，等首帧画出、静置 2 秒，再取 `/proc/<pid>/stat` 的
`utime+stime` 差值算百分比。两态：`--no-mods`（内置外框、不加载模块）与带模块
（链接 `examples/mods/spaces`，模块区域停靠在左侧 24 列）。

```console
$ bash scripts/m9-idle-cpu.sh 5
state=no-mods  window_s=5 pid=942100 idle_cpu_pct=1.800
state=with-mods window_s=5 pid=954022 idle_cpu_pct=2.000
state=no-mods  window_s=5 pid=965904 idle_cpu_pct=1.800
state=with-mods window_s=5 pid=977862 idle_cpu_pct=1.800
state=no-mods  window_s=5 pid=989063 idle_cpu_pct=2.000
state=with-mods window_s=5 pid=1000973 idle_cpu_pct=1.800
```

5 秒窗口的量化步长是 `100/CLK_TCK/5 = 0.2` 个百分点，两态都落在 1.8–2.0%。
20 秒窗口复核，量化误差降到 0.05 个百分点：

```console
$ bash scripts/m9-idle-cpu.sh 20
state=no-mods  window_s=20 pid=1031583 idle_cpu_pct=1.850
state=with-mods window_s=20 pid=1063834 idle_cpu_pct=1.800
```

结论：空闲 CPU ≈ 1.8%，带模块与 `--no-mods` 的差 ≤ 0.05 个百分点。

脚本收尾按 `XDG_RUNTIME_DIR=<隔离根>` 精确回收自己起的进程（`hc-tui`、`hc connect`、
`herdr-core`）再 `HC_SESSION=hcp-m9a hc server stop`；`hc-tui` 收到 SIGTERM 会走
detach 留着，所以这里用 SIGKILL。收尾前后 `pgrep -c -f 'herdr-core serve'` 相同。

## 4. builtin↔mod 对拍脚手架

`scripts/mods-parity.sh` 用 `src/ui/m9.rs` 的 `m9_parity_dump` 把同一份状态在同一个
矩形上的两侧字符网格转储到 `target/m9/parity/`，再逐格比较。任意一个区域都能跑：

```console
$ ./scripts/mods-parity.sh            # 默认 status：builtin 模式行 vs fixture 模块
left=builtin-status right=mod-status size=54x1 total=54 diff=0
PARITY OK

$ ./scripts/mods-parity.sh --negative  # 负对照：builtin 侧栏 vs builtin Tab 栏
left=builtin-sidebar right=builtin-tabs size=54x24 total=1296 diff=221
NEGATIVE OK diff=221

$ ./scripts/mods-parity.sh sidebar     # 仓库里的示例模块 examples/mods/spaces
left=builtin-sidebar right=mod-sidebar size=26x24 total=624 diff=247
PARITY GAP diff=247 cells of 624
```

- 差异为 0 打 `PARITY OK` 并退出 0；非 0 打 `PARITY GAP`、退出 1，并打印
  `#` 差异图。
- 负对照必须非 0（`builtin` 侧栏对 `builtin` Tab 栏），证明脚本不是恒真；为 0 时
  打 `NEGATIVE FAILED` 并退出 1。
- `status` 一格的 mod 侧是 fixture 模块（`src/ui/m9.rs` 的 `FIXTURE_TSX`），它按
  启动参数里的左标签、正文、右标签、宽度自己排版；`sidebar` 一格的 mod 侧直接编译
  `examples/mods/spaces/App.tsx`。其余区域用空内容的 fixture，差异即是迁移缺口。
- 只比字符格，不比 fg/bg/修饰：nsh 的 TUI 画布把应用颜色按终端 16 色主题重映射
  （`nsh-platform-tui` 的 `palette.rs`），颜色本来就不该与 builtin 的 Rgb token
  逐格相等；对拍要固定的是布局与文本。
- `sidebar` 的 247/624 是当前示例模块与内置侧栏的真实差距，是 M9 迁移前的基线，
  不是脚手架故障。

## 5. 结论

- 单表面 8 ms 预算在当前代码上余量 ≥ 7.2 ms：最重的 help 浮层在 120×40 上有数据
  时 p90 实测 0.42–0.80 ms（九次复跑，随整机负载漂移），最坏 0.80 ms。
- 空闲 CPU 与是否加载模块无关，都在 1.8% 左右；模块运行时的常驻代价不在 CPU 上。
- 十六份帧快照是可提交的回归基线，`M9_UPDATE_SNAPSHOTS=1` 是唯一的更新入口。
- 对拍脚手架已能在存量状态下给任意区域出「差异格数 / 总面积 + 差异图」，并在
  builtin 与 mod 输出一致时给出 `PARITY OK`。
