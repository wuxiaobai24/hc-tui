# hc-tui

基于 [herdr-core](https://github.com/herdrdev/herdr-core)（`hc`）的 Rust 原生终端工作台。
在一个终端里管理工作区、Tab、分屏和编码 Agent，可与 herdr-app 共享多台机器的布局。

## 安装与启动

运行需要 `hc` 在 `PATH` 中；本轮完整契约验收使用 core 0.1.18。
从源码构建还需要 Rust stable 和 **Zig 0.16.0**，Zig 0.17 不兼容当前 vendored VT。

```sh
mise install                   # 按 mise.toml 安装 Zig 0.16.0
mise exec -- cargo build --release -p hc-tui
mkdir -p ~/.local/bin
install -m 755 target/release/hc-tui ~/.local/bin/hc-tui
hc-tui probe                   # 验证与 core 的连接；未运行的 core 会自动启动
hc-tui                         # 恢复工作区；没有布局时创建 shell
```

确保 `~/.local/bin` 在 `PATH` 中。没有 mise 时安装 Zig 0.16.0，必要时用
`ZIG=/path/to/zig-0.16.0` 指定编译器。发行包和原子升级见 [开发与部署](docs/development.md)。

## 先认识界面

- **Space / Workspace**：工作区，组织机器、目录和多个 Tab；两种名称指同一层对象。
- **Tab**：一组分屏及浮动 Pane。
- **Pane**：一个终端视图或脚本面板。进程由 core 持有，视图由 TUI 管理。
- **Agent**：上报过状态的终端会话；普通 shell 不会因打开、命名或置顶就成为 Agent。

侧栏 `spaces` 在每个工作区下显示 `2 tabs · 4 panes`，未选中也能看到；有分支时接在后面。
计数包含该工作区的所有 Tab（含 settled）与 Pane（含浮动和模块）。
`agents` 展示状态、父子关系、分组与共享已读；普通终端从工作区或 Go to 搜索访问。

## 常用操作

以下按键都先按一次 `Ctrl+B`，松开后再按表中按键。双按 `Ctrl+B` 把它发送给程序。

| 操作 | 按键 / 鼠标 |
|---|---|
| 新建 / 切换 Tab | `c`；`n` / `p` 或 `1`–`9` |
| 左右 / 上下分屏 | `v` / `-` |
| 切换 Pane | `h j k l`；`Tab` / `Shift+Tab`；鼠标点击 |
| 调整大小 | 拖动分屏分隔线；`r` 后按 `h j k l` |
| 调整侧栏宽度 | 拖动侧栏右边界，松开后保存配置 |
| 新建 / 选择工作区 | `Shift+N` / `w`；点击侧栏 |
| 搜索工作区与终端 | `g` |
| 工作台命令面板 | `:` |
| 复制模式 / 只读历史 | `[` / `/` |
| 下一个 Agent / 最近待处理 | `a` / `o` |
| 设置 / 所有键位 | `s` / `?` |
| 退出客户端，保留程序 | `q` |

鼠标调整布局需要 `ui.mouse_capture = true`。浮动 Pane 拖标题行移动、拖右下角改尺寸。
全套键位、鼠标、手机布局和 pi Chat 见 [使用指南](docs/usage.md)。

## 关闭与退出的区别

| 操作 | 程序和布局的结果 |
|---|---|
| 关闭 Pane / Tab / Space | 结束相关程序和子 Agent，删除对应布局 |
| 程序自己退出 | 自动移除 Pane，清理空 Tab / Space；其他 Pane 继续运行 |
| `terminal.park` | 移除视图、禁用 keep，程序继续运行 |
| `tab.settle` | 暂时收起 Tab，保留程序；需要注意时重新出现 |
| `app.quit` / 前缀 `q` | 客户端 detach，core 中的程序继续运行 |
| 连接失败 / attach 错误 | 保留错误视图，等待恢复；不冒充程序正常退出 |

## 功能与边界

| 能力 | 当前支持 |
|---|---|
| 工作区和布局 | 多机、稳定 ID、GUI 双向同步、CAS 冲突保护、布局恢复 |
| 分屏 | BSP、七种预设、Stack、zoom、浮动、pin、移动、拖动改大小 |
| Agents | 状态与未读、置顶、父子树、按机器/工作区或优先级分组、双向导航 |
| 终端 | 独立 attach、历史与搜索、复制、按键/消息发送、剪贴板图片上传 |
| 扩展 | 文件/变更只读入口、定时脚本面板、JSON props、个人布局和模块克隆 |
| 外观 | 12 套主题、自定义颜色、Pane 默认色及 ANSI 0–15 色随主题切换 |
| 图片 | Kitty PNG/RGB/RGBA、虚拟 placement、裁切遮挡；通用宿主半块预览 |
| 手机 | 窄屏单 Pane、switch 面板、触摸滚动、pi Chat 动作条 |
| 自动化 | 命令面板、action 快捷键、`commands` / `ls` / `do` 本地控制 |

显式真彩色和程序通过 OSC 指定的颜色由程序控制。Sixel/iTerm 图片协议、GPUI JS
模块运行时、专门的 worktree 管理、通用 Agent 会话续接尚未实现。pi Chat 读取本机会话文件，
不能当作任意 Agent 或远端会话的通用聊天协议。英文输入/滚动时的重复字符报告尚未稳定复现，
本版补充真实后端回放测试并改为整帧同步输出；不将它列为已确认修复。

## 配置与自动化

```sh
hc-tui --default-config                  # 当前全部默认字段与绑定
hc-tui --config ./hc-tui.toml
hc-tui --session work
hc-tui --attach t3
hc-tui --no-restore
hc-tui commands                         # 命令目录，无需已运行的 TUI
hc-tui ls --session work                 # 以下操作需要对应 session 的 TUI 已运行
hc-tui do agent.next '{}' --session work
hc-tui do pane.split '{"direction":"right"}' --session work
```

配置默认在 `$XDG_CONFIG_HOME/hc-tui/config.toml`，回落 `~/.config/hc-tui/config.toml`。
前缀 `Shift+R` 重载配置。主题设置会同时更新外部 UI 和已有 Pane 的默认色与 ANSI 调色板。

## 文档导航

| 文档 | 内容 |
|---|---|
| [使用指南](docs/usage.md) | 完整键位、鼠标、状态、生命周期、Chat 和常见问题 |
| [配置说明](docs/configuration.md) | 配置项、颜色、绑定、通知、脚本面板和路径 |
| [命令参考](docs/commands.md) | 全部工作台命令、参数、目标选择和返回值 |
| [开发与部署](docs/development.md) | 模块结构、构建、验证、原子安装和回滚 |
| [herdr-app 对齐契约](docs/herdr-app-alignment.md) | 共享数据、冲突、生命周期和验证边界 |
| [发布检查清单](docs/release.md) | 版本、构建、标签和发行包验收 |
| [协议实测基线](docs/protocol-facts.md) | 0.1.17 时期的协议记录，需结合当前 schema |
| [Ghostty ADR](docs/adr/0001-ghostty-vt.md) | VT 选型、风险和升级方式 |

`plan-v1.0.md`、`plan-v1.1.md`、`review-v1.0.md`、`gap-vs-herdr.md` 是历史资料，
不作为当前功能清单。许可：Apache-2.0；见 [第三方组件](THIRD-PARTY-NOTICES.md)。
