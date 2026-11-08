# 变更记录

## 未发布

扩展机制开发计划（docs/plan-extensions.md）前三项已交付：

- M9 toast 表面化：新增 `toast` 表面形态（内容区之上的提示层，不抢焦点、不参与 `Esc` 关覆盖层，z 序高于 region/pane、低于 dialog/popup/window），外框接受并默认带上 `toasts` 节点（零尺寸标记，模块提示层以此开关，内置 Rust toast 不受影响）；打包清单 `[module].place` 支持 `toast`；宿主把通知队列以 `props.changed` 推成 `{ toasts, max }`（`{ id, level, text, action?, target?, created_ms, timeout_ms }`，条数由 `notifications.toast_limit` 限制，默认 3），`notify` 的 `surface` 可指定提示表面、未指定走默认，宿主自身提醒（`status: true`、Agent blocked/done）进同一条队列；`dismiss` 按 id 撤销（不再按文本相等匹配），超时由宿主计时并撤；`state.changed` 在只有 revision 变化时不发空 `domains`。新增 `src/app/mods/toast_tests.rs` 六个用例，`docs/mods.md`、`docs/configuration.md` 与 `docs/m9-gap.md` 同步。

- M10 插件契约硬化：`src/store.rs` 改为记录先原子落盘（同目录临时文件 + `fsync` + `rename` + 父目录 `fsync`）、`current` 指针原子切换、`store/<id>/.lock` 包级互斥与锁内代际分配，`Store::open` 幂等恢复清理临时残渣、按记录重建缺失指针，并把「有指针没记录」报成 issue 而不删除；`plugins install`/`link` 默认不启用（`--enable` 显式打开），更新保留启用状态，运行中按注册表磁盘指纹重载并发布 `registry.changed`，停用包的动作调用与 `plugins pane open` 返回 `disabled`，已排队调用继续跑完；参数校验递归到嵌套 object 与 array `items`，错误带完整路径，清单期校验 `default` 并限制 schema 深度 8、单层属性 64、`items` 形状；`docs/plugins.md` 补发布事务与恢复、启用语义、参数校验三节。
- M11a 统一包模型：新增 `src/pkg/`，`hctui.toml`(v2)、`plugin.toml`(v1) 与旧 `[mod]` 形态读成同一个 `Package`；`[module]` 与 `[grants]` 由统一 TOML 严格解析负责，能力 `capabilities { actions, terminals, module }` 由清单推导，删除 `src/mods/store.rs::manifest_id` 与 `src/mods/mod.rs::manifest_grants` 的逐行手写解析；`Record` 增加 `capabilities` 与 `module`，旧记录按清单回填；注册表合并插件与模块两个 store 及两个开发目录（已安装代际优先），新增 `get`/`modules`/`entries`/`revision`；新增 `hc-tui packages list|install|update|link|unlink|uninstall|enable|disable|gc|log`，`plugins` 与 `mods` 的安装类子命令成为别名并在 stderr 打弃用提示；`packages list` 带 `A`/`T`/`M` 能力徽章；四个 smoke 脚本的 `cleanup()` 在全部通过时清理 `$ROOT`；新增 9 个用例，`docs/plugins.md` 与 `docs/plan-extensions.md` 同步。
- M11b 统一包运行时：模块改为按包记录加载（`Record.module` + `Record.capabilities.module` 决定是否加载，bundle 相对该代际目录解析，`src/pkg::resolve` 是唯一判断），旧记录在同一代际目录读一次清单回填 `capabilities`/`module`/`grants`、不要求重装，坏包只给一条 issue；`Record.grants` 成为运行期授予唯一来源，`src/mods/mod.rs::manifest_grants` 删除，安装/链接/签名包三条路径都写入记录，`mods grant --revoke` 后表面收到 `grant.changed`、后续调用返回 `denied`；`packages enable|disable` 成为动作、终端入口与模块表面的单一闸门，250 毫秒注册表指纹重载同时撤下/恢复模块区域，停用后外框回落内置界面、启用后按 `[module].place` 原地停靠，无需重启，已开终端不 kill、已排队动作不腰斩；`--no-mods` 只跳过模块表面，动作与终端入口照常；`src/app/mods.rs` 的代际 prune 只碰模块商店，插件代际仍由 `plugin::lease` 按 pin 回收；新增 `examples/packages/git-board` 三合一示例与 `scripts/packages-smoke.sh`，新增 7 个用例，`docs/plugins.md`、`docs/mods.md` 与 `docs/plan-extensions.md` 同步。
- M11c 包分发：`packages install|update` 接三种来源——本地目录照旧、git 仓库（`git clone --depth 1`）与 `.tar.gz`/`.tgz`（`tar -xzf`）落到临时目录后校验清单、安装、删除临时目录（`src/pkg/source.rs`）；新增 `packages info <id|来源>`（id/名字/版本/来源/代际/能力/模块/服务/授予/安装时间/来源端版本）与 `packages update --check [<id>] [--json]`（git 来源用 `git ls-remote --symref` 找默认分支再浅取一份提交只读清单，不 clone 整仓；有更新时输出 `local 0.1.0 → remote 0.2.0`，`up to date` 为另一态，退出码都 0，只读不写记录）；记录 `origin` 保存真实来源（`plugin::registry::install_from`、`mods::store::install_local_from`），非法清单或 id 不符在发布代际前失败、原记录与指针不动、暂存目录删除，网络不可用报 `clone failed` 不退回本地；新增 6 个用例，`docs/plugins.md` §9.4 与 `docs/mods.md` 同步。

