# 插件（plugins）规格 v0.1

[返回 README](../README.md) · [界面模块](mods.md) · [命令](commands.md) · [开发计划](plan-extensions.md) · [开发与部署](development.md)

版本：v0.3（2026-11-07）
状态：实施中。M6（清单与动作、CLI、代际注册）、M7（终端入口、多机、`plugins pane`、代际引用）、M8（事件流）与 M10（发布事务、启用闸门、参数校验纵深）已实现。清单错误格式为 `文件:行:列: 原因`。
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
| `[[actions]].args` | JSON Schema 子集：`type`、`properties`、`required`、`enum`、`default`、`additionalProperties`、`items`；逐层嵌套校验，深度上限 8、单层属性上限 64，`items` 只接受 object 或标量 schema |
| `[[terminals]].placement` | 终端在 Tab 中的打开方式，映射到现有 `terminal.open`、`pane.split`、浮动与 popup |
| `[[terminals]].requires_programs` | 启动前由实际执行位置（core）在目标机器检查 PATH，缺失时报 `missing_program` 并列出缺失项 |

动作名 `<package-id>.<action-id>`，终端名 `<package-id>.<terminal-id>`。包 id 允许点号做命名空间，元素 id 限 `[a-z0-9_-]{1,32}`。内置命令与动作名不可被覆盖。

## 3. 身份

| 身份 | 含义 | 生命周期 |
|---|---|---|
| Package | 已注册的插件：id、版本、来源、代际、启用状态 | 安装到卸载 |
| Generation | 一次安装的代码目录，`current` 指针指向当前代际 | 创建到记录不再指向它且没有任何 pin |
| Pin | 一份显式租约：一次动作调用（排队中也算）或一个带 core/session 身份的终端地址 | 登记到持有者结束；终端只在 core 报告终结时释放 |
| Invocation | 一次动作运行：请求 id、状态、退出码、日志 | queued → running → done / failed / cancelled / timed_out |
| Terminal | core 中的真实终端地址 | 按现有 keep / park / kill 契约 |
| TerminalPane | TUI 中承载终端入口的 Pane | 创建到关闭；关闭不删除终端 |

进程内 u32 Pane 编号只作本地视图句柄；跨实例、跨机器使用包 id、代际、终端地址与 Space 稳定 ID。

## 4. 动作执行

- 动作在客户端主机运行：cwd 为插件目录，环境为宿主进程环境加上注入变量。
- 上下文通过 stdin 传入一个 JSON 文档，读完即关闭 stdin；上限 256 KiB，超过时在调用入口按 `invalid` 拒绝。stdin 写入、stdout 与 stderr 的读取各在独立线程里，同样受取消与 deadline 约束，父进程不会堵在管道上。stdout 与 stderr 作为日志捕获，不作为协议。
- 工作区操作通过 `hc exec --on <machine> --cwd <folder>`、`hc-tui do`、`hc-tui read`、`hc-tui ls` 完成；动作进程本身不 chdir 到远端路径。
- 注入变量：`HCTUI_PLUGIN_ID`、`HCTUI_PLUGIN_ROOT`、`HCTUI_PLUGIN_CONFIG_DIR`、`HCTUI_PLUGIN_STATE_DIR`、`HCTUI_BIN`、`HCTUI_SOCKET`、`HCTUI_SESSION`、`HCTUI_CONFIG`、`HCTUI_CONTEXT_ID`、`HCTUI_HOST_API`。宿主注入的键不会被参数覆盖。
- 时限：默认 30 秒，`timeout_ms` 上限 600000；到期限发送 SIGTERM，2 秒后 SIGKILL，进程组一并结束。
- 取消：`hc-tui plugins action cancel <request>` 触发同样的终止流程。
- 输出：stdout 与 stderr 各保留最近 64 KiB；落盘时单份日志超过 1 MiB 轮转，每个包最多保留 32 个日志文件、总计 8 MiB，超出删最旧的请求日志。超出继续读取以免堵塞管道，只在内存与界面显示中截断。
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
- 目标在调用时冻结：Space、Tab、机器与 cwd 一次解析成不可变的 `ExecutionTarget`，上下文与环境都由它生成；动作执行期间焦点变化不影响 `target`。
- 对象已删除时返回 `target_gone`：显式给出的 Pane 或终端不存在、数字 Pane 不存在、Space 已移除都直接失败，**不回退到当前焦点**；断线或机器不可达返回 `unavailable`，不换机器执行。

## 5. 终端入口

