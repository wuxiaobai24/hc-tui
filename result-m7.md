# M7 插件终端与多机 — 交付记录

日期：2026-11-07。范围：`[[terminals]]` 执行、包内相对程序规则、`requires_programs` 目标机器检查、
`plugins pane open|status|focus|close`、错误与提示、`git-tools` 与 `review` 示例插件。
结论：M7 列出、本次实现的全部退出条件都有真实运行证据（远端 Space 的 lazygit、detach、关闭 Pane 不结束程序、
卸载保留代际、`unsupported` 不回落本机、缺失程序 / 离线 / `target_gone`）；未完成项与限制见 §5。

## 1. 做了什么

1. **`[[terminals]]` 入口**
   - `src/plugin/terminal.rs`：plan 把清单与请求（`cwd`、`machine`、`placement`）合成 `terminal.run` 参数；
     `tab` / `right` / `down` / `float` / `popup` / `zoomed` 六种放置映射到现有 Tab、分屏、浮动与覆盖层；
     覆盖 placement 只在合法取值内接受，分屏没有空间报 `unavailable`。
   - `Launch` 增加 `env`，经 `Command::Spawn` 与 `PaneIntent` 贯通到 `terminal.run` 的 `env`
     （core 在自身环境之上叠加）；`src/app/plugin.rs` 注入动作同一套 `HCTUI_*` 变量并另加
     `HCTUI_CONTEXT`（≤32 KiB，超出后写 `truncated` 标记）与 `HCTUI_PLACEMENT`。
   - attach 成功后调用 `terminal.label` 写 `hctui`、`hctui.plugin`、`hctui.terminal`、`where`，Space/Tab
     有共享结构来源时同时写 `herdr.space`、`herdr.tab`；本地摘要同步更新，控制通道快照带
     `plugin`/`plugin_terminal`/`cwd`。
2. **包内相对程序与目标机器程序检查**
   - `argv[0]` 以 `./` 开头时相对代际目录解析；请求带机器时直接返回 `unsupported` 并说明，不换本机执行，
     也不创建 Pane；解析到的包内文件不存在报 `missing_program`。
   - `requires_programs` 与 PATH 形式的 `argv[0]` 都在目标机器检查：本机扫 PATH（可执行位），远端用
     `machine.exec` 跑 `sh -c 'for p in ...; do command -v "$p" ...'` 探测；缺失报 `missing_program` 并列出缺失项。
     清单校验新增 `requires_programs` 名字限 `[A-Za-z0-9._+-]`，探测脚本按词引用。
3. **`plugins pane open|status|focus|close`**
   - App 控制通道新增 `plugins.pane.open|status|focus|close`；`open` 返回请求 ID（`tp1`），远端探测期间状态
     `probing`，拿到地址后 `opened`，失败带 `error_kind`（`unsupported`/`missing_program`/`offline`/`target_gone`/`unavailable`）。
   - `focus`/`close` 先按 Pane 编号与内存登记匹配，再按 core 里的 `hctui.plugin` + `hctui.terminal` 标签定位，
     所以视图关闭后仍能找回终端；`close` 只断视图，程序继续运行。
   - CLI `hc-tui plugins pane open|status|focus|close`：`open` 轮询状态直到拿到终端地址或明确错误
     （20 秒上限），`--cwd`/`--machine`/`--placement`/`--space` 可覆盖缺省。
4. **错误与提示**
   - `machine_unknown` / `offline` / `io` / `closed` / `timeout` 统一归入 `offline`；目标 Space 或终端不存在报
     `target_gone`；未知入口名报 `unavailable`。所有错误带稳定 `error_kind` 与说明，不静默回退本机。
   - attach 尚未拿到地址就断开时，请求状态置 `failed`（不再停在 `opening`）。
5. **代际引用与卸载**
   - `src/plugin/lease.rs`：终端打开时为 `<state>/plugins/leases/<package>.<pid>.<generation>.json` 增加引用，
     最后一条终端关闭后删文件并回收代际；已退出进程的引用文件在扫描时清理。
   - `registry::uninstall` 只删记录与 `current` 指针；有存活引用时保留代际目录，无引用才删存储。
     `Store::next_generation` 现在取「记录与磁盘代际的较大者 +1」，卸载后重装不会覆盖运行中的旧代际。
