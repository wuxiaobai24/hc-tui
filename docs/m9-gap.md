# M9c 缺口调研：picker / dialogs / settings / help / toast

- 票据：`m9gap`，工作区 `/home/wxb24/code/hc-tui-m9gap`
- 基线：`fc29a85c9ffba62ed0fe0ae26e9ee7680a354721`；`cargo test -p hc-tui` 全绿（输出留档在票据目录 `evidence/01-baseline-cargo-test.txt`）
- 事实来源：快照 `src/app/mods.rs:517`、域白名单 `src/app/mods.rs:641`、方法清单 `src/mods/host.rs:17`、文档表 `docs/mods.md:310`、事件清单 `src/mods/mod.rs:14`；区域实现分布在 `src/app.rs`、`src/app/interaction.rs`、`src/app/actions.rs`、`src/app/form.rs`、`src/ui/mod.rs`
- 本文只列缺口，文档承诺与代码不符的位置单列为证据

## 0 底座现状

### 0.1 现有宿主方法

方法清单 `src/mods/host.rs:17-33`，分发表 `src/app/mods.rs:66-374`，授予名与文档 `docs/mods.md:310-325`。

| 方法 | 实现位置 | 五块里谁需要 | 判定 |
|---|---|---|---|
| `state` | `src/app/mods.rs:134-140` | 五块全部 | 域不够，见 0.2 |
| `run` | `src/app/mods.rs:146-178` | picker 激活、dialogs 提交、settings 应用 | 语义不足，见 G7 |
| `request` | `src/app/mods.rs:179-195` | 无 | 够用 |
| `exec` | `src/app/mods.rs:196-218` | 无 | 够用 |
| `readFile` / `writeFile` | `src/app/mods.rs:219-284` | dialogs 的「发送文件」需要读本地文件 | `readFile` 返回文本，二进制文件无法原样带回，见 2.3 |
| `notify` | `src/app/mods.rs:285-296` | toast | `level` 被丢弃，见 5.2 |
| `theme` | `src/app/mods.rs:141-145` | settings、help | 只含当前主题，见 3.2 |
| `overlay` | `src/app/mods.rs:297-324` | picker、dialogs、settings、help | 缺打开参数与非聚焦形态，见 5.4 |
| `closeOverlay` | `src/app/mods.rs:325-342` | 四块模态 | 够用 |
| `focus` | `src/app/mods.rs:343-356` | 四块模态 | 够用；`mods.open` 已自动抢焦点，见 0.4 |
| `surface` | `src/app/mods.rs:77-90` | 五块 | 够用（cols/rows/place） |
| `setAgentOrder` | `src/app/mods.rs:357-373` | 无 | 够用 |
| `requestGrants` | `src/app/mods.rs:91-120` | 无 | 够用 |

启动参数 `src/mods/host.rs:124-144` 提供 `props`、`space`、`tab`、`pane`、`cell`、`locale`、`theme`，实际赋值在 `src/app/mods.rs:760-789`。配置项（indicators、通知投递、sound、键位）不在参数里。

### 0.2 快照域与实际内容

| 域 | 内容 | 五块需要的补充 | 位置 |
|---|---|---|---|
| `machines` | 工作区出现过的机器名，排序去重 | 缺 `state`/`target`/`host`，与 `App::machines` 不同 | `src/app/mods.rs:530-538`；`src/app.rs:292-300` |
| `spaces` | id/name/machine/folder/tabs 数 | 够 help/picker 用 | `src/app/mods.rs:540-555` |
| `tabs` | id/space/name/settled | 够 | `src/app/mods.rs:556-572` |
| `panes` | id/terminal/title，上限 512 | 缺所属 space/tab、label、cwd、live、焦点以外的行信息 | `src/app/mods.rs:573-587`；`src/model.rs:213-222` |
| `agents` | address/name/title/cwd/turn/pinned | 缺 badge 与 live 状态 | `src/app/mods.rs:588-604` |
| `terminals` | address/title/cwd/seq | 缺 live/finished（goto 右列要用） | `src/app/mods.rs:605-619`；`src/app.rs:3466-3471` |
| `focused` | space/tab/pane/mod | 够 | `src/app/mods.rs:620-633` |
| `theme` | name/tokens/palette | 够 | `src/app/mods.rs:634-637`；`src/app/mods.rs:670-682` |

`state` 的 `space?` 参数在文档里写了（`docs/mods.md:312`），代码只读 `domains`（`src/app/mods.rs:134-140`、`src/app/mods.rs:641-663`）。`state.changed` 每次固定广播 7 个域且不含 `theme`（`src/app/mods.rs:1105-1116`）。事件清单 12 个（`src/mods/mod.rs:14-27`），其中 `command` 没有任何发送点：`emit_mod_event`/`send_mod_event` 的调用点里没有它（`src/app/mods.rs:172`、`src/app/mods.rs:1064`、`src/app/mods.rs:1070`、`src/app/mods.rs:1089`、`src/app/mods.rs:1114`、`src/app/mods.rs:1498`、`src/app/mods.rs:1586`、`src/app/actions.rs:815`），与 `docs/mods.md:236` 的投递承诺不符。

### 0.3 输入与绘制顺序