- M1 渲染基线：升级 ratatui 0.30.2 与 crossterm 0.29，导入全部迁到 ratatui-core / ratatui-widgets / ratatui-crossterm 子 crate；80×24 与 120×40 帧快照升级前后逐格一致，单元、nvim 字节回放、图片遮挡、E2E 与 ui-smoke 全部通过。
- M2 nsh 探针：以 git 依赖固定 longbridge/nsh `951556dc`，引入 `nsh-platform-tui` 与 `nsh-engine`（js）；探针测试在 TestBackend 的 Pane 矩形内加载最小 TSX bundle，验证按键、鼠标、粘贴、resize、唤醒、退出与干净停止；musl 双目标冷构建基线与 nsh/QuickJS 第三方声明写入 docs/release.md。
- M8 事件与发行收尾：`hc-tui events` 经控制 socket 长连接按行输出事件，支持 `--plugin` 与 `--kinds` 过滤，每个订阅者 256 条有界队列，溢出补发 `events.dropped`，客户端断开即停止；App 发布 `terminal.status`/`terminal.turn`/`terminal.finished`/`terminal.opened`/`terminal.closed`（带 machine、terminal、turn、at_ms 与插件 ID），`HCTUI_SOCKET` 让插件进程直接连到发起它的 TUI；示例 `examples/plugins/events`；新增 `scripts/events-smoke.sh`、`scripts/degraded-smoke.sh`、`scripts/perf-baseline.sh` 与共享契约回归；测试与文档同步。\n- M7 插件终端与多机：新增 `[[terminals]]` 的完整入口（`terminal.run {argv,cwd,on,keep,env}`、`tab/right/down/float/popup/zoomed` 映射、attach 后写 `hctui.plugin`/`hctui.terminal`/`where` 标签）；包内 `./` 相对程序只在安装所在主机运行，远端请求返回 `unsupported`，`requires_programs` 与本机 PATH 程序均在目标机器检查（远端用 `machine.exec` 探测），缺失、离线、目标消失分别返回 `missing_program`、`offline`、`target_gone`，不回落本机；新增 `plugins pane open/status/focus/close`（按标签定位，关闭只断视图），终端持有时保留代码代际到关闭后回收；示例 `git-tools` 与 `review`。
- M6 插件清单与动作：新增 `src/plugin/`（标准 TOML 清单严格校验并给出 `文件:行:列`；注册表扫描配置目录与 `<state>/plugins` 当前代际，动作随禁用/卸载消失；受限动作进程支持超时、取消、进程组 SIGTERM→2 秒→SIGKILL、stdout/stderr 各保留 64 KiB 尾部、全局 8/单包 4/队列 64 与按调用日志轮转）。App 把插件动作接入命令目录、命令面板与 Pane 右键菜单，校验参数 schema、冻结目标、注入 `HCTUI_*` 环境并通过控制通道暴露状态、取消与日志；新增 `hc-tui plugins action list|invoke|status|cancel|log` 与 `plugins list/install/update/link/uninstall/enable/disable`，示例 `examples/plugins/hello`。
- M5 安装与分发：新增通用代际存储 `src/store.rs`（`store/<id>/<generation>`、`current` 指针、records、引用回收、日志轮转）与 `src/mods/store.rs`（本地目录、链接目录、签名包三种来源；预览、安装、更新、卸载与授予侧车）。签名包经 `nsh-package` 校验信任根、撤销列表、构建号回退与 `min_runtime`，通过后才写入新代际并记录在 `trust/nsh-trust.json`；`config.toml` 新增 `[mods] trust_roots` 与 `revocations`。App 从存储当前代际加载 bundle 与清单，表面记录安装代际，关闭后回收旧代际；新增 `hc-tui mods list/check/test/build/install/update/uninstall/link/unlink/log/grant/enable/disable`，运行中优先经控制 socket 转发 `open/close/reload/grant/enable/disable`，无窗口时 `grant`/`enable`/`disable` 直接写持久化状态。
- M4 宿主通道与实例：模块表面注册表按代际、授予与请求管理，宿主通道接通 `state`/`run`/`request`/`exec`/`readFile`/`writeFile`/`notify`/`theme`/`overlay`/`closeOverlay`/`focus`/`surface`/`setAgentOrder`/`requestGrants`；键鼠、粘贴与覆盖层 Esc 按 z 序路由到焦点表面，模块未处理的键不回落后台 PTY；`state.changed` 按 revision 合并发送，`app.ready`、`terminal.*`、`focus.changed`、`theme.changed`、`grant.changed` 与 `action.finished` 在对应触发点投递；staging 重载失败保留旧实例与旧代际；`state.json` schema 4 增加 `mods_grants`；新增 `mods.open/close/reload/grant/enable/disable`、`hc-tui mods check` 与 `examples/mods/{agents,spaces,prs}`。
- M3 外框与表面：新增 `src/mods/chrome.rs` 外框树，界面布局改由树驱动，默认帧与升级前逐格一致；侧栏可换到右侧、Tab 栏换到下方；本地 state 升到 schema 4（`mods_chrome` / `mods_windows`）；新增 `mods.dock` / `mods.undock` / `mods.chrome` 动作与 `--no-mods`；Kitty 图片按模块区域遮挡、关闭后重传；`pane.go` 焦点循环为模块区域预留扩展点。

