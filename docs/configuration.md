# 配置说明

[返回 README](../README.md) · [使用指南](usage.md)

## 文件与优先级

默认配置文件是 `$XDG_CONFIG_HOME/hc-tui/config.toml`，未设置时为 `~/.config/hc-tui/config.toml`。
`--config` 选择另一文件；内置默认值 → 配置文件 → CLI 的 session/attach/no-restore 覆盖。
前缀 Shift+R 重载；格式错误或无效值显示提示，能解析的设置继续生效。
这是项目实现的 TOML 子集，写法以 `hc-tui --default-config` 和下例为准。
session 可通过 `--session`、`[session] session` 或 HC_SESSION 指定；CLI 显式值优先。

| 路径 | 用途 |
|---|---|
| 配置目录中的 config.toml | 外观、键位、通知和终端策略 |
| 配置目录中的 panels/*.json | 克隆的脚本面板，下次加载配置时读取 |
| 配置目录中的 layouts/*.json | 个人 Pane 布局，通过 layout.switch 选择 |
| `$XDG_STATE_HOME/hc-tui/state.json`（默认 ~/.local/state/...） | 本地已读与分组折叠；schema 4 起含外框树 `mods_chrome`、窗口覆盖层位置 `mods_windows` 与已批准的模块授予 `mods_grants` |
| `$XDG_STATE_HOME/hc-tui/mods/` | 安装的模块：`store/<id>/<generation>/`、`current/<id>`、`records/<id>.json`、`grants/<id>.json`、`logs/<id>/`、`trust/` |
| 同一 state 目录的 control-*.sock | session 对应的本地命令 socket，Unix 用户权限 0600 |
| core 中的 herdr-app / hctui-layout-v1 文档 | 共享结构 / TUI 屏幕布局，不在 config.toml 中 |

自定义 --config 会改变面板、布局、模块开发目录与撤销列表的父目录；state、安装存储与 socket 仍由 XDG_STATE_HOME 决定。

## 模块与签名包

```toml
[mods]
# 签名包信任根：base64 Ed25519 公钥，或 nsh keygen 生成的 .pub 文件路径。
trust_roots = ["~/.config/hc-tui/release.pub"]
# 撤销列表；默认读配置目录的 mods-revocations.json。
revocations = "/etc/hc-tui/mods-revocations.json"
```

`mods install` 对 `.nshpkg` 先做证书链、包签名、文件哈希、撤销列表、构建号回退与
`min_runtime` 校验；全部通过才写入新代际。`trust/nsh-trust.json` 保存已接受的撤销序列
与每个 app 的最高构建号。

## 默认配置参考

以下按当前默认配置输出整理；`hc-tui --default-config` 是安装版本的准确参考。
空字符串键位表示未绑定。默认存在三组重复绑定，覆盖办法见 [使用指南](usage.md#默认绑定冲突)。

```toml
# hc-tui configuration
# Place at ~/.config/hc-tui/config.toml

[ui]
sidebar = true
sidebar_width = 26
sidebar_min_width = 18
sidebar_max_width = 36
mobile_width_threshold = 64   # 宽度 <= 此值走手机单列布局，0 关闭
sidebar_start_collapsed = false
lang = "auto"    # auto | en | zh，auto 跟随 locale
tab_bar_position = "top"   # top | bottom
pane_borders = "auto"      # auto | always | titles | off
pane_gaps = true
pane_outer_borders = true
pane_scrollbars = true
show_agent_labels_on_pane_borders = false
hide_tab_bar_when_single_tab = false
mouse_capture = true
mouse_scroll_lines = 3
copy_on_select = false
confirm_close = true
sidebar_collapsed_mode = "compact" # compact | hidden
host_cursor = "native"      # native | drawn
status_indicators = "dots"   # dots | symbols
agent_panel_sort = "spaces" # spaces | priority
prompt_new_tab_name = true
prompt_new_workspace_name = false
window_title = "{hostname}: {workspace}"
tab_bar_right = []
tab_bar_right_separator = " "

[terminal]
scrollback_bytes = 8388608
new_cwd = "follow"   # follow | home | current | 固定路径
default_shell = ""

[theme]
name = "default"
# [theme.custom]
# accent = "#89b4fa"

[notifications]
delivery = "toast"   # off | toast | system
sound = false
# sound_path = "/path/to/notification.mp3"
# done_path = "/path/to/done.mp3"
# request_path = "/path/to/request.mp3"

[session]
restore = true

[keys]
prefix = "ctrl+b"
help = "prefix+?"
settings = "prefix+s"
detach = "prefix+q"
reload_config = "prefix+shift+r"
open_notification_target = "prefix+o"
goto = "prefix+g"
new_tab = "prefix+c"
rename_tab = "prefix+shift+t"
next_tab = "prefix+n"
previous_tab = "prefix+p"
move_tab_previous = ""
move_tab_next = ""
close_tab = "prefix+shift+x"
new_workspace = "prefix+shift+n"
rename_workspace = "prefix+shift+w"
close_workspace = "prefix+shift+d"
workspace_picker = "prefix+w"
next_workspace = ""
previous_workspace = ""
split_vertical = "prefix+v"
split_horizontal = "prefix+minus"
focus_pane_left = "prefix+h"
focus_pane_down = "prefix+j"
focus_pane_up = "prefix+k"
focus_pane_right = "prefix+l"
cycle_pane_next = "prefix+tab"
cycle_pane_previous = "prefix+backtab"
last_pane = ""
zoom = "prefix+z"
stack_panes = "prefix+t"
layout_next = "prefix+shift+l"
layout_previous = "prefix+shift+j"
float_pane = "prefix+f"
float_pin = "prefix+shift+f"
float_hide = "prefix+shift+h"
close_pane = "prefix+x"
rename_pane = "prefix+shift+p"
kill_pane = "prefix+shift+k"
move_pane = "prefix+m"
resize_mode = "prefix+r"
toggle_sidebar = "prefix+b"
scroll_bottom = ""
copy_mode = "prefix+leftbracket"
next_agent = "prefix+a"
chat = "prefix+shift+a"
send_text = "prefix+i"
interrupt = "prefix+shift+c"
toggle_keep = "prefix+shift+o"
remove_terminal = "prefix+shift+u"
open_link = ""
edit_history = "prefix+e"
read_pane = "prefix+slash"
send_keys = "prefix+shift+i"
wait_output = "prefix+shift+w"
put_file = "prefix+shift+p"
templates = "prefix+shift+t"
machines = "prefix+shift+m"
paste_image = "prefix+shift+v"

# 自定义命令：key 支持 prefix+... 或 ctrl+alt+...；type 为 shell / pane / popup / action
# [[keys.command]]
# key = "prefix+alt+g"
# type = "popup"
# command = "lazygit"
# width = "80%"
# height = "80%"

# 内置主题：default, catppuccin, catppuccin-latte, tokyo-night, dracula, nord, gruvbox, one-dark, solarized, kanagawa, rose-pine, vesper
# 主题 token：accent, agent_blocked, agent_done, agent_error, agent_idle, agent_unknown, agent_working, badge_bg, badge_fg, bg, border, border_focused, cursor_bg, cursor_fg, dim, fg, selection_bg, selection_fg, sidebar_bg, sidebar_title, status_bar_bg, status_bar_fg, tab_active_bg, tab_active_fg, tab_bar_bg, tab_bar_fg, toast_bg, toast_fg

```

## 常用选项的含义

| 配置 | 行为 |
|---|---|
| ui.sidebar_width / min_width / max_width | 终端列数，拖动宽度夹在范围内；不是像素 |
| ui.lang | auto 跟随 `LC_ALL` / `LC_MESSAGES` / `LANG`；en / zh 强制指定 |
| ui.pane_borders | auto 分屏才画；always 总是画；titles 仅标题；off 不画 |
| ui.pane_gaps | true 保留相邻 Pane 各自边框；false 使用单条共享分隔线 |
| ui.pane_outer_borders | 是否保留内容区最外沿边框 |
| ui.host_cursor | native 使用宿主光标；drawn 用单元格绘制 |
| ui.sidebar_collapsed_mode | compact 留窄栏；hidden 完全隐藏 |
| ui.confirm_close | 关闭 Tab、Space、最后一个 Pane 时确认，不控制 app.quit |
| ui.agent_panel_sort | spaces 按工作区分组；priority 按注意力分组 |
| terminal.new_cwd | follow 当前前台目录；home 用户目录；current 客户端启动目录；也可写固定路径 |
| terminal.scrollback_bytes | 每个本地 VT 的历史容量预算 |
| terminal.default_shell | 空串使用 core 默认 shell |
| session.restore | 恢复 core 中布局；--no-restore 可对单次启动关闭 |
| no_mods | 使用内置外框，不加载模块运行时；--no-mods 可对单次启动开启，mods.* 动作在此模式下报错 |
| ui.window_title | 可用 {hostname}、{workspace} |
| ui.tab_bar_right | 支持 zoom、hostname、datetime 和 cmd:命令；cmd 取第一行输出，约每 5 秒刷新 |

## 主题和终端颜色

内置主题：default、catppuccin、catppuccin-latte、tokyo-night、dracula、nord、gruvbox、
one-dark、solarized、kanagawa、rose-pine、vesper。主题修改外部 UI 和 Pane 默认颜色与 ANSI 0–15。
可以在设置里切换，也可运行 `app.theme {"name":"nord"}`。

```toml
[theme]
name = "catppuccin"
[theme.custom]
bg = "#1e1e2e"
fg = "#cdd6f4"
accent = "#89b4fa"
ansi_1 = "#f38ba8"       # ANSI 0–15 可逐项覆盖；推荐显式 RGB
```

颜色支持 #RRGGBB、颜色名与解析器支持的索引表示；完整 UI token 见 --default-config 末尾。
ANSI 16–255 使用标准扩展色；程序显式 RGB/OSC 颜色不被主题重写。默认色通过 viewer FACTS
同步给 core，避免程序查询颜色时得到另一套默认值。程序自有主题需要在程序里设置。宿主进程若设置 NO_COLOR，crossterm 会禁用颜色输出；排查配色时也要检查该环境变量。

## 自定义快捷键

`prefix+...` 先按前缀；`ctrl+alt+...` 是直接绑定；空串禁用。Shift 可以用 shift+ 显式写。
符号支持 minus、comma、ampersand、leftbracket、slash 等命名形式。自定义绑定不要与内置绑定重叠。

```toml
[[keys.command]]
key = "prefix+alt+g"
type = "popup"
command = "lazygit"
width = "80%"
height = "80%"

[[keys.command]]
key = "prefix+alt+t"
type = "pane"
command = "cargo test"

[[keys.command]]
key = "prefix+alt+n"
type = "shell"
command = "notify-send done"

[[keys.command]]
key = "ctrl+alt+a"
type = "action"
command = 'agent.prev {}'
```

| type | 含义 |
|---|---|
| pane（默认） | 临时 Pane，命令结束自动移除 |
| popup | 会话级浮层终端，格数或百分比宽高；Esc 关闭视图，程序可继续运行 |
| shell | 后台执行，不占界面 |
| action | 调用同一工作台动作分发器；command 为动作名和 JSON 对象 |

shell/pane/popup 通过 sh -c（Windows 为 cmd /d /c）执行，跟随工作区机器与目录。
action 参数见 [命令参考](commands.md)。新增普通交互终端与临时命令的 keep 策略不同。

## 脚本面板与个人克隆

```toml
[[panels]]
name = "status"
command = "git status --short"
refresh = 5
```

用 `hc-tui do mod.open '{"name":"status","props":{"scope":"all"}}'` 打开。
可见面板按 refresh 秒刷新（范围 1–3600，默认 5）；命令在工作区机器和目录运行，
一次超时 30 秒。stdout 和 stderr 经 ANSI VT 显示，超时与截断会提示。
`HC_TUI_PANEL_PROPS` 是 JSON 字符串，不要当作已经 shell 转义的代码。

`mod.clone {"name":"files","as":"my-files"}` 写 panels/my-files.json，
`layout.clone {"as":"my-layout"}` 写 layouts/my-layout.json。
名称限英文字母、数字、-、_；已有文件不覆盖。克隆面板下次加载配置时可用。
布局克隆保留当前布局对象与 Pane 身份，不负责复制或续接程序。

## 通知与剪贴板

notifications.delivery 为 off / toast / system，system 另调用 notify-send。
sound=true 时可分别设置 sound_path、done_path、request_path；可用播放器优先
paplay，其次 ffplay / afplay / mpv，无法播放回落终端响铃。
复制同时尝试 OSC 52 和平台剪贴板工具；图片读取在 macOS 使用系统接口，Linux 需要 wl-paste / xclip。
`HC_TUI_GRAPHICS=kitty|blocks` 控制图形模式，tmux 默认 blocks。

## 键盘方案

默认使用 Ctrl+B 前缀。可通过操作菜单切换，或在配置中设置：

```toml
[keys]
preset = "zellij"  # default | zellij
```

Zellij 模式键、锁定直通和鼠标替代操作见 [交互操作](interaction.md)。
