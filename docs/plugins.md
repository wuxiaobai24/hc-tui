# 插件（plugins）规格 v0.1

[返回 README](../README.md) · [界面模块](mods.md) · [命令](commands.md) · [开发计划](plan-extensions.md) · [开发与部署](development.md)

版本：v0.1（2026-11-06）
状态：实施中。M6（清单与动作、CLI、代际注册）已实现；终端入口（§5–§6）随 M7，事件流（§8）随 M8。清单错误格式为 `文件:行:列: 原因`。
定位：可分发的工作流包，提供一次性动作与真实终端入口。界面由 [界面模块](mods.md) 负责。

---

## 1. 定位与边界

插件是一个目录，包含清单与可执行入口。动作由 hc-tui 在客户端主机启动，进程退出即结束；终端入口由 core 在 Space 的机器上创建真实 PTY，复用现有终端能力。

| 能做 | 不能做 |
|---|---|
| 声明动作、菜单上下文、终端入口、最低版本与平台 | 绘制 hc-tui 界面；声明界面模块 |
| 任意语言编写动作与终端程序 | 依赖 core 未提供的方法、建立第二条 core 连接 |
| 通过 `hc` 与 `hc-tui` CLI 访问工作台、远端机器与文件 | 修改共享布局文档、绕过动作分发器 |
| 生命周期、日志、并发与超时受宿主约束 | OS 沙箱；插件以用户身份运行 |

插件与界面模块相互独立：插件不贡献 TSX 模块，模块不携带可执行动作。两者共用动作分发器、状态快照与控制 socket。

## 2. 目录与清单

```
~/.config/hc-tui/plugins/<name>/
  plugin.toml       清单
  bin/...           可执行文件与脚本
  README.md
```

清单使用标准 TOML 解析器（新增 `toml` 依赖），严格校验：未知键、未知表、重复 `id`、非法 `id`、空 `argv` 都直接拒绝加载。

```toml
manifest_version = 1
id = "dev.example.git-tools"
name = "Git tools"
version = "0.1.0"
host_api = 1
platforms = ["linux", "macos"]
description = "工作区 Git 摘要与 lazygit 入口"

[[actions]]
id = "summary"
title = "Git 摘要"
description = "读取当前 Space 的 Git 状态并在 toast 中显示"
contexts = ["global", "space", "pane"]
argv = ["python3", "./summary.py"]
timeout_ms = 10000

[actions.args]
type = "object"
properties = { short = { type = "boolean", default = true } }
additionalProperties = false

[[terminals]]
id = "git-ui"
title = "Git UI"
argv = ["lazygit"]
placement = "tab"            # tab | right | down | float | popup | zoomed
requires_programs = ["lazygit"]
```

| 字段 | 说明 |
|---|---|
| `manifest_version` | 清单格式版本，当前为 1 |
| `host_api` | 宿主接口版本，与 hc-tui 内嵌版本不一致时拒绝加载 |
| `platforms` | `linux` / `macos`；不匹配时跳过并给警告 |
| `[[actions]].contexts` | `global`（命令面板与 `do`）、`space`（侧栏 Space 菜单）、`pane`（Pane 右键菜单） |
| `[[actions]].argv` | 工作目录为插件目录；`./` 开头的程序相对插件目录解析 |
| `[[actions]].args` | JSON Schema 子集：`type`、`properties`、`required`、`enum`、`default`、`additionalProperties` |
| `[[terminals]].placement` | 终端在 Tab 中的打开方式，映射到现有 `terminal.open`、`pane.split`、浮动与 popup |
| `[[terminals]].requires_programs` | 启动前在目标机器检查 PATH，缺失时报 `missing_program` 并列出缺失项 |

动作名 `<package-id>.<action-id>`，终端名 `<package-id>.<terminal-id>`。包 id 允许点号做命名空间，元素 id 限 `[a-z0-9_-]{1,32}`。内置命令与动作名不可被覆盖。

## 3. 身份

| 身份 | 含义 | 生命周期 |
|---|---|---|
| Package | 已注册的插件：id、版本、来源、代际、启用状态 | 安装到卸载 |
| Generation | 一次安装的代码目录，`current` 指针指向当前代际 | 创建到最后一个引用释放 |
| Invocation | 一次动作运行：请求 id、状态、退出码、日志 | queued → running → done / failed / cancelled / timed_out |
| Terminal | core 中的真实终端地址 | 按现有 keep / park / kill 契约 |
| TerminalPane | TUI 中承载终端入口的 Pane | 创建到关闭；关闭不删除终端 |

