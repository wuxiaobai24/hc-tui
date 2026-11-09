# hc-tui

接 herdr-core、与 herdr-app 共享布局的终端工作台：core 持有终端，`hc-tui` 是客户端。

```sh
cargo build --release
./target/release/hc-tui
./target/release/hc-tui packages install examples/packages/git-board --yes --enable
```

运行前提：`hc`（herdr-core）在 PATH 中；已验证的协议基线为 core 0.1.18。发行二进制不需要 Rust 或 Zig，本文命令按源码树里的 `target/release/hc-tui` 写，装好后可以省略这段前缀，发行包的安装与升级见[使用指南](docs/usage.md#安装与升级)。

## 它是什么

终端进程与 PTY 由 core 持有，`hc-tui` 只负责视图、布局与输入；`q` 退出客户端，程序继续在 core 里跑。

一个 Space 对应一台机器上的一个目录，Tab 里放分屏与浮动 Pane，Pane 装终端或界面模块。终端地址（`t3`、`peer/t3`）标识 core 中的程序，与 Pane 编号无关。

一个包只有一份 `hctui.toml`（`manifest_version = 2`），四类能力写在同一处：`[[actions]]` 一次性动作、`[[terminals]]` 终端入口、`[[services]]` 常驻服务、`[module]` TSX 界面模块。动作在客户端主机启动，读完 stdin 的上下文 JSON 后退出；终端入口由 core 在 Space 的机器上开真实 PTY；服务物化成 core 的 keep 终端；模块停靠到外框。

## 五分钟上手

[examples/packages/git-board](examples/packages/git-board/hctui.toml) 一个清单声明了动作、终端入口和模块三样，开头的 `packages install` 命令安装它，并展开成：

```console
$ ./target/release/hc-tui packages install examples/packages/git-board --yes --enable
package dev.hctui.pkg.git-board (Git board 0.1.0)
  provides ATM
  actions dev.hctui.pkg.git-board.summary
  terminal dev.hctui.pkg.git-board.git-ui (right)
    requires lazygit
  module dist/bundle.js (right)
  platforms linux, macos
  grants state, exec, notify, state.changed, grant.changed, terminal.status
installed dev.hctui.pkg.git-board generation 1 (enabled)
```

`packages list` 的能力徽章就是这三类：`A` 动作、`T` 终端入口、`M` 模块。动作同时进入 `hc-tui commands` 与界面里的 `:` 命令面板：

```sh
./target/release/hc-tui commands | grep git-board
```

动作的目标是 Space；Space 带机器与目录，TUI 刚启动时那个隐式 Space 没有目录。另开一个终端启动 `./target/release/hc-tui`，然后建一个指向仓库的 Space 并运行动作：

```sh
./target/release/hc-tui do space.new "{\"name\":\"repo\",\"folder\":\"$PWD\"}"
request=$(./target/release/hc-tui plugins action invoke dev.hctui.pkg.git-board.summary '{"short":true}' | sed -n 's/.*"request": "\([^"]*\)".*/\1/p')
./target/release/hc-tui plugins action status "$request"
```

`status` 的 `stdout` 就是该目录的 `git status --short --branch`；动作在客户端主机上以独立进程启动，工作目录是包目录，目标 Space 的机器与目录冻结在调用那一刻。终端入口同样落在 Space 的机器与目录上：

```sh
./target/release/hc-tui plugins pane open dev.hctui.pkg.git-board.git-ui
```

`placement = "right"` 在右侧开真实 `lazygit`；`requires_programs` 里的程序缺失时返回 `missing_program`，不会静默换成别的程序。模块按 `[module].place` 停靠到右侧。

一条命令让三类一起停用，不需要重启：

```sh
./target/release/hc-tui packages disable dev.hctui.pkg.git-board
```

动作从 `commands` 和命令面板消失，`plugins pane open` 返回 `disabled`，模块区域退回内置界面；`packages enable` 全部恢复。已经打开的终端不会被 kill，已排队动作跑完为止。

## 能力边界

能做：多机工作区与布局（Space/Tab/Pane 树写 herdr-app 共享文档，CAS 三方合并）、多 Tab 分屏与浮动、终端 attach 与只读历史、Agent 状态与父子导航、命令面板与 `commands`/`ls`/`do` 自动化、Kitty 图片与遮挡、主题与手机窄屏布局。

不做：终端内容、光标、复制模式、历史搜索和 Kitty 图片留在宿主进程，它们逐格在 VT → Ratatui 的热路径上，模块通道只传快照与动作。输入法组合事件不在宿主输入通道（`src/mods/input.rs` 只翻译按键、粘贴、焦点与鼠标），IME 下的行为未验证。Sixel/iTerm 图片协议未实现。包以用户身份运行，没有 OS 沙箱；服务只在本机 core 上起，没有自动重启与多机调度；插件不能建立第二条 core 连接，也不能修改共享布局文档。

## 排查与续订

不开窗口也能查机器、反查键位、从上一个 revision 续订事件：

```console
$ hc-tui machine ls --json
{"machines":[{"capabilities":["attach","automatic_input","events","framed","resize_notifications","shell","status"],"endpoint_protocol_generation":1,"host":"omarchy","name":"local","protocol":"framed","state":"connected","target":"","version":"0.1.18"}]}

$ hc-tui keys where help --json
{"action":"help","keys":["ctrl+b","?"]}

$ hc-tui events --since 41 --count 2
{"subscribed":true,"queue":256}
{"kind":"events.snapshot","revision":47,"resync":false,"snapshot":{…}}
```

`machine ls` 的四项新增字段是 `version`、`protocol`、`endpoint_protocol_generation` 与
`capabilities`；本机行来自自己的 hello 握手，远端机器尚未握手时为 `null` / `[]`。
`keys where` 给完整前缀链（前缀键 + 动作键）。`events --since <rev>` 先推一帧带 `revision` 的
快照，此后只推 `revision` 更大的事件；`rev` 已被裁剪或快于当前值时首帧带 `"resync":true`。

## 文档

| 文档 | 内容 |
|---|---|
| [使用指南](docs/usage.md) | 键位、鼠标、Agent、图片与颜色、生命周期、安装与升级 |
| [交互操作](docs/interaction.md) | 点击菜单、拖拽、手机手势 |
| [配置说明](docs/configuration.md) | 配置项、颜色、绑定、脚本面板与本机路径 |
| [命令参考](docs/commands.md) | 全部工作台命令、参数、目标选择与返回值 |
| [插件规格](docs/plugins.md) | 包清单、动作、终端入口、常驻服务、事件流、安装事务 |
| [界面模块](docs/mods.md) | TSX 模块、外框树、宿主方法与授予 |
| [共享契约](docs/sync-contract.md) | 与 herdr-app 双向同步的字段与冲突边界 |
| [行为对照](docs/herdr-app-alignment.md) | 共享字段在当前实现里的行为记录 |
| [开发与部署](docs/development.md) | 构建、验证、本机部署与回滚 |
| [发布检查清单](docs/release.md) | 版本、构建、标签与发行包验收 |
| [扩展计划](docs/plan-extensions.md) | 里程碑状态、验收与风险，§3.1 是当前进度 |
| [协议实测基线](docs/protocol-facts.md) | core 0.1.17 时期的记录，需结合当前 schema |
| [core 泄漏治理](docs/ops-cores.md) | 判定、清理与会话 socket 规则 |
| [ADR](docs/adr/) | VT 选型与进程拓扑 |

历史与评估资料：[plan-v1.0](docs/plan-v1.0.md)、[plan-v1.1](docs/plan-v1.1.md)、[review-v1.0](docs/review-v1.0.md)、[gap-vs-herdr](docs/gap-vs-herdr.md)、[tern-tsp-compat](docs/tern-tsp-compat.md)、[m9-gap](docs/m9-gap.md)，均不代表当前功能，以本 README 与使用指南为准。

## 开发与测试

工具链用 `mise install` 装，固定 Zig 0.16.0（vendored VT 不支持 0.17）。`justfile` 里是等价的 cargo 命令，没有 `just` 也可以直接照抄。

```sh
just build-smoke                # cargo build -p hc-tui
just test                       # cargo test -p hc-tui
HC_TUI_GRAPHICS=kitty just test # 在 tmux / TERM=dumb 下必须加
just smoke                      # 真实 core + tmux 的 UI 冒烟（隔离 session）
bash scripts/packages-smoke.sh  # 一个包三类入口的端到端冒烟（需先 just build-smoke）
```

`HC_TUI_GRAPHICS` 控制原生 Kitty 图形；tmux 里默认退回半块字符预览，图形用例 `mod_regions_block_kitty_images_until_undocked` 会因此失败，加 `=kitty` 才是测试要验证的路径。普通 `cargo test` 里的真实 core 用例默认跳过，端到端用 `just e2e`（`HC_TUI_E2E=1`）。`just --list` 还有 `parity`、`plugins-smoke`、`events-smoke`、`degraded-smoke`、`plugin-terminals-smoke`、`perf-baseline`、`release-musl` 与 `dist`。

## 状态

`main @ 2527acc`。近期交付：M10 契约硬化、M11a–c 统一包与分发、`[[services]]` 常驻服务、参数 schema 动作表单、无窗口 `do`。里程碑表见[扩展计划 §3.1](docs/plan-extensions.md#31-里程碑交付状态)：M1–M8、M10、M11a–c 已交付，M9（默认界面模块迁移）未开始。未发布条目见 [CHANGELOG.md](CHANGELOG.md)。许可 Apache-2.0，第三方组件见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。