- 按键、鼠标、粘贴翻译：`src/mods/input.rs:7-57`；只有 `Key`/`KeyRepeat`/`Paste`/`Focus`/`Mouse` 五类。
- 路由：`route_key_to_mod` `src/app/mods.rs:1126-1146`、`route_paste_to_mod` `src/app/mods.rs:1148-1164`、`route_mouse_to_mod` `src/app/mods.rs:1166-1224`。
- Rust 模态优先于模块表面：`src/app.rs:1395-1417`；粘贴也先给 Rust 模态（`src/app.rs:1549-1594`）。
- `Esc` 在焦点位于模块表面时先尝试关闭顶层覆盖层（`src/app.rs:1470-1477`，`src/app/mods.rs:1236-1262`）。
- 绘制顺序：模块表面 `src/ui/mod.rs:104` → Rust toast `src/ui/mod.rs:105` → Rust 模态 `src/ui/mod.rs:112`；表内 z 序 `src/mods/render.rs:89-97`；`dialog` 先清空整个内容区 `src/mods/render.rs:51-54`。
- `mods.open` 打开后无条件夺取键盘焦点（`src/app/mods.rs:814`）。

### 0.4 表面形态

`SurfacePlace` 五种：`region`、`pane`、`dialog`、`popup`、`window`（`src/mods/mod.rs:55-93`）。矩形计算 `src/app/mods.rs:974-1010`，绘制 `src/mods/render.rs:24-87`。外框内置区域只接受 `spaces`/`tabs`/`status`（`src/mods/chrome.rs:474-481`、`src/mods/chrome.rs:531-541`），默认树 `src/mods/chrome.rs:219-245`。`toast` 作为内置区域名被测试明确拒绝（`src/mods/chrome.rs:853-859`）。

## 1 picker

### 1.1 它现在渲染什么

两套形态。

**Chooser（不可过滤）**：结构 `src/app.rs:278-291`，行为 `on_chooser_key` `src/app.rs:5274-5349`、`activate_chooser` `src/app.rs:5351-5381`；绘制 `src/ui/mod.rs:2240-2385`（宽 64 居中，行高 1 或 2，Machines 底部两排按钮，超出按 `selected` 反推起点）。

| 来源 | 构造位置 | 数据来源 | 专属交互 |
|---|---|---|---|
| 动作菜单 | `src/app/interaction.rs:36-60` | 硬编码动作组 + `binding_title` | 子菜单内联、`Back` 行（`src/app/interaction.rs:62-155`） |
| 工作区选择 | `src/app/interaction.rs:178-186` | `workspaces` | 数字键/回车 |
| 移动 Pane | `src/app/interaction.rs:187-196` | 当前工作区 tabs | 同上 |
| Resize | `src/app/interaction.rs:197-206` | 固定四项 | 选择后继续弹 |
| Swap Pane | `src/app/interaction.rs:141-171` | `tab.tree.panes()` | 同上 |
| 终端菜单 | `src/app/actions.rs:1096-1133` | 固定六项 + 插件菜单 | `ChooserKind::Commands`，值为 `动作名 + JSON` |
| 插件菜单 | `src/app/plugin.rs:122-132` | `plugins.actions()` 按 context 过滤 | 追加进终端菜单 |
| 模板 | `src/app.rs:5832-5866` | core `templates` RPC 结果 | `ChooserKind::Templates` |
| 机器 | `src/app.rs:5921-5939` | core `machine.list` 结果 | `a`/`c`/`r`/`d`/`n`（`src/app.rs:5306-5349`） |
| 授予确认 | `src/app/mods.rs:1451-1475` | `pending_grants` | 批准单条或全部（`src/app/mods.rs:1476-1500`） |

键盘：`Esc`/`q` 关闭、`j`/`k`/方向键循环、`1`-`9` 选中并立即激活（`src/app.rs:5280-5305`）。鼠标：点击行先设选中，除 Machines 外的种类立刻激活（`src/app/interaction.rs:551-562`）；滚轮被翻译成上/下键（`src/app/interaction.rs:281-291`）；粘贴被吞掉（`src/app.rs:1586-1596`）。

**Goto/Palette（可过滤）**：结构 `src/app.rs:250-268`，行构造 `goto_rows_for` `src/app.rs:3328-3491`，键盘 `on_goto_key` `src/app.rs:3493-3561`，绘制 `src/ui/mod.rs:2001-2130`（占满内容区并清屏，标题右侧计数，底部两行显示选中项上下文与 cwd）。

- `goto` 形态：工作区标题行 + 其下 Pane 行（`src/app.rs:3379-3443`），再加「其它客户端的终端」分组（`src/app.rs:3445-3490`）；过滤同时匹配名称与终端地址（`src/app.rs:3333-3340`）。
- `palette` 形态：`actions::COMMANDS`（`src/app/actions.rs:6-100`）加插件 `global` 动作（`src/app.rs:3329-3377`），选中后把 `命令名 {JSON}` 交给 `execute_action`（`src/app.rs:3510-3522`）。

### 1.2 迁移需要哪些 props

现有域不够。

| 需要的数据 | 现状 | 结论 |
|---|---|---|
| 统一候选行结构 | `ChooserItem { label, value }`，`value` 是 `group:`/`binding:`/`action:`/`custom:`/`swap:`/`move:`/`resize:`/`workspace:`/`preset:` 混合编码（`src/app.rs:278-282`、`src/app/interaction.rs:62-260`） | 模块无法解释，必须由宿主归一为 `{id, label, section?, right?, context?, cwd?, action: {name, args}, enabled}` |
| 命令目录 | `actions::COMMANDS` 是编译期表（`src/app/actions.rs:6-100`），快照没有 | 新增快照域 `commands` |
| 插件动作 | `ActionInfo` 含 name/title/description/contexts/args（`src/plugin/registry.rs:169-192`），快照没有 | 新增快照域或在 `commands` 里带 `package`、`contexts`、`args` |
| 机器状态与目标 | 快照 `machines` 只有机器名（`src/app/mods.rs:530-538`）；真实机器表在 `App::machines`（`src/app.rs:292-300`，来源 `src/app.rs:5921-5939`） | 快照 `machines` 换成对象数组，或新增 `machineInfo` 域 |
| 模板列表 | core RPC 结果，仅落进 Chooser（`src/app.rs:5832-5866`） | 新增快照域或 props 直接带候选 |
| Pane 归属与显示名 | 快照 `panes` 缺 space/tab/label/cwd/live（`src/app/mods.rs:573-587`；显示名规则 `src/model.rs:213-222`） | 扩 `panes` 字段 |
| 行分组、右列、底部上下文 | 只在 `GotoRow` 里（`src/app.rs:257-268`、`src/app.rs:3400-3443`） | 由宿主在 props 里算好 |
| 标题、提示、按钮文案 | 宿主侧 `t()` 拼在 title 与按钮里（`src/app.rs:5934-5939`、`src/ui/mod.rs:2327-2364`） | props 带已本地化文案，模块只翻自身静态串 |
| 当前选中与滚动 | 宿主字段（`src/app.rs:284-290`） | 模块自持 |

