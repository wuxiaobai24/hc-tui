# 工作台命令参考

[返回 README](../README.md) · [使用指南](usage.md) · [配置](configuration.md)

## 调用和返回值

前缀 `:` 打开命令面板，搜索动作后可编辑 JSON 参数；action 快捷键与 CLI 使用同一分发器。
`hc-tui commands` 返回当前命令目录，无需正在运行的 TUI。
`ls` 和 `do` 通过 Unix socket 控制已运行的客户端，不是直接连接 core 的 `hc` 命令。

```sh
hc-tui commands
hc-tui ls --session work
hc-tui do pane.split '{"direction":"down"}' --session work
hc-tui do space.rename '{"id":"sp123","name":"backend"}' --session work
```

CLI 参数顺序：`do <动作名> [JSON 对象] [--session 名称] [--config 路径]`。
省略 JSON 按 `{}` 处理。使用同一 XDG_STATE_HOME 和 session；--config 本身不会另开控制 socket。
同一 session 的 TUI 未运行时 ls/do 报连接失败；目前本地控制 socket 只实现于 Unix。

`ls` 返回 state、connection、terminals。state 包含 workspaces、tabs、panes、稳定来源 ID 和当前选择；
terminals 是 core 终端快照，包含普通终端，不等于侧栏 Agents 列表。
`do` 返回 accepted=true 仅表示 UI 已受理；confirmation_pending=true 表示仍需 UI 确认。
core 操作异步完成，应通过后续 ls、core 状态或 UI 提示验证，不把受理当作成功结束。

## 目标与通用参数

- `space.*`（除 new）可用 `id` 选择稳定 Space ID 或 TUI 数字 ID 的字符串。
- `tab.*`（除 new）可用 `id` 选择稳定 Tab ID 或 TUI 数字 ID 的字符串，并切到所在工作区。
- `pane.*` 可用 `pane` 选择稳定 Pane ID 或 TUI 数字 ID 的字符串。
- `terminal` 是 core 地址，如 `t3` 或 `peer/t3`；通常省略时使用当前输入终端。
- `index` 从 **1** 开始；机器名称和稳定 ID 不按显示名称猜测，先用 ls 读取。
- 选定目标可能改变当前焦点。复制、搜索和滚动需要已有打开的终端视图。

## Space

| 动作 | 参数与结果 |
|---|---|
| space.new | 可选 name、folder、machine、argv（字符串数组）；创建工作区和首个终端 |
| space.close | 可选 id、confirmed=true；结束工作区程序和子 Agent，默认遵循确认设置 |
| space.rename | 可选 id；name 指定新名，省略弹出输入框 |
| space.move | 可选 id；index 必填，在工作区列表中移动 |
| space.select | id 或 index；切换工作区 |
| space.next / space.prev | 循环选择工作区 |

## Tab

| 动作 | 参数与结果 |
|---|---|
| tab.new | 可选 name、space、folder、argv；place 为 tab（默认）、right 或 down |
| tab.close | 可选 id、confirmed=true；结束相关程序和子 Agent |
| tab.rename | 可选 id；name 指定名称，省略弹出输入框 |
| tab.move | 可选 id；index 指定在本工作区的顺序 |
| tab.select | id 或 index；选择并取消 settled |
| tab.next / tab.prev | 切换 Tab |
| tab.settle / tab.unsettle | 可选 id；收起 / 恢复，程序保持运行 |

space.new、tab.new、terminal.open 也可传 mod 和 props 创建模块 Pane。
Tab 数量及 Space 概览包含 settled Tab；settle 只是屏幕状态，不删除共享结构。

## Pane

| 动作 | 参数与结果 |
|---|---|
| pane.split | direction=right（默认）或 down |
| pane.rename | 可选 pane / terminal 指定目标；name 设置别名，省略弹出输入框，空字符串清除 |
| pane.close | 关闭目标 Pane 并结束程序；最后一个 Pane 按设置确认 |
| pane.zoom | 切换当前 Pane 放大 |
| pane.focus | terminal 指定 core 终端；也可通过通用 pane 参数定位现有 Pane |
| pane.go | direction=left/right/up/down，默认 right |
| pane.swap | pane 与 with 指定同一 Tab 的两个 Pane，交换位置并保留各自的终端和程序身份；只给 direction 时与方向邻居交换 |
| pane.move | pane 指定目标（默认焦点 Pane），to 指定目标 Tab（默认当前 Tab 末尾），before 或 after 指定落点；源 Tab 空掉时随开关闭，程序不结束 |

拖动分隔线属于鼠标交互，不需要先运行命令。键盘比例调整用前缀 r。

## Terminal 与 Agent