## 0.4.3 — 2026-11-06

- Mac 系统截图 PNG 直接从剪贴板读取，无需额外安装 pngpaste；仍先上传到目标机器，成功后才插入远端路径。
- 修复 Chat 提前截获图片粘贴快捷键：Ctrl+B 后 Shift+V、配置的直接图片按键和 Ctrl+V 均可触发上传；保留普通文字输入。
- 增加 macOS 原生剪贴板往返与无图片错误回归，并在 Mac CI 显式执行。
- 发布按 0.0.1 递增，同一轮修复合并验证后发布。

## 0.4.2 — 2026-11-06

- 新增一键安装脚本，自动识别 Linux/macOS、amd64/arm64；支持指定版本和目录，校验 SHA-256/版本后原子安装，并在 Release 提供固定 install.sh 入口。
- 修复 Mac 本机 TUI 粘贴图片文件路径时直接将 /Users/... 转发给远端的问题：现在读取本机图片字节，通过 terminal.put 上传到原始目标终端所在机器，成功后才插入远端路径。
- 支持带引号、反斜杠转义、中文文件名和 file:// 图片路径；识别 PNG/JPEG/GIF/WebP，遵守 core 的 8 MiB 限制。文件不可读或上传失败会明确报错，不再把未上传的本机路径当成附件。
- Chat 图片上传完成后放进原 Chat 的输入框；上传期间关闭或更换 Chat 不会把路径误发到终端。
- 增加图片路径、失败路径和双 core 图片传输回归，直接核对远端文件字节。

