# M7.5 交付：修复评审发现的三个 P1 契约缺陷

日期：2026-11-07。来源：`review-plugins.md` 的 §1 三个 P1。

三个 commit：`fix(lease):`、`fix(runner):`、`fix(target):`。每个 P1 一个 commit，
每个 commit 只改本 P1 的代码与对应文档；交付记录 `result-m75.md` 随第三个 commit
一并提交（评审来源 `review-plugins.md` 只保留在磁盘上，不进 git）。每个 P1 的失败用例先于修复提交；
下面是修复前的原始失败输出与修复后的通过输出。

复现命令（仓库根目录）：

```sh
cargo test -p hc-tui --lib plugin::lease::tests
cargo test -p hc-tui --lib plugin::runner::tests
cargo test -p hc-tui --lib app::plugin::tests
```

---

## P1-1 代际引用保护的是视图，依赖代码的是进程

### 修复前失败输出（原始）

```
---- app::plugin::tests::a_pane_close_does_not_release_the_generation_of_a_running_terminal stdout ----

thread 'app::plugin::tests::a_pane_close_does_not_release_the_generation_of_a_running_terminal' (1053140) panicked at src/app/plugin.rs:1229:9:
generation removed although the terminal still runs

---- app::plugin::tests::a_queued_action_still_runs_after_the_plugin_is_uninstalled stdout ----

thread 'app::plugin::tests::a_queued_action_still_runs_after_the_plugin_is_uninstalled' (1053142) panicked at src/app/plugin.rs:1274:9:
assertion `left == right` failed: View { request: "p2", package: "dev.example.queue", action: "run", status: Failed, exit_code: None, error: Some("cannot start sh: No such file or directory (os error 2)"), stdout: "", stderr: "", queued_ms: 523, runtime_ms: Some(1) }
  left: Failed
 right: Done

test result: FAILED. 7 passed; 3 failed; 0 ignored; 0 measured; 271 filtered out; finished in 0.55s
```

其中 `uninstalling_during_a_run_keeps_the_action_directory_contents` 的失败同为
`cannot start sh: No such file or directory`：运行中的动作 cwd 已被 `uninstall` 删掉。

用例 d（正向回收）在本 commit 前无法编译：`lease::gc`、`RetentionEvidence`、
`ResourceOwner` 这些契约类型当时不存在，`cargo test` 的原始失败是
`error[E0433]: failed to resolve: could not find `gc` in `lease``。这一点如实记录，
没有把它写成“修复前也稳定红”。

### 修复后通过输出

```
running 5 tests
test plugin::lease::tests::two_generations_are_kept_side_by_side ... ok
test plugin::lease::tests::repin_moves_the_terminal_to_its_core_address ... ok
test plugin::lease::tests::stale_pins_of_other_processes_survive_until_evidence_arrives ... ok
test plugin::lease::tests::queued_invocations_hold_their_generation ... ok
test plugin::lease::tests::a_pane_close_is_not_evidence_and_keeps_the_generation ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 278 filtered out; finished in 0.00s

running 3 tests
test app::plugin::tests::a_pane_close_does_not_release_the_generation_of_a_running_terminal ... ok
test app::plugin::tests::a_queued_action_still_runs_after_the_plugin_is_uninstalled ... ok
test app::plugin::tests::a_stale_space_is_reported_as_target_gone ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 280 filtered out; finished in 1.07s
```

（`uninstalling_during_a_run_keeps_the_action_directory_contents` 与
`closing_a_pane_keeps_the_terminal_pin_until_core_ends_the_terminal` 在后一个
commit 之后的整组运行里同样通过，见文末“全量结果”。）

### 改了什么

- `src/plugin/lease.rs` 重写为显式租约：`ResourceOwner::{Invocation, Terminal(TerminalAddress)}`、
  `GenerationPin`、`RetentionEvidence`、`gc(package, evidence)`、`gc_all`、`repin`。
  pin 落在 `<state>/plugins/pins/<package>/<generation>/<owner>.json`；PID 存活判断被删除。
- `src/plugin/registry.rs::uninstall` 只删记录与 `current` 指针，然后做一次空证据的 `gc`。
  新增 `hc-tui plugins gc`（`--json`）作为显式回收入口。
- `src/plugin/registry.rs::ActionInfo` 与 `src/plugin/runner.rs` 的调用规格带上代际：
  动作入队即 pin，调用结束（done/failed/cancelled/timed_out）才释放；队列满被拒时也释放。