- 通过 core `terminal.run {argv, cwd, on, keep, env}` 在 Space 的机器上创建真实 PTY，随后按 `placement` 放入 Tab、分屏、浮动或 popup。
- `placement` 映射：`tab` 新 Tab；`right` / `down` 在当前 Pane 右/下分屏；`float` 分屏后浮起；`popup` 覆盖层（80%）；`zoomed` 新 Tab 并放大。分屏没有空间时返回 `unavailable`，不改成别的布局。
- `argv[0]` 以 `./` 开头时相对代际目录解析，只允许在安装所在的客户端主机上启动；目标机器不同时返回 `unsupported` 并说明原因，不回落本机。包内文件缺失报 `missing_program` 并给出解析到的路径。
- PATH 中的程序允许在任何已连接机器上启动。`requires_programs` 与 PATH 形式的 `argv[0]` 都在目标机器上检查：本机按 PATH 逐一查找，远端用 `machine.exec` 执行 `command -v` 探测；缺失时报 `missing_program` 并列出缺失项。清单里这两类程序名限 `[A-Za-z0-9._+-]`，探测脚本按词引用。
- 注入变量与动作相同；另加 `HCTUI_CONTEXT`（JSON，≤32 KiB，超出时退化成 `{"api_version":1,"truncated":true}`）与 `HCTUI_PLACEMENT`；`HCTUI_CONTEXT_ID` 用完整入口名。大块数据通过文件或 CLI 获取，不塞进环境变量。
- 终端标签在 attach 成功后写入 `hctui`、`hctui.plugin=<package-id>`、`hctui.terminal=<terminal-id>` 与 `where`；Space 与 Tab 有共享结构来源时同时写 `herdr.space`、`herdr.tab`。`plugins pane focus/close` 先按 Pane 编号与内存登记匹配，再按 core 里的这两条标签定位，所以视图关闭后仍能找回终端。
- 关闭 Pane 不结束程序，按现有 keep / park 语义；`terminal.kill` 才结束程序与子进程。
- 打开时为目标代际写 pin 文件 `<state>/plugins/pins/<package>/<generation>/<owner>.json`；动作入队即登记 `invocation` pin，调用结束后释放。终端先按请求登记占位身份，attach 后换成带 session 与机器名的核心地址；**关闭 Pane 不释放终端 pin**，只有 core 报告该终端结束（`finished`/`removed`/`lost` 事件、程序退出）才释放。进程 PID 存活不参与判断。
- `plugins pane open` 先返回请求 ID；需要程序探测时状态为 `probing`，拿到终端地址后为 `opened`。探测结果回来后只验证冻结的目标仍然存在，不重新解析；目标 Space 已移除则报 `target_gone`，Pane 建在目标 Space 里，与探测期间的用户切换无关。失败状态带 `error_kind`：`unsupported`、`missing_program`、`offline`、`target_gone`、`disabled`、`unavailable`；`disabled` 表示包已安装但被停用。
- 不自动上传插件目录、不执行远端安装、不复制凭证、不转发本机 socket。需要回调客户端时走 `hc-tui` CLI 或事件流。

## 6. 多机

| 位置 | 含义 | 插件在哪一侧运行 |
|---|---|---|
| `client` | hc-tui 进程所在主机 | 动作进程、控制 socket、插件代码 |
| `core` | 当前连接的入口 core | JSON-RPC 与 `hc` CLI |
| `workspace` | Space 所属机器 | 终端程序与工作区命令 |

