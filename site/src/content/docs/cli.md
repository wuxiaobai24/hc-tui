---
title: CLI
summary: 命令目录、工作台控制与事件流；同一个 session 和 XDG_STATE_HOME 才能连上控制 socket。
order: 3
spec: commands
---

## 同一个分发器

命令面板、`[[keys.command]] type = "action"` 与 `hc-tui do` 使用同一分发器；插件动作在启用前不出现在目录中。

```sh
hc-tui commands
hc-tui ls --session work
hc-tui do pane.split '{"direction":"down"}' --session work
hc-tui do space.rename '{"id":"sp123","name":"backend"}' --session work
```

`hc-tui commands` 返回当前命令目录，不需要正在运行的 TUI。`ls` 和 `do` 通过 Unix socket 控制已运行的客户端，不是直接连接 core 的 `hc` 命令。

出处：`docs/commands.md` · 调用和返回值、`docs/plugins.md` · 7. 命令、菜单与表单。

## 连接前提

CLI 参数顺序为 `do <动作名> [JSON 对象] [--session 名称] [--config 路径]`。省略 JSON 按 `{}` 处理。使用同一 `XDG_STATE_HOME` 和 session；`--config` 本身不会另开控制 socket。

同一 session 的 TUI 未运行时 `ls` / `do` 报连接失败；本地控制 socket 目前只实现于 Unix。

出处：`docs/commands.md` · 调用和返回值。

## 返回值

`do` 返回 `accepted=true` 仅表示 UI 已受理；`confirmation_pending=true` 表示仍需 UI 确认。core 操作异步完成，应通过后续 `ls`、core 状态或 UI 提示验证，不把受理当作成功结束。

```console
$ hc-tui do pane.split '{"direction":"right"}'
{"accepted":true,"confirmation_pending":false}
```

出处：`docs/commands.md` · 调用和返回值；输出为本机实测。

## 参数与表单

命令面板与 Pane 右键菜单选中带参数 schema 的插件动作时，先按 schema 打开参数表单。表单打开期间控制通道对同名动作回 `confirmation_pending = true`，关闭后 `do` 与 `plugins action invoke` 照旧直接派发并返回真实 `request`。

`hc-tui do <action> '<json>'` 与无窗口路径不经过表单。

出处：`docs/plugins.md` · 7. 命令、菜单与表单。

## 事件流

```sh
hc-tui events --plugin <id> --kinds terminal.status,terminal.turn,terminal.finished
```

控制 socket 的长连接，先回 `{"subscribed":true,"queue":256}`，随后按行输出 NDJSON；客户端断开或界面退出即停止推送。每个订阅者 256 条队列，溢出时丢弃并补发 `events.dropped`。

kind 包括 `terminal.status`、`terminal.turn`、`terminal.finished`、`terminal.opened`、`terminal.closed` 与 `registry.changed`。

出处：`docs/plugins.md` · 8. 事件。

## packages 子命令

```sh
hc-tui packages list [--json]
hc-tui packages info <id|来源> [--json]
hc-tui packages install <目录|git 仓库|.tar.gz> [--enable|--disable] [--yes]
hc-tui packages update [<id>] <目录|git 仓库|.tar.gz|.nshpkg>
hc-tui packages update --check [<id>] [--json]
hc-tui packages link <目录>
hc-tui packages enable <id> / disable <id>
hc-tui packages gc [--json]
hc-tui packages log <id> [--service <service>] [-n 100]
```

`plugins <同名子命令>` 与 `mods install|update|uninstall|link|unlink|enable|disable` 是别名：行为一致、退出码一致，只在 stderr 打一行弃用提示。`plugins action ...` 与 `plugins pane ...` 是动作与终端专用入口，名字不变。

`packages list` 一行一个包，在启用状态后打出能力徽章：`A` 动作、`T` 终端入口、`M` 界面模块；有服务时再跟 `s:在跑/总数`。

出处：`docs/plugins.md` · 9. 安装、更新与卸载。
