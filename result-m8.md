# M8 事件与发行收尾 — 交付记录

日期：2026-11-07。范围：事件流、共享契约回归、发行与文档收尾。
结论：事件流（长连接、过滤、有界队列、`events.dropped`、断开即停）、`hc-tui events` 与插件消费示例
已实现并有真实运行证据；共享契约回归与三种退化状态通过；Linux 两个 musl 目标构建、打包与安装脚本验证
通过；macOS 两个目标在本机无法构建（缺 SDK/target，见 §5）。`just` 未安装，按 justfile 逐条执行了等价
命令。体积与常驻内存相对 M1 有大幅回归，来源已定位到 M4 接入 nsh 运行时，属已知且可解释的回归。

## 1. 做了什么

1. **事件长连接**（`src/control.rs`）
   - 同一控制 socket：`method = "events"` 升级为长连接，先回 `{"subscribed":true,"queue":256}`，之后逐行
     输出 NDJSON；每个订阅在独立线程里推送，不阻塞一问一答的命令。
   - 过滤：`plugin`（只看带该 `plugin` 字段的事件）与 `kinds`（数组或逗号分隔）。
   - 每个订阅者一条 256 条有界队列；`try_send` 失败累加计数，推送线程在队列排空后补发
     `{"kind":"events.dropped","count":N}`，随后流继续可用。
   - 客户端断开检测：无积压时对 socket 非阻塞读，读到 0 或写失败即注销订阅并结束线程；界面退出时随
     `Server` 的停止标记结束。
   - `HCTUI_SOCKET` 覆盖：`socket_path` 优先采用该变量，插件进程无需自行推导会话路径。
2. **事件发布**（`src/app.rs`）
   - `terminal.status`（`data.status`）、`terminal.turn`（`data.turn`）、`terminal.finished`
     （`data.reason`，来自 core 的 finished/removed/lost 或 attach 流以程序退出结束）、`terminal.opened`
     与 `terminal.closed`（`data.reason = "pane_closed"`）。
   - 负载带 `machine`（地址前缀或 `local`）、`terminal`、`turn`、`at_ms` 与 `data`；终端属于插件时带
     `plugin`（按标签与插件 Pane 记录解析，本地地址与带机器前缀的地址按最后一段比对）。
3. **CLI 与示例**
   - `hc-tui events [--plugin <id>] [--kinds a,b] [--count N] [--config PATH]`：读一条确认后逐行输出，
     `--count` 读到 N 条即退出，stdout 关闭/断开即结束。
   - `examples/plugins/events`：动作 `watch`（读若干条后退出，参数 `count`/`kinds`）与终端 `feed`
     （长运行的事件查看器）；README 说明用法。
4. **共享契约回归**
   - `src/shared.rs`：新增模块节点往返测试（module 节点、未知 `look`/`meta`/`size`、props 中的未知嵌套、
     Space/Tab 根上的未知字段与顶层未知字段全部原样保留）；新增“不同模块的 props 可合并、同一 props
     字段冲突时报错并给出 `props.root` 路径”。
   - `src/app/shared_sync.rs`：冲突时断言用户提示包含 `Shared layout:` 与 `layout.reload`，并验证
     `layout.reload` 解除阻塞并重新取共享版本。
5. **退化状态**
   - `scripts/degraded-smoke.sh`：`--no-mods`、安装并禁用全部模块、安装并禁用全部插件三种状态，各自
     验证侧栏渲染、终端回显输入、分屏；模块与插件确实安装过并显示 `enabled: false`。
6. **发行测据**
   - `scripts/perf-baseline.sh`：隔离 XDG/session 下测体积、VmRSS、10 秒空闲 CPU 与按键到屏幕可见的
     p50/p90 延迟（pane 输出经 `pipe-pane` 接入文件，去 ANSI 后匹配，避免 tmux 进程启动开销）。
   - 构建：musl amd64/arm64 成功；macOS 两目标失败，原因与原始报错见 §3.6。`scripts/package-release.sh`
     打包、解包运行、`install.sh` 语法与 `scripts/test-install.py` 通过。
7. **文档**：`docs/plan-extensions.md`（状态与 M8 行）、`docs/plugins.md`（§7 CLI、§8 事件语义）、
   `docs/commands.md`、`docs/development.md`、`docs/release.md`（M8 实测表与处置）、`justfile`、README、
   CHANGELOG。没有新增调试开关或临时探针；`scripts/*` 新增项均为验收脚本。

## 2. 改动文件与行数

修改（`git diff --numstat`）：