- 动作默认以 `client` 为宿主，通过参数里的 `workspace` 与 `folder` 决定远端操作的目标。
- 远端操作使用 `hc exec --on`、`hc-tui do`，文件使用 `hc file read/write --on` 或 `hc-tui read`。
- 终端入口在 `workspace` 上启动；包内相对程序仅限 `client` 与安装同机的场景，请求远端时返回 `unsupported`。
- 目标机器由 `--machine` 或 Space 的机器决定；`local` 与空值表示本机。`requires_programs` 在目标机器上检查。
- 离线与目标不存在分别返回 `offline`、`target_gone`，不静默回退到本机：机器被移除或连接断开（`machine_unknown`、`offline`、`io`、`closed`、`timeout`）都归入 `offline`。

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
hc-tui plugins pane open <id>.<terminal> [--cwd DIR] [--machine M] [--placement P] [--space spN]
hc-tui plugins pane status <request>
hc-tui plugins pane focus <terminal|id.terminal|pane>
hc-tui plugins pane close <terminal|id.terminal|pane>
hc-tui events [--plugin <id>] [--kinds terminal.status,terminal.turn] [--count N]
```

结果语义：

- `accepted` 表示已排队；`confirmation_pending` 表示仍需界面确认；两者都不等于完成。
- 停用包的调用在分发器入口被拒绝：控制回包与 `plugins action invoke` 带 `error_kind = "disabled"`，不进入队列。
- 完成状态为 `done`、`failed`、`cancelled`、`timed_out`，附退出码与日志末尾。
- core 操作或布局 CAS 失败时，动作以 `failed` 结束，`error.kind` 区分 `conflict`、`offline`、`unavailable`。
- 终端入口的请求状态为 `probing`、`opening`、`opened`、`closed`、`failed`；`opened` 才带终端地址，`failed` 带 `error_kind`。`open` 等到拿到地址或明确错误才返回，超时 20 秒。

命令面板、`[[keys.command]] type = "action"` 与 `hc-tui do` 使用同一分发器；插件动作在启用前不出现在目录中。

## 8. 事件

```sh
hc-tui events --plugin <id> --kinds terminal.status,terminal.turn,terminal.finished
```

- 控制 socket 的长连接（`method = "events"`），先回 `{"subscribed":true,"queue":256}`，随后按行输出 NDJSON；客户端断开或界面退出即停止推送。
- 过滤：`--plugin <id>` 只看该包的终端事件（事件带 `plugin` 字段），`--kinds a,b` 只看列出的 kind；都缺省时收到全部。
- 作用域是本客户端：同一个 TUI 的多个订阅者各收到一次；另一台机器或另一个 hc-tui 实例不共享。
- 投递不保证恰好一次；每个订阅者 256 条队列，溢出时丢弃并补发 `{"kind":"events.dropped","count":N}`，随后应由消费者重新读取状态。
- kind：`terminal.status`（`data.status`）、`terminal.turn`（`data.turn`）、`terminal.finished`（`data.reason`）、`terminal.opened`、`terminal.closed`（`data.reason = "pane_closed"`），以及 `registry.changed`（`data.plugins` / `data.actions` / `data.terminals`，运行中的实例重新加载注册表后发出，不带 `plugin` 字段）。终端类负载带 `machine`、`terminal`、`turn`、`at_ms` 与 `data`；属于插件终端时带 `plugin`。
- 注入的 `HCTUI_SOCKET` 被 `hc-tui events` 优先采用，插件动作与终端无需自行推导会话。
- 邮件、部署、通知外部服务等副作用不绑定到多客户端事件流；需要单次执行时用独立 runner（systemd 单元、远端分离进程）读取同一 `hc-tui events` 或直接调用 core。

## 9. 安装、更新与卸载

| 操作 | 行为 |
|---|---|
| `plugins link <目录>` | 注册为开发来源，代码原地读取，清单变化后重载 |
| `plugins install <目录>` | 预览清单后复制到新代际，`current` 原子切换，默认不启用 |
| `plugins update <id> <目录>` | 写入新代际；运行中的调用与终端继续使用旧代际，pin 释放后回收 |
| `plugins unlink <id>` | 移除开发注册，不删除目录 |
| `plugins uninstall <id>` | 只删除记录与 `current` 指针；有 pin 的代际保留（pin 在 `<state>/plugins/pins/`） |
| `plugins gc` | 显式回收：删除没有记录指向、也没有任何 pin 的代际 |
| `plugins enable` / `plugins disable <id>` | 只改变是否出现在命令目录与菜单 |

`records/<id>.json` 记录来源、清单哈希、代际、启用状态与安装时间。安装前显示 `id`、版本、`host_api`、平台、动作与终端入口、`requires_programs`；确认后才注册。构建失败不替换旧代际；`current` 指针切换使用临时目录加 rename。

`plugins install` 与 `plugins link` 默认**不启用**，需要时加 `--enable`；`--disable` 显式关闭。`plugins update` 保留记录里已有的启用状态，不会把已停用的包重新打开。

### 9.1 发布事务与恢复

记录是唯一发布真相，`current` 只是可重建索引：

- `save_record` 在记录目录写临时文件，`fsync` 后 `rename` 覆盖，再 `fsync` 父目录；失败时旧记录保持原样，不会留下半截 JSON。
- 包级互斥是 `store/<id>/.lock`（`O_CREAT|O_EXCL`、写入持有者 PID、30 秒超时、持有者进程已消失或锁文件超过 120 秒视为残留）；锁的粒度是一个包，不同包的安装互不阻塞。
- 代际分配在锁内：`next_generation` → 复制到 `store/<id>/.tmp-<gen>` → 同一文件系统 `rename` 到 `store/<id>/<gen>` → 原子写记录 → 最后切 `current`。复制失败清理暂存目录并返回错误；记录写失败删除刚发布的代际目录，旧记录与旧指针不变。
- `Store::open` 每次打开做一次幂等恢复：清理持有者已消失的 `.tmp-*` 与记录临时文件；记录存在而 `current` 缺失、指向不存在的目标或停在旧代际（切换中途失败）时按记录重建指针（linked 来源按记录里的 `origin` 重建）；`current` 存在但没有记录时不静默删除，作为 issue 报出来并拒绝加载该包。
- 旧格式记录仍可读：新增字段都带 `#[serde(default)]`，schema 变化不要求重装。
- `hc-tui plugins gc` 语义不变：只回收没有记录指向、也没有任何 pin 的旧代际。

