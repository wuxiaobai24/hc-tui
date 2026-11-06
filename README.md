# hc-tui

**接 herdr-core、与 herdr-app 共享布局的终端工作台。**

这个仓库是 `wuxiaobai24/hc-tui`，主线是多机工作区、Pi 会话和可靠的终端输入/渲染。
core 持有程序，TUI 负责界面；远程传输直接复用 core 的 machine 链路。

## 安装与升级

运行需要 [herdr-core](https://github.com/herdrdev/herdr-core) 的 `hc` 在 PATH 中；
已验证的协议基线为 core 0.1.18。**发行二进制不需要 Rust 或 Zig。**

从 [Releases](https://github.com/wuxiaobai24/hc-tui/releases) 选择平台：

| 平台 | 发行 target |
|---|---|
| Linux amd64，musl 静态 | `x86_64-unknown-linux-musl` |
| Linux arm64，musl 静态 | `aarch64-unknown-linux-musl` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |

例如 Linux amd64：

```sh
version=0.4.1
target=x86_64-unknown-linux-musl
archive="hc-tui-$version-$target.tar.gz"
curl -fLO "https://github.com/wuxiaobai24/hc-tui/releases/download/v$version/$archive"
curl -fLO "https://github.com/wuxiaobai24/hc-tui/releases/download/v$version/$archive.sha256"
sha256sum -c "$archive.sha256"  # macOS: shasum -a 256 -c "$archive.sha256"
mkdir -p hc-tui-unpacked ~/.local/bin
tar -xzf "$archive" -C hc-tui-unpacked
install -m 755 hc-tui-unpacked/hc-tui ~/.local/bin/hc-tui
hc-tui probe
hc-tui
```

确保 `~/.local/bin` 在 PATH 中。macOS 包独立携带 VT 静态库，仍使用系统库；不是完全静态的 Mach-O。

```sh
hc-tui update --check          # 查询发行版本
hc-tui update                  # 下载当前平台的最新正式版，校验后原子替换
hc-tui update --version 0.4.1   # 指定版本
hc-tui update --from ./hc-tui-0.4.1-x86_64-unknown-linux-musl.tar.gz
```

升级使用 curl、tar 及 sha256sum（macOS 为 shasum），本地包也必须有同名 `.sha256`。
已运行的客户端继续用旧版本，重开后生效。源码构建见文末附录。

## 多机与 Pi

```sh
ssh box 'hc-tui --session work'    # 在 box 上恢复该 core/session 的同一共享文档
hc-tui --session work             # 在已配置 hc machine 的 core 上浏览机器与布局
ssh box 'hc-tui session-hook install'  # 在 Pi 实际运行的机器安装会话映射
```

Pi 需在 core 管理的终端中启动，让 hook 获得 `HC_TERMINAL_ID`；安装后在 Pi 执行 `/reload`。
Chat 对 `machine/t7` 使用 core `file.read` 读取 **machine 上** session label 指定的 JSONL。
没有远端 label 时明确显示“远端会话未映射”，不猜本机同目录会话。本机回退发现会明确标注“本机推断”。
没有独立的 TUI remote 协议；机器登录、重连和传输由 `hc machine` 管理。

## 先认识界面

- **Space / Workspace**：工作区，组织机器、目录和多个 Tab；两种名称指同一层对象。
- **Tab**：一组分屏及浮动 Pane。
- **Pane**：一个终端视图或脚本面板。进程由 core 持有，视图由 TUI 管理。
- **Agent**：上报过状态的终端会话；普通 shell 不会因打开、命名或置顶就成为 Agent。

侧栏 `Spaces` 使用单行名称、彩色标识与分支/待处理提示；点击机器标题折叠整组，折叠状态会保留。
Spaces 与 Agents 分区独立滚动，Tab/Pane 数量保留在详细切换视图。
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
模块运行时、专门的 worktree 管理、通用 Agent 会话续接尚未实现。pi Chat 支持本机 JSONL 和 core machine 路由下的远端映射 JSONL，
不是任意 Agent 的通用聊天协议。英文输入/滚动时的重复字符报告尚未稳定复现，
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

## 项目定位与同名项目

- 本仓库：`wuxiaobai24/hc-tui`。重点是现有 core 多机链路、herdr-app 共享布局和 Pi JSONL 会话；同步边界见 [共享契约](docs/sync-contract.md)。
- [followcat/hc-tui](https://github.com/followcat/hc-tui)：其 README 将旧 herdr 0.9.5-followcat 的界面、按键与行为一致性作为目标。本仓库按多机/Pi 的实际工作流取舍，不以逐个镜像旧 Desktop 控件为目标。
- [HerdR Terminal](https://github.com/ChyuWei/herdr-terminal)：`ht` 客户端采用独立的 `herdr-terminal` 文档和自己的屏幕组织。本仓库直接与 herdr-app 的结构文档互通。

代码 diff 审阅通过 `mod.open` / 自定义外部 Pane 接 revdiff 或 Plannotator；不在 TUI 核心实现审阅器。
输入、渲染与 SSH 延迟可靠性优先于主题扩展和手机界面，详见 [工作主线](docs/priorities.md)。

## 附录：源码构建

贡献代码或维护 VT 时才需要 Rust stable 与 Zig 0.16.0（当前 vendored VT 不支持 0.17）。

```sh
mise install
mise exec -- cargo build --release -p hc-tui
```

模块结构、测试与发行打包见 [开发与部署](docs/development.md)。
