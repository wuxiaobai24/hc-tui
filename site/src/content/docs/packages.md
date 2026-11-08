---
title: 包与插件
summary: 一份清单声明动作、终端入口、常驻服务与界面模块；安装、启停与回收都按包进行。
order: 2
spec: plugins
---

## 一个包是什么

插件是一个目录，包含清单与可执行入口。动作由 hc-tui 在客户端主机启动，进程退出即结束；终端入口由 core 在 Space 的机器上创建真实 PTY，复用现有终端能力。

动作、终端入口与界面模块共用同一份清单、同一个存储代际与同一个注册表。一个包可以只贡献其中一部分。

出处：`docs/plugins.md` · 1. 定位与边界。

## 清单

清单使用标准 TOML 解析器，严格校验：未知键、未知表、重复 `id`、非法 `id`、空 `argv`、未知 `contexts` / `placement`、越界授予都直接拒绝加载。错误格式为 `文件:行:列: 原因`。

```toml
manifest_version = 2
id = "dev.hctui.pkg.git-board"
name = "Git board"
version = "0.1.0"
host_api = 1
platforms = ["linux", "macos"]
description = "动作、终端入口与右侧模块，一次安装三样都有"

[[actions]]
id = "summary"
title = "Git 摘要"
contexts = ["global", "space", "pane"]
argv = ["sh", "./bin/summary.sh"]

[actions.args]
type = "object"
properties = { short = { type = "boolean", default = true } }

[[terminals]]
id = "git-ui"
argv = ["lazygit"]
placement = "right"
requires_programs = ["lazygit"]

[module]
title = "Git board"
bundle = "dist/bundle.js"
place = "right"
size = 32
```

出处：`docs/plugins.md` · 2. 目录与清单、`examples/packages/git-board/hctui.toml`。

### 关键字段

| 字段 | 含义 |
| --- | --- |
| `host_api` | 宿主接口版本；与内嵌版本不一致时拒绝加载 |
| `[[actions]].contexts` | `global`（命令面板与 `do`）、`space`、`pane` |
| `[[actions]].args` | JSON Schema 子集；逐层校验，深度上限 8、单层属性上限 64 |
| `[[terminals]].placement` | `tab`、`right`、`down`、`float`、`popup`、`zoomed` |
| `requires_programs` | 由实际执行位置在目标机器检查 PATH，缺失报 `missing_program` |
| `[module].bundle` | 包内相对路径；安装时校验文件存在且非空 |

出处：`docs/plugins.md` · 2. 目录与清单。

## 常驻服务

`[[services]]` 是需要活过窗口的进程：宿主把它物化成 core 的 keep 终端，hc-tui 只做「声明 → 作业」的翻译。

```toml
[[services]]
id = "watch"
title = "Watcher"
argv = ["sh", "./bin/watch.sh"]
cwd = "."
on = "install"
log = "services/watch.log"
```

`on = "install"` 只在安装或更新后拉起；默认 `enable` 在启用与窗口重开时补齐。运行期台账写在 `hctui-layout-v1` 文档的可选 `services` 段。

出处：`docs/plugins.md` · 5.1 常驻服务。

## 安装与启停

```sh
hc-tui packages install <目录|git 仓库|.tar.gz> --yes --enable
hc-tui packages list
hc-tui packages disable <id>
hc-tui packages enable <id>
hc-tui packages uninstall <id>
hc-tui packages gc
```

`packages enable|disable` 只改写记录。250 毫秒的注册表指纹轮询接上同一次重载：动作从命令目录消失、终端入口返回 `disabled`、模块表面从外框撤下；启用后按 `[module].place` 原地停靠，不需要重启。正在跑的终端不 kill，已排队的动作不腰斩。

安装写入新代际，`current` 指针原子切换；更新时运行中的调用与终端继续使用旧代际，pin 释放后才回收。

出处：`docs/plugins.md` · 9. 安装、更新与卸载、`docs/mods.md` · 包：安装、权威、启用、授权。

## 示例：git-board

`examples/packages/git-board` 用一份清单同时提供三类入口：`bin/summary.sh` 读当前 Space 的 Git 分支与改动，`lazygit` 在 Space 的机器上开真实终端，`dist/bundle.js` 停靠外框右侧显示最近提交。

```sh
hc-tui packages install examples/packages/git-board --yes --enable
hc-tui plugins action list
hc-tui packages disable dev.hctui.pkg.git-board
hc-tui plugins action list
```

停用后动作目录为空，模块区域撤下，终端入口返回 `error_kind = "disabled"`；`enable` 恢复，全部不需要重启。

出处：`docs/plugins.md` · 2.3 一次安装三样都有、`examples/packages/git-board/`。

## 边界

- 插件是以用户身份运行的受信本地代码，没有 OS 沙箱；`capabilities` 不作为隔离承诺。
- 不提供插件市场、远程安装、自动更新与跨客户端单次执行保证。
- 包内相对程序只在安装主机有效；远端入口只接受目标机器 PATH 中的程序。
- 动作 stdout 不作为结构化协议；需要交互界面时使用终端入口或界面模块。

出处：`docs/plugins.md` · 13. 限制、`docs/plugins.md` · 授权的说明。