进程内 u32 Pane 编号只作本地视图句柄；跨实例、跨机器使用包 id、代际、终端地址与 Space 稳定 ID。

## 4. 动作执行

- 动作在客户端主机运行：cwd 为插件目录，环境为宿主进程环境加上注入变量。
- 上下文通过 stdin 传入一个 JSON 文档，读完即关闭 stdin；stdout 与 stderr 作为日志捕获，不作为协议。
- 工作区操作通过 `hc exec --on <machine> --cwd <folder>`、`hc-tui do`、`hc-tui read`、`hc-tui ls` 完成；动作进程本身不 chdir 到远端路径。
- 注入变量：`HCTUI_PLUGIN_ID`、`HCTUI_PLUGIN_ROOT`、`HCTUI_PLUGIN_CONFIG_DIR`、`HCTUI_PLUGIN_STATE_DIR`、`HCTUI_BIN`、`HCTUI_SOCKET`、`HCTUI_SESSION`、`HCTUI_CONFIG`、`HCTUI_CONTEXT_ID`、`HCTUI_HOST_API`。宿主注入的键不会被参数覆盖。
- 时限：默认 30 秒，`timeout_ms` 上限 600000；到期限发送 SIGTERM，2 秒后 SIGKILL，进程组一并结束。
- 取消：`hc-tui plugins action cancel <request>` 触发同样的终止流程。
- 输出：stdout 与 stderr 各保留最近 64 KiB，日志文件按包轮转保留两份各 1 MiB；超出继续读取以免堵塞管道，只在内存与界面显示中截断。
- 并发：全局 8 条、单包 4 条；超出排队，队列上限 64 条，满了返回 `busy`。
- 退出码 0 为成功；非零在界面提示 stderr 末尾若干行，并记录 `failed`。

### 上下文文档

```json
{
  "api_version": 1,
  "request": "r17",
  "package": "dev.example.git-tools",
  "action": "summary",
  "args": { "short": true },
  "context": {
    "space": { "id": "sp1", "name": "api", "machine": "peer", "folder": "/srv/api" },
    "tab": { "id": "tb1" },
    "pane": { "id": "7", "terminal": "peer/t7" },
    "target": { "pane": "7", "terminal": "peer/t7", "kind": "pane" }
  },
  "machine": { "client": "box", "core": "box", "workspace": "peer" },
  "locale": "zh"
}
```

- `target` 是触发动作的对象：右键菜单为被点击的 Pane 与终端，Space 菜单为 Space，命令面板为当前焦点。
- 目标在调用时冻结；动作执行期间焦点变化不影响 `target`。
- 对象已删除时返回 `target_gone`；断线或机器不可达返回 `unavailable`，不换机器执行。

## 5. 终端入口

- 通过 core `terminal.run {argv, cwd, on, env}` 在 Space 的机器上创建真实 PTY，随后按 `placement` 放入 Tab、分屏、浮动或 popup。
- `argv[0]` 以 `./` 开头时相对代际目录解析，只允许在安装所在的客户端主机上启动；目标机器不同时返回 `unsupported` 并说明原因。
- PATH 中的程序允许在任何已连接机器上启动，启动前按 `requires_programs` 检查。
- 注入变量与动作相同；另加 `HCTUI_CONTEXT`（JSON，≤32 KiB）与 `HCTUI_PLACEMENT`。大块数据通过文件或 CLI 获取，不塞进环境变量。
- 终端标签写入 `herdr.space`、`herdr.tab`、`where` 与 `hctui.plugin=<package-id>`、`hctui.terminal=<terminal-id>`；`plugins pane focus/close` 按标签定位。
- 关闭 Pane 不结束程序，按现有 keep / park 语义；`terminal.kill` 才结束程序与子进程。
- 不自动上传插件目录、不执行远端安装、不复制凭证、不转发本机 socket。需要回调客户端时走 `hc-tui` CLI 或事件流。

## 6. 多机

| 位置 | 含义 | 插件在哪一侧运行 |
|---|---|---|
| `client` | hc-tui 进程所在主机 | 动作进程、控制 socket、插件代码 |
| `core` | 当前连接的入口 core | JSON-RPC 与 `hc` CLI |
| `workspace` | Space 所属机器 | 终端程序与工作区命令 |

- 动作默认以 `client` 为宿主，通过参数里的 `workspace` 与 `folder` 决定远端操作的目标。
- 远端操作使用 `hc exec --on`、`hc-tui do`，文件使用 `hc file read/write --on` 或 `hc-tui read`。
- 终端入口在 `workspace` 上启动；包内相对程序仅限 `client` 与安装同机的场景。
- 离线、无权限、目标不存在分别返回 `offline`、`forbidden`、`target_gone`，不静默回退到本机。