| 文件 | +/- |
|---|---|
| `src/control.rs` | +323 / -24 |
| `src/main.rs` | +113 / -0 |
| `src/app.rs` | +88 / -1 |
| `src/shared.rs` | +58 / -0 |
| `docs/release.md` | +46 / -0 |
| `justfile` | +12 / -0 |
| `src/app/plugin.rs` | +11 / -0 |
| `docs/commands.md` | +10 / -0 |
| `docs/plugins.md` | +8 / -5 |
| `src/app/shared_sync.rs` | +7 / -0 |
| `docs/development.md` | +3 / -0 |
| `docs/plan-extensions.md` | +3 / -3 |
| `CHANGELOG.md` | +1 / -1 |
| `README.md` | +1 / -1 |

新增：

| 文件 | 行数 |
|---|---|
| `scripts/events-smoke.sh` | 156 |
| `tests/events_stream.rs` | 150 |
| `scripts/perf-baseline.sh` | 105 |
| `scripts/degraded-smoke.sh` | 84 |
| `examples/plugins/events/plugin.toml` | 26 |
| `examples/plugins/events/bin/watch.sh` | 15 |
| `examples/plugins/events/bin/feed.sh` | 5 |
| `examples/plugins/events/README.md` | 16 |

工作区里另有一个未跟踪的 `review-plugins.md`（不是本次任务创建，未修改、未纳入提交）。

## 3. 验证命令与原始输出摘录

### 3.1 格式与静态检查

```
$ cargo fmt -p hc-tui --check
fmt exit=0

$ cargo clippy -p hc-tui --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.88s
clippy exit=0
```

### 3.2 `cargo test -p hc-tui`

```
$ cargo test -p hc-tui
running 278 tests
test result: ok. 278 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.51s
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s   # events_stream
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # g1_attach (skips without HC_TUI_E2E)
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s   # mods_probe
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # multi_pane (skips)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s   # net_recovery
test exit=0
```

M8 新增用例：

```
slice::events_stream::two_clients_each_get_one_copy_and_disconnect_stops_the_subscription ... ok
slice::events_stream::overflow_drops_and_reports_events_dropped ... ok
control::tests::filters_by_kind_and_plugin_and_counts_overflow ... ok
control::tests::event_filter_accepts_arrays_and_comma_lists ... ok
shared::tests::module_nodes_keep_unknown_fields_props_and_look ... ok
shared::tests::props_merge_per_module_and_conflicts_name_the_path ... ok
app::shared_sync::tests::conflicting_edits_block_automatic_overwrite_until_reload ... ok（新增冲突提示与 reload 断言）
```

### 3.3 `just smoke` / `just parity`（`just` 未安装，执行等价命令）

本机 `PATH` 与 mise 均无 `just`（`which just` 为空），因此按 `justfile` 的定义逐条执行：

- `just smoke` → `cargo build -p hc-tui` + `HC_TUI_BIN=target/debug/hc-tui scripts/ui-smoke.sh`
- `just parity` → `python3 scripts/herdr-parity-smoke.py`

```
$ HC_TUI_BIN=target/debug/hc-tui bash scripts/ui-smoke.sh
ok   临时 Pane 自动消失
ok   shell 自定义命令
ui-smoke: 全部通过
ui-smoke exit=0

$ python3 scripts/herdr-parity-smoke.py
...（21 条 ok）
ok   quit leaves other programs alive
herdr parity smoke: passed; artifacts: /tmp/hctui-parity-ew996plm
parity exit=0
```

### 3.4 `HC_TUI_E2E=1` 的 `g1_attach` / `multi_pane`

```
$ HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane -- --nocapture
nvim real core: ASCII typing, multilingual paste, resize and save passed
test result: ok. 1 passed; ... finished in 1.34s
test result: ok. 1 passed; ... finished in 0.09s
e2e exit=0
```

### 3.5 事件流冒烟（`scripts/events-smoke.sh`，真实 TUI + 真实 core）

16 条检查全部通过：

```
== two clients each receive one copy
ok   first client got the status
ok   second client got the status
ok   same event payload
ok   status payload
ok   one delivery only
== kind filter excludes other events
ok   status event not delivered to the finished-only client
ok   finished event delivered
ok   finished payload
== plugin filter
ok   plugin event delivered
ok   plugin event carries the package
== disconnect stops the subscription and the socket stays usable
ok   stream usable after a disconnect
== queue overflow emits events.dropped
ok   events.dropped sent
ok   dropped count
     delivered=357 dropped={"count":8466,"kind":"events.dropped"}
== plugin action consumes the stream
ok   watch action finished
ok   action printed an event
ok   log carries the event line
events smoke: 全部通过
```

### 3.6 构建、打包与安装脚本