- `src/app/plugin.rs`：终端在 spawn 时按请求登记占位 pin，`terminal.label` 拿到 core 地址后
  `repin` 成 `Terminal(session, machine, address)`；`release_plugin_terminal` 只断视图、
  **不释放 pin**。
- 释放条件全部指向 core 侧事实：`finished`/`removed`/`lost` 事件、`AttachClosed { program_exit }`，
  以及启动后第一次终端快照的对账（快照里已不存在的上进程 pin 视为结束）。`core` 说结束才 `gc`。

### 验收对照

| 票要求 | 用例 | 结果 |
|---|---|---|
| a) 包内程序启动 → 卸载 → 关 Pane → 程序仍能读包内文件 | `a_pane_close_does_not_release_the_generation_of_a_running_terminal`：真实进程以代际目录为 cwd 启动，`release_plugin_terminal` + `registry::uninstall` 后断言代际目录还在，触发后 `cat ./payload.txt` 内容为 `payload-v1` | 通过（修复前：目录被删） |
| b) 动作入队后卸载，队列中的调用仍能完成 | `a_queued_action_still_runs_after_the_plugin_is_uninstalled`：全局/单包上限 1，第二条排队，卸载后排队项 `done` 且 stdout 有 `payload-v1` | 通过（修复前：`cannot start sh`） |
| c) 运行期卸载，正在跑的动作 cwd 内容不被删除 | `uninstalling_during_a_run_keeps_the_action_directory_contents`：断言 stdout 打出的 `pwd` 仍是代际目录且 `payload.txt` 可读 | 通过（修复前：`cannot start sh`） |
| d) 终端真正结束后 `gc` 才删除该代际 | `a_pane_close_is_not_evidence_and_keeps_the_generation`：空证据不删；`RetentionEvidence{ended: local/t7}` 才释放 pin 并删代际 | 通过（修复前：类型不存在，编译期红） |
| Pane 关闭不释放终端依赖 | `closing_a_pane_keeps_the_terminal_pin_until_core_ends_the_terminal` | 通过 |
| 排队中的动作也持 pin | `queued_invocations_hold_their_generation` | 通过 |
| 显式 GC、拿不到证据时保守保留 | `stale_pins_of_other_processes_survive_until_evidence_arrives`、`plugins gc`（手工验证见下） | 通过 |
| 孤儿代际（无记录无 pin）也能显式回收 | `gc_all_reclaims_generations_without_record_or_pin` | 通过 |

---

## P1-2 “受限子进程”没有覆盖完整生命周期

### 修复前失败输出（原始）

```
test plugin::runner::tests::a_large_context_does_not_deadlock_a_child_that_writes_first ... FAILED
test plugin::runner::tests::stalled_invocations_release_their_slots ... FAILED

failures:

---- plugin::runner::tests::a_normal_exit_does_not_leave_descendants_behind stdout ----

thread 'plugin::runner::tests::a_normal_exit_does_not_leave_descendants_behind' (1486719) panicked at src/plugin/runner.rs:828:9:
descendant 1486746 survived a normal exit

---- plugin::runner::tests::a_large_context_does_not_deadlock_a_child_that_writes_first stdout ----

thread 'plugin::runner::tests::a_large_context_does_not_deadlock_a_child_that_writes_first' (1486718) panicked at src/plugin/runner.rs:704:13:
request p1 stuck

---- plugin::runner::tests::stalled_invocations_release_their_slots stdout ----

thread 'plugin::runner::tests::stalled_invocations_release_their_slots' (1486724) panicked at src/plugin/runner.rs:704:13:
request p1 stuck

test result: FAILED. 5 passed; 3 failed; 0 ignored; 0 measured; 279 filtered out; finished in 8.03s

thread 'app::plugin::tests::shutdown_cancels_running_plugins_and_cleans_the_group' (1494114) panicked at src/app/plugin.rs:1454:9:
View { request: "p1", package: "dev.example.stop", action: "run", status: Running, exit_code: None, error: None, stdout: "", stderr: "", queued_ms: 0, runtime_ms: Some(3024) }

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 286 filtered out; finished in 3.03s
```

### 修复后通过输出

