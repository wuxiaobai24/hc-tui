# M6 插件清单与动作 — 交付记录

日期：2026-11-07。范围：清单解析、动作注册、受限子进程、`plugins action` CLI、文档。
结论：M6 范围内全部实现并验证通过；终端入口、事件流与 Space 右键菜单入口不属于本次范围，列在文末“未完成项”。

## 1. 做了什么

1. **清单解析与校验**（`src/plugin/manifest.rs`）
   - 标准 `toml` 解析，所有结构 `deny_unknown_fields`：未知键、未知表直接拒绝。
   - 校验 `manifest_version = 1`、`host_api = 1`、包 ID（点分 `[a-z0-9_-]{1,32}`）、语义版本、非空 `name`、平台取值、动作/终端 ID 唯一、`contexts` 取值、`argv` 非空、`timeout_ms ≤ 600000`、`placement` 取值。
   - `[actions.args]` 按 JSON Schema 子集校验：`type`、`properties`、`required`、`enum`、`default`、`additionalProperties`、`description`；未知 schema 键拒绝。调用参数按同一 schema 校验并填默认值。
   - 所有错误带 `文件:行:列: 原因`；serde 解析错误使用 TOML span，语义错误使用 `toml::Spanned`。
2. **注册表与代际联动**（`src/plugin/registry.rs`）
   - 扫描 `<config>/plugins/*` 与 `<state>/plugins` 的 `current/<id>`；安装、链接、卸载复用 M5 的 `crate::store::Store`。
   - 重复 ID 以安装代际为准；平台不匹配跳过并给出问题；坏清单只记问题，不影响其它插件。
   - 动作完整名 `<package-id>.<action-id>`；禁用与卸载后 `Registry::actions()` 立即不再包含该动作。
3. **受限子进程**（`src/plugin/runner.rs`、`src/plugin/logs.rs`）
   - cwd 为插件目录；stdin 一次性写入上下文 JSON；注入 `HCTUI_*` 环境；`./` 开头的程序相对插件根解析。
   - 默认超时 30 秒、上限 600 秒；超时/取消先对**进程组**发 SIGTERM，2 秒后 SIGKILL，`wait()` 回收。
   - stdout/stderr 由独立线程持续读取，内存各保留最近 64 KiB 尾部，磁盘按调用双流写日志，单份超过 1 MiB 轮转到 `.1`。
   - 并发：全局 8、单包 4、队列 64；队列满返回 `busy`。状态 `queued/running/done/failed/cancelled/timed_out`。
4. **App 与 CLI**
   - App 接入：命令目录（`hc-tui commands` 带 `source/contexts/args`）、命令面板、终端/Pane 右键菜单；参数 schema 校验、目标冻结（`_target` 内部键）、结果提示、控制通道 `plugins.status/cancel/log`。
   - CLI：`plugins action list|invoke|status|cancel|log`；另实现 `plugins list/install/update/link/uninstall/enable/disable` 以完成“卸载后动作消失”的联动验收。
   - 示例：`examples/plugins/hello`（summary / fail / slow 三个动作）。
5. **文档**：`docs/mods.md`、`docs/commands.md`、`docs/plugins.md`、`docs/plan-extensions.md`、`docs/development.md`、`README.md`、`CHANGELOG.md`。

## 2. 改动文件与行数

新增（`+` 为新增行）：

| 文件 | +/- |
|---|---|
| `src/plugin/mod.rs` | +9 |
| `src/plugin/manifest.rs` | +769 |
| `src/plugin/registry.rs` | +480 |
| `src/plugin/runner.rs` | +794 |
| `src/plugin/logs.rs` | +103 |
| `src/app/plugin.rs` | +420 |
| `examples/plugins/hello/plugin.toml` | +36 |
| `examples/plugins/hello/bin/summary.sh` | +6 |
| `examples/plugins/hello/README.md` | +12 |
| `scripts/plugins-smoke.sh` | +91 |

修改（`git diff --numstat`）：

| 文件 | +/- |
|---|---|
| `Cargo.toml` | +2 / -0 |
| `Cargo.lock` | +65 / -4 |
| `src/app.rs` | +69 / -3 |
| `src/app/actions.rs` | +13 / -1 |
| `src/config.rs` | +16 / -0 |
| `src/lib.rs` | +1 / -0 |
| `src/main.rs` | +293 / -1 |
| `docs/commands.md` | +22 / -0 |
| `docs/plugins.md` | +4 / -1 |
| `docs/plan-extensions.md` | +2 / -2 |
| `docs/mods.md` | +2 / -0 |
| `docs/development.md` | +2 / -0 |
| `README.md` | +1 / -1 |
| `CHANGELOG.md` | +1 / -0 |

## 3. 验证命令与原始输出摘录

### 3.1 `cargo fmt -p hc-tui --check`

```
$ cargo fmt -p hc-tui --check
fmt exit=0
```

### 3.2 `cargo clippy -p hc-tui --all-targets -- -D warnings`

```
$ cargo clippy -p hc-tui --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.87s
clippy exit=0
```

### 3.3 `cargo test -p hc-tui`

```
$ cargo test -p hc-tui
test result: ok. 259 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.52s
...
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test exit=0
```

新增用例（`cargo test -p hc-tui --lib plugin::`，17 条全过）：

