# hc-tui

基于 [herdr-core](https://github.com/herdrdev/herdr-core)（`hc`）的 Rust 原生终端工作台。
Core 拥有终端与进程，本仓库拥有 Workspace / Tab / Pane / 渲染。

当前状态：**P2 + 交互基线**。真实 core 上可交互运行 sh / less / nvim / htop / fzf；
Workspace → Tab → BSP Pane，每个 Pane 一条独立 attach 流，detach 后进程存活。
布局写入 core 的 `doc`（CAS）并在重启后恢复，Agent 有完成/未读模型与注意力队列，
Copy Mode 支持选区、搜索与系统剪贴板。与 Herdr 的逐项差距见
[docs/gap-vs-herdr.md](docs/gap-vs-herdr.md)。

## 快速开始

```sh
# 需要 Zig 0.16.0（libghostty-vt 硬性要求）与 Rust stable
mise install                 # 或自行安装 zig 0.16
cargo build
cargo run -- probe           # 连上 core，打印握手结果
cargo run                    # 打开 TUI，起一个新的 shell 并 attach
cargo run -- --attach t3     # attach 一个已存在的终端
cargo run -- --config ./hc-tui.toml   # 指定配置文件
cargo run -- --no-restore    # 忽略 doc 里保存的布局
cargo run -- --default-config         # 打印全部可配字段与默认键位
cargo test                   # 单元测试
HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach -- --nocapture   # 真实 core 的端到端验收
```

`probe` 在 core 未运行时会由 `hc connect` 自动拉起。

## 已经有什么

| 模块 | 状态 |
|---|---|
| `hc::codec` | framed 编解码（9 B header、DATA/CREDIT/CLOSE/RESIZE/FOCUS/FACTS），有单测 |
| `hc::client` | `hc connect` 子进程、明文 hello、generation 与方法校验、帧读写 |
| `hc::net` | 后台线程：单 reader 按帧序分发、RPC 关联、每个 Pane 一条 attach 流、两条流都回 credit、FACTS、断线退避重连与换新 shell 恢复 |
| `hc::proto` | 终端记录、事件、attach/subscribe 结果、CLOSE reason 的宽松解析 |
| `term::view` | 本地 VT → Ratatui Buffer 的桥：样式、宽字符、真彩、反显、光标；key/mouse 编码、bracketed paste、透传查询的否定应答 |
| `term::input` | crossterm 键盘 → ghostty 键码与修饰键映射 |
| `layout::split` | BSP 分屏：分裂、折叠、比例调整、按几何找焦点邻居，有单测 |
| `app` / `ui` | Workspace/Tab/BSP Pane 的事件循环、尺寸对齐、侧栏（工作区 + Agent）、Tab 栏、状态栏、提示行、toast、鼠标与拖选 |
| `model` | Workspace / Tab / Pane / Agent 状态、Pane 跨 Tab 移动、布局序列化 |
| `config` | `~/.config/hc-tui/config.toml`（XDG 优先）的 TOML 子集解析、键位绑定、`--default-config` |
| `theme` | 12 套内置主题 + `[theme.custom]` 逐 token 覆盖 |
| `copy` / `clipboard` | Copy Mode 的移动/选区/搜索，OSC 52 + 平台剪贴板工具 |
| `tui` | raw mode / alt screen / 鼠标 / bracketed paste 的进入与 RAII 恢复 |

测试里有两条防线：`tests/fixtures/core-first-paint.bin`（真实 core 首帧）锁住
「字节 → cell」的还原；`tests/g1_attach.rs` 在隔离 session 里跑真实 core 的
attach → 首帧 → 输入回环 → 跨 credit 窗口的大流量 → detach → 重连 → 程序结束 CLOSE。

## 配置

文件位置 `$XDG_CONFIG_HOME/hc-tui/config.toml`（回落 `~/.config/...`）；不存在就用默认值。
解析失败写进状态栏，不阻止启动。`hc-tui --default-config` 打印当前支持的全部字段。

```toml
[ui]
sidebar = true
sidebar_width = 26
tab_bar_position = "top"      # top | bottom
pane_borders = "auto"         # auto | always | off
pane_gaps = true              # false 时相邻 Pane 共用一条分隔线
mouse_capture = true
mouse_scroll_lines = 3
copy_on_select = false
confirm_close = true
status_indicators = "dots"    # dots | symbols
agent_panel_sort = "priority" # spaces | priority
prompt_new_tab_name = false
tab_bar_right = ["zoom", "hostname"]

[terminal]
scrollback_bytes = 8388608

[theme]
name = "catppuccin"           # 见 --default-config 的内置主题列表
[theme.custom]
accent = "#89b4fa"

[notifications]
delivery = "toast"            # off | toast | system（system 另发 notify-send）

[session]
restore = true                # 从 core doc 恢复布局

[keys]
prefix = "ctrl+b"
new_tab = "prefix+c"
kill_pane = "prefix+shift+d"
# ... 全部动作见 --default-config

# 自定义命令：key 支持 prefix+... 或 ctrl+alt+...
[[keys.command]]
key = "prefix+alt+g"
type = "popup"          # shell | pane | popup
command = "lazygit"
width = "80%"
height = "80%"

[[keys.command]]
key = "prefix+alt+t"
type = "pane"           # 临时 Pane，命令退出即消失
command = "just test"

[[keys.command]]
key = "prefix+alt+n"
type = "shell"          # 后台 detach，不占界面
command = "notify-send done"
```

`type` 省略时按 `pane` 处理；`width` / `height` 接受格数或百分比，只对 popup 生效。
命令用 `sh -c`（Windows 为 `cmd /d /c`）执行，起始目录沿用当前 Pane 的前台目录。

其他可配项：

```toml
[ui]
sidebar_collapsed_mode = "compact"  # compact: 留一条窄栏；hidden: 完全隐藏
host_cursor = "native"              # native: 宿主光标；drawn: 画成单元格
tab_bar_right = ["zoom", "hostname", "cmd:git rev-parse --short HEAD"]
# cmd: 条目每 5 秒跑一次，取第一行输出显示在 Tab 栏右侧

[notifications]
sound = true
sound_path = "~/.config/hc-tui/notify.mp3"
done_path = "~/.config/hc-tui/done.mp3"
request_path = "~/.config/hc-tui/request.mp3"
```

声音优先用 `paplay`，其次 `ffplay` / `afplay` / `mpv`；都不可用时退回终端响铃。

键位语法：`prefix+<key>` 需要前缀，`ctrl+alt+<key>` 是终端模式直达绑定；字母跟随
Shift 大小写，`minus` / `comma` / `ampersand` / `leftbracket` 等命名符号可用。
`prefix+shift+r` 重新加载配置，无需重启。

## 键位

前缀键是 `ctrl+b`，按下后单独跟一个字母/符号；连按两次把字面 `ctrl+b` 交给程序。

| 键 | 作用 |
|---|---|
| `q` | 退出 TUI（只断视图，程序继续跑） |
| `?` | 键位提示 |
| `c` | 新 Tab |
| `,` | 命名当前 Tab（留空恢复默认） |
| `n` / `p` | 下一个 / 上一个 Tab |
| `shift+h` / `shift+l` | Tab 左移 / 右移 |
| `1`–`9` | 切到第 n 个 Tab |
| `&` | 关闭当前 Tab（连带其所有视图） |
| `shift+n` | 新 Workspace（先问名字） |
| `w` | Workspace 选择：`1`–`9` 或 `h`/`l` |
| `shift+x` | 关闭当前 Workspace（需确认） |
| `v` | 左右分屏 |
| `-` | 上下分屏 |
| `h` `j` `k` `l` | 焦点移动到左/下/上/右的 Pane |
| `tab` / `shift+tab` | 在当前 Tab 内循环焦点 |
| `z` | 缩放当前 Pane 占满内容区 |
| `r` 然后 `h j k l` | 调整分隔线比例，其他键退出调整 |
| `x` | 关闭当前 Pane 视图（程序继续跑） |
| `m` 然后 `1`–`9` / `t` | 把当前 Pane 移到目标 Tab / 移成新 Tab |
| `shift+k` | 命名当前 Pane（写入 `terminal.label`） |
| `shift+d` | 结束当前 Pane 里的程序（需输入 `y` 确认） |
| `i` | 给当前 Pane 的程序发一条消息（`terminal.send`，粘贴 + Enter） |
| `shift+c` | 中断当前轮（`terminal.interrupt`） |
| `shift+o` | 切换 keep（丢失后用 shell 恢复） |
| `shift+u` | 移除已结束的终端记录（`terminal.rm`） |
| `o` | 打开光标处的链接 |
| `shift+e` | 把视口 ANSI 交给 `$EDITOR`，在 popup 里打开 |
| `/` | 只读文本浮层：`terminal.read` 的屏幕 + 历史（`j`/`k`/`PgUp`/`PgDn` 滚动，`q` 关闭） |
| `shift+i` | 按名字按键：`terminal.keys`，输入 `enter, ctrl+c` 这类序列 |
| `shift+w` | 等输出出现：`terminal.wait`（输入要等待的文本） |
| `shift+p` | 发送文件：`terminal.put`（输入本地文件路径） |
| `shift+t` | 模板选择器：在新 Tab 里运行 `terminal.run` 模板 |
| `shift+m` | 机器选择器：`Enter`/`n` 在目标机器开新 Pane，`a` 添加、`c` 连接、`r` 改名、`d` 移除 |
| `shift+v` | 把剪贴板图片交给 `terminal.put`，再把落盘路径贴进程序 |
| `[` | 进入 Copy Mode |
| `a` | 跳到下一个需要处理的 Agent |
| `b` | 显示/隐藏侧栏 |
| `e` | 滚动回底部 |
| `shift+r` | 重新加载配置文件 |

Copy Mode：`h j k l` 移动，`w`/`b` 按词，`0`/`$` 行首行尾，`g`/`G` 视口顶/底，
`Ctrl+u`/`Ctrl+d` 翻页，`v` 起选区，`R` 切换矩形选区，`y` 或 Enter 复制并退出，
`/` 搜索（命中会高亮）、`n` 下一个，`o` 打开光标处的链接，`q`/`Esc` 退出。
复制同时走 OSC 52 与平台剪贴板工具。

侧栏分「workspaces」与「agents」两段：工作区行可点击切换，Agent 行按
`blocked > error > done > working > idle` 排序，点击即跳过去并标记已读；
没有状态上报的 Pane 也会列在后面。列表超过侧栏高度时用滚轮滚动。
已读轮次写在 `~/.local/state/hc-tui/state.json`，重启不会把旧完成重新当未读。

鼠标左键点 Pane 切焦点、点 Tab 栏切 Tab、点侧栏行跳转工作区或 Agent；滚轮在子程序
未启用鼠标时按 `mouse_scroll_lines` 翻本地历史，启用后转发给它。`copy_on_select = true`
时普通模式拖选即复制；否则宿主终端被 TUI 持有鼠标，要拖选文字请按住 `Shift`。

每个 Pane attach 的是它自己的终端，因此不同 Pane 的尺寸不会互相抢；
同一个终端被两个客户端 attach 时才会出现「最后活动者定尺寸」。

隐藏 Tab 的 Pane 仍保持 attach（输出会继续写进本地 VT），切回去立刻是完整的；
关掉 Tab 或 Pane 视图只发 CLOSE，core 里的程序继续运行，可以用 `hc ls` 找回。

## 尚未做

按「能力归属」分类，避免把接线工作误当成 core 缺口。

**core 已提供，本仓库还没接**

- `machine.exec`：不走终端的远程一次性命令。
- `events.subscribe` 的 `terminal` / `children_of` / `label` 过滤：当前订阅全部终端，侧栏才能看到不属于本客户端的终端。

**原语已具备（vendored crate 或本地环境）**

- Kitty Graphics 渲染：已接非虚拟 placement 的采样渲染（RGB/RGBA/PNG，每个单元格取对应像素作背景）；虚拟 placement（Unicode 占位符）尚未处理。
- 图片粘贴依赖本机 `wl-paste` / `xclip` / `pngpaste`。
- Worktree：git 子进程 + 目录选择，core 不参与。

**core 确实不提供（主动降级，暂缓）**

- 插件系统（herdr 自有 API 与运行沙箱）。
- Agent 会话续接（依赖各 Agent 的 `integration install` 钩子与它自己记录的 session 引用）。
- 发行：`just release-musl` 与 `just dist` 已就绪，CI 有 musl 任务；macOS 产物需要在 macOS runner 上构建。

**暂缓的具体理由**

- Worktree：日常多任务用 Tab 与 Workspace 已能覆盖。
- Agent 会话续接：各家 hook 格式分散，收益集中在重启恢复一处，排在远程机器之后。
- 插件系统：没有稳定的自有 API 层，先有 `terminal.*` / `machine.*` 的完整包装再谈扩展点。

`scripts/ui-smoke.sh` 覆盖首次绘制、Tab/分屏、布局跨重启恢复、Copy Mode、reader、
自定义命令的三种形态与 shell 落盘；`just smoke` 运行它。

G1 手工门禁里的 `pi` 一项尚未在有凭证的环境验证；其余四个程序已交互验证。

## 发行

产物命名与内容：

```
hc-tui-<version>-x86_64-unknown-linux-musl.tar.gz
  hc-tui                  静态链接（musl），无运行时依赖
  LICENSE
  THIRD-PARTY-NOTICES.md
  CHANGELOG.md
hc-tui-<version>-x86_64-unknown-linux-musl.tar.gz.sha256
```

```sh
just dist          # cargo zigbuild → tar.gz + sha256（Linux musl）
sha256sum -c hc-tui-*.tar.gz.sha256
tar -xzf hc-tui-*.tar.gz && ./hc-tui --version
```

macOS（aarch64 / x86_64）由 CI 在 macOS runner 上构建，触发方式是推送 `v*` 标签：
`.github/workflows/release.yml` 会构建三平台产物并创建 GitHub Release。
发布前的最低检查清单见 [docs/release.md](docs/release.md)。

运行时只需要 `hc`（>= 0.1.17）在 `PATH` 里；Zig 只在从源码构建时需要。

## 文档

| 文档 | 作用 |
|---|---|
| [docs/plan-v1.1.md](docs/plan-v1.1.md) | 权威实施方案，冲突时以本文为准 |
| [docs/review-v1.0.md](docs/review-v1.0.md) | 对 v1.0 的评审、证据与更正理由 |
| [docs/protocol-facts.md](docs/protocol-facts.md) | hc 协议实测事实，实现的直接依据 |
| [docs/gap-vs-herdr.md](docs/gap-vs-herdr.md) | 与 Herdr 的差距矩阵与补齐顺序 |
| [docs/release.md](docs/release.md) | 发布检查清单与产物验证 |
| [docs/adr/0001-ghostty-vt.md](docs/adr/0001-ghostty-vt.md) | Ghostty 选型、风险与升级路径 |
| [docs/plan-v1.0.md](docs/plan-v1.0.md) | v1.0 原文归档 |

## 环境基线

- `hc` 0.1.17（以 `hc schema` 为准；源码 checkout 可能更旧）
- **Zig 0.16.0**（0.17 编译失败，见 ADR-0001）
- Rust stable（骨架验证于 1.99.1）

许可：Apache-2.0。第三方组件见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。