```
running 9 tests
test plugin::runner::tests::output_is_truncated_to_the_tail ... ok
test plugin::runner::tests::shutdown_releases_the_pins_of_queued_invocations ... ok
test plugin::runner::tests::cancel_stops_a_running_process_and_frees_the_slot ... ok
test plugin::runner::tests::a_normal_exit_does_not_leave_descendants_behind ... ok
test plugin::runner::tests::stalled_invocations_release_their_slots ... ok
test plugin::runner::tests::cancel_before_start_removes_it_from_the_queue ... ok
test plugin::runner::tests::a_large_context_does_not_deadlock_a_child_that_writes_first ... ok
test plugin::runner::tests::concurrency_queues_and_reports_busy_when_full ... ok
test plugin::runner::tests::timeout_terminates_the_process_group ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 284 filtered out; finished in 2.46s

test app::plugin::tests::shutdown_cancels_running_plugins_and_cleans_the_group ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 286 filtered out; finished in ...
```

### 改了什么

- `src/plugin/runner.rs`：
  - `Spec` → `InvocationSpec`，新增 `stdin_limit`；超过 `min(stdin_limit, MAX_STDIN=256 KiB)`
    在入口按 `invalid` 拒绝。
  - deadline 在 spawn 之前建立；stdin 写入在独立线程，stdout/stderr reader 各自独立线程，
    三者都受取消与 deadline 管理。父进程不再可能堵在 `write_all`。
  - 收尾统一走 `terminate_group`：SIGTERM → 等进程组消失（最多 2 秒）→ SIGKILL → `wait`；
    随后 `drain` 等 reader 排空管道（最多 2 秒）、join stdin writer。**全部收尾完成才记录最终状态**。
  - 新增 `Runner::shutdown(deadline) -> Result<(), ShutdownError>`：置 `shutting_down`、取消排队项
    （同时释放它们持有的代际 pin）、向运行中的调用发取消、到期后对残留进程组 SIGKILL；
    `invoke` 在关闭期间直接拒绝。
- `src/app.rs::shutdown` 改为调用 `plugin_runner.shutdown(now + 3s)`。
- `src/plugin/logs.rs` 增加包级配额：单份 1 MiB 轮转，每包 ≤ 32 个文件、总计 ≤ 8 MiB，
  超出删最旧的请求日志（`a_package_keeps_a_bounded_number_of_log_files`）。

### 验收对照

| 票要求 | 用例 | 结果 |
|---|---|---|
| a) 上下文 > 64 KiB + 子进程先大量输出再读 stdin，不得死锁，超时后结束 | `a_large_context_does_not_deadlock_a_child_that_writes_first`：96 KiB 上下文 + `head -c 200000 /dev/zero; sleep 30` + 300 ms 超时 | 通过（修复前：`request p1 stuck`，永久阻塞） |
| b) `sh -c 'sleep 9999 & exit 0'` 收尾前不得为 `done`，收尾后不留孤儿 | `a_normal_exit_does_not_leave_descendants_behind`：`Done` 之后断言后代 PID 已不存在（收尾先于状态公布） | 通过（修复前：`descendant ... survived a normal exit`） |
| c) 连续 8 个死锁型调用后运行槽仍可获取 | `stalled_invocations_release_their_slots`：8 条死锁型调用全部结束，随后一条 `true` 正常 `done` | 通过（修复前：第 1 条就 stuck） |
| d) 宿主 shutdown 期间运行的动作被终止，进程组被清理 | `shutdown_cancels_running_plugins_and_cleans_the_group`：`app.shutdown()` 后状态已结束，脚本派生的后代 PID 不再存活 | 通过（修复前：3 秒后仍 `Running`，后代存活） |
| stdin 字节上限显式 | `InvocationSpec::stdin_limit` + 入口拒绝 | 通过（编译期契约） |
| 日志包级有界 | `a_package_keeps_a_bounded_number_of_log_files`（52 次写入后 ≤ 32 个文件、≤ 8 MiB） | 通过 |
| 关闭时排队项的 pin 一并释放 | `shutdown_releases_the_pins_of_queued_invocations` | 通过 |

---

## P1-3 “目标冻结”只冻了一半

### 修复前失败输出（原始）