```
$ cargo zigbuild --locked --release --target x86_64-unknown-linux-musl -p hc-tui
Finished `release` profile ... in 1m 58s
46,487,512 B  statically linked

$ cargo zigbuild --locked --release --target aarch64-unknown-linux-musl -p hc-tui
Finished `release` profile ... in 23.40s
41,800,296 B  statically linked
$ qemu-aarch64 target/aarch64-unknown-linux-musl/release/hc-tui --version
hc-tui 0.4.3

$ cargo build --locked --release --target x86_64-apple-darwin -p hc-tui
error: ... consider downloading the target with `rustup target add x86_64-apple-darwin`
darwin-amd64 exit=101

$ cargo build --locked --release --target aarch64-apple-darwin -p hc-tui
cc: error: unrecognized command-line option '-arch'
cc: error: unrecognized command-line option '-mmacosx-version-min=11.0'
darwin-arm64 exit=101

$ cargo zigbuild --locked --release --target aarch64-apple-darwin -p hc-tui
error: unable to find framework 'CoreFoundation'
note: pass the path of an Xcode installation via the DEVELOPER_DIR environment variable, or an SDK with the SDKROOT environment variable
zigbuild-darwin-arm64 exit=101

$ bash scripts/package-release.sh x86_64-unknown-linux-musl
dist/hc-tui-0.4.3-x86_64-unknown-linux-musl.tar.gz        19,388,210 B
dist/hc-tui-0.4.3-x86_64-unknown-linux-musl.tar.gz.sha256
$ tar -xzf ... && /tmp/m8pkg/hc-tui --version
hc-tui 0.4.3
$ /tmp/m8pkg/hc-tui --default-config > /dev/null   # ok

$ sh -n install.sh
sh -n exit=0
$ python3 scripts/test-install.py
Ran 6 tests in 1.205s
OK
```

`cargo-zigbuild 0.24.1` 与 `cargo-bloat 0.12.1` 安装在 `/tmp/m8-tools`，未改用户环境。

### 3.7 性能与体积（M1 → M8，`scripts/perf-baseline.sh`）

```
label=m1 size_bytes=7231376 rss_kb=8052  idle_cpu_pct=1.700 input_us_p50=39458 input_us_p90=39895 samples=20
label=m8 size_bytes=66427456 rss_kb=14312 idle_cpu_pct=1.700 input_us_p50=38854 input_us_p90=39222 samples=20
label=m1 size_bytes=7231376 rss_kb=8152  idle_cpu_pct=1.500 input_us_p50=38424 input_us_p90=39041 samples=20
label=m8 size_bytes=66427456 rss_kb=14448 idle_cpu_pct=1.700 input_us_p50=38288 input_us_p90=38818 samples=20
label=m1 size_bytes=7231376 rss_kb=8088  idle_cpu_pct=1.500 input_us_p50=38110 input_us_p90=38949 samples=20
label=m8 size_bytes=66427456 rss_kb=14476 idle_cpu_pct=1.800 input_us_p50=38421 input_us_p90=38923 samples=20
```

M1 的 musl 发行体积在本机重建为 5,647,592 B（与 docs/release.md 的 5.65 MiB 基线一致）；M8 为
46,487,512 B。体积分解与处置写在 docs/release.md（`nsh_base` 10.1 MiB、`nsh_core` 3.2 MiB、QuickJS
stdlib 2.5 MiB 等；`js-core` 替代因特性合并无效；thin LTO 可降到 42.4 MiB 但构建 2→5 分钟，未采纳）。

### 3.8 退化状态（`scripts/degraded-smoke.sh`）

```
== --no-mods
ok   sidebar renders / terminal echoes typing / split still works
== all modules disabled
ok   no module is enabled
ok   sidebar renders / terminal echoes typing / split still works
== all plugins disabled
ok   plugin catalog is empty
ok   sidebar renders / terminal echoes typing / split still works
degraded smoke: 全部通过
```

模块与插件确实安装后再禁用：`mods list --json` 显示 `agents/prs/spaces` 均 `enabled:false`；
`plugins list --json` 显示 `dev.hctui.plugins.events/hello` 均 `enabled:false`。

### 3.9 插件与终端回归（M6/M7 规格中本次可重跑的项）

```
$ bash scripts/plugins-smoke.sh      # 11 条 ok，plugins-smoke exit=0
$ bash scripts/plugin-terminals-smoke.sh
plugin terminals smoke: 全部通过      # 38 条 ok，terminals exit=0
```

## 4. 退出条件逐条对照

