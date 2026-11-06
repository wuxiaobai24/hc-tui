# herdr-app 对齐契约

基准是 2026-11-06 本机 herdr-app 的 `commands`、`herdr.d.ts`、
`mods/agents/main.js`、`mods/spaces/main.js`，以及 core 0.1.18 的 schema 和真实记录。
本文替代旧版 `gap-vs-herdr.md` 作为当前行为对照；旧文档只保留历史。

## 对象和共享状态

- 读取本机与已连接机器的 `herdr-app` core 文档，使用原始 space/tab/pane ID。
  远端 ID 和终端地址带 `machine/` 前缀；同名 space 不合并。
- TUI 的结构修改通过 `doc.set` CAS 回写同一文档。名称、树、顺序、终端和模块 props
  能双向同步；未知字段保留，未改动的 GUI 树与样式原样保留。
- 选择、焦点、zoom、settled 属于屏幕状态，保存在 `hctui-layout-v1` 文档的 schema 3，
  不写进 GUI 共享结构。兼容读取旧版 TUI 存档。
- 共享结构变化复用已有 VT 和 attach 流，保留视图 ID、内容和焦点。
  不会为了同步一次重命名而关闭全部流。
- CAS 冲突做三方合并，包含防抖期间尚未提交的本地修改。不同 space 的修改合并；
  同一字段冲突停止自动覆盖，保留本地屏幕并提示 `layout.reload` 接受共享版本。
- 终端标签 `herdr.space`、`herdr.tab`、`where` 随位置与名称更新。

## Agents

| 行为 | 实现 |
| --- | --- |
| 无需点击获取名称 | 从订阅快照读取 title、program、cwd、labels、turn；名字不以地址作为首选 |
| 分组 | 机器和 space；分组身份用稳定 ID，重名或重命名不混淆折叠状态 |
| 优先级 | Pinned、Needs you、Done、Working、Idle；置顶和 `herdr.seen` 与 GUI 共享 |
| 父子关系 | 按 parent 组成树，父项按子树最紧急状态参与排序；可折叠，防止坏 parent 环递归 |
| Done | 查看时共享已读；当前焦点停留期间保留 Done 的位置，离开或新状态到来才变化 |
| 导航 | agent.next / agent.prev 沿实际显示顺序循环；agent.focus 支持尚未打开的子 agent |
| 点击已有 agent | 回到已有 space/tab/pane，不创建第二个视图 |
| 外部终端 | 按稳定 space ID 定位，独立 Tab 打开，保留原分屏；缺少来源标签时才按目录归组 |
| 完成提醒 | 已 settle 的 Tab 遇到注意力状态重新出现在列表中，不抢当前焦点 |

## 生命周期

| 操作 | 结果 |
| --- | --- |
| pane.close / tab.close / space.close | 结束相应终端及其子 agent，并移除结构；Tab/space 按确认设置询问 |
| terminal.park | 禁用 keep，移除视图，程序继续运行 |
| terminal.keep | 启用 keep，没有视图时重新打开 |
| terminal.kill | 结束指定程序及子 agent，不把它替换成一个不相干的新 shell |
| tab.settle | 在当前屏幕收起 Tab，保留终端，显式选择或需要注意时恢复 |
| app.quit / prefix q | detach 整个客户端，程序继续运行，退出前尽力写完布局与已读状态 |
| 新建普通终端 | 在 core 中启用 keep；临时命令和 popup 不启用 |
| 不存在的已存终端 | 报告不可用，不偷偷在本机启动替代 shell；keep 恢复由 core 负责 |

## 多机与输入

- 已知机器会自动刷新和导入布局；未连接/空机器有状态与连接、新建入口。
- 新建 space/tab/split、脚本面板和自定义命令跟随工作区机器、目录。
- Ctrl+C/Ctrl+D 等按子终端模式编码。松键不消费前缀、不重复触发输入框；直接绑定同样生效。
- 输入和粘贴时滚回底部。模态浮层拦截鼠标，不让点击落到下面的 Pane。
- RPC 按 ID 异步分发；慢命令、读文件和等待输出不阻塞 attach 输入、输出与 credit。
- terminal.right_click 决定终端右键去程序还是菜单；Shift+右键始终打开菜单。