| 动作 | 参数与结果 |
|---|---|
| terminal.open | 同 tab.new；argv 指定程序，place=tab/right/down |
| terminal.keep | 可选 terminal；启用 keep，无视图时打开 |
| terminal.park | 可选 terminal；禁用 keep、移除视图，程序继续运行 |
| terminal.pin | 可选 terminal、pinned（默认 true）；共享置顶，不改变 Agent 分类 |
| terminal.kill | 可选 terminal；结束程序及子 Agent，直接执行，无交互确认参数 |
| terminal.peek | 可选 terminal；读取最近 500 行到只读浮层 |
| terminal.show | terminal 临时替换当前视图的显示；省略 terminal 恢复原显示 |
| terminal.copy | 可选 terminal；复制已打开视图的可见文本 |
| terminal.paste | text 必填，可选 terminal；发送粘贴文本，**不自动回车** |
| terminal.paste_image | 本机剪贴板图片上传并粘贴路径到当前 Pane |
| terminal.scroll | 可选 terminal；to=top/bottom，或 lines（有符号）/pages（可为小数）；负值向历史上方 |
| terminal.search | 可选 terminal、query；进入 Copy Mode 搜索 |
| terminal.right_click | 可选 terminal；to=program 或 menu，设置右键路由；Shift+右键总是菜单 |
| agent.rename | 可选 terminal 指定目标；name 设置终端别名，省略弹框，空字符串清除；无需已有 Pane |
| agent.focus | terminal 指定 Agent，可打开尚未显示的子 Agent |
| agent.next / agent.prev | 沿当前可见 Agents 顺序循环，不包含普通 shell |

重命名示例：

```sh
hc-tui do tab.rename '{"name":"开发"}'
hc-tui do pane.rename '{"pane":"3","name":"测试"}'
hc-tui do agent.rename '{"terminal":"peer/t7","name":"审查 Agent"}'
```

Agent 和对应 Pane 共用终端别名，写入 core 的 `hctui` 标签；Tab 名独立保存。
重命名对话框预填现有别名，清空后恢复默认显示。

前缀 i 的 terminal.send 是“文本 + Enter”，不同于 terminal.paste。
按名字发送按键、等待输出、上传文件、模板选择器是键位入口，未另列为同名工作台命令。
程序正常退出会自动移除 Pane；连接错误保留视图。退出全部客户端不会替代 terminal.kill。

## Machine 与文件/模块

| 动作 | 参数与结果 |
|---|---|
| machine.list | 请求机器列表，在 UI 中显示；CLI 受理响应不含列表结果 |
| machine.connect | 参数按 core 的 machine.connect 传递，如 machine |
| machine.add | 参数原样传给 core machine.add，如 name、via、session；以本机 hc schema 为准 |
| machine.remove | 参数原样传给 core machine.rm |
| files.open | 当前工作区 Git 文件列表，只读浮层 |
| changes.open | 当前工作区相对 HEAD 的 git diff，只读浮层 |
| file.open | path 必填，当前工作区机器上的文件，只读浮层 |
| mod.open | name 必填，可选 props 对象；新 Tab 中打开脚本模块 |
| mod.clone | name 必填，可选 as（默认 name-copy）；写入个人 panels JSON |
| mods.dock | name 必填；place 为 left/right/top/bottom（默认 left），size 为 {cols}/{rows}/{ratio}，可选 index 与 props；把模块区域绑入外框树并写入本地 state |
| mods.undock | name 必填；从外框树移除该模块区域 |
| mods.chrome | tree 必填；替换整棵外框树，拒绝非法结构（必须恰好一个 panes 叶子） |
| mods.open | name 必填；place 为 region/pane/dialog/popup/window（默认 region），可选 props、size（`[cols, rows]` 或 `{cols, rows}`）、at、key；打开模块表面 |
| mods.close | name 或 surface 必填；关闭该模块的全部表面或指定表面 |
| mods.reload | 可选 name；重载一个或全部模块表面，失败保留旧实例与旧帧 |
| mods.grant | name 必填，host/events 至少一项；追加宿主授予，revoke 为 true 时撤销 |
| mods.enable / mods.disable | name 必填；启用或禁用模块，禁用时关闭其表面 |

`hc-tui machine ls` 不开窗口也能查机器，每台一行：

```sh
hc-tui machine ls [--json] [--session <name>] [--config <path>]
```

本机行（`name = "local"`）来自自己的 hello 握手：`version` 是它的版本，`protocol` 是
`framed` 或 `plain`，`endpoint_protocol_generation` 是它的协议代次，`capabilities` 是它声明的
features。其它机器行只报 core `machine.list` 知道的字段：`version` 与 `state`；尚未握手的
`protocol` / `endpoint_protocol_generation` / `capabilities` 为 `null` / `[]`，`why` 原样转出。
握手失败报 `core_unavailable`（退出码 3），代次不匹配报 `protocol_incompatible`。