即时过滤不需要宿主参与：过滤规则是本地 `contains`（`src/app.rs:3146-3172`、`src/app.rs:3333-3340`），模块拿到全量候选即可。

### 1.3 迁移需要哪些动作

现有 `run` 覆盖 `binding:`/`action:`/`custom:` 三条路径的全部落点，因为 `activate_chooser` 最终都进 `execute_action` 或 `activate_pointer_action`（`src/app.rs:5359-5379`、`src/app/interaction.rs:62-260`）。缺三处。

1. `machine.rename` 没有动作。机器面板的 `r` 直接发 core RPC（`src/app.rs:5332-5338`），`COMMANDS` 里没有这一项（`src/app/actions.rs:50-53`、`src/app/actions.rs:551-559`）。补动作即可。
2. 列表类数据没有回读通道。`machine.list` 动作只触发 RPC 并把结果写进 `self.machines`（`src/app/actions.rs:551`、`src/app.rs:5933-5939`），`run` 只返回 `{request, accepted, confirmation_pending}`（`src/app/mods.rs:162-167`），模块拿不到列表。只能靠 props 或新域提供。
3. 打开与关闭的动作入口。`mods.open` 动作已能带 `props`、`place`、`size`、`at`、`key`（`src/app/actions.rs:744-781`），打开后自动获得焦点（`src/app/mods.rs:814`），关闭用 `closeOverlay`（`src/app/mods.rs:325-342`）。这一条够用。默认按键仍然指向 Rust 实现：`keys.goto`（`src/config.rs:402`、`src/app/bindings.rs:86-94`）、`app.palette`（`src/app/actions.rs:667`）。

### 1.4 难点判断

即时过滤、选中与滚动都在模块内完成，过滤语义简单（`src/app.rs:3146-3172`）。真正的难点是把五种来源的 `value` 字符串协议归一成结构化动作（`src/app/interaction.rs:62-260` 与 `src/app/actions.rs:1096-1133` 两套编码并存），以及补齐 `panes` 的归属字段供分组与面包屑使用（`src/app/mods.rs:573-587`）。排第三，见 §7。

## 2 dialogs

### 2.1 它现在渲染什么

**Prompt（单行输入）**：结构 `src/app.rs:191-201`，16 种语义 `src/app.rs:139-158`，标题映射 `src/app.rs:158-176`，确认语义 `src/app.rs:181-187`。

- 输入：光标为字节偏移、`Backspace`/`Delete`/左右/Home/End、`Ctrl+C` 清空、上下翻历史、历史上限 50 条且去重相邻重复（`src/app.rs:2715-2795`）。
- 确认：`CloseTab`/`CloseWorkspace`/`KillPane` 需要输入 `y` 再回车，否则按取消处理（`src/app.rs:3770-3775`）。
- 提交：按语义直接改 `App` 或发 core RPC（`src/app.rs:3768-3919`）。
- 绘制：宽 56、高 7 居中，标题、说明、输入行、页脚、`Save`/`Confirm` 与 `Cancel` 两个按钮、右上关闭按钮（`src/ui/mod.rs:1652-1741`）。
- 鼠标：按钮映射 `Confirm`/`Close`（`src/app/interaction.rs:545-551`）；滚轮在 `prompt` 打开时被忽略（`src/app/interaction.rs:290-292`）；粘贴按字段插入并把换行换成空格（`src/app.rs:1550-1557`）。

**ActionForm（参数表单）**：结构 `src/app/form.rs:72-95`，控件类型 `src/app/form.rs:37-54`。

- 行表由 schema 递归展开，含分组、文本、枚举、布尔、数组项与「添加一项」（`src/app/form.rs:143-206`）。
- 枚举菜单单独一层（`src/app/form.rs:363-397`、`src/ui/mod.rs:1607-1650`）。
- 键盘：`Tab`/`Shift+Tab` 走焦点、`Enter` 在添加项行追加、在枚举行开菜单、其余提交；`Ctrl+A`/`Ctrl+D` 增删数组项（`src/app/form.rs:611-676`）。
- 校验：提交时用 `manifest::validate_args` 填默认值并校验，错误贴到对应行并聚焦（`src/app/form.rs:678-717`、`src/app/form.rs:236-256`、`src/app/form.rs:539-543`）。
- 目标冻结：打开时取 `_target`，提交时原样带回（`src/app/form.rs:597-601`、`src/app/form.rs:706-712`）。
- 绘制：宽 64 居中、高度随行数、字段区滚动、标题带动作标识（`src/ui/mod.rs:1413-1500`）。

表单只为插件动作打开：`open_plugin_form` 要求动作存在且带 `args` schema（`src/app/form.rs:586-609`）。