```
test plugin::manifest::tests::unknown_keys_and_invalid_fields_report_a_line ... ok
test plugin::manifest::tests::unknown_context_and_schema_keys_are_rejected ... ok
test plugin::registry::tests::broken_manifests_are_reported_with_the_line ... ok
test plugin::registry::tests::development_plugins_register_actions_and_unregister_with_the_directory ... ok
test plugin::registry::tests::installed_plugins_use_the_store_generation_and_disappear_after_uninstall ... ok
test plugin::registry::tests::disabled_plugins_leave_the_action_catalog ... ok
test plugin::runner::tests::timeout_terminates_the_process_group ... ok
test plugin::runner::tests::output_is_truncated_to_the_tail ... ok
test plugin::runner::tests::concurrency_queues_and_reports_busy_when_full ... ok
test plugin::runner::tests::cancel_stops_a_running_process_and_frees_the_slot ... ok
test plugin::runner::tests::cancel_before_start_removes_it_from_the_queue ... ok
test app::plugin::tests::invoking_an_action_queues_and_reports_through_the_runner ... ok
test app::plugin::tests::menu_items_carry_the_frozen_target_and_disappear_with_the_plugin ... ok
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 242 filtered out; finished in 2.44s
```

对应验收点：

- 清单非法时报错：`unknown_keys_and_invalid_fields_report_a_line`（`id` 第 2 行、`host_api` 第 5 行、未知键第 6 行、空 `argv` 第 10 行）、`unknown_context_and_schema_keys_are_rejected`（context 与 `patternProperties`）、`broken_manifests_are_reported_with_the_line`（`:6:`）。
- 超时杀进程组：`timeout_terminates_the_process_group`，脚本 `sleep 2; touch marker` 超时 100 ms；断言 `timed_out`、运行 < 2 s，等待 2.3 s 后 marker 不存在。
- 输出截断：`output_is_truncated_to_the_tail`，`yes x | head -c 200000` 后 stdout ≤ 64 KiB 且 > 32 KiB。
- 并发队列：`concurrency_queues_and_reports_busy_when_full`，全局 1、单包 1、队列 2；第三条排队，第四条返回 `Busy`。
- 取消回收：`cancel_stops_a_running_process_and_frees_the_slot`、`cancel_before_start_removes_it_from_the_queue`；取消后 `running()==0` 并成功提交下一条。

### 3.4 `scripts/ui-smoke.sh`

```
$ HC_TUI_BIN=target/debug/hc-tui bash scripts/ui-smoke.sh
ok   临时 Pane 自动消失
ok   shell 自定义命令
ui-smoke: 全部通过
smoke exit=0
```

### 3.5 `HC_TUI_E2E=1` 的 `g1_attach` / `multi_pane`

```
$ HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane -- --nocapture
nvim real core: ASCII typing, multilingual paste, resize and save passed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.34s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
e2e exit=0
```

### 3.6 端到端插件冒烟（真实 TUI + 控制通道）

命令：`scripts/plugins-smoke.sh`（隔离 `XDG_*` 与 hc session；复制 `examples/plugins/hello`）。

```
$ scripts/plugins-smoke.sh
== catalog without a window
ok   three actions listed
== install into the store and list
installed dev.hctui.plugins.hello generation 1
== invoke summary
request=p1
ok   summary done
ok   context id in stdout
ok   log carries stdout
== invoke failure
ok   failed status
ok   exit code 7
ok   stderr in log
== invoke slow then cancel
ok   cancelled status
== disable removes actions from the catalog
ok   catalog empty
ok   catalog restored
== uninstall removes the installed generation
ok   no installed plugins
plugins e2e exit=0
```

## 4. 未完成项与原因

1. **终端入口（`[[terminals]]` 的执行）**：属于 M7。M6 只做清单校验、注册表暴露（`Registry::terminals()`）与预览，未创建真实 PTY、未做 `requires_programs` 检查、未写入 `hctui.plugin` 标签。
2. **事件流 `hc-tui events`**：属于 M8；本次未实现长连接、过滤与 `events.dropped`。
3. **Space 上下文动作的菜单入口**：现有 hc-tui 没有 Space 右键菜单（侧栏 Space 右键只做重命名等既有路径）；`contexts = ["space"]` 的动作已进入命令目录与命令面板，但没有新增 Space 菜单。Pane 右键菜单已接入。
4. **插件签名包安装**：M6 复用 M5 的目录/代际安装（`plugins install/link`），未做 `.nshpkg` 签名校验；签名与撤销在 M5 只针对界面模块。
5. **权限声明**：规格 §10 明确插件是受信本地代码、`capabilities` 不作为隔离承诺；清单没有 `capabilities` 字段。本次实现的可执行范围由 `host_api`、`platforms`、`contexts` 与键位绑定约束，未新增 OS 级权限声明。
6. **日志组织粒度**：规格写“日志文件按包轮转保留两份各 1 MiB”。实现为每个调用双流文件（`logs/<package>/<request>.stdout.log` / `.stderr.log`）各自 1 MiB 轮转，包目录下文件数随调用增长；满足有界与可查询，但不是单一包级日志文件。
7. **`plugins action invoke/status/cancel/log` 需要运行中的 TUI**：运行状态在客户端进程内，经控制 socket 转发；`list/install/link/uninstall/enable/disable` 与 `log` 的磁盘读取部分无窗口也可用（`log` 目前同样走控制通道，因此需要窗口）。
8. **`error.kind`**：规格 §7 的 `conflict/offline/unavailable` 用于 core 操作与布局 CAS；M6 动作是本地子进程，失败只给退出码与 stderr 尾部，未引入 core 错误分类。
