# 使用指南

[返回 README](../README.md) · [配置](configuration.md) · [命令](commands.md)

## 工作区、Tab 与 Pane

Space 与 Workspace 指工作区；一个工作区对应机器和目录，下面有多个 Tab。
Tab 内可以有 BSP 分屏、Stack 和浮动 Pane。Pane 可以承载终端或脚本模块。
终端地址（如 `t3`、`peer/t3`）标识 core 中的程序，不等于 TUI 的 Pane ID。

侧栏每个 Space 的第二行显示 Tab 和 Pane 总数，随后是待处理数量（如 `2!`）与 Git 分支。
数量包含 settled Tab、浮动 Pane 和模块，切换工作区前就能了解规模。分支只在本机目录读取。

鼠标点击 Space、Tab 或 Pane 切换位置。`g` 搜索也包含普通终端：已有视图直接跳转，
外部终端按来源工作区独立开 Tab，不拆掉当前分屏。远端地址带机器前缀，同名工作区不合并。

## 键位

前缀键是 `ctrl+b`，按下后单独跟一个字母/符号；连按两次把字面 `ctrl+b` 交给程序。

| 键 | 作用 |
|---|---|
| `q` | 退出 TUI（只断视图，程序继续跑） |
| `:` | 工作台命令面板：搜索命令、编辑 JSON 参数并执行 |
| `?` | Keybinds 浮层：`/` 过滤，`j`/`k`/`PgUp`/`PgDn` 滚动，`Esc` 关闭 |
| `s` | Settings 浮层：theme / indicators / toasts / sound，回车应用并写回配置 |
| `g` | Go to 浮层：搜索工作区与终端，`Enter` 跳到已有视图或新开一个视图 |
| `o` | 跳到最近一次需要处理的 Agent（没有则找下一个待处理） |
| `c` | 新 Tab（默认先问名字） |
| `shift+t` | 命名当前 Tab（留空恢复默认） |
| `n` / `p` | 下一个 / 上一个 Tab |
| `1`–`9` | 切到第 n 个 Tab |
| `shift+x` | 关闭当前 Tab 并结束其终端和子 Agent |
| `shift+n` | 新 Workspace（`prompt_new_workspace_name` 决定是否先问名字） |
| `shift+w` | 命名当前 Workspace |
| `w` | Workspace 选择：`1`–`9` 或 `h`/`l` |
| `shift+d` | 关闭当前 Workspace 并结束其终端和子 Agent（按配置确认） |
| `v` | 左右分屏 |
| `-` | 上下分屏 |
| `h` `j` `k` `l` | 焦点移动到左/下/上/右的 Pane |
| `tab` / `shift+tab` | 在当前 Tab 内循环焦点 |
| `t` | 把当前 Tab 的所有 Pane 收成一个 Stack（标题排在顶部，焦点那个展开） |
| `shift+l` / `shift+j` | 下一个 / 上一个布局预设 |
| `f` | 焦点 Pane 在分屏与浮动层之间切换 |
| `shift+f` | 切换浮动 Pane 的 pin |
| `shift+h` | 收起 / 展开浮动层 |
| `z` | 缩放当前 Pane 占满内容区 |
| `r` 然后 `h j k l` | 调整分隔线比例，其他键退出调整 |
| `x` | 关闭当前 Pane 并结束程序；最后一个 Pane 按确认设置询问 |
| `m` 然后 `1`–`9` / `t` | 把当前 Pane 移到目标 Tab / 移成新 Tab |
| `shift+p` | 命名当前 Pane（写入 `terminal.label`） |
| `shift+k` | 结束当前 Pane 里的程序（需输入 `y` 确认） |
| `i` | 给当前 Pane 的程序发一条消息（`terminal.send`，粘贴 + Enter） |
| `shift+c` | 中断当前轮（`terminal.interrupt`） |
| `shift+o` | 切换 core 的 keep 策略（恢复由 core 负责） |
| `shift+u` | 移除已结束的终端记录（`terminal.rm`） |
| `e` | 把视口 ANSI 交给 `$EDITOR`，在 popup 里打开 |
| `/` | 只读文本浮层：`terminal.read` 的屏幕 + 历史（`j`/`k`/`PgUp`/`PgDn` 滚动，`q` 关闭） |
| `shift+i` | 按名字按键：`terminal.keys`，输入 `enter, ctrl+c` 这类序列 |
| `shift+m` | 机器选择器：`Enter`/`n` 在目标机器开新 Pane，`a` 添加、`c` 连接、`r` 改名、`d` 移除 |
| `shift+v` | 把剪贴板图片交给 `terminal.put`，再把落盘路径贴进程序 |
| `[` | 进入 Copy Mode |
| `a` | 跳到下一个需要处理的 Agent |
| `b` | 显示/隐藏侧栏 |
| `shift+r` | 重新加载配置文件 |

