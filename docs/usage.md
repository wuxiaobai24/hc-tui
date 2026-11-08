# 使用指南

[返回 README](../README.md) · [配置](configuration.md) · [命令](commands.md) · [包与插件](plugins.md)

站点与这里互为入口：在 `site/` 里执行 `npm run build` 后，站点入口是 `site/dist/index.html`
（仓库中的页面源码是 [site/src/pages/index.astro](../site/src/pages/index.astro)）；
站点五篇文档逐节标注了摘录自哪份 `docs/` 文件。

## 安装与首次运行

```console
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 4m 32s

$ ./target/release/hc-tui --version
hc-tui 0.4.3
```

运行前提是 PATH 里有 `hc`（herdr-core），本仓库实测的协议基线是 core 0.1.18。
直接运行 `./target/release/hc-tui` 打开窗口；要独立会话就带 `HC_SESSION`：
`HC_SESSION=hcp-docs2 ./target/release/hc-tui`（会话隔离的做法见[配置说明](configuration.md#会话隔离)）。

首次运行装一个包。`examples/packages/git-board` 同时贡献动作、终端入口与界面模块：

```console
$ ./target/release/hc-tui packages install examples/packages/git-board --enable --yes
package dev.hctui.pkg.git-board (Git board 0.1.0)
  provides ATM
  actions dev.hctui.pkg.git-board.summary
  terminal dev.hctui.pkg.git-board.git-ui (right)
    requires lazygit
  module dist/bundle.js (right)
  platforms linux, macos
  grants state, exec, notify, state.changed, grant.changed, terminal.status
installed dev.hctui.pkg.git-board generation 1 (enabled)
```

安装默认不启用，`--enable` 才打开动作、终端入口与模块表面。没有终端（管道、CI、脚本）
时还要 `--yes`，否则只报 `Error: pass --yes to install without a terminal`。
发行包的安装与升级见[安装与升级](#安装与升级)。

## 包与安装类命令

`packages` 是包注册与代际管理的唯一入口。`plugins <子命令>` 与
`mods install|update|uninstall|link|unlink|enable|disable` 是别名：行为与退出码一致，
只在 stderr 多一行提示；`plugins action` 与 `plugins pane` 是动作与终端专用入口，名字不变。

```console
$ hc-tui plugins list
note: `plugins list` is deprecated, use `packages list`
dev.hctui.demo.form              local     gen 1   enabled  A
```

| 子命令 | 作用 |
|---|---|
| `packages list [--json]` | 一行一个包；启用状态后是能力徽章 `A`（动作）/`T`（终端入口）/`M`（界面模块），有服务时跟 `s:在跑/总数` |
| `packages info <id\|来源> [--json]` | id、名字、版本、来源、代际、能力、模块、服务、授予、安装时间与来源端版本 |
| `packages install <来源> [--enable\|--disable] [--yes]` | 校验清单后复制到新代际并切 `current`，默认不启用 |
| `packages update [<id>] <来源>` | 写入新代际，保留记录里已有的启用状态 |
| `packages update --check [<id>] [--json]` | 只读比较记录里的版本与来源端版本 |
| `packages link <目录>` | 注册开发来源，代码原地读取，清单变化后重载 |
| `packages unlink <id>` | 移除开发注册，目录原样保留 |
| `packages enable\|disable <id>` | 只改记录里的启用状态，动作、终端入口与模块表面同时生效 |
| `packages gc` | 回收没有记录指向、也没有 pin 的代际 |
| `packages log <id> [--service <服务>] [-n 行数]` | 读 `engine.log` 或服务清单里 `log` 声明的文件尾部 |
| `packages uninstall <id>` | 删记录与 `current` 指针；有 pin 的代际留给 `gc` |

```console
$ hc-tui packages list
dev.hctui.pkg.git-board          local     gen 1   enabled  ATM
    dev.hctui.pkg.git-board.git-ui           right · Git UI
    dist/bundle.js                           module · Git board
```

```console
$ hc-tui packages info dev.hctui.pkg.git-board
dev.hctui.pkg.git-board (Git board 0.1.0)
  source       examples/packages/git-board
  generation   1 · enabled
  path         /tmp/hcp-docs2/xdg/state/hc-tui/plugins/store/dev.hctui.pkg.git-board/1
  capabilities ATM
  module       dist/bundle.js · right · Git board
  services     none
  installed    1794124380 (unix)
  update       up to date (0.1.0)
```

```console
$ hc-tui packages update dev.hctui.pkg.git-board examples/packages/git-board --yes
  terminal dev.hctui.pkg.git-board.git-ui (right)
    requires lazygit
installed dev.hctui.pkg.git-board generation 2 (enabled)
```

`--check` 只读，两种状态的退出码都是 0：

```console
$ hc-tui packages update --check
dev.hctui.demo.svc up to date (0.1.0)
dev.hctui.pkg.git-board local 0.2.0 → remote 0.3.0
```

```console
$ hc-tui packages link /tmp/hcp-docs2/dev-board --yes
linked dev.hctui.pkg.git-board (enabled)

$ hc-tui packages list
dev.hctui.pkg.git-board          linked    gen 1   enabled  ATM

$ hc-tui packages unlink dev.hctui.pkg.git-board
unlinked dev.hctui.pkg.git-board
```

`link` 的包在 `packages list` 里来源列是 `linked`，`info` 的 `path` 直接指向那个目录。

```console
$ hc-tui packages disable dev.hctui.demo.svc
disabled dev.hctui.demo.svc

$ hc-tui packages enable dev.hctui.demo.svc
enabled dev.hctui.demo.svc
```

停用只改记录：已排队的调用跑完，已开的终端不结束，新的动作、终端入口与模块表面被拒绝（`error_kind = "disabled"`）。

```console
$ hc-tui packages gc
dev.hctui.pkg.git-board: 1

$ hc-tui packages gc
no generations to reclaim
```

```console
$ hc-tui packages log dev.hctui.demo.svc --service watch
watcher up
watcher up

$ hc-tui packages log dev.hctui.demo.svc --service nope
Error: dev.hctui.demo.svc declares no service nope
```

日志文件还不存在时打印 `no log for <id>: No such file or directory` 并退出 0；服务日志的落点见[配置说明](configuration.md#文件与优先级)。

```console
$ hc-tui packages uninstall dev.hctui.pkg.git-board
uninstalled dev.hctui.pkg.git-board
```

### 从来源安装

三种来源按输入形态判定：

| 来源 | 判据 | 落地方式 |
|---|---|---|
| 本地目录 | 路径是目录 | 直接复制到新代际 |
| git 仓库 | `https://`、`http://`、`ssh://`、`file://`、`git://`、`git@` 开头或 `.git` 结尾 | `git clone --depth 1` 到临时目录 |
| 归档 | `.tar.gz` / `.tgz` | `tar -xzf` 解到临时目录 |

暂存目录按「有 `hctui.toml` / `plugin.toml` 的那一层」判根，最多下行三层；校验清单后才发到
新代际，失败时记录与 `current` 指针不动。`.nshpkg` 走签名包校验，见[包与插件 §9](plugins.md#9-安装更新与卸载)。

```console
$ hc-tui packages install file:///tmp/hcp-docs2/repo.git --enable --yes
package dev.hctui.pkg.git-board (Git board 0.2.0)
  provides ATM
  ...
  source file:///tmp/hcp-docs2/repo.git
installed dev.hctui.pkg.git-board generation 1 (enabled)
```

```console
$ hc-tui packages install /tmp/hcp-docs2/archive/git-board-0.1.0.tar.gz --yes
  grants state, exec, notify, state.changed, grant.changed, terminal.status
  source /tmp/hcp-docs2/archive/git-board-0.1.0.tar.gz
installed dev.hctui.pkg.git-board generation 1 (disabled)
```

`packages list` 的来源列只区分 `local`（从来源装出来的代际）与 `linked`（开发来源）；
真实来源记在记录的 `origin` 里，`packages info` 与 `update --check` 都按它回读。

### 常驻服务

清单里的 `[[services]]` 是要活过窗口的进程，宿主把它物化成 core 的 keep 终端，
同一 `(包, 服务)` 至多一个活终端。`on = "install"` 只在安装、更新后拉起，
`on = "enable"`（默认）在启用与窗口重开时补齐；没有 `restart`，已结束的终端记录不会被自动重起。
字段与不变量见[包与插件 §5.1](plugins.md#51-常驻服务)。

`packages list` 的状态由 core 的终端清单推出：`running`（还没拿到 pid 时是 `starting`）、
`exited`、`failed`（启动后 1 秒内非 0 退出或 `argv` 程序缺失）、`stopped`（没有终端记录）：

```console
$ hc-tui packages list
dev.hctui.demo.form              local     gen 1   enabled  A
dev.hctui.demo.svc               local     gen 1   enabled  - s:1/1
    dev.hctui.demo.svc.watch                 service · running · t4 · pid 622005
dev.hctui.pkg.git-board          local     gen 2   enabled  ATM
    dev.hctui.pkg.git-board.git-ui           right · Git UI
    dist/bundle.js                           module · Git board
```

没有 core 时这一行会先打印 `no core for this session; service state comes from declarations only`，
只按声明推状态，不按需拉起 core。

## 无窗口 `do`

控制 socket 连不上（没有窗口）时，`do` 直接读改本地 core 的布局文档，
写入走 `hctui-layout-v1` 的 CAS 与三方合并。参数顺序是
`do <动作名> [<终端地址>] [JSON 对象] [--session] [--config]`，省略 JSON 按 `{}` 处理。

```console
$ hc-tui do space.new '{"name":"backend"}'
{"accepted":true,"confirmation_pending":false}
```

```console
$ hc-tui do space.rename '{"name":"api"}'
{"accepted":true,"confirmation_pending":false}
```

寻址终端的动作可以把终端地址写成裸位置参数：

```console
$ hc-tui do terminal.park t1
{"accepted":true,"confirmation_pending":false}
```

`accepted` 只表示已受理，拒绝都带 `error_kind`：

```console
$ hc-tui do pane.zoom
{"error":"pane.zoom needs a TUI window","error_kind":"needs_window","method":"pane.zoom"}
```

```console
$ hc-tui do tab.close '{"id":"1"}'
{"error":"tab.close needs input; pass it in the arguments","error_kind":"needs_input","method":"tab.close"}
```

```console
$ hc-tui do terminal.park t1 --session hcp-docs2-nocore
{"error":"no core for session hcp-docs2-nocore","error_kind":"no_core","method":"terminal.park"}
```

- `needs_window`：只改当前窗口屏幕的动作，焦点、缩放、方向移动、`space/tab/agent` 的浏览式切换、
  侧栏与覆盖层、需要本地 VT 的滚动与搜索、模块面板、`chat.*`。
- `needs_input`：动作要一行输入。确认类只认 `confirmed: true`（等价于窗口里敲 `y`），
  改名与新建类只认 `name`，给不出就拒绝，不把提问丢掉。
- `no_core`：这个 session 没有 core。无窗口路径不按需拉起 core；先在有窗口的客户端里连上，
  或按[配置说明](configuration.md#会话隔离)起一个自己的会话。
- `layout_error`：布局 CAS 冲突或写入没在超时内完成；冲突要先在有窗口时用 `layout.reload` 接受共享版本。

写入的效果在下次开窗口时可见，下面这一屏就是上面两次调用之后再打开窗口的侧栏：

```console
$ tmux capture-pane -p -t hcpdocs2 -S 0 -E 4 | cut -c1-25
 Spaces                + 
                         
 ▾ This computer      ●  
  A  api        hcp/docs2
  1  1                   
```

有窗口但界面不应答时报错返回，不转交无窗口路径，避免两个写入者同时改布局文档。

## 动作表单

命令面板或 Pane 右键菜单选中带 `args` schema 的插件动作时，先按 schema 弹表单：
`string` 单行输入、`string` + `enum` 单选菜单、`boolean` 开关、`integer` / `number` 数字输入、
嵌套 `object` 分组缩进、标量 `array` 逐项增删。底部提示是
`↵ submit  ⇥ next  esc cancel  ^a add  ^d remove`。

```console
$ tmux send-keys C-b : deploy Enter   # 命令面板里选中带参数的动作
$ tmux capture-pane -p -S 16 -E 20
                         ││              ┌──────────────────────────────────────────────────────────────┐              │
                         ││              │ 部署  dev.hctui.demo.form.deploy                             │              │
                         ││              │  count: 1▏                                                   │              │
                         │└──────────────│  mode: —  ◂ ▸                                                │──────────────┘
```

提交复用调度端同一套递归校验：非法值留在表单里，错误贴在出错行下面，焦点落到那一行，动作不派发。

```console
$ tmux send-keys BSpace abc Enter     # 把 count 改成非法值并提交
$ tmux capture-pane -p -S 16 -E 20
                         ││              ┌──────────────────────────────────────────────────────────────┐              │
                         ││              │ 部署  dev.hctui.demo.form.deploy                             │              │
                         ││              │  count: abc▏                                                 │              │
                         │└──────────────│    args.count must be integer                                │──────────────┘
```

表单打开期间，控制通道对同名动作回 `confirmation_pending = true`；`hc-tui do` 与无窗口路径不经过表单，
直接派发并返回真实 `request`。

## 事件订阅与 `--no-mods`

`hc-tui events` 是控制 socket 上的长连接，按行输出 NDJSON；`--plugin <id>` 只看该包的终端事件，
`--kinds a,b` 只看列出的 kind，`--count N` 收到 N 条后退出。每个订阅者 256 条队列，溢出补发
`events.dropped`。

```console
$ hc-tui events --kinds terminal.opened --count 1
（另一个终端）
$ hc-tui do terminal.open '{"argv":["sleep","300"],"place":"down"}'
{"accepted":true,"confirmation_pending":false}
（订阅端随后打印）
{"at_ms":1794124496755,"data":{},"kind":"terminal.opened","machine":"local","terminal":"t3","turn":0}
```

`--no-mods` 只跳过模块表面：动作目录、终端入口与已安装的包照常可用，外框回落到内置界面。
同一个 session、同一个已启用模块的包，两种启动方式的右栏：

```console
$ tmux capture-pane  # 默认启动（模块运行，右侧是 Git board）
 Spaces                + │Git board                          1      +                                            menu
                         │recent commits                  ┌───────────────────────────────────────────────── :: ─[...]─┐

$ hc-tui --no-mods
$ tmux capture-pane  # --no-mods
 Spaces                + │   1      +                                                                            menu
                         │┌───────────────────────────────────────────────────────────────────────────────── :: ─[...]─┐
```

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

### Kitty 键盘协议

`ui.keyboard_protocol` 默认 `auto`：启动时发 `CSI ? u` 探测，100ms 内收到
`CSI ? <flags> u` 就发 `CSI > 1 u` 打开 disambiguate；退出、panic、SIGTERM 时
发 `CSI < u` 恢复。`kitty` 跳过探测直接启用，`legacy` 完全不碰终端。

启用后，legacy 下会撞车的键才分得开：

| 键 | legacy | kitty |
|---|---|---|
| `ctrl+i` | 与 `tab` 同一个字节 `09` | `CSI 105;5u`，独立键 |
| `ctrl+m` | 与 `enter` 同一个字节 `0d` | 独立键 |
| `ctrl+[` | 与 `esc` 同一个字节 `1b` | 独立键 |

只开 disambiguate 一个标志，不请求 key release 与 alternate keys，所以松开按键
不会额外产生事件。宿主不支持时按键仍是原来的样子：`ctrl+i` 照旧当 `tab`。
`hc-tui keys probe [--protocol auto|kitty|legacy] [--seconds n]` 打印宿主回复的原始
字节与解出的按键，`hc-tui keys decode <hex>` 解一段十六进制字节。


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
| Agent 行 | 左键跳转；右键打开重命名、共享置顶等菜单 |
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
macOS 使用系统自带的剪贴板接口，无需安装 pngpaste；Linux 需要 wl-paste / xclip。通过 SSH 运行 TUI 时读取的是远端剪贴板。

Mac 系统截图直接入剪贴板：按 Control+Shift+Command+4 框选，然后在本机运行的 hc-tui 中按 Ctrl+B，松开后按 Shift+V；终端 Pane 和 Chat 都支持。图片先上传到目标 Pane 所在机器，成功后插入那台机器上的路径。普通 Command+V 是否能携带图片由宿主终端决定，不能替代这个专用按键。

更多点击入口、拖拽与可选 Zellij 模式见 [交互操作](interaction.md)。

## 手机与窄屏

宽度不超过 `mobile_width_threshold`（默认 64）且鼠标捕获开启时使用单 Pane 布局。
顶栏显示连接、工作区与 Tab 信息，switch 打开工作区、Tab、Pane、Agents 与动作菜单。
Pane 段按当前 Tab 的布局顺序列出全部 Pane（浮动层标 `浮动`），点哪一行就把焦点交给哪个 Pane，
不依赖 Pane 里有没有 agent；Agents 段仍只收有 agent 状态的 Pane，用于跨工作区看 Attention。
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
- **`hc-tui ls/do` 连接不上？** `ls` 要有运行中的窗口；`do` 在没有窗口时走无窗口路径，依赖同一 session 已有的 core（没有就报 `no_core`，不按需拉起）。两者都要保持同一 XDG_STATE_HOME，自定义 session 每次传入。
- **看到 another TUI already accepts commands？** 同一状态目录和 session 只有一个控制 socket 服务；第二个客户端仍可打开，但 CLI 控制的是首个。首个窗口的终端已经关掉、进程还在时，`ls`/`do` 会等满超时并报 `UI did not answer`，先结束那个进程再重开窗口。
- **英文输入或滚动时疑似重复？** 本版加入真实后端回放和整帧同步输出，尚未稳定复现该报告。区分程序输出实际重复与显示暂留；保留程序名、宿主和复现步骤便于继续定位。
- **退出再进布局变了？** 检查 session、restore 配置、core 文档和是否同时有 GUI 写入；同字段冲突用 layout.reload 明确接受共享版本。

## 侧栏导航

点击机器标题左侧的 ▾ / ▸ 或整行标题，可折叠/展开该机器的全部 Spaces，状态保存在本地并在重启后恢复。
Spaces 与 Agents 分区独立滚动，将鼠标移到相应区域滚动即可。空间以单行显示，彩色首字母标识和整行背景突出当前空间；分支在有足够宽度时显示，Tab/Pane 数量仍可在详细切换视图查看。

### 远端 Pi Chat

`machine/t7` 的 Chat 通过现有 core 的 `file.read {on: machine}` 异步、按行增量读取 session label 指定的路径，不直接打开本机同名文件。上限保留最近 6000 行；读取失败保留已加载内容并明确标明失败；半行 JSONL 等写完再显示，会话切换后丢弃旧请求的回包。

在 Pi 实际运行的机器执行 `hc-tui session-hook install`，然后在 Pi `/reload`。Pi 必须运行在 core 管理的终端内。也可以用 `hc label <terminal> session=/absolute/path.jsonl` 显式映射。
远端没有 label 时显示“远端会话未映射”，不会推断本机路径；无法读取时显示“远端读取失败”。Chat 面板里的本地 hook 操作不会写到错误机器。
`ssh -t box 'hc-tui --session work'` 仍读取 box 上的同一 core/session 文档；直接用 `hc-tui --session work` 则通过该 core 已配置的 machine 链路汇总远端工作区。

远端 Chat 的命令面板只列内置 Pi 命令，不把本机的 prompt/skill 扫描结果冒充远端可用命令。

## 安装与升级

```sh
curl -fsSL https://github.com/wuxiaobai24/hc-tui/releases/latest/download/install.sh | sh
hc-tui update --check
hc-tui update
```

首次安装自动选择 Linux/macOS、amd64/arm64 的正式发行包，默认写入 `~/.local/bin`。
可用 `sh -s -- --bin-dir /chosen/bin --version 0.4.2` 指定目录和版本，或设置 `HC_TUI_INSTALL_DIR`。
安装器和更新器均验证发行包 SHA-256 与可执行版本，使用原子替换；失败时不会覆盖原安装。
`hc-tui update` 更新当前正在执行的二进制所在位置，重开客户端后生效，core 中的程序和配置保留。

### Mac 文件路径粘贴到远端 Pane

Mac 宿主可能把图片复制为 `/Users/.../截图.png`、带引号/转义的路径，或 `file://` URL。
普通粘贴现在识别这些独立图片路径，读取 **运行 TUI 的本机文件**，通过 `terminal.put` 将字节传给目标终端所在机器，等待成功后再粘贴返回的目标路径。PNG/JPEG/GIF/WebP 都按实际格式保留扩展名，最大 8 MiB。
Chat 模式下返回路径进入原 Chat 的输入框，不直接输入底层 shell；期间关闭 Chat 时只提示已上传位置。
不可读、超限、无效图片或 RPC 失败会显示未上传/上传失败，原始本机路径不会继续发给 agent。普通文字及非图片路径维持原处理；`terminal.paste` 可显式发送不作附件识别的字面文本。

这条链路适用于 Mac 本机 TUI + 远端 Pane。若在 SSH 里运行 TUI，远端进程无法读取 Mac 的 `/Users/...`，需要由 Mac 宿主先提供文件字节；单独的路径文本不包含图片。