### 2.2 迁移需要哪些 props

| 需要的数据 | 现状 | 结论 |
|---|---|---|
| Prompt 的标题、说明、初值、光标、确认语义 | 宿主字段（`src/app.rs:191-201`、`src/app.rs:181-187`） | 打开参数带齐 |
| 字段类型与校验状态 | 只有纯文本（`src/app.rs:191-201`） | 新增 `fields` 描述与 `error`/`error_field` |
| 输入历史 | 宿主内存 50 条（`src/app.rs:550`、`src/app.rs:2731-2740`） | props 带或模块自持；跨会话都不持久 |
| 动作参数 schema | `ActionInfo.args`（`src/plugin/registry.rs:182`），快照与 `theme`/`state` 都不含 | 新增 props 或宿主校验方法，见 G1 |
| schema 展开后的行表 | `ActionForm::rows()`（`src/app/form.rs:121`） | 模块可自行展开，前提是 schema 完整 |
| `_target` | 冻结在表单里（`src/app/form.rs:597-601`） | props 带上并在 `run` 参数里带回（`src/app/plugin.rs:142` 会剥离） |
| 校验错误路径格式 | `display_path`（`src/app/form.rs:539-543`）、`focus_error`（`src/app/form.rs:236-256`） | 需要宿主给出同形态的 `{field, message}` |

### 2.3 迁移需要哪些动作

16 种 Prompt 语义里 11 种有对应动作，5 种没有。

| Prompt 语义 | 现有通道 | 动作覆盖 |
|---|---|---|
| `Command` | `src/app.rs:3776-3785` | `run` 直接带参数 |
| `NewTab` | `src/app.rs:3787` | `tab.new`（`src/app/actions.rs:284`） |
| `NewWorkspace` | `src/app.rs:3788` | `space.new`（`src/app/actions.rs:190`） |
| `RenameTab` | `src/app.rs:3789-3799` | `tab.rename`（`src/app/actions.rs:327`） |
| `RenameWorkspace` | `src/app.rs:3800-3805` | `space.rename`（`src/app/actions.rs:242`） |
| `RenamePane` | `src/app.rs:3806-3810` | `pane.rename`（`src/app/actions.rs:398`） |
| `RenameAgent` | `src/app.rs:3811-3815` | `agent.rename`（`src/app/actions.rs:418`） |
| `SendText` | `src/app.rs:3816-3831`，core `terminal.send`（粘贴 + 回车） | 无动作 |
| `SendKeys` | `src/app.rs:3832-3850`，core `terminal.keys` | 无动作 |
| `WaitOutput` | `src/app.rs:3851-3864`，core `terminal.wait` | 无动作 |
| `PutFile` | `src/app.rs:3865-3892`，本地读文件后 core `terminal.put` | 无动作；`readFile` 返回文本，二进制与 base64 不可用（`src/app/mods.rs:219-248`） |
| `AddMachine` | `src/app.rs:3893-3896` | `machine.add`（`src/app/actions.rs:552`） |
| `RenameMachine` | `src/app.rs:3897-3903`，core `machine.rename` | 无动作 |
| `CloseTab` | `src/app.rs:3904` | `tab.close`（`src/app/actions.rs:317`） |
| `CloseWorkspace` | `src/app.rs:3905` | `space.close`（`src/app/actions.rs:232`） |
| `KillPane` | `src/app.rs:3906-3918` | `terminal.kill`（`src/app/actions.rs:449`） |

`run` 的三处语义缺口（细节见 G7）：

- `confirmation_pending` 恒为 `false`（`src/app/mods.rs:166`），即使 `execute_action` 弹出了确认框（`src/app/actions.rs:232-241`、`src/app/actions.rs:317-326`）。
- 返回即 `done`（`src/app/mods.rs:146-178`），异步 RPC 失败不能回传。
- 表单不会自动打开：`execute_action` 遇插件动作缺参数直接校验失败（`src/app/plugin.rs:143-145`），打开表单的分支只在 Chooser 激活时存在（`src/app.rs:3504-3516`）。

结果回传通道不存在：计划里承诺 `overlay.result` 返回选择或输入、取消返回 `cancelled`（`docs/plan-extensions.md:159`），`METHODS` 与分发表都没有对应项（`src/mods/host.rs:17-33`、`src/app/mods.rs:66-374`），`EVENTS` 也没有（`src/mods/mod.rs:14-27`）。宿主发起的输入（默认键位）目前可以靠模块自己 `run` 终结，模块之间的调用无法闭环。

### 2.4 难点判断

最麻烦的两件事在这里：schema 校验语义要与宿主一致（`src/app/form.rs:678-717` 复用 `manifest::validate_args`），输入类控件的焦点、粘贴替换换行、枚举菜单、数组增删都要在模块内重建（`src/app/form.rs:363-464`、`src/app/form.rs:611-676`）。排第四，见 §7。

## 3 settings

### 3.1 它现在渲染什么

- 状态：`SettingsOverlay { section, selected }`（`src/app.rs:243-248`）。
- 段固定四项 `theme`/`indicators`/`toasts`/`sound`（`src/app.rs:3212`）。
- 每段项目数由代码写死（`src/app.rs:3215-3223`），打开时选中项落在当前值上（`src/app.rs:3225-3239`）。
- 键盘：`Esc`/`q` 关闭、`Tab`/左右换段、`j`/`k` 移动、`Enter` 应用并写配置（`src/app.rs:3241-3277`）。
- 应用：改内存配置并调用 `config::write_setting` 持久化（`src/app.rs:3279-3325`、`src/config.rs:1277`）。
- 绘制：宽 `min(60, content-4)`、高随项目数居中，段标题在窄屏折两行，项目带 `▸` 与 `✓`，底部按键提示（`src/ui/mod.rs:1840-1967`）；项目文案与生效状态 `src/ui/mod.rs:1968-1999`。
- 鼠标：段标题与项目都可点，点项目等于选中并回车（`src/app/interaction.rs:568-580`）；粘贴被吞掉（`src/app.rs:1586-1596`）。