Copy Mode：`h j k l` 移动，`w`/`b` 按词，`0`/`$` 行首行尾，`g`/`G` 视口顶/底，
`Ctrl+u`/`Ctrl+d` 翻页，`v` 起选区，`R` 切换矩形选区，`y` 或 Enter 复制并退出，
`/` 搜索（命中会高亮）、`n` 下一个，`o` 打开光标处的链接，`q`/`Esc` 退出。
复制同时走 OSC 52 与平台剪贴板工具。


### 默认绑定冲突

当前默认值中，`wait_output` 与 `rename_workspace` 都是 `prefix+shift+w`，
`put_file` 与 `rename_pane` 都是 `prefix+shift+p`，`templates` 与 `rename_tab`
都是 `prefix+shift+t`。分发时重命名先匹配，所以另外三个功能需要重新绑定。
不要把同一个按键当作同时执行两个功能。可在配置中改为：

```toml
[keys]
wait_output = "prefix+alt+w"
put_file = "prefix+alt+p"
templates = "prefix+alt+t"
```

## 鼠标与调整布局

需要 `ui.mouse_capture = true`，宿主需要把鼠标事件送到 TUI；按住 Shift 的拖动可能被宿主用于选字。

| 区域 | 操作 |
|---|---|
| 分屏分隔线 | 左键按住拖动改变比例，松开完成；嵌套分屏只调整抓到的分隔线 |
| 侧栏右边界 | 左键拖动改宽度，受最小/最大宽度限制；松开写入配置 |
| 浮动 Pane 标题行 | 左键拖动移动 |
| 浮动 Pane 右下角 | 左键拖动改变宽高 |
| Agent 行 | 左键跳转；右键切换共享置顶 |
| Agent 分组 / 父项 | 点击折叠或展开 |
| agents 标题 | 切换工作区分组 / 优先级分组 |
| 终端内容 | 点选焦点；滚轮翻历史，程序开启鼠标跟踪后转发给程序 |

鼠标拖分隔线不会把拖动当作文本选区，也不会发给 Pane 内程序。浮层和浮动窗口优先接收自己的鼠标。
zoom、手机单 Pane 和隐藏边框的区域没有可拖动的分屏线；可用前缀 `r` 加 `h j k l` 调整。
`pane_gaps = false` 时相邻 Pane 保留单条共享边界。分屏比例通过共享布局保存，侧栏宽度是本机配置。
`copy_on_select = true` 时拖选复制；默认关闭，文本可用 Copy Mode 或宿主 Shift 选字。

## Agents 与注意力

Agents 仅展示 core 上报过状态的会话；从未上报状态的 shell 不进入列表。
标题、程序名、目录、置顶或存在 Pane 都不能单独证明它是 Agent。
Agent 状态清空后退出列表；普通终端仍在工作区与 Go to 搜索中。

| 状态 | 含义 |
|---|---|
| blocked / error | 等待处理或错误，需要注意 |
| done | 有尚未读过的完成轮次 |
| working | 正在工作 |
| idle | 空闲；已读的 done 也归入这一状态 |
| unknown | 上报了当前不认识的状态词，保留展示 |

工作区分组使用机器与稳定 Space ID，同名或重命名不会混淆折叠状态。
优先级为 Pinned → Needs you → Done → Working → Idle；父项按子树最紧急状态排序。
`agent.next` / `agent.prev` 按实际显示顺序循环，折叠内容不混入导航。
查看 Agent 会更新共享 `herdr.seen`；Done 在当前焦点停留期间保持位置，离开后再变化。
置顶使用 `herdr.pinned`，与 GUI 共享；本地已读和分组折叠也写入 state.json。

## 关闭、退出与恢复

关闭 Pane、Tab 或 Space 会结束对应程序及其子 Agent。Tab、Space 和最后一个 Pane
按 `confirm_close` 决定是否询问；普通多分屏里关闭一个 Pane 不会逐个询问。

程序正常退出后自动删除对应 Pane，折叠分屏，空 Tab/Space 一并清理。
后台 Tab 退出不会把当前焦点切走。所有 Pane 退出后留在空工作台，可再次新建，不自动启动替代 shell。
连接中断、core 重连、attach 失败保留错误视图，不按正常退出处理。

想保留程序但不占位置，用 `terminal.park`；它同时禁用 keep。想继续管理它，用
`terminal.keep` 或 Go to 搜索重新打开。想暂时藏起整组任务，用 `tab.settle`；
再次选择、`tab.unsettle` 或注意力事件可恢复。退出 TUI 用前缀 `q`，只 detach。

新建普通终端启用 core keep，临时命令和 popup 不启用。keep 的具体恢复由 core 负责，
不是 TUI 把丢失程序替换为一个无关 shell。外部会话续接仍依赖 Agent 自己的机制。