## 0.4.1 — 2026-11-06

- 更新器通过公开 Release 跳转发现版本，不依赖 GitHub 匿名 API 的额度。
- Pi Chat 跟随 machine/ 终端，通过 core file.read 增量读取该机器上 session label 指定的 JSONL；远端未映射或失败明确显示，不猜本机会话。新增 session-hook CLI，正确跟随会话切换并忽略旧请求回包。
- 输入/渲染 P0：修复中文双宽字尾列丢失高亮；遵守程序同步重绘帧，网络分块期间保留完整画面。加入真实 nvim 的选区、滚动、分屏、缩放和编辑固定回放，逐格比对文字与颜色。
- 修复浮层继承终端 DIM 等属性造成文字明暗不一；侧栏机器可折叠并记忆，Spaces/Agents 独立滚动，空间使用单行和彩色标识。
- Ctrl+V 支持本机剪贴板 PNG；图片回填绑定原始 Pane，避免上传时切换窗口后误贴。
- 固定共享布局/名称及 agent 已读/置顶的同步契约，屏幕焦点、zoom、settle、主题和命令面板保持本地。
- 发布 Linux amd64/arm64 musl 静态包、macOS Intel/Apple Silicon 独立包，所有平台成功后才发布；新增 hc-tui update，校验 SHA-256、版本后原子替换。

## 0.3.1 — 2026-11-06

- Agents 仅展示上报过状态的会话，普通 shell 不再混入；每个 Space 直接展示 Tab/Pane 数量。
- 支持鼠标拖动分屏分隔线与侧栏边界；无间隙布局保留单条可拖动分隔线，侧栏宽度保存到配置。
- Pane 默认颜色与 ANSI 调色板跟随主题，显式真彩色保留；修正把颜色索引交给宿主导致的配色不一致。
- 终端单元格完整清理、宽字符边缘裁切与整帧同步输出，减少重绘残留及撕裂；英文重复现象尚未稳定复现。
- 程序退出自动移除 Pane，收起空 Tab/Space；后台 Tab 退出保持当前焦点，连接错误保留恢复入口。
- 重写用户文档，补齐命令参数、配置、生命周期与部署说明，历史计划不再作为当前功能依据。

- 与 herdr-app 共享稳定 space/tab/pane 身份和多机器布局，CAS 合并并发修改；同步时复用 VT 与 attach 流。
- 补齐父子 Agent 树、共享置顶/已读、Done 焦点保持、双向 Agent 导航和 settle 恢复。
- 区分 close、park、keep、kill 与客户端 detach；普通终端由 core keep 恢复，禁止静默换成无关 shell。
- 新增命令面板、`commands`/`ls`/`do` 本地控制、直接快捷键、action 绑定、脚本面板与布局/模块克隆。
- RPC 不再阻塞键盘；修复前缀松键、搜索 j/k、输入/粘贴不回底部和模态鼠标穿透。
- 原生 Kitty 图形支持源裁剪、Unicode placement、浮动层遮挡和资源清理，通用宿主使用半块预览。
- 新增双 core 机器传输和 GUI 文档双向同步的隔离契约验收：`just parity`。

## 0.3.0 — 2026-11-05

### Chat 视图

- 新增 `chat` 模块：跟随 pi 的会话 JSONL（`~/.pi/agent/sessions/--<cwd>--/*.jsonl`），
  沿 `parentId` 只走当前叶子，增量读取、文件重写时整体重读、`/new` 换文件时自动跟过去。