### 3.2 迁移需要哪些 props

| 需要的数据 | 现状 | 结论 |
|---|---|---|
| 段列表与每段项目 | 常量与函数代码（`src/app.rs:3212-3239`） | 新增快照域 `settings`，含 `{section, items: [{value, label, active}]}` |
| 当前值 | `config` 字段（`src/app.rs:3225-3239`） | 同上 |
| 主题候选 | `crate::theme::names()`（`src/theme.rs:552`） | 放进 `settings` 段项目 |
| 主题名称 | `theme` 域（`src/app/mods.rs:634-637`） | 够用 |
| 通知投递三态 | `Delivery`（`src/config.rs:103-111`） | 放进 `settings` |
| 状态图标与提示音 | `config.indicators`/`config.sound`（`src/config.rs:707-709`） | 放进 `settings` |
| 持久化结果提示 | `self.note`（`src/app.rs:3311-3317`） | 模块不可写，见 G6 |

### 3.3 迁移需要哪些动作

- 主题有动作：`app.theme {name}`（`src/app/actions.rs:681-694`），并且它是唯一会发 `theme.changed` 与递增 revision 的路径（`src/app/mods.rs:1068-1072`）。
- 状态图标、通知投递、提示音没有动作；`COMMANDS` 里也没有写配置的通用动作（`src/app/actions.rs:6-100`）。三个设置项要各补一个动作，或补一个受限的 `config.set {key, value}`。
- `app.settings` 动作打开的是 Rust 浮层（`src/app/actions.rs:668-673`），`keys.settings` 走命名绑定（`src/app/bindings.rs:79-85`、`src/app.rs:2422-2425`）。迁移后这两处入口都要改向模块。
- 主题切换的不一致要先修：`apply_setting` 改主题时不发 `theme.changed`、不递增 revision（`src/app.rs:3279-3325` 与 `src/app/actions.rs:685` 对比）。模块若用 `app.theme` 没有这个问题，但同一份设置面板混用两条路径就会漏事件。

### 3.4 难点判断

四处固定段与少量项目，渲染和交互都简单（`src/ui/mod.rs:1840-1967`）。缺口集中在三个设置动作与写配置动作，属补动作即可。排第二，见 §7。

## 4 help

### 4.1 它现在渲染什么

- 状态：`HelpOverlay { query, selected, scroll }`（`src/app.rs:227-232`）。
- 行数据全由宿主拼：`help_rows`（`src/app.rs:3007-3128`），五组键位（global/navigation/panes/tabs and workspaces/agents and terminals）加一段硬编码的 copy-mode 行（`src/app.rs:3112-3132`）。
- 键位来源 `config.keys.entries()`（`src/config.rs:527`），按键串由 `Binding` 的 `Display` 输出（`src/config.rs:249-300`），标题来自 `binding_title`（`src/app/bindings.rs:6-66`）。
- 过滤：`/` 直接进入输入态（没有独立标志），大小写不敏感地匹配按键串与标题，命中时插入一个 `matches` 分组（`src/app.rs:3146-3172`）。
- 键盘：`j`/`k`/方向键/`PageUp`/`PageDown` 移动，`Backspace` 退格并重置，`Esc`/`Enter` 关闭，查询为空时 `q` 也关闭（`src/app.rs:3173-3213`）。
- 绘制：占满内容区并清屏，标题行、查询行、分隔线、`key`/`text` 两栏加右侧滚动条列、页脚（`src/ui/mod.rs:1743-1838`）；滚动由 `selected` 每帧推出（`src/ui/mod.rs:1783-1787`）。
- 鼠标：只有右上关闭按钮（`src/ui/mod.rs:171-177`），行不可点。粘贴写进查询（`src/app.rs:1574-1579`）。

### 4.2 迁移需要哪些 props

| 需要的数据 | 现状 | 结论 |
|---|---|---|
| 键位表（名称、按键串、标题、分组） | `entries()` 与 `binding_title`（`src/config.rs:527`、`src/app/bindings.rs:6-66`） | 新增快照域 `keybindings`，或打开时 props 直传 |
| 分组顺序 | `const groups`（`src/app.rs:3010-3099`） | 放进 props（含组标题） |
| copy-mode 固定行 | 硬编码（`src/app.rs:3112-3132`） | 放进 props |
| 键位预设 | `config.zellij_keys`（`src/app/keyboard.rs:6-10`） | 放进 props，模块只显示 |
| 本地化文案 | 宿主 `t()` 结果（`src/app.rs:3093-3111`） | props 带已本地化文本 |

`selected`/`scroll`/`query` 由模块自持，不需要宿主。

### 4.3 迁移需要哪些动作

不需要新动作。打开与关闭改向即可：`keys.help` → `run_named_binding("help")`（`src/app.rs:2418-2421`、`src/app/bindings.rs:71-78`），`app.help` 动作直接构造 Rust 浮层（`src/app/actions.rs:674-680`）；迁移后两处改成打开模块表面，关闭用 `closeOverlay`（`src/app/mods.rs:325-342`）。

### 4.4 难点判断

最容易。数据全在宿主、无动作依赖、过滤与滚动都在模块内（`src/app.rs:3146-3172`、`src/ui/mod.rs:1783-1787`）。排第一，见 §7。

## 5 toast

### 5.1 它现在渲染什么