```console
$ hc-tui machine ls --json
{"machines":[{"capabilities":["attach","automatic_input","events","framed","resize_notifications","shell","status"],"endpoint_protocol_generation":1,"host":"omarchy","name":"local","protocol":"framed","state":"connected","target":"","version":"0.1.18"},{"capabilities":[],"endpoint_protocol_generation":null,"host":"box","name":"box","protocol":null,"state":"unreachable","target":"box","version":"0.1.18","why":"cannot reach box: link down"}]}
```

反查一个动作现在按什么键触发：

```sh
hc-tui keys where <action> [--json] [--config <path>]
```

绑定是前缀模型，所以给完整前缀链（`["ctrl+b","?"]`），不是只给末尾的键。`off` 的动作
报 `action_not_found`（退出码 1）。直接绑定的动作只有一键。

```console
$ hc-tui keys where help --json
{"action":"help","keys":["ctrl+b","?"]}
```

```sh
hc-tui do mod.open '{"name":"changes"}'
hc-tui do file.open '{"path":"README.md"}'
hc-tui do mod.open '{"name":"status","props":{"scope":"all"}}'
hc-tui do mod.clone '{"name":"files","as":"my-files"}'
hc-tui do mods.dock '{"name":"agents","place":"right","size":{"cols":20}}'
hc-tui do mods.undock '{"name":"agents"}'
hc-tui do mods.open '{"name":"agents","place":"dialog","size":[60,16]}'
hc-tui do mods.reload '{"name":"agents"}'
hc-tui do mods.grant '{"name":"agents","host":["exec"],"events":["state.changed"]}'
hc-tui mods check examples/mods/agents
```