## 多机与共享布局

前缀 `Shift+M` 打开机器选择器：Enter / n 在目标机器开 Pane，a 添加、c 连接、r 改名、d 移除。
已连接机器自动导入布局；未连接或空机器在侧栏提供连接、新建入口。
新建 Space、Tab、分屏和脚本命令沿用工作区机器、目录。

结构使用每台机器的 `herdr-app` 文档和 CAS 写入，名称、树、顺序、终端和模块 props 双向同步。
焦点、zoom、settled 等屏幕状态在 TUI 自己的 `hctui-layout-v1` schema 3 中保存。
并发修改不同字段时合并；同字段冲突暂停自动覆盖并提示，执行 `layout.reload`
接受共享版本。详见 [对齐契约](herdr-app-alignment.md)。

## 文件、变更与脚本面板

`files.open`、`changes.open`、`file.open` 是当前工作区机器/目录的只读浮层入口。
文件列表以 Git 为基础，不能当作完整文件管理器或编辑器；`changes.open` 读取相对 HEAD 的 diff。

`mod.open {"name":"files"}` 和 `mod.open {"name":"changes"}` 则在 Tab 中创建定时刷新模块。
内置 files 模块在非 Git 目录回落 ls；changes 模块展示 status、未暂存和已暂存 diff。
自定义 `[[panels]]` 也走这个入口，可输出 ANSI，props 由 `HC_TUI_PANEL_PROPS` 提供。
GUI 的 GPUI JS 模块不会直接在终端执行；未知模块保留结构及 props，并提示配置脚本适配。

## 图片与颜色

Kitty / Ghostty / Herdr / WezTerm 宿主自动选原生 Kitty 像素输出；tmux 默认半块字符预览。
可用 `HC_TUI_GRAPHICS=kitty` 或 `HC_TUI_GRAPHICS=blocks` 强制选择。
支持 PNG/RGB/RGBA、源裁切、Unicode 虚拟 placement、透明度、层级和浮层遮挡；切换、关闭及退出清理图像。
没有 Sixel / iTerm 协议支持；真实宿主窗口视觉验收范围见对齐契约。

外部 UI 与 Pane 默认前景、背景以及 ANSI 0–15 色使用当前主题，索引色先转为明确 RGB，
不再依赖宿主的另一套调色板。ANSI 16–255 的标准色立方保留，显式 24 位真彩色和程序 OSC
改色尊重程序；因此自行设置主题的 nvim / Agent 不会被强行统一着色。

Ctrl+V 在本机剪贴板有 PNG 图片时，通过 core 上传到目标终端所在机器，并将文件路径粘贴给发起上传的 Pane；上传期间切换 Pane 不会改变目标。无图片时 Ctrl+V 仍交给终端程序。
也可用前缀 Shift+V 或命令 `terminal.paste_image` 显式粘贴，失败时显示原因。
宿主的 Ctrl+Shift+V 通常只发送文本粘贴事件，无法保证传入图片；这时请用上述按键。
依赖 TUI 所在机器的 wl-paste / xclip / pngpaste；通过 SSH 运行时读取的是远端剪贴板。

## 手机与窄屏

宽度不超过 `mobile_width_threshold`（默认 64）且鼠标捕获开启时使用单 Pane 布局。
顶栏显示连接、工作区与 Tab 信息，switch 打开工作区、Tab、Agents 与动作菜单。
工作区下同样显示 Tab/Pane 数量，Agents 使用相同过滤规则。点击目标后关闭面板。
其余 Pane 继续 attach；内容单指拖动翻历史，选字用 Copy Mode；面板滚动一次两行。
设 `mobile_width_threshold = 0` 关闭自动窄屏布局。

## Chat 视图

`prefix+shift+a`（手机上在 `switch` 面板里点 `chat 视图`）把当前 Pane 的 pi 会话
渲染成一列消息：正文按当前宽度重折、思考内容折成两行、工具调用收成一行（`Enter`
或点击展开结果）。Chat 只是同一份 transcript 的另一种渲染，Pane 的终端照常 attach，
`Ctrl+O` 随时退回终端，两边不会分叉。

数据直接读 pi 自己写的会话文件 `~/.pi/agent/sessions/--<cwd>--/<时间>_<id>.jsonl`
（`PI_CODING_AGENT_SESSION_DIR` 可改根目录），沿 `parentId` 只走当前叶子，所以
`/tree`、`/fork` 之后被放弃的分支不会出现在视图里。`/new`、`/resume` 换了会话文件时
会自动跟过去。