- 状态：单个 `Toast { text, until }`（`src/app.rs:202-207`），`App::toast` 是 `Option`（`src/app.rs:477`），后到的通知覆盖前一条。
- 生命周期：固定 4 秒（`src/app.rs:109`），`tick` 里过期清空（`src/app.rs:974-978`）。
- 来源两条：模块 `notify` 落到 `push_mod_toast`（`src/app/mods.rs:285-296`、`src/app/mods.rs:685-698`）；Agent 进入 blocked/done 且不可见时由 `notify_attention` 生成（`src/app.rs:6596-6648`），该路径同时更新 `last_attention`（`src/app.rs:6619`）并按配置决定是否再发系统通知（`src/app.rs:6628-6643`）。
- 绘制：内容区右下角 3 行带边框盒，宽度随文本、最短 12 格，直接覆盖在 Pane 上，不占布局、不聚焦、不吃鼠标（`src/ui/mod.rs:1307-1335`）；绘制顺序在模块表面之后、Rust 模态之前（`src/ui/mod.rs:104-112`）；Kitty 图片遮挡把它当成覆盖层（`src/term/graphics.rs:47`）。
- 第二条短消息通道是 `self.note`（状态行），变更即重新计时 4 秒并过期清空（`src/app.rs:531`、`src/app.rs:979-987`），模块没有写入口。

### 5.2 迁移需要哪些 props

| 需要的数据 | 现状 | 结论 |
|---|---|---|
| 队列（id、标题、正文、级别、超时、创建时刻、可执行动作、目标 Pane） | 只有单槽位与文本（`src/app.rs:202-207`） | 新增 props `toasts: [...]`；队列由宿主维护 |
| 级别 | `notify` 声明了 `level`，处理时只读 `title`/`body`/`timeout`（`src/app/mods.rs:285-296`） | 补齐并写入 props |
| 可撤销的 id | 返回 `n{request}`，但没有登记任何映射（`src/app/mods.rs:685-698`） | 宿主需要 id → 条目的表 |
| 位置与尺寸 | 由文本长度与内容区推（`src/ui/mod.rs:1311-1319`） | 模块可用 `cell`（`src/mods/host.rs:139`）与 `surface` 返回的 cols/rows（`src/app/mods.rs:77-90`）自算 |
| 最大同时条数 | 无（单槽位） | props 里带 `max` 或由宿主截断 |

### 5.3 迁移需要哪些动作

- 宿主→模块的投递通道不存在。`notify` 是模块调用宿主（`src/mods/host.rs:17-33`），宿主生成的通知写进 `self.toast`（`src/app/mods.rs:691`、`src/app.rs:6620-6644`），不会到模块。
- 已打开表面的 props 热更新不存在：`props.changed` 只在打开后立即发一次（`src/app/mods.rs:1049-1051`），`EVENTS` 里有事件名（`src/mods/mod.rs:22`）但没有第二次发送点。
- 撤销与点击动作不存在：没有 `dismiss` 方法（`src/mods/host.rs:17-33`），`notify` 也没有 `action` 参数（`src/app/mods.rs:285-296`）。
- 状态行短消息没有写入口（`src/app.rs:531`、`src/app.rs:979-987`）。

### 5.4 现有表面形态装不下 toast

| 约束 | 代码位置 | 对 toast 的影响 |
|---|---|---|
| 只有 region/pane/dialog/popup/window | `src/mods/mod.rs:55-93` | 没有「覆盖但不聚焦」的形态 |
| `dialog` 先清空整个内容区 | `src/mods/render.rs:51-54` | 常驻会擦掉 Pane |
| 打开即抢焦点 | `src/app/mods.rs:814` | 弹一条通知就抢走键盘 |
| `popup` 点击会设置焦点 | `src/app/mods.rs:1166-1200` | 点通知会把焦点从 Pane 拿走 |
| `Esc` 关闭顶层覆盖层 | `src/app.rs:1470-1477`、`src/app/mods.rs:1236-1262` | 按 `Esc` 会误关通知 |
| 内置区域名白名单 | `src/mods/chrome.rs:474-481`、`src/mods/chrome.rs:531-541` | `builtin:"toast"` 解析失败 |
| 测试明确拒绝 `toasts` | `src/mods/chrome.rs:853-859` | 与外框区域方案冲突 |
| `[module].focusable` 未参与焦点循环 | `src/pkg/mod.rs:82`、`src/pkg/mod.rs:140` 与 `src/app.rs:4113-4133` | 声明了不聚焦也不生效（文档 `docs/mods.md:144` 与实现不一致） |
| z 序只有 0/1/2/3 | `src/mods/render.rs:89-97` | toast 需要低于 dialog、高于 region |

计划要求默认外框绑定 `toast`（`docs/plan-extensions.md:344`），当前外框语法与焦点规则都表达不了「非聚焦常驻覆盖层」。

### 5.5 难点判断

渲染面积最小，宿主侧改动最深：新增表面形态、通知路由、队列、撤销与 z 序。排最后，见 §7。

已由 base2 票解决：新增 `toast` 表面（不抢焦点、不参与 `Esc` 关覆盖层、z 序高于 pane 低于模态）；外框接受 `toasts` 节点并默认绑定；`notify`/宿主提醒进同一条队列，`props.changed` 推 `{ toasts, max }`，`dismiss` 按 id 撤销，超时由宿主计时；没有提示模块时内置 toast 行为不变。

## 6 通用缺口