6. **示例插件**
   - `examples/plugins/git-tools`：`summary` 动作在 Space 的机器上读取 Git 摘要；`git-ui` 终端在远端开 `lazygit`
     （`requires_programs = ["lazygit"]`，`placement = "right"`）。
   - `examples/plugins/review`：`review` 终端用包内相对程序 `./bin/review.sh` 长运行，打印 cwd 与注入变量；
     `note` 动作打印上下文。
7. **一处多机缺陷修复**：`space.new` 带着 `machine` 换机器时，新建 Space 的来源 ID 仍按上一台机器的
   `assign_origins` 生成（`peer/sp…`），随后共享布局合并会替换掉该 Space 里的 Pane（插件终端打开后
   悬在 `opening`）。现在换机器会清掉刚建 Space 的 `origin` 与 Tab/Pane 来源，由本地重新生成；
   `tests` 中补了 `a_local_space_created_from_a_remote_space_gets_local_origins`。

## 2. 改动文件与行数

修改（`git diff --numstat`，+ 为新增行）：

| 文件 | +/- |
|---|---|
| `src/app/plugin.rs` | +751 / -13 |
| `src/main.rs` | +186 / -0 |
| `src/app.rs` | +104 / -5 |
| `src/store.rs` | +44 / -9 |
| `docs/plugins.md` | +27 / -17 |
| `src/plugin/registry.rs` | +26 / -5 |
| `src/app/actions.rs` | +21 / -6 |
| `src/plugin/manifest.rs` | +18 / -0 |
| `src/hc/net.rs` | +17 / -0 |
| `src/hc/proto.rs` | +14 / -0 |
| `src/i18n/app.rs` | +14 / -0 |
| `docs/commands.md` | +12 / -0 |
| `src/control.rs` | +11 / -5 |
| `src/plugin/mod.rs` | +6 / -3 |
| `justfile` | +5 / -1 |
| `docs/development.md` | +3 / -2 |
| `docs/plan-extensions.md` | +3 / -3 |
| `src/model.rs` | +4 / -0 |
| `tests/multi_pane.rs` | +2 / -0 |
| `CHANGELOG.md` | +1 / -0 |
| `README.md` | +1 / -1 |

新增：

| 文件 | 行数 |
|---|---|
| `src/plugin/terminal.rs` | 380 |
| `src/plugin/lease.rs` | 291 |
| `tests/plugin_terminals.rs` | 361 |
| `scripts/plugin-terminals-smoke.sh` | 214 |
| `examples/plugins/git-tools/plugin.toml` | 27 |
| `examples/plugins/git-tools/bin/summary.sh` | 22 |
| `examples/plugins/git-tools/README.md` | 14 |
| `examples/plugins/review/plugin.toml` | 21 |
| `examples/plugins/review/bin/review.sh` | 15 |
| `examples/plugins/review/bin/note.sh` | 5 |
| `examples/plugins/review/README.md` | 13 |

## 3. 验证命令与原始输出摘录

### 3.1 格式与静态检查

```
$ cargo fmt -p hc-tui --check
fmt exit=0

$ cargo clippy -p hc-tui --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.27s
clippy exit=0
```

### 3.2 `cargo test -p hc-tui`

```
$ cargo test -p hc-tui
test result: ok. 274 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.51s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test exit=0
```

M7 新增单元用例（`cargo test -p hc-tui --lib plugin::` 与 `app::plugin`）：

```
test plugin::terminal::tests::remote_requests_never_run_package_relative_programs ... ok
test plugin::terminal::tests::missing_package_file_and_path_programs_are_reported ... ok
test plugin::terminal::tests::argv_programs_are_checked_and_unknown_placement_is_rejected ... ok
test plugin::terminal::tests::probe_scripts_quote_programs_and_parse_only_reported_lines ... ok
test plugin::terminal::tests::rpc_failures_map_to_stable_kinds ... ok
test plugin::terminal::tests::terminal_env_carries_context_and_placement ... ok
test plugin::lease::tests::live_leases_keep_generations_until_the_last_terminal_closes ... ok
test plugin::lease::tests::two_generations_are_kept_side_by_side ... ok
test plugin::lease::tests::dead_process_leases_are_cleaned_up ... ok
test app::plugin::tests::placements_create_panes_with_injected_environment ... ok
test app::plugin::tests::closing_a_pane_releases_the_generation_lease ... ok
test app::plugin::tests::unknown_names_and_targets_report_stable_kinds ... ok
test app::plugin::tests::a_stale_space_is_reported_as_target_gone ... ok
test app::space_origin_tests::a_local_space_created_from_a_remote_space_gets_local_origins ... ok
test store::tests::retained_generations_are_not_reused_after_uninstall ... ok
```

