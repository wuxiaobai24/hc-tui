# CLI 错误契约

`hc-tui` 的失败输出、错误码与退出码是三份可以写进脚本的稳定契约。改词表必须同时
改这份文档——`src/error.rs` 里的单测会逐行对拍，多一个码、少一个码都会红。

## 错误形状

`--json` 时失败只写 **stderr** 一个对象：

```json
{"error":{"code":"package_not_found","message":"no.such.pkg is not installed"},"id":"cli:packages:update"}
```

人类形态先写事实与你的情况，末行固定是码：

```console
$ hc-tui packages update --check no.such.pkg
no.such.pkg is not installed
(code: package_not_found)
$ echo $?
1
```

- `message` 必须回显用户输入的 token（包 id、pane、终端、文件路径、选项名），
  报 `not found` 却不说是哪个对象的输出算缺陷。
- `id` 是 `cli:` 加动词路径，用 `:` 连接，只取命令名与子命令，参数不算：
  `cli:packages:install`、`cli:plugins:action:invoke`、`cli:config:check`、`cli:do`。

## 错误码

| 码 | 含义 | 退出码 |
|---|---|---|
| `usage` | 参数或子命令不认识，或选项缺值 | 2 |
| `package_not_found` | 按 id 找不到包、插件或模块 | 1 |
| `pane_not_found` | 指定的 pane 不存在 | 1 |
| `terminal_not_found` | 指定的终端不存在 | 1 |
| `machine_not_found` | 指定的机器不存在 | 1 |
| `file_not_found` | 文件或安装来源不存在 | 1 |
| `file_exists` | 目标文件已经存在，且没带覆盖开关（`config init` 不带 `--force`） | 1 |
| `service_not_found` | 包没有声明这个服务，或服务没有日志 | 1 |
| `manifest_invalid` | 包清单非法，或声明的 id 与命令给的 id 不一致 | 1 |
| `config_invalid` | 配置文件非法（`config check` 报错时用它） | 1 |
| `core_unavailable` | 本会话没有 core，或连不上 | 3 |
| `host_api_too_old` | 包清单声明的 host api 与当前宿主不兼容 | 1 |
| `protocol_incompatible` | core 的应答解析不了，协议对不上 | 3 |
| `permission_denied` | 权限拒绝 | 1 |
| `timeout` | 操作超时（等 pane 开出来、写布局没落地） | 1 |
| `window_required` | 这个动作需要运行中的 TUI 窗口 | 1 |
| `layout_invalid` | 共享布局文档缺失或写入冲突 | 1 |
| `install_cancelled` | 安装被取消 | 1 |
| `update_failed` | 更新检查或更新失败 | 1 |
| `internal_error` | 未归类的内部错误，含外部工具失败 | 1 |

归类顺序是固定的：命令自己给的码优先；其次看底层 `io::Error` 的类型
（权限、超时、连接失败、找不到文件）；最后才按文案认出对象
（`unknown pane` / `unknown terminal` / `unknown machine` / `host_api` / `manifest`，
中英文形态都认）与缺参文案。都认不出就是 `internal_error`，`message` 原样保留。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 操作失败（错误对象已输出） |
| 2 | 用法错误（参数/子命令不认识） |
| 3 | core 不可用（没有会话在跑、连不上、或协议不兼容） |

四个值只有一处定义：`hc_tui::error::Exit`。命令里不再写裸数字。

## reload 类命令的结果

`theme reload`、`packages install|update`、`mods reload` 是热重载类命令，结果是
`{status, diagnostics}`：

```json
{"status":"rejected","diagnostics":[{"severity":"error","message":"/home/me/.config/hc-tui/config.toml:2:8: ..."}]}
```

- `applied`：新的配置、代际或模块已经生效。
- `rejected`：拒绝应用，运行时留下的还是旧的那一份；人类形态会另起一行写
  「当前使用的是旧配置」，退出码 1。
- `diagnostics` 每项带 `severity`（`error` / `warning`）与 `message`；`applied`
  也可以是空数组。

## `--help` 的输出契约

每个子命令的 `--help` 在用法行之后固定四行：

```console
$ hc-tui packages list --help
usage: hc-tui packages list [--json] [--config <path>]
输出:
  人类形态: one line per package: id, source, generation, enabled/disabled, capability badges A/T/M; ...
  --json:   [{"id","name","version","source","generation","enabled","capabilities","module","issues","services"}]
  退出码:   0 成功 / 1 失败 / 2 用法 / 3 core 不可用
常见错误:  usage
```

`--json` 只在这些地方有意义：`packages list|info|gc|install|update`、`mods list|reload`、
`plugins action list`、`theme reload`。别的子命令传 `--json` 是用法错误，不会被静默忽略。

`config dump`、`config init` 与 `doctor` 同样在这套契约里：参数错 `usage`（退出 2）、配置有错
`config_invalid`（退出 1）、`config init` 目标已存在且没带 `--force` 是 `file_exists`。`doctor`
全 ok 退出 0，任何一节失败退出 1，并在分节报告之后以 `(code: <码>)` 收尾。