**G1 选择与输入的结果回传。** 计划承诺 `overlay.result`（`docs/plan-extensions.md:159`），代码里没有（`src/mods/host.rs:17-33`、`src/mods/mod.rs:14-27`）。模块打开另一个模块的选择器后拿不到选中值，宿主发起的输入也只能靠模块自己 `run` 结束。建议：**新增宿主方法 `ask`**（宿主→模块带 `props` 与超时，模块用 `answer` 回复），并**新增事件 `overlay.result`** 供打开方接收；只扩 props 无法闭环。

已由 base 票解决：新增宿主方法 `ask`/`answer` 与事件 `overlay.result`，超时返回 `timeout`。

**G2 输入法组合事件。** 宿主只翻译按键、粘贴、焦点、鼠标（`src/mods/input.rs:7-57`），`Input` 枚举没有预编辑/组合项（nsh 固定提交 `951556d`，`~/.cargo/git/checkouts/nsh-9218b667fc8c8325/951556d/crates/nsh-platform-tui/src/input.rs:85-108`）；项目文档也把 IME 列为未验证（`docs/plan-v1.1.md:286`）。建议：**扩输入通道**（nsh `Input` 增加组合事件，`src/mods/input.rs` 加翻译，`docs/mods.md:222-229` 表同步），不塞进 props，也不新增宿主方法。

**G3 滚动位置的双向同步。** 不需要。命中测试按表面矩形在宿主侧完成（`src/app/mods.rs:1166-1224`），模块滚动是模块内部状态；三个 Rust 浮层的滚动都是每帧从 `selected` 推出的局部值（`src/ui/mod.rs:1783-1787`、`src/ui/mod.rs:1920-1923`、`src/ui/mod.rs:2072-2076`、`src/ui/mod.rs:2277-2283`）。宿主已通过 `surface` 返回 cols/rows（`src/app/mods.rs:77-90`），每帧也会刷新 `surface.area`（`src/mods/render.rs:37-40`）。结论：不新增能力。

**G4 模态所有权。** `Esc` 在宿主侧先关覆盖层（`src/app.rs:1470-1477`），模块无法先处理「返回上一层过滤」。打开参数里没有「模态/不抢焦点/是否响应外部点击」。建议：**扩 `overlay` 与 `mods.open` 的参数**（`modal`、`autofocus`、`onEsc: "host"|"module"`、`clickThrough`），属打开时的静态属性，不新增方法。

已由 base 票解决：`overlay` 与 `mods.open` 增加 `modal`/`autofocus`/`onEsc`/`clickThrough`。

**G5 已打开表面的 props 热更新。** 只有打开瞬间的 `props.changed`（`src/app/mods.rs:1049-1051`）。toast 队列、settings 的当前值变化都需要推送。建议：**新增宿主方法 `setProps {surface, props}`**；`notify` 也可以路由到指定表面，但语义混淆，单独方法更清楚。

已由 base 票解决：新增宿主方法 `setProps {surface, props}`。

**G6 提醒通道。** `notify` 丢 `level`（`src/app/mods.rs:285-296`），返回的 id 没登记（`src/app/mods.rs:685-698`），没有撤销方法，宿主自身生成的提醒不进模块（`src/app/mods.rs:691`）。状态行 `note` 也没有模块写入口（`src/app.rs:531`）。建议：**扩 `notify` 参数**（`level`、`action`、`target`、`surface`），**新增 `dismiss {id}`**，并把 `note` 并入 `notify {status: true}`。

已由 base 票解决：`notify` 扩 `level`/`action`/`target`/`surface`/`status`，新增 `dismiss {id}`，宿主自身提示走 `status: true`。

已由 base2 票补齐：宿主自身提醒（`status: true` 与 Agent blocked/done）进提示表面同一队列；`dismiss` 按 id 撤销，不再按文本相等匹配。

**G7 动作覆盖与确认语义。** 补 5 个动作：`terminal.send`、`terminal.keys`、`terminal.wait`、`terminal.put`、`machine.rename`（对照 `src/app.rs:3816-3903` 与 `src/app/actions.rs:6-100`）。`run` 的 `confirmation_pending` 恒 `false`（`src/app/mods.rs:166`），返回即 `done`（`src/app/mods.rs:146-178`），异步失败不回传。建议：**扩 `run` 的语义**（如实置 `confirmation_pending`，异步动作在 `action.finished` 里带终态），不新增方法。

已由 base 票解决：补 `terminal.send`/`terminal.keys`/`terminal.wait`/`terminal.put`/`machine.rename`；`run` 的 `confirmation_pending` 如实，异步动作在 `action.finished` 里带终态。

**G8 命令事件未投递。** `command` 在 `EVENTS` 里（`src/mods/mod.rs:14-27`），文档承诺 `<mod>.<command>` 经它投递（`docs/mods.md:236`），实际没有发送点（§0.2）。命令面板接入模块命令依赖这条通道。建议：**补发送点**，不新增方法。

已由 base 票解决：分发器把 `<mod>.<command>` 经 `command` 事件投递给该模块表面。

**G9 快照域不完整且粒度过粗。** 缺 `commands`、`keybindings`、`settings`、`machineInfo`、`templates`、`notifications`；`panes` 缺归属与显示字段（`src/app/mods.rs:573-587`）；`state` 的 `space` 参数被忽略（`docs/mods.md:312` 与 `src/app/mods.rs:134-140`）；`state.changed` 固定广播 7 个域且不含 `theme`（`src/app/mods.rs:1105-1116`）。建议：**扩 props**（新增域与字段，`domains` 如实反映变化），不新增方法。

已由 base 票解决：新增 `commands`/`keybindings`/`settings`/`machineInfo`/`templates`/`notifications` 六域，`panes` 补归属与显示字段，`state` 支持 `space`，`state.changed` 的 `domains` 如实反映变化并包含 `theme`。