### 3.3 `scripts/ui-smoke.sh`

```
$ HC_TUI_BIN=target/debug/hc-tui bash scripts/ui-smoke.sh
ok   临时 Pane 自动消失
ok   shell 自定义命令
ui-smoke: 全部通过
smoke exit=0
```

### 3.4 `HC_TUI_E2E=1` 的 `g1_attach` / `multi_pane`

```
$ HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane -- --nocapture
nvim real core: ASCII typing, multilingual paste, resize and save passed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.31s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
e2e exit=0
```

### 3.5 `HC_TUI_E2E=1` 的 `plugin_terminals`（真实 core + ssh localhost 的 peer）

```
$ HC_TUI_E2E=1 cargo test -p hc-tui --test plugin_terminals -- --nocapture
remote terminal peer/t1 runs env and cwd as planned
label reply address "peer/t1"
probe on peer reports ["definitely-missing-hctui-prog"]
terminal peer/t1 is still live after the view closed
test remote_terminal_keeps_argv_cwd_env_and_survives_view_close ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.68s
```

### 3.6 `scripts/plugin-terminals-smoke.sh`（真实 TUI + 控制通道 + 真实 core）

隔离 XDG 与 session；peer 是 localhost 上 `--session hctui-m7-peer-<pid>` 的独立 core，结束时停掉。
38 条检查全部通过；最终代码连续运行 12 次全部通过（`ok=12 fail=0`）：

```
$ bash scripts/plugin-terminals-smoke.sh
== install the three example plugins
ok   terminal entries listed
== machine peer over ssh to localhost
ok   peer reachable
== run the TUI
== remote space and lazygit terminal
ok   opened on peer
ok   cwd from the space
ok   keep stays on
ok   plugin label
ok   terminal label
ok   where label
ok   injected plugin id
ok   injected placement
ok   injected context
ok   injected socket
ok   lazygit draws its panels
ok   status reports the opened terminal
== uninstall keeps the generation while the terminal runs
ok   generation installed
ok   generation kept for the running terminal
ok   installed record is gone
== closing the pane keeps the program running
ok   generation reclaimed after the last pane closed
ok   request reports the closed pane
ok   program survives the closed pane
ok   focus finds the terminal by plugin label
ok   close finds the recreated view by label
== package-relative program runs on the client host
ok   review opened locally
ok   relative program started
ok   cwd from the space
ok   plugin id in env
ok   placement in env
== package-relative program is unsupported on a remote machine
ok   kind is unsupported
ok   explains the machine
ok   no fallback to this machine
== missing program on the target machine
ok   kind is missing_program
ok   lists the missing program
ok   checks the target machine
== stale targets
ok   unknown space
ok   unknown terminal
== offline machine
ok   kind is offline
== detach keeps both terminals running
ok   git terminal survives detach
ok   review terminal survives detach
plugin terminals smoke: 全部通过
```

运行时观察到的实际 JSON（脚本输出）：

```
{"request":"tp1","status":"opened","pane":4,"terminal":"peer/t3f",...}
{"status":null,"error_kind":"unsupported","error":"`./bin/review.sh` runs only on the client that installed the plugin; machine `peer` cannot run package files"}
{"status":"failed","error_kind":"missing_program","error":"missing program on peer: definitely-missing-hctui-prog"}
{"status":"failed","error_kind":"offline","error":"machine.exec: machine_unknown: unknown machine \"peer\" (no machines yet: hc machine add <ssh-target>)"}
```

### 3.7 环境与残留