## TUI 入口和适配

`prefix :` 是可搜索命令面板；选择后可以编辑 JSON 参数再执行。
`hc-tui commands` 列出当前真正支持的命令，不列假动作。

```sh
hc-tui ls
hc-tui do space.rename '{"id":"sp...","name":"backend"}'
hc-tui do tab.settle '{}'
hc-tui do agent.next '{}'
hc-tui do pane.swap '{"direction":"left"}'
hc-tui do terminal.park '{"terminal":"peer/t7"}'
hc-tui do mod.open '{"name":"files"}'
```

同样支持 `--session` 和 `--config`。控制 socket 限本机用户访问。
返回 `accepted` 表示命令已被 UI 接受，core 的异步完成通过状态和事件反馈；
`confirmation_pending` 表示仍需在 UI 中确认，不冒充已经完成。

| GUI 功能 | TUI 对应机制 |
| --- | --- |
| space/tab 选择、移动、重命名 | 同名命令，支持稳定 ID 与位置参数 |
| pane split/zoom/go/swap | 同名动作，保持 Pane/终端身份 |
| terminal peek/show/copy/paste/search/scroll | 只读浮层、临时显示、复制模式和 attach 输入 |
| 文件/变更面板 | files.open、file.open、changes.open；本机和远端经 core 执行 |
| 模块 | mod.open 与 `[[panels]]`，脚本输出可含 ANSI；props 通过 HC_TUI_PANEL_PROPS 传入 |
| mod.clone | 将内置/自定义脚本面板复制到配置目录 panels/*.json，下次启动加载 |
| layout.set/select | 支持当前 Tab 的 SavedNode 或 GUI pane 树；Stack 可选择子 Pane |
| layout.clone/switch/reset | 个人 Pane 布局 JSON、七个内置预设；拒绝覆盖已有克隆文件 |
| 快捷键运行命令 | `[[keys.command]] type = "action"` 调用相同分发器 |

GUI 的 GPUI JS 模块不能作为终端代码直接执行；其共享内容和 props 保留，
TUI 用脚本面板提供扩展。GUI 全局 chrome 树和 TUI Pane 布局不是同一种布局对象。
原生窗口最小化、系统隐藏、字体缩放、GUI 安装器、欢迎页/反馈服务没有逐字复制成空命令；
它们属于宿主终端或 GUI 产品。`commands` 是 TUI 可操作能力的准确边界。

## 图片

- 解析 Kitty PNG/RGB/RGBA 和 Unicode 虚拟 placement，保留源裁剪、透明度、位置和层级。
- Kitty/Ghostty/Herdr/WezTerm 宿主自动使用原生像素输出；可显式设置
  `HC_TUI_GRAPHICS=kitty` 或 `HC_TUI_GRAPHICS=blocks`。tmux 默认字符预览。
- 原生图形按 Pane 和浮动层遮挡裁剪；对话框等模态浮层期间隐藏。
  相同帧不重复上传，切 Tab、移除和退出清理本客户端的图像。
- 通用宿主使用上下半块预览，遵守源裁剪和透明度。
- 没有声称实现 Sixel/iTerm 协议，也没有把协议往返测试当作所有真实宿主的视觉验收。

## 验证入口

```sh
cargo test -p hc-tui
cargo clippy -p hc-tui --all-targets -- -D warnings
HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach --test multi_pane
just smoke
just parity
```

`just parity` 在临时目录启动隔离的本地 core、第二个 core 与 TUI，通过真实 machine
传输验证完整远端布局导入、远端新建、GUI 文档双向修改、未知样式保留、settle、
park/keep/close/quit。不会读写默认 session。

本轮自动化运行环境是 Linux + tmux；第二个 core 也在本机，经 machine 传输调用。
实体 SSH 主机、macOS/Windows 和真实宿主窗口视觉仍需对应环境验收，不能声称已测。

参考：[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)。