## 7. 命令与结果

`commands` 目录包含内置动作与插件动作；插件动作带 `source = "plugin:<id>"`、参数 schema 与上下文。
M6 起 `hc-tui commands` 输出同样带 `source`、`contexts` 与 `args`；`plugins action list --json` 每项包含
`name/package/action/title/description/contexts/args/source`。`plugins action status` 返回
`status`、`exit_code`、`detail`（失败或超时原因）、`stdout`/`stderr` 尾部与 `queued_ms`/`runtime_ms`。

```sh
hc-tui plugins action list [--json]
hc-tui plugins action invoke <id>.<action> '{"short":true}' [--pane t7]
hc-tui plugins action status <request>
hc-tui plugins action cancel <request>
hc-tui plugins action log <request> [-n 100]
hc-tui plugins pane open <id>.<terminal> [--cwd DIR] [--machine M] [--placement P]
hc-tui plugins pane focus <terminal>
hc-tui plugins pane close <terminal>
```

结果语义：

- `accepted` 表示已排队；`confirmation_pending` 表示仍需界面确认；两者都不等于完成。
- 完成状态为 `done`、`failed`、`cancelled`、`timed_out`，附退出码与日志末尾。
- core 操作或布局 CAS 失败时，动作以 `failed` 结束，`error.kind` 区分 `conflict`、`offline`、`unavailable`。

命令面板、`[[keys.command]] type = "action"` 与 `hc-tui do` 使用同一分发器；插件动作在启用前不出现在目录中。

## 8. 事件

```sh
hc-tui events --plugin <id> --kinds terminal.status,terminal.turn,terminal.finished
```

- 控制 socket 的长连接，按行输出 NDJSON，客户端断开即停止；插件在动作进程或终端程序中读取。
- 作用域是本客户端：另一台机器或另一个 hc-tui 实例各收到一次。
- 投递不保证恰好一次；队列上限 256 条，溢出时丢弃并补发 `{"kind":"events.dropped","count":N}`，随后应由消费者重新读取状态。
- 事件负载带 `machine`、限定终端地址、`turn`、事件产生时间与相关性 id。
- 邮件、部署、通知外部服务等副作用不绑定到多客户端事件流；需要单次执行时用独立 runner（systemd 单元、远端分离进程）读取同一 `hc-tui events` 或直接调用 core。

## 9. 安装、更新与卸载

| 操作 | 行为 |
|---|---|
| `plugins link <目录>` | 注册为开发来源，代码原地读取，清单变化后重载 |
| `plugins install <目录>` | 预览清单后复制到新代际，`current` 原子切换，默认不启用 |
| `plugins update <id> <目录>` | 写入新代际；运行中的调用与终端继续使用旧代际，引用释放后回收 |
| `plugins unlink <id>` | 移除开发注册，不删除目录 |
| `plugins uninstall <id>` | 先禁用；有运行中的终端时保留代际直到它们结束 |
| `plugins enable` / `plugins disable <id>` | 只改变是否出现在命令目录与菜单 |

`records/<id>.json` 记录来源、清单哈希、代际、启用状态与安装时间。安装前显示 `id`、版本、`host_api`、平台、动作与终端入口、`requires_programs`；确认后才注册。构建失败不替换旧代际；`current` 指针切换使用临时目录加 rename。

## 10. 信任与权限

- 插件是以用户身份运行的受信本地代码，没有 OS 沙箱；`capabilities` 不作为隔离承诺。
- 信任边界是安装来源、安装前预览与显式启用。共享文档、界面模块或远端布局中的插件引用不自动获得执行权。
- 键位由用户绑定到命令名；插件不能静默改写全局键位，也不能覆盖内置动作。
- 日志、配置与状态目录由宿主创建，权限 0700；插件目录只读挂载不强制，路径由清单解析。

## 11. 生命周期与故障

- 注册表在启动时扫描配置目录与 state 中的安装记录；清单无效时保留记录并标记 `broken`，不执行。
- 动作进程按调用创建与回收；宿主退出时等待运行中的调用完成或超时，终端继续由 core 持有。
- 连续三次启动即失败或超时的包标记 `failed`，停止自动重试；`plugins enable` 或下一次显式调用恢复。
- 崩溃与超时写日志并附告警；进程组、管道与子进程一并回收，阻塞在输出管道上的后代不会拖住完成报告。
- 卸载或更新不触碰用户的终端与动作日志；只回收代码代际。