| 条件 | 状态 | 证据 |
|---|---|---|
| 事件流客户端断开即停止推送 | ✅ | `tests/events_stream.rs::two_clients_each_get_one_copy_and_disconnect_stops_the_subscription`（断开后 `subscribers()==1`）；`scripts/events-smoke.sh` 的 `stream usable after a disconnect` |
| 队列溢出发出 `events.dropped` | ✅ | `overflow_drops_and_reports_events_dropped`（50,000 条，收到 `events.dropped` 且计数>0）；冒烟中 `delivered=357 dropped={"count":8466,...}` |
| 两个客户端各收到一次 | ✅ | 冒烟 `first client got the status` + `second client got the status` + `one delivery only`；Rust 测试同样断言 |
| `--no-mods` 界面与终端完整 | ✅ | §3.8 |
| 禁用全部模块 | ✅ | §3.8（3 个示例模块安装后禁用，仍可输入与分屏） |
| 禁用全部插件 | ✅ | §3.8（2 个插件安装后禁用，目录为空，仍可输入与分屏） |
| `just smoke` / `just parity` | ⚠️ 等价命令通过 | `just` 未安装，按 justfile 执行 `scripts/ui-smoke.sh` 与 `scripts/herdr-parity-smoke.py`，均 exit=0（§3.3） |
| 4 目标构建 | ❌ 2/4 | Linux musl amd64/arm64 构建成功并打包；macOS 两目标缺少 target 与 Apple SDK（§3.6）。未跳报 |
| 输入延迟与 M1 对比 | ✅ | p50 38.1–39.5 ms（M1） vs 38.3–38.9 ms（M8），p90 38.8–39.9 ms vs 38.8–39.2 ms，无可见退化 |
| 空闲 CPU 与 M1 对比 | ✅ | 1.5–1.7% vs 1.7–1.8%，同一 30 fps 循环，无可见差异 |
| 常驻内存与 M1 对比 | ⚠️ 回归已解释 | 8.0–8.2 MB → 14.3–14.5 MB，来源为 nsh/QuickJS 运行时接入（M4），非 M8 事件流 |
| 二进制体积与 M1 对比 | ⚠️ 回归已解释 | musl 5.65 MiB → 44.33 MiB；分解与可选处置见 `docs/release.md` |
| 共享契约：module 节点/未知字段/props/CAS/冲突提示 | ✅ | `src/shared.rs` 两个新测试 + `shared_sync` 冲突提示与 `layout.reload` 断言；`just parity` 等价命令 21 条 ok（含“保留 GUI root styling and unknown properties”“rename reaches GUI document through CAS”） |
| 两份规格的验收清单全部通过 | ❌ 部分 | plugins 规格 §14 的条目由 M6/M7/M8 记录与本次 3.9 覆盖；mods 规格 §17 中依赖 M9 的条目（8 个默认模块接管、80×24/120×40 模块帧快照一致、被替换代码≥60%、`scripts/mods-coverage.py`）未实现也未验收，见 §5 |
| fmt / clippy / test / ui-smoke / E2E | ✅ | §3.1–3.4 |

## 5. 未完成项与原因

1. **macOS 两个发行目标未构建**：本机是 Linux，`x86_64-apple-darwin` 未安装，`aarch64-apple-darwin`
   交叉链接缺 Apple SDK（`CoreFoundation` 框架）；macOS 构建必须在 macOS runner 上完成，本机无法验证。
   因此“4 目标构建通过”没有达成，不能按发布条件出包。
2. **`just` 未安装**：`which just` 与 mise 均无该命令。未擅自安装或改用户配置；按 justfile 执行了
   等价命令并通过，CI/开发机上仍需确认 `just` 本身可用。
3. **体积与常驻内存回归**：musl 发行体积 8.2×、RSS +约 6.2 MB，来源是 M4 接入的 nsh/QuickJS 运行时。
   已尝试 `js-core` 替换（被 `nsh-platform-tui` 无条件 `js` 特性合并，无效）与 thin LTO（−8.9%，
   构建 2→5 分钟，未采纳）。这是可解释但未消除的回归；削减需要上游 feature 或替换运行时。
4. **mods 规格中依赖 M9 的验收项**：8 个默认模块、模块帧快照一致、60% 行数覆盖率与 `mods-coverage.py`
   属于 M9 工作，M8 未实施，也不再宣称“两份规格全部通过”。其余 mods 规格条目见 M4/M5 交付记录，
   本次只重跑了 `mods_probe`、`mods check` 相关的既有测试与 `just parity` 等价命令。
5. **事件流作用域**：仅本 TUI 客户端（同一进程的多个订阅者各收到一次）；不跨 hc-tui 实例或机器聚合。
   `--plugin` 只匹配带 `plugin` 字段的事件（插件终端）；动作进程本身没有事件。
6. **`--plugin` 与 `kinds` 无历史回放**：订阅从连接时刻开始，不补发历史事件；消费者需要先建立订阅再
   触发动作（示例 README 已说明）。
7. **已知 M7 遗留问题（外部评审 `review-plugins.md` 指出的租约语义）**：代际引用按“客户端进程 + Pane 视图”
   释放，而程序在 core 中继续运行；关闭 Pane 或 detach 后可能过早回收仍被程序使用的代际，动作排队/运行
   期间卸载也没有代际持有。M8 未修改该契约；如实记录为待修问题，不把 M7 的“关闭 Pane 后回收”当作
   “直到终端结束”的正确证明。