```
---- app::plugin::tests::a_missing_target_is_reported_as_target_gone_without_falling_back stdout ----

thread 'app::plugin::tests::a_missing_target_is_reported_as_target_gone_without_falling_back' (1774570) panicked at src/app/plugin.rs:1563:43:
unknown target must fail: ()

---- app::plugin::tests::a_probe_keeps_the_target_space_even_if_the_user_switches stdout ----

thread 'app::plugin::tests::a_probe_keeps_the_target_space_even_if_the_user_switches' (1774574) panicked at src/app/plugin.rs:1590:9:
assertion `left == right` failed: pane landed in Space 0 instead of the target
  left: 0
 right: 1

---- app::plugin::tests::an_action_targeted_at_another_space_uses_that_space stdout ----

thread 'app::plugin::tests::an_action_targeted_at_another_space_uses_that_space' (1774580) panicked at src/app/plugin.rs:1539:9:
assertion `left == right` failed: {"action":"run","api_version":1,"args":{},"context":{"pane":2,"space":{"folder":"/home/local","id":"sp1","machine":null,"name":""},"tab":{"id":"tb1"},"target":{"kind":"pane","pane":2,"space":{"folder":"/home/local","id":"sp1","machine":null,"name":""},"tab":{"id":"tb1"},"terminal":"peer/t7"},"terminal":"peer/t7"},"locale":"en","machine":{"client":"omarchy","core":"omarchy","workspace":"local"},"package":"dev.example.target","request":"p1"}
  left: String("local")
 right: "peer"

test result: FAILED. 11 passed; 3 failed; 0 ignored; 0 measured; 277 filtered out; finished in 1.03s
```

第三条的负载就是问题本身：终端是 `peer/t7`（Space B），
`machine.workspace` 却是 `local`、`space.folder` 是 `A` 的 `/home/local`。

### 修复后通过输出

```
running 15 tests
test app::plugin::tests::closing_a_pane_keeps_the_terminal_pin_until_core_ends_the_terminal ... ok
test app::plugin::tests::a_probe_keeps_the_target_space_even_if_the_user_switches ... ok
test app::plugin::tests::a_missing_terminal_target_is_reported_as_target_gone ... ok
test app::plugin::tests::a_missing_numeric_pane_is_rejected_instead_of_using_the_focus ... ok
test app::plugin::tests::placements_create_panes_with_injected_environment ... ok
test app::plugin::tests::a_pane_close_does_not_release_the_generation_of_a_running_terminal ... ok
test app::plugin::tests::an_action_targeted_at_another_space_uses_that_space ... ok
test app::plugin::tests::shutdown_cancels_running_plugins_and_cleans_the_group ... ok
test app::plugin::tests::uninstalling_during_a_run_keeps_the_action_directory_contents ... ok
test app::plugin::tests::a_queued_action_still_runs_after_the_plugin_is_uninstalled ... ok
...（其余用例略）

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 277 filtered out; finished in 1.05s
```

### 改了什么

- `src/plugin/terminal.rs`：新增 `TargetSelector`、`ExecutionTarget`（稳定 `sp`/`tb` 身份、
  下标、机器、cwd、Pane、终端地址）与 `PreparedTerminal`、`prepare()`。
  `probe_params` 的机器参数改为 `Option<&str>`，本机（`on = null`）同样交给 core 探测；
  删除 `local_missing`/`is_executable` —— TUI 环境不再替 core 做 PATH 判决。
- `src/app/plugin.rs`：
  - `resolve_plugin_target`：显式 Pane/终端/Space 一律只解析目标；数字 Pane 不存在、
    终端未知、Space 已移除都返回 `target_gone`，**不回退当前焦点**。
  - 动作上下文与环境由 `ExecutionTarget` 生成（`machine.workspace`、`space.machine`、
    `space.folder`、`tab.id` 全部来自目标）。
  - `prepare` 冻结完整目标进 `plugin_pending`；`plugin_probe_reply` 只验证 Space 仍然存在
    （不存在报 `target_gone`），随后 `open_plugin_terminal` 切到目标 Space 放置 Pane，
    不重新解析。
  - 本机终端入口也先走 `machine.exec` 探测（`on = null`），因此 `plugins pane open`
    在本机同样先返回 `probing`。

### 验收对照

| 票要求 | 用例 | 结果 |
|---|---|---|
| a) Space A 中请求 Space B 的终端，上下文 machine/cwd 全来自 B | `an_action_targeted_at_another_space_uses_that_space`：断言 `machine.workspace == peer`、`space.folder == /srv/remote`、`space.machine == peer`、`pane` 与 `terminal` 都是 B 的 | 通过（修复前：`local` / `/home/local`） |
| b) 目标不存在 → `target_gone`，不回退当前焦点 | `a_missing_terminal_target_is_reported_as_target_gone` | 通过（修复前：`invoke_plugin` 成功返回 `()`） |
| c) 探测期间切换 Space → Pane 建在 B | `a_probe_keeps_the_target_space_even_if_the_user_switches`：`probing` 后 `select_workspace(A)`，再回探测结果，Pane 落在 Space 1 | 通过（修复前：落在 Space 0） |
| d) 数字 Pane 不存在 → 拒绝并给稳定 `error_kind` | `a_missing_numeric_pane_is_rejected_instead_of_using_the_focus`：错误文本含 `target_gone` | 通过（修复前：成功返回） |
| PATH/`requires_programs` 由执行位置负责 | 所有入口（含本机）都发 `machine.exec` 探测；`terminal.rs` 删掉本机 PATH 判决 | 通过（`probe_scripts_quote_programs_and_parse_only_reported_lines` 覆盖 `on = null`；真实 core 冒烟里 `kind is missing_program` / `kind is offline` 同时通过） |
| 异步探测只验证、不重解析 | `open_plugin_terminal` 只按 `target.space` 找 Space，找不到即 `target_gone` | 通过 |