## 12. 与界面模块、脚本面板的关系

| 层 | 执行 | 界面 | 分发 |
|---|---|---|---|
| 插件 | 独立进程 | core 终端或 CLI 输出 | 目录、代际、显式启用 |
| 界面模块 | hc-tui 进程内 | TSX 外框区域、Pane、覆盖层 | nsh bundle 或签名包 |
| 脚本面板 | core 终端 | ANSI 文本 | `[[panels]]` 与 panels/*.json |

共享布局中的 module 节点继续保留原始 JSON 与未知字段；插件不声明 module 名，模块需要展示插件数据时通过 `hc-tui` CLI 或动作读取。真实插件终端在共享结构中是普通终端地址，各客户端可 attach。

## 13. 限制

- 不提供插件市场、远程安装、自动更新与跨客户端单次执行保证。
- 包内相对程序只在安装主机有效；远端入口只接受目标机器 PATH 中的程序。
- 动作 stdout 不作为结构化协议；需要交互界面时使用终端入口或界面模块。
- 生命周期跟随 hc-tui 客户端；detach 后动作进程结束，终端继续由 core 保留。
- 不兼容旧 Herdr plugin 的 manifest、CLI 与 socket 语义。

## 14. 验收

- 两个真实插件：`git-tools`（动作读取远端 Git 摘要，终端入口在远端开 lazygit）与 `review`（长运行终端，验证 cwd、退出、park、detach 后保留）。
- 目标冻结：动作执行中切换焦点，实际请求仍指向调用时的 Space 与 Pane。
- 多机：远端动作与终端在正确机器执行；缺失程序、离线、目标删除各自给出明确错误，不回落本机。
- 超时与取消：SIGTERM 后 2 秒 SIGKILL，进程组与后代管道全部回收，完成状态为 `timed_out` 或 `cancelled`。
- 并发与队列：超过全局与单包上限时排队，队列满返回 `busy`，界面仍可输入。
- 输出洪泛：持续输出超过 64 KiB 时内存有界、管道不堵塞、日志轮转正常。
- 更新：新代际切换后旧调用与旧终端继续运行；失败安装不影响旧代际；卸载不删除用户终端。
- 事件：两个客户端各收到一次，断开即停；队列溢出发出 `events.dropped`。
- 命令目录：插件动作带参数 schema 与来源；禁用后从目录与菜单消失；内置动作不可覆盖。
- 旧配置：`[[panels]]`、`[[keys.command]]`、`mod.open` 行为不变。

## 15. 实施清单

新增：

| 文件 | 内容 |
|---|---|
| `src/plugin/manifest.rs` | 标准 TOML 解析、严格校验、命令描述与参数 schema |
| `src/plugin/registry.rs` | 安装记录、代际、启用状态、来源与预览 |
| `src/plugin/runner.rs` | 动作进程、上下文 stdin、超时、取消、进程组回收、有界输出 |
| `src/plugin/logs.rs` | 运行日志、轮转与查询 |
| `src/plugin/cli.rs` | `plugins` 子命令 |
| `src/app/plugin.rs` | 菜单与命令目录接入、上下文构造、结果展示 |
| `docs/plugins.md` | 本文 |

修改：

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | `toml` 依赖 |
| `src/app/actions.rs` | `COMMANDS` 增加 `plugins.*`；分发器接入插件动作 |
| `src/app/interaction.rs` | Pane 与 Space 菜单加入上下文匹配的插件动作 |
| `src/control.rs` | 动作调用、状态查询、取消、`events` 长连接 |
| `src/main.rs` | `plugins` 子命令与 `events` 子命令 |
| `src/config.rs` | `plugins_dir()`、安装记录路径、启用状态 |
| `src/i18n/*` | 提示文案 |
| `README.md`、`docs/development.md` | 文档导航与模块表 |

## 16. 版本

- `manifest_version` 描述清单格式，`host_api` 描述宿主接口，`version` 是包自身的 semver；三者分别校验。
- hc-tui 升级 `host_api` 时保留对旧版本的加载与诊断，不静默降级动作语义。
- 清单与命令目录随版本生成 `--plugins-schema` 输出，供作者与 Agent 校对。

## 17. 参考

- 旧 Herdr plugin 清单校验：`https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/app/api/plugins/manifest.rs`
- 运行环境与上下文：`https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/app/api/plugins/runtime.rs`、`.../context.rs`
- 安装代际与租约：`https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/plugin_installations.rs`
- 终端入口：`https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/app/api/plugins/panes.rs`
