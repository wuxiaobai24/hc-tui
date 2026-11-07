# Events 插件

演示插件进程消费 `hc-tui events`：

- 动作 `watch`：读取若干条 `terminal.status` / `terminal.turn` 事件后退出；参数
  `count`（默认 3）与 `kinds`（逗号分隔）。
- 终端 `feed`：长运行的事件查看器，关闭 Pane 只断视图，事件流随进程结束而停止。

```sh
cp -r examples/plugins/events ~/.config/hc-tui/plugins/events
hc-tui plugins action invoke dev.hctui.plugins.events.watch '{"count":1}'
hc-tui plugins pane open dev.hctui.plugins.events.feed
```

事件来自本客户端；注入的 `HCTUI_BIN` 与 `HCTUI_SOCKET` 让动作与终端直接连到
发起它的 TUI。