files.open 与 mod.open files 是不同入口：前者一次性只读浮层，后者定期刷新的 Tab 模块。
脚本面板的注册与执行环境见 [配置](configuration.md#脚本面板与个人克隆)。
外框树决定侧栏、Tab 栏、内容区、模式行与模块区域的排列，保存在本地 state 的 `mods_chrome`，
`--no-mods` 回到内置外框；详细结构见 [界面模块](mods.md#4-外框与表面)。

插件动作与管理走 `hc-tui plugins`：

```sh
hc-tui plugins action list [--json]
hc-tui plugins action invoke <id>.<action> '{"short":true}' [--pane t7]
hc-tui plugins action status <request> [--json]
hc-tui plugins action cancel <request>
hc-tui plugins action log <request> [-n 100]
hc-tui plugins list [--json]
hc-tui plugins install <目录> [--yes]
hc-tui plugins update <目录> [--yes]
hc-tui plugins link <目录>
hc-tui plugins unlink <id>
hc-tui plugins uninstall <id>
hc-tui plugins enable|disable <id>
hc-tui plugins pane open <id>.<terminal> [--cwd DIR] [--machine M] [--placement P] [--space spN]
hc-tui plugins pane status <request>
hc-tui plugins pane focus <terminal|id.terminal|pane>
hc-tui plugins pane close <terminal|id.terminal|pane>
```

`action list`、`list`、`install`、`link`、`unlink`、`uninstall`、`enable`、`disable` 不要求窗口；
`action invoke/status/cancel/log` 经本地控制 socket 转给运行中的 TUI（运行状态与进程都在客户端进程内）。插件动作名 `<package-id>.<action-id>`，
内置动作不可被覆盖；禁用或卸载后动作立即从命令目录、命令面板与右键菜单消失。动作在客户端主机运行，
stdin 收到上下文 JSON，超时、取消、进程组回收与并发队列由宿主约束。

事件流读同一个控制 socket：

```sh
hc-tui events [--plugin <id>] [--kinds terminal.status,terminal.turn] [--count N] [--since <rev>]
```

先收到一条 `{"subscribed":true,"queue":256}` 确认，之后按行输出事件 JSON；断开或 Ctrl-C 即停止。
`--plugin` 只看该插件终端的事件，`--kinds` 只看列出的 kind（逗号分隔）。每个订阅者 256 条队列，
溢出时补发 `{"kind":"events.dropped","count":N}`。插件进程可直接使用注入的 `HCTUI_SOCKET`。

每条事件带单调递增的 `revision`；快照（`hc-tui ls` 与 `method = "snapshot"`）同样带 `revision`，
起点取 core `events.subscribe` 的 `seq`，之后每条发布事件 +1。`--since <rev>` 断线续订时先收到一帧
`{"kind":"events.snapshot","revision":R,"resync":false,"snapshot":{…}}`，此后只推 `revision` 比 R 大的
事件（快照已经包含更早的状态，不重发旧号）。`rev` 早于保留的 1024 条、或快于当前 revision 时
首帧标 `"resync":true`，不静默。模块侧 `state.changed` 负载里的 `revision` 是同一把尺子。

`plugins pane` 要求运行中的 TUI。`open` 在 Space 的机器上创建真实终端：`--machine` 缺省用 Space
的机器，`--cwd` 缺省用 Space 目录，`--placement` 覆盖清单里的 placement。`tab` / `right` / `down` /
`float` / `popup` / `zoomed` 分别对应新 Tab、右分屏、下分屏、浮动、覆盖层与放大。包内相对程序
（`./`）在请求远端机器时返回 `unsupported`，不回落本机；`requires_programs` 在目标机器上检查，
缺失返回 `missing_program` 并列出缺失项；机器不可达返回 `offline`；目标 Space 或终端消失返回
`target_gone`。`focus` 与 `close` 可用终端地址、`<id>.<terminal>` 或 Pane 编号定位（按标签查找）；
`close` 只关视图，程序继续运行。

安装与分发走 `hc-tui mods`：

```sh
hc-tui mods list [--json]
hc-tui mods check <path|id>...
hc-tui mods test <path|id>
hc-tui mods build <path|id>
hc-tui mods install <目录或 .nshpkg> [--yes]
hc-tui mods update <id> <目录或 .nshpkg> [--yes]
hc-tui mods uninstall <id>
hc-tui mods link <目录>
hc-tui mods unlink <id>
hc-tui mods log <id> [-n 100]
hc-tui mods open <name> [--place dialog] [--props JSON]
hc-tui mods close <name>
hc-tui mods reload [name]
hc-tui mods grant <id> [--host state] [--events state.changed] [--revoke]
hc-tui mods enable|disable <id>
```

安装写入 `$XDG_STATE_HOME/hc-tui/mods/store/<id>/<generation>/`，`current/<id>` 原子切换；
运行中的表面继续使用已加载的代际，关闭后回收旧代际。`link` 不拷贝，`current` 直接
指向源目录，改动后 `mods reload` 生效。签名包需要 `config.toml` 的 `[mods] trust_roots`，
撤销列表默认读配置目录的 `mods-revocations.json`。

## Layout、外观与应用

| 动作 | 参数与结果 |
|---|---|
| layout.clone | as 指定个人布局名；缺省为 name-copy（name 默认 layout）；拒绝覆盖 |
| layout.set | layout 必填；接受 SavedNode 或 GUI Pane 树，具体结构见 model/shared 实现 |
| layout.select | 当前根为 Stack 时用 child（Pane ID 字符串）或 index 选择项 |
| layout.reset | 当前 Tab 重排为 balanced |
| layout.switch | name 为内置预设或个人布局名；省略则下一个预设 |
| layout.reload | 接受共享文档版本以解除冲突；machine 可选，默认当前工作区机器 |
| layout.show / layout.hide / layout.toggle | 显示 / 隐藏 / 切换侧栏 |
| layout.resize | px 必填；历史参数名，实际单位为**终端列数**，受 min/max 限制 |
| app.palette | 打开工作台命令面板 |
| app.settings | 打开设置；装了 `settings` 模块时开模块浮层，否则用内置浮层 |
| app.help | 打开键位提示 |
| app.theme | name 必填；切换内置主题并写入配置 |
| app.indicators | value 为 `dots` / `symbols`；写 `ui.status_indicators` |
| app.notifications | value 为 `off` / `toast` / `system`；写 `ui.toast.delivery` |
| app.sound | value 为 `true`/`false` 或 `on`/`off`；写 `ui.sound.enabled` |
| app.quit | detach 客户端，程序继续运行 |
| overlay.close | 清除打开的本地浮层；popup 仅关闭视图 |

七个内置预设：balanced、main-left、main-top、columns、rows、stack、main-stack。
layout.set 使用当前布局身份，不是任意 JSON 启动器；SavedNode 路径要求 Pane 属于当前 Tab，
每个 Pane 最多出现一次，未使用的视图会移除。切布局前可用 ls 查看和 layout.clone 保存。
个人布局克隆不是程序的启动模板，不能用来复制一个运行中的 Agent。

```sh
hc-tui do layout.switch '{"name":"columns"}'
hc-tui do layout.clone '{"as":"my-columns"}'
hc-tui do app.theme '{"name":"catppuccin-latte"}'
hc-tui do terminal.park '{"terminal":"peer/t3"}'
```