- `ui::chat` 把会话渲染成一列：正文按当前宽度重折（宽字符算 2 列）、
  思考内容折成两行、工具调用收成一行并可展开、内容不足一屏时贴底。
- `prefix+shift+a` 打开/关闭（手机 `switch` 面板里是 `chat 视图`）；
  手机上额外有 `发送 / 中断 / 命令 / 终端` 动作条，点击工具卡展开结果。
- 输入行走 Pane 的 attach 流（文本 + 回车）发回同一个终端，pi 自己的权限、
  模型、扩展与上下文照旧生效；`Ctrl+C` 中断、`Ctrl+N` `/new`、`Ctrl+Y` 复制最后回复、
  `Ctrl+O` 退回终端、`Ctrl+U` 清空输入、`Ctrl+E` 回到底部，`↑`/`↓` 选块或翻输入历史，
  `PgUp`/`PgDn` 翻页。
- `chat::commands`：输入 `/` 打开命令面板，内置斜杠命令加上 `~/.pi/agent/prompts`
  里的模板与标准位置发现的技能；标为需要浮层的命令（设置、会话选择器、登录等）
  采纳后先关掉 Chat 再发，由终端显示。
- `chat::hook`：命令面板里的 `安装会话映射 hook`（本地动作）往
  `<agent-dir>/extensions/` 写一个 pi 扩展，由它在 `session_start` / `agent_start`
  把会话文件路径报到 `hc label <t> session=<path>`；hc-tui 读 `labels.session`
  得到确定映射，读不到时退回「目录里最新文件」并在顶栏标 `· 推断`。

## 0.2.0 — 2026-11-05

### herdr 体验对齐

- 默认键位改成 herdr 0.9.3 的那套：`prefix+s` settings、`prefix+g` Go to、
  `prefix+o` 跳到最近的注意力目标；`rename_tab = prefix+shift+t`、
  `close_tab = prefix+shift+x`、`rename_pane = prefix+shift+p`、
  `kill_pane = prefix+shift+k`、`edit_scrollback = prefix+e`、
  `close_workspace = prefix+shift+d`、`rename_workspace = prefix+shift+w`；
  `prompt_new_tab_name` 默认打开，新建 Tab 先问名字并预填序号。
- 布局：侧栏占满整列高度，Tab 栏只在内容区上方；Tab 栏改成 `1 2 3 +`，
  缩放时当前 Tab 追加 `Z`，`+` 可点击新建；`hide_tab_bar_when_single_tab`
  只有一个 Tab 时收起 Tab 栏；`tab_bar_right` 默认空，右对齐条目用
  `tab_bar_right_separator` 连接。
- 分隔线：`pane_borders = auto` 只在分屏时画框；新增 `pane_outer_borders`
  （去掉 Pane 区域外沿的框）、`pane_scrollbars`、
  `show_agent_labels_on_pane_borders`；边框不再写 `user@host:path · chN`。
- 分隔线新增 `pane_borders = "titles"`：只画每个 Pane 顶上的标题行，
  其余三边不画；相邻 Pane 与 `pane_outer_borders = false` 都不会把标题行去掉。

### 布局预设

- `src/layout/preset.rs`：七个预设 —— `balanced`、`main-left`、`main-top`、
  `columns`、`rows`、`stack`、`main-stack`。都是 `SplitNode` 的生成函数，
  按当前分屏 Pane 的先后顺序填进去，不碰浮动层。
- `prefix+shift+l` / `prefix+shift+j` 顺序切换，当前用的是哪个记在
  `Tab.layout` 里并随布局持久化；分屏少于两个时只给提示。

### 浮动层

- `prefix+f` 把焦点 Pane 在分屏与浮动层之间切换：浮起后位置与大小按内容区
  百分比保存，窗口尺寸变化时保持相对位置；分屏里至少要留一个 Pane。