| 按键 | 作用 |
|---|---|
| `Enter` | 发送输入行；输入为空时展开/收起选中的工具卡 |
| `Ctrl+C` | 给 Pane 发 `Ctrl+C` |
| `Ctrl+N` | 发送 `/new` |
| `Ctrl+P` | 命令面板 |
| `Ctrl+O` | 退回终端视图 |
| `Ctrl+Y` | 复制最后一条回复 |
| `Ctrl+U` / `Ctrl+E` | 清空输入 / 回到底部 |
| `↑` `↓` | 输入为空时在块之间移动；有输入时翻输入历史 |
| `PgUp` `PgDn` | 翻消息 |
| `Esc` | 关命令面板，或退出 Chat |

输入行写回 Pane 的 attach 流（文本 + 回车），所以 pi 自己的权限、模型、扩展与项目
上下文都照原样生效。输入 `/` 打开命令面板：pi 的内置斜杠命令，加上 `~/.pi/agent/prompts`
里的模板与各标准位置发现的技能；标了 `终端` 的命令（设置、会话选择器、登录等）会先
关掉 Chat 再发，因为它们的浮层只有终端能画。

### 会话映射

同一目录可能有多个 pi 会话，按「最近写入」挑文件会挑错。hc 不提供 agent 会话的接口，
所以让 agent 自己报：命令面板里的 `安装会话映射 hook`（标 `本地`）会在
`<agent-dir>/extensions/hc-tui-session.ts` 写一个 pi 扩展（`<agent-dir>` 默认
`~/.pi/agent`，可用 `PI_CODING_AGENT_DIR` 改），它每次开轮都把会话文件路径报到
`hc label <t> session=<path>`。hc-tui 读到这个 label 就用它，顶栏标明“本机映射”。

仅本机会话在没装 hook 时按目录里最新的 `.jsonl` 推断，顶栏标 `· 本机推断`。两者都可用；装了之后
同一目录多个活跃会话也不会串。删掉扩展就是面板里那条 `移除会话映射 hook`。

手机上除了快捷键，Chat 底部还有一条 `发送 / 中断 / 命令 / 终端` 动作条。
Agent 处于 `blocked` 或 `error` 时顶栏变红，提示需要回终端处理。

找不到会话文件、或 Pane 的 cwd 对不上时，顶栏写明原因，其它视图不受影响。


## 常见问题

- **Agents 没有我的 shell？** 这是预期行为。只有上报状态的会话进入 Agents；用 Space 或 Go to 找普通终端。
- **为什么 Pane 的某些颜色仍与 UI 不一样？** 显式真彩色和程序 OSC 改色属于程序；检查程序自己的主题。
- **无法拖动分隔线？** 检查 mouse_capture、宿主鼠标转发、是否按住 Shift，以及是否处于 zoom/手机/浮层状态。
- **`hc-tui ls/do` 连接不上？** 要先运行同一 session 的 TUI，并保持同一 XDG_STATE_HOME；自定义 session 要每次传入。
- **看到 another TUI already accepts commands？** 同一状态目录和 session 只允许一个控制 socket 服务；第二个客户端可能仍可打开，但 CLI 控制对应首个服务。
- **英文输入或滚动时疑似重复？** 本版加入真实后端回放和整帧同步输出，尚未稳定复现该报告。区分程序输出实际重复与显示暂留；保留程序名、宿主和复现步骤便于继续定位。
- **退出再进布局变了？** 检查 session、restore 配置、core 文档和是否同时有 GUI 写入；同字段冲突用 layout.reload 明确接受共享版本。

## 侧栏导航

点击机器标题左侧的 ▾ / ▸ 或整行标题，可折叠/展开该机器的全部 Spaces，状态保存在本地并在重启后恢复。
Spaces 与 Agents 分区独立滚动，将鼠标移到相应区域滚动即可。空间以单行显示，彩色首字母标识和整行背景突出当前空间；分支在有足够宽度时显示，Tab/Pane 数量仍可在详细切换视图查看。

### 远端 Pi Chat

`machine/t7` 的 Chat 通过现有 core 的 `file.read {on: machine}` 异步、按行增量读取 session label 指定的路径，不直接打开本机同名文件。上限保留最近 6000 行；读取失败保留已加载内容并明确标明失败；半行 JSONL 等写完再显示，会话切换后丢弃旧请求的回包。

在 Pi 实际运行的机器执行 `hc-tui session-hook install`，然后在 Pi `/reload`。Pi 必须运行在 core 管理的终端内。也可以用 `hc label <terminal> session=/absolute/path.jsonl` 显式映射。
远端没有 label 时显示“远端会话未映射”，不会推断本机路径；无法读取时显示“远端读取失败”。Chat 面板里的本地 hook 操作不会写到错误机器。
`ssh box 'hc-tui --session work'` 仍读取 box 上的同一 core/session 文档；直接用 `hc-tui --session work` 则通过该 core 已配置的 machine 链路汇总远端工作区。

远端 Chat 的命令面板只列内置 Pi 命令，不把本机的 prompt/skill 扫描结果冒充远端可用命令。