---

## 全量结果

```
$ cargo test -p hc-tui
test result: ok. 294 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
（其余集成测试二进制 0/1/2 条全绿）

$ cargo clippy -p hc-tui --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.04s
```

## 真实 core 的验证

环境具备 `hc`、`tmux`、`ssh localhost`，因此除单元/应用层用例外，还跑了真实 core：

```
$ bash scripts/plugin-terminals-smoke.sh
ok   opened on peer
ok   generation installed
ok   generation kept for the running terminal
ok   installed record is gone
ok   generation kept after the pane closed
ok   program survives the closed pane
ok   focus finds the terminal by plugin label
ok   generation reclaimed after core reported the terminal ended
ok   review opened locally
ok   kind is unsupported
ok   kind is missing_program
ok   unknown space
ok   unknown terminal
ok   kind is offline
ok   review terminal survives detach
plugin terminals smoke: 全部通过

$ HC_TUI_E2E=1 cargo test -p hc-tui --test plugin_terminals -- --nocapture
remote terminal peer/t1 runs env and cwd as planned
probe on peer reports ["definitely-missing-hctui-prog"]
terminal peer/t1 is still live after the view closed
test result: ok. 1 passed; 0 failed
```

`plugin-terminals-smoke.sh` 已按新契约更新：关 Pane 后不再断言“代际被回收”，
改为断言代际保留；随后 `hc kill` 让 core 结束终端，才断言代际被回收。
这两条是 P1-1 的“真验证”：回收判据来自 core 的终端结束事件，不是 PID 或视图。

按票里说的手工路径也跑了一遍（安装 → 卸载 → `ls` 代际目录 → 显式 GC）：

```
$ hc-tui plugins install /tmp/m75gc/src/plug --yes
$ ls /tmp/m75gc/state/hc-tui/plugins/store/dev.example.gc
1
# 手工写一份终端 pin（模拟仍在运行的终端）后卸载
$ hc-tui plugins uninstall dev.example.gc
$ ls /tmp/m75gc/state/hc-tui/plugins/store/dev.example.gc
1                      # 卸载不动代际
$ hc-tui plugins gc
no generations to reclaim   # 有 pin，空证据不删
# 删掉 pin（等价于拿到终端结束证据）后再 GC
$ hc-tui plugins gc --json
[{ "package": "dev.example.gc", "generations": [1] }]
$ ls /tmp/m75gc/state/hc-tui/plugins/store/dev.example.gc
                       # 目录已回收
```

这一步暴露并修掉了一个真实缺陷：`packages()` 原先只看记录与 pin，
手工删记录后的孤儿代际对显式 GC 不可见；现在它也扫描 `store/<id>/` 与
`current/<id>/`，由 `gc_all_reclaims_generations_without_record_or_pin` 覆盖。

## 未完成项

1. **attach 之前的占位终端 pin 无法被证据释放。** 从 spawn 到 `terminal.label` 之间进程崩溃时，
   pin 的身份是 `pending/<request>`，core 的终端列表里没有对应条目，`plugins gc` 也不会删它
   （保守保留）。要彻底解决需要在 `terminal.run` 返回时就把 core 地址交给 pin，
   这要求把终端身份取得从 attach 流程里解耦，属于本票“不做”的边界重构。
2. **P1-2 与 P1-3 的真实 core 覆盖不完整。** `plugin-terminals-smoke.sh` 与
   `tests/plugin_terminals.rs` 覆盖了终端入口与代际回收；P1-2 的死锁、后代收尾与
   `shutdown` 只在真实进程/线程层验证（真实 `sh`、真实后代 PID、真实管道），
   没有经过 core 的终端路径。P1-3 的“探测期间切换 Space”是应用层用例，
   没有在真实 UI 里手点验证。
3. **`store.rs` 的发布事务与 `current` 指针原子性没有改动**（票里明确“本票不做”）。
   `gc` 只依赖 record 与 pin，不依赖 `current` 指针的可信度。