**G10 模态的默认入口。** 默认键位与三个动作都指向 Rust 实现：`keys.help`/`keys.settings`/`keys.goto`（`src/app/bindings.rs:69-94`）、`app.help`/`app.settings`/`app.palette`（`src/app/actions.rs:667-680`）、`overlay.close`（`src/app/actions.rs:829-841`）。可配置绑定表是固定的 `Keys` 结构（`src/config.rs:395-460`），不能把某个名字改指向模块。建议：**扩 props/配置**（新增「绑定名 → 表面」的表，或让三个 `app.*` 动作优先派发到已绑定的模块），不新增方法。

**G11 文案本地化归属。** `locale` 已在启动参数里（`src/mods/host.rs:140`），但宿主派生的标签（`binding_title` `src/app/bindings.rs:6-66`、设置项 `src/ui/mod.rs:1968-1999`、机器状态、模板标题）是宿主侧 `t()` 的结果。建议：**扩 props 的字段约定**，宿主派生文案按 `locale` 生成后放进 props，模块只翻自身静态串。

## 7 汇总

| 区域 | 缺 props | 缺动作 | 难度（1 最容易） | 预估改动面 |
|---|---|---|---|---|
| help | 快照域 `keybindings`（名称/按键串/标题/分组/预设），或打开时直传；copy-mode 固定行 | 无；改 `keys.help` 与 `app.help` 的指向 | 1 最容易（无动作依赖，过滤与滚动都在模块内） | 3 文件 · 约 385 行（`src/app/mods.rs` +45、`src/app/actions.rs` +20、`ui/help` 约 320） |
| settings | 快照域 `settings`（段、项目、当前值、主题候选、Delivery、sound） | 新增状态图标、通知投递、提示音三个设置动作与 `config.set`；`keys.settings`/`app.settings` 改向；统一 `theme.changed` 发送 | 2（固定四段，补动作即可） | 3 文件 · 约 440 行（`src/app/mods.rs` +60、`src/app/actions.rs` +80、`ui/settings` 约 300） |
| picker | 结构化候选行（动作名 + 参数），命令目录、插件动作、机器对象、模板列表、`panes` 归属与显示字段、分组与右列与底部上下文 | `machine.rename`；列表数据回读只能靠 props；`keys.goto`/`app.palette` 改向 | 3（过滤简单，难点在五套 `value` 协议归一与数据装配） | 4 文件 · 约 900 行（`src/app/mods.rs` +140、`src/app/actions.rs` +50、`src/app/interaction.rs` 迁移面约 300、`ui/picker` 约 410） |
| dialogs | 字段 schema 与校验状态、`_target`、历史、错误路径、行表描述 | `terminal.send`/`keys`/`wait`/`put`、`machine.rename`；`overlay.result`；`confirmation_pending` 与异步终态；表单 schema 暴露 | 4（校验语义与输入控件要重建，结果回传缺闭环） | 5 文件 · 约 1100 行（`src/app/mods.rs` +90、`src/app/actions.rs` +120、`src/plugin/registry.rs` +20、`src/app/form.rs` +40、`ui/dialogs` 约 830） |
| toast | 队列（id/级别/超时/创建时刻/动作/目标）、`level`、可撤销 id、最大条数 | 宿主→模块投递、`setProps`、`dismiss`；`notify` 补级别与动作 | 5 最麻烦（表面形态、焦点、z 序、通知路由都要改） | 7 文件 · 约 500 行（`src/mods/mod.rs` +40、`src/mods/render.rs` +60、`src/app/mods.rs` +120、`src/mods/host.rs` +10、`src/mods/chrome.rs` +30、`src/app.rs` +40、`ui/toast` 约 200） |
| 通用 | G9 的新增域与字段、G11 的文案约定、G4 的打开参数 | G1 `ask`/`overlay.result`、G5 `setProps`、G6 `dismiss`、G7 五个动作与 `run` 语义、G8 发送点、G2 输入通道 | — | 5 文件 · 约 260 行（`src/mods/host.rs`、`src/app/mods.rs`、`src/mods/mod.rs`、`src/mods/input.rs`、`docs/mods.md`；不含 nsh 仓库改动） |

难度顺序：help → settings → picker → dialogs → toast。理由分别是「无动作依赖」「补少量动作」「数据装配与编码归一」「校验与结果闭环」「表面形态与焦点规则」。

## 8 迁移边界

- 五个区域中 `picker`/`dialogs`/`settings`/`help` 都以模块覆盖层出现，依赖 G4 的打开参数与 G10 的入口改向；`toast` 依赖 G5、G6 与非聚焦表面形态。
- `dialogs` 的表单部分与插件动作 schema 强耦合（`src/app/form.rs:586-609`、`src/plugin/registry.rs:182`），schema 暴露方式要先定，否则模块无法渲染表单。
- `picker` 的列表数据全部来自宿主导出的域（§1.2），域定稿前不要开始模块实现。
- 本次调研不改 `tests/`、`scripts/mods-parity.sh`、`docs/m9-baseline.md` 与任何 `src/` 文件。

## 9 核验

改动边界相对基线 `fc29a85c9ffba62ed0fe0ae26e9ee7680a354721`：

```console
$ git diff --name-only fc29a85c9ffba62ed0fe0ae26e9ee7680a354721
docs/m9-gap.md
$ git status --porcelain
```

第二条命令没有输出，工作树干净。完整转录：`/home/wxb24/tickets/m9gap/evidence/02-git-boundary.txt`。

```console
$ cargo test -p hc-tui 2>&1 | grep -c "test result: ok"
11
```

`tests/`、`scripts/mods-parity.sh`、`docs/m9-baseline.md`、`src/` 均未改动。