- 支持多个浮动 Pane，焦点所在的那个自动提到最上面；`prefix+shift+f` 切换 pin
  （同一层里不随焦点沉下去）。
- `prefix+shift+h` 收起 / 展开整个浮动层。收起的 Pane 不重排终端、也不断流，
  焦点落上去时浮动层自动展开。
- 鼠标拖标题行移动浮动 Pane，拖右下角改尺寸；浮动的 Pane 上分屏会再浮一个
  错开的新 Pane。
- 浮动层写进 core doc，随布局一起恢复；移动 Pane 到别的 Tab、关闭 Tab、
  关闭工作区都会连带处理浮动层的成员。

### Stack

- 布局树新增 `Stack` 节点：`prefix+t` 把当前 Tab 的所有 Pane 收成一个纵向堆叠，
  折叠项集中排在顶部各占一行标题，焦点那个占剩下的高度。
- 折叠项的终端沿用上次展开时的尺寸（不重排、不重绘内容）；切成展开项时
  才按展开后的矩形重新上报尺寸。
- `prefix+h/j/k/l` 落到折叠项即把它切成展开项；点标题行同样切换。
- Stack 写进 core doc（`schema_version` 升到 2，旧的 version 1 文档仍可读）；
  在 Stack 里关掉一个 Pane 会把它从堆叠里摘出，关到只剩一个时变回普通 Pane；
  对 Stack 分屏会把整个 Stack 和新 Pane 分左右/上下。
- 底部状态栏改成 herdr 式临时模式行：只有 PREFIX / COPY / RESIZE / NAVIGATE /
  MOVE 或提示出现时占一行，右侧显示连接状态；普通状态下 Pane 一直铺到底。
- 新增三个浮层：可过滤滚动的 Keybinds 面板、Settings 面板（theme / indicators /
  toasts / sound，回车应用并写回 `config.toml`）、Go to 面板（搜索工作区与终端，
  选中行显示面包屑与目录）。
- 提示输入改成 herdr 样式的居中面板：标题 + 输入行 + `↵ save  ^c clear  esc cancel`，
  确认类提示是 `↵ confirm  esc cancel`；`^c` 清空输入。
- 侧栏与 herdr 同构：`spaces` 段每个工作区两行（目录名 + git 分支），
  底行 `new ● menu`（可点击），分隔线后是 `agents` 段与右侧 `grouped`/`priority`
  标注，超出高度时右下角显示滚动方向；`agent_panel_sort` 默认改为 `spaces`。
- 其它客户端（`hc run`、另一个 hc-tui）在同一个 core 里创建的终端会出现在侧栏
  和 Go to 里；选中后按需 attach 出一个新视图，不再只显示自己创建的 Pane。
- 新增 `terminal.new_cwd`（`follow` / `home` / `current` / 固定路径，默认 `follow`）
  与 `terminal.default_shell`：新建 Tab 与分屏沿用来源 Pane 的前台目录。
- 工作区名字在创建时按目录名固定下来，不再随焦点 Pane 的目录变化。
- 新增 `window_title`（默认 `{hostname}: {workspace}`，支持
  `{tab}` / `{pane}` / `{terminal_title}`）与 `prompt_new_workspace_name`。
- 界面提示文案统一成 herdr 的英文措辞（键位、模式行、面板页脚、操作回执）。

### 手机与窄屏

- 新增 `ui.mobile_width_threshold`（默认 64）：宽度不超过阈值且开启 `mouse_capture`
  时进入手机单列布局，Tab 栏与状态栏收成一行顶栏，内容区只摆当前 Pane。
- 顶栏右侧 ` switch ` 打开全屏切换面板：workspaces / tabs / agents / menu 四段全部可点，
  新建 tab、分屏、关闭 pane、复制模式、下一个待处理、退出都能点完，
  不依赖前缀键；面板打开时按钮变 ` close `。
- 手机布局下单指上下拖动翻 scrollback，不触发拖选复制；单击 Pane 仍切焦点。
- 修正滚轮方向：向上滚看更早的行，向下滚回到底部。

