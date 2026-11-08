---
title: 架构
summary: core 持有程序，TUI 负责界面；布局走 herdr-app 共享文档，扩展跑在宿主帧里。
order: 4
spec: mods
---

## core 与 TUI

hc-tui 接 herdr-core，与 herdr-app 共享布局。core 持有程序，TUI 负责界面；远程传输直接复用 core 的 machine 链路，没有独立的 TUI remote 协议。

- Pane 是一个终端视图或脚本面板；进程由 core 持有，视图由 TUI 管理。
- 客户端退出只 detach，core 中的程序继续运行。
- 关闭 Pane、Tab 或 Space 才结束对应程序与子 Agent。

出处：`README.md` · 先认识界面、关闭与退出的区别。

## 共享布局

结构使用每台机器的 `herdr-app` 文档和 CAS 写入，名称、树、顺序、终端和模块 props 双向同步。焦点、zoom、settled 等屏幕状态在 TUI 自己的 `hctui-layout-v1` schema 3 中保存。

并发修改不同字段时合并；同字段冲突暂停自动覆盖并提示，执行 `layout.reload` 接受共享版本。外框与模块状态不写共享文档。

出处：`docs/usage.md` · 多机与共享布局、`docs/mods.md` · 能力边界。

## 扩展运行在宿主帧里

界面模块是 TSX 应用，由 hc-tui 在自己的帧里嵌入运行。hc-tui 负责外框组合、表面矩形、输入仲裁、数据与动作；nsh 负责表面内部的布局、组件、动画、键鼠、文本输入、沙箱与权限。

调度上有明确预算：一帧内所有表面渲染总预算 32 ms，单表面 8 ms；超出的表面保留上一帧并记账。沙箱限制为内存 128 MiB、调用 500 ms、加载 10 s、节点数 200000，按实例设置，清单只能下调。

出处：`docs/mods.md` · 5. 调度、6. 输入与焦点、沙箱限制。

## 三条契约

| 契约 | 内容 |
| --- | --- |
| 代码寿命 | 一次安装是一个代际；更新写新代际，旧调用与旧终端继续用旧代际，直到没有任何 pin 指向它才回收。 |
| 进程寿命 | 动作进程组跟随 hc-tui 客户端回收；终端继续由 core 保留。 |
| 目标冻结 | 调用时把 Space、Tab、机器与 cwd 解析成不可变的 `ExecutionTarget`；焦点变化不改目标，目标消失报 `target_gone`。 |

出处：`docs/plugins.md` · 3. 身份、5. 终端入口、13. 限制。

## 迁移边界

默认界面迁移到模块还有明确缺口：picker、dialogs、settings、help 与 toast 的 props、校验状态、目标冻结字段需要补齐后，模块才能接管对应区域。内置实现保留为回退路径。

出处：`docs/m9-gap.md` · 2.1 它现在渲染什么、2.2 迁移需要哪些 props。