失败注入与并发覆盖范围：

| 分支 | 直接用例 | 断言 |
| --- | --- | --- |
| 两个真进程同时安装同一目录 | `tests/plugin_install_race.rs::two_processes_installing_the_same_plugin_serialize` | 两个进程都退出码 0（锁超时不算通过）、各自报告不同代际、一条记录、`current` 跟随记录、无 `.tmp-*`、`.lock` 已删除 |
| 持有者被 SIGKILL 后锁残留 | `tests/plugin_install_race.rs::a_killed_installer_leaves_a_lock_the_next_process_takes_over` | 锁文件带着死掉的 PID 留下，下一个进程接管并装完，发布态一致 |
| 复制完、写记录前崩溃 | `src/store.rs::a_crash_between_copy_and_record_keeps_the_old_generation` | 旧记录与旧代际不变，暂存目录被恢复清掉 |
| `save_record` 写入失败 | `src/plugin/registry.rs::a_failed_record_write_keeps_the_published_generation`（记录目录 0555） | 命令报 `Permission denied`、旧记录逐字节不变、`current` 与旧代际完好、新代际目录被删除、重新打开仍列旧版本 |
| `set_current` 切换失败 | `src/plugin/registry.rs::a_failed_pointer_switch_is_repaired_from_the_record`（`current` 目录 0555） | 记录已指向新代际、指针仍为旧代际、下一次 `Store::open` 按记录修好指针并加载包 |
| 缺 `current` / 有指针无记录 / 旧格式记录 / 死锁接管 / 保留代际不复用 | `src/store.rs` 同名用例 | 见各用例断言 |

只靠实现保证、没有直接用例的分支：

- 30 秒 `LOCK_TIMEOUT` 真正走完超时返回 `TimedOut`：用例覆盖的是串行等待与死锁接管，没有真的等 30 秒；
- `copy_tree` 中途 I/O 失败后清理暂存目录（`discard_staged` 路径）：只验证了「写记录失败删除代际目录」；
- `clean_temporary_records` 清理记录临时文件：暂存目录的清理有用例，记录临时文件没有；
- `sync_directory` 失败被忽略：只影响持久性，无法在不破坏文件系统语义的前提下注入。

### 9.2 启用语义

- `plugins enable|disable <id>` 只改变是否出现在命令目录与菜单，不移动代际、不删除代码。
- 运行中的 TUI 无需重启：每 250 毫秒比较注册表磁盘指纹（记录、`disabled.json`、开发清单的内容哈希），处理控制请求前还会无节流对齐一次；变化时重新加载注册表并发布 `registry.changed` 事件。命令目录、命令面板、Pane 右键菜单、Space 菜单与 `[[keys.command]] type = "action"` 绑定因此立即同步。
- `plugins action list` 不再列出停用包的动作；`plugins action invoke` 返回 `{"error": ..., "error_kind": "disabled"}`；`plugins pane open` 返回 `error_kind = "disabled"`。
- 已排队或已运行的调用继续跑到结束，不因停用而腰斩；已打开的终端不 kill，只拒绝新的入口请求。

### 9.3 参数校验

- 递归校验 `args`：object 逐层检查未知键、嵌套必填与 `additionalProperties`，array 按 `items` 逐元素校验；错误带完整路径（`args.remote.name must be string`、`args.hosts[1].name is required`）。
- 清单加载期逐个校验 `default`，不合法时报 `文件:行:列: 原因`，不会把非法默认值带进运行期结果。
- `integer` 只接受整数，`number` 接受整数与浮点；未知 `type` 在清单期拒绝。
- 防爆：schema 递归深度上限 8、单层属性上限 64、`items` 只允许 object 或标量 schema，超限在清单期报错；运行期递归深度由清单约束，不会递归到栈溢出。
- `plugins action list --json` 里的 `args` 原样输出 schema，不注入默认值、不改形状。

