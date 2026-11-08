---
title: FAQ
summary: Agents 列表、颜色、连接失败、插件沙箱与退出后的布局，这些边界先看这一页。
order: 5
spec: usage
---

## Agents 里为什么没有我的 shell？

这是预期行为。只有上报过状态的会话进入 Agents；普通终端不因打开、命名或置顶就成为 Agent。用 Space 或 Go to 搜索找普通终端。

出处：`docs/usage.md` · Agents 与注意力、常见问题。

## 为什么 Pane 的某些颜色仍与 UI 不一样？

显式真彩色和程序通过 OSC 指定的颜色由程序控制。外部 UI 与 Pane 默认前景、背景以及 ANSI 0–15 色跟随当前主题，但程序自己设置的颜色不会被强行统一。

出处：`docs/usage.md` · 图片与颜色。

## `hc-tui ls` / `do` 连接不上？

要先运行同一 session 的 TUI，并保持同一 `XDG_STATE_HOME`；自定义 session 要每次传入。同一状态目录和 session 只允许一个控制 socket 服务，第二个客户端可能仍可打开，但 CLI 控制对应首个服务。

出处：`docs/usage.md` · 常见问题、`docs/commands.md` · 调用和返回值。

## 插件有沙箱吗？

没有 OS 沙箱。插件是以用户身份运行的受信本地代码，`capabilities` 不作为隔离承诺。界面模块另有 nsh 侧的内存与调用限额，但同样不是 OS 级隔离。

出处：`docs/plugins.md` · 13. 限制、`docs/mods.md` · 沙箱限制。

## 动作能跑多久？输出会无限增长吗？

默认 30 秒，`timeout_ms` 上限 600000；到期发送 SIGTERM，2 秒后 SIGKILL，进程组一并结束。stdout 与 stderr 各保留最近 64 KiB，落盘日志单份超过 1 MiB 轮转，每个包最多保留 32 个日志文件、总计 8 MiB。

出处：`docs/plugins.md` · 4. 动作执行、输出与日志。

## 退出再进，布局为什么变了？

检查 session、restore 配置、core 文档，以及是否同时有 GUI 写入。同一字段冲突时暂停自动覆盖，执行 `layout.reload` 明确接受共享版本。焦点、zoom、settled 属于本机屏幕状态，名称、树、顺序与终端属于共享结构。

出处：`docs/usage.md` · 常见问题、多机与共享布局。

## `--no-mods` 会关掉插件动作吗？

不会。`--no-mods` 只跳过模块表面并回到内置外框，动作与终端入口照常；单个包 `disable` 才会让三类入口一起消失。

出处：`docs/mods.md` · 回退、`docs/plugins.md` · 验收。