## 0.1.0 — 2026-11-04

首个可发布版本：基于 herdr-core（`hc`）的终端工作台，客户端自带
Workspace / Tab / BSP Pane 模型、本地 VT 渲染与布局持久化。

### 核心协议

- framed 编解码（9 字节头，DATA/CREDIT/CLOSE/RESIZE/FOCUS/FACTS），单 reader 按帧序分发。
- 每条 attach 流与 events 流都回 credit；断线指数退避重连，重连后重订阅并按意图重新 attach。
- attach 后立即上报 FACTS；`size` 事件先改本地 VT 尺寸再解析后续字节；`missed` 走全量快照。
- 通用 JSON-RPC 通道：`terminal.label/kill/read/send/keys/wait/interrupt/put/keep/rm`、
  `machine.*`、`templates.list`、`doc.get/set`；连接层失败自动在重连后重放。

### 对象模型与布局

- Workspace → Tab → BSP Pane；Pane 跨 Tab 移动时 ID 与 attach 流保持不变。
- 布局写入 core 的 `doc`（CAS + 400 ms 防抖），重启后按终端地址恢复与重新 attach；
  `conflict` 时取最新版本重放本地意图，并监听 `doc` 事件刷新版本。
- 焦点按几何移动、Tab 循环与重排、缩放、分隔线比例调整、最小尺寸保护。
- 分隔线支持 `auto` / `always` / `off`；`pane_gaps = false` 时相邻 Pane 共用一条边。

### 交互

- 前缀键与全部动作可配置；支持直接绑定与自定义命令（`shell` / `pane` / `popup`），
  命令沿用当前 Pane 的前台目录，临时 Pane 在程序退出后自动折叠。
- Copy Mode：视口移动、按词、行首行尾、翻页、矩形选区、搜索与命中高亮、链接打开。
- 复制走 OSC 52 与平台剪贴板工具；`copy_on_select` 支持普通模式拖选即复制。
- 只读文本浮层（`terminal.read`）、`terminal.keys` / `wait` / `put` 输入行、剪贴板图片
  （`terminal.put` 后贴回路径）、`$EDITOR` 打开 ANSI 历史、滚动条。
- 提示行支持光标移动、删除与历史；侧栏支持滚轮滚动、折叠成窄栏或隐藏。

### Agent

- 消费 `status` / `turn` 事件，维护 `working` / `blocked` / `error` / `done` / `idle` / `unknown`。
- 完成未读用本地 `seen_turn` 判定并落盘到 `state.json`；侧栏按注意力排序，`prefix a` 跳转。
- 后台完成按配置投递 toast 或系统通知，可选提示音。

### 多机与图形

- 机器选择器：`machine.add/list/connect/rename/rm`，`terminal.run --on` 开远程 Pane，
  侧栏显示机器连接状态。
- 启用 libghostty 的 kitty graphics：查询由本地 VT 回答，非虚拟 placement 采样成单元格背景
  （RGB/RGBA/PNG）。

### 配置与主题

- `$XDG_CONFIG_HOME/hc-tui/config.toml`（回落 `~/.config`）；12 套内置主题与逐 token 覆盖；
  `--default-config` 输出全部字段；`prefix shift+r` 热重载。

### 发行

- `linux x86_64 musl` 静态产物（`just release-musl`），`just dist` 生成 tar.gz 与 sha256。
- CI：fmt / clippy / test（ubuntu + macOS）、musl 构建；标签 `v*` 触发
  `.github/workflows/release.yml` 发布 Linux 与 macOS 产物。

### 测试

- 65 个单元测试、2 个真实 core 端到端测试（attach → 首帧 → 输入回环 → 大流量 → 重连）。
- `scripts/ui-smoke.sh`：真实 core + tmux 的 UI 冒烟（绘制、Tab/分屏、跨重启恢复、
  Copy Mode、reader、三种自定义命令）。