## 10. 信任与权限

- 插件是以用户身份运行的受信本地代码，没有 OS 沙箱；`capabilities` 不作为隔离承诺。
- 信任边界是安装来源、安装前预览与显式启用。共享文档、界面模块或远端布局中的插件引用不自动获得执行权。
- 键位由用户绑定到命令名；插件不能静默改写全局键位，也不能覆盖内置动作。
- 日志、配置与状态目录由宿主创建，权限 0700；插件目录只读挂载不强制，路径由清单解析。

## 11. 生命周期与故障

- 注册表在启动时扫描配置目录与 state 中的安装记录，运行中按磁盘指纹重新加载；清单无效时保留记录并标记 `broken`，不执行。
- 动作进程按调用创建与回收；宿主退出走与取消相同的终止路径（取消排队项、SIGTERM 后 SIGKILL 终止进程组、等待收尾，3 秒 deadline），终端继续由 core 持有。
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
- 生命周期跟随 hc-tui 客户端；客户端退出（`app.quit`、SIGTERM、SIGHUP）时动作的整个进程组被回收，
  客户端被 SIGKILL 时直接子进程由 `PR_SET_PDEATHSIG` 回收（后台孙进程不在这个兜底里）；终端继续由 core 保留。
- 不兼容旧 Herdr plugin 的 manifest、CLI 与 socket 语义。

## 14. 验收

- 两个真实插件：`git-tools`（动作读取远端 Git 摘要，终端入口在远端开 lazygit）与 `review`（长运行终端，验证 cwd、退出、park、detach 后保留）。
- 目标冻结：动作执行中切换焦点，实际请求仍指向调用时的 Space、Tab、机器与 cwd；显式目标不存在时报 `target_gone` 而不是换用当前焦点。
- 多机：远端动作与终端在正确机器执行；缺失程序、离线、目标删除各自给出明确错误，不回落本机。
- 超时与取消：SIGTERM 后 2 秒 SIGKILL，进程组与后代管道全部回收，完成状态为 `timed_out` 或 `cancelled`。
- 并发与队列：超过全局与单包上限时排队，队列满返回 `busy`，界面仍可输入。
- 输出洪泛：持续输出超过 64 KiB 时内存有界、管道不堵塞、日志轮转正常。
- 更新：新代际切换后旧调用与旧终端继续运行；失败安装不影响旧代际；卸载不删除用户终端。
- 事件：两个客户端各收到一次，断开即停；队列溢出发出 `events.dropped`。
- 命令目录：插件动作带参数 schema 与来源；禁用后从目录与菜单消失（无需重启），动作调用返回 `disabled`；内置动作不可覆盖。
- 旧配置：`[[panels]]`、`[[keys.command]]`、`mod.open` 行为不变。

## 15. 实施清单

新增：

| 文件 | 内容 |
|---|---|
| `src/plugin/manifest.rs` | 标准 TOML 解析、严格校验、命令描述与参数 schema |
| `src/plugin/registry.rs` | 安装记录、代际、启用状态、来源与预览 |
| `src/plugin/runner.rs` | 动作进程、上下文 stdin、deadline、取消、进程组收尾、`shutdown` 与有界输出 |
| `src/plugin/logs.rs` | 运行日志、轮转与查询 |
| `src/plugin/terminal.rs` | 目标解析（`TargetSelector`/`ExecutionTarget`）、入口规划、placement 映射、包内相对程序规则、执行位置程序探测 |
| `src/plugin/lease.rs` | 代际的显式 pin、core 终结证据与显式 GC |
| `src/plugin/cli.rs` | `plugins` 子命令 |
| `src/app/plugin.rs` | 菜单与命令目录接入、上下文构造、结果展示、终端入口与标签 |
| `examples/plugins/git-tools/` | 动作读取远端 Git 摘要；终端入口在远端开 lazygit |
| `examples/plugins/review/` | 包内相对程序的长运行终端（cwd、park、detach） |
| `docs/plugins.md` | 本文 |

修改：

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | `toml` 依赖 |
| `src/app/actions.rs` | `COMMANDS` 增加 `plugins.*`；分发器接入插件动作；新建 Space 换机器时重置来源 |
| `src/app/interaction.rs` | Pane 与 Space 菜单加入上下文匹配的插件动作 |
| `src/control.rs` | 动作调用、状态查询、取消、`plugins.pane.*`、原始应答 |
| `src/main.rs` | `plugins` 子命令（含 `pane`）与 `events` 子命令 |
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