peer 的 core session 在脚本结束时被停掉，`~/.local/state/herdr-core/` 只剩原有的
`default`、`hctui-keys-439905`、`hcwprobe`、`herdterm`；默认会话里没有本次测试创建的终端。

## 4. 退出条件逐条对照

| 退出条件 | 证据 |
|---|---|
| 远端 Space 打开 lazygit，cwd 与环境变量正确 | 3.6：`opened on peer`（`terminal: peer/t3f`）、`cwd from the space`、`injected plugin id/placement/context/socket`（读 `/proc/<pid>/environ`）、`lazygit draws its panels`；3.5：远端终端 `FOO=bar` 与 `/tmp` 出现在屏幕上 |
| detach 后终端继续运行 | 3.6：`git terminal survives detach`、`review terminal survives detach`（kill tmux 后 `hc get` 仍 `state: live`） |
| 关闭 Pane 不结束程序 | 3.6：`program survives the closed pane` + 3.5 `terminal peer/t1 is still live after the view closed`；同时 `status == closed` |
| 卸载保留代际直到终端结束 | 3.6：`generation kept for the running terminal` → 关闭 Pane 后 `generation reclaimed after the last pane closed`；3.2 `live_leases_keep_generations_until_the_last_terminal_closes`、`two_generations_are_kept_side_by_side`、`retained_generations_are_not_reused_after_uninstall` |
| 包内相对程序在远端请求时返回 `unsupported` 并说明、不回落本机 | 3.6：`kind is unsupported`、`explains the machine`、`no fallback to this machine`（终端计数不变）；3.2 `remote_requests_never_run_package_relative_programs` |
| 缺失程序 | 3.6：`missing_program` 且列出 `definitely-missing-hctui-prog`、`checks the target machine`（`on peer`）；3.5 探测只报告缺失项；本机路径由 `missing_package_file_and_path_programs_are_reported` 覆盖 |
| 离线 | 3.6：机器移除后 `error_kind: offline`；`rpc_failures_map_to_stable_kinds` 覆盖 `io`/`closed` |
| `target_gone` | 3.6：过期 `sp999` 与 `peer/t999`；3.2 `a_stale_space_is_reported_as_target_gone`、`unknown_names_and_targets_report_stable_kinds` |

## 5. 未完成项与原因

1. **跨真实主机的验证**：多机用例的 peer 是 `ssh localhost` 的独立 core session（同一台物理机、独立会话与
   独立 core），传输、远端 core、远端 PTY 都走真实路径；但没有第二台物理机器，跨主机网络抖动、不同 shell
   与不同 PATH 未在本轮实测。
2. **`requires_programs` 探测依赖 POSIX shell**：远端探测用 `machine.exec` 执行 `sh -c` 与 `command -v`；
   目标机器没有 `sh` 或程序不在标准 PATH 时探测结论可能偏保守（报缺失），Windows 远端未支持。
3. **`opening` 状态的界面内超时**：attach 若一直不完成（对应 Tab 不可见、core 拒绝 attach 之外的异常），
   请求会停在 `opening`，由 CLI 的 20 秒轮询给出明确超时；App 侧没有自动失败。attach 直接报错时已经置
   `failed`。
4. **Tab 不可见时的 attach 时机**：reflow 只计算当前 Tab，若打开后立刻切到别的 Tab，终端要等回到该 Tab
   才建立 attach；程序本身已在 core 创建（`terminal.run` 已发出）。
5. **本机判定口径**：`./` 相对程序的“本机”是「请求没有目标机器」；hc-tui 的 core 固定跑在本机，因此与
   「安装所在的客户端主机」一致。若将来支持连接远端 core，这条判定需要跟着改。
6. **事件流（§8）**：属于 M8，本次未实现 `hc-tui events` 与 `events.dropped`。
7. **一次未复现的 `opening` 停留**：开发过程中（`space.new` 来源修复之后、`store`/`lease` 细化之前）有
   一次运行里 `plugins pane open` 的请求停在 `opening` 直到 CLI 超时，当时没有 AttachClosed 记录，
   根因未独立确认（最可能是共享布局合并替换了新建的 Tab）。之后 24 次与最终 12 次均未复现；这里按
   已知风险记录，不宣称已定位。
