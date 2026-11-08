# Git board：一个包，三类入口

这个包用一份 `hctui.toml` 同时声明三类能力，安装一次三样都有：

| 能力 | 清单段 | 入口 |
|---|---|---|
| 动作 | `[[actions]]` | 命令面板与 `hc-tui plugins action invoke dev.hctui.pkg.git-board.summary` |
| 终端 | `[[terminals]]` | `hc-tui plugins pane open dev.hctui.pkg.git-board.git-ui`（起 lazygit） |
| 界面模块 | `[module]` | 安装后自动停靠到外框右侧，显示最近提交 |

`bin/summary.sh` 读取宿主注入的单行 JSON 上下文，在当前 Space 的目录里执行
`git status`；远端 Space 经 `hc exec --on <machine>` 在同一台机器上运行。
`dist/bundle.js` 是直接维护的 bundle 源码（与 `nsh build` 产物同形态），因此
安装前不需要额外的构建步骤；改动后运行 `hc-tui mods check .` 试渲染一次。

## 安装

```sh
hc-tui packages install examples/packages/git-board --yes --enable
hc-tui packages list                 # dev.hctui.pkg.git-board 带 A·T·M 徽章
```

运行中的 TUI 在 250 ms 内发现新记录：动作进入命令面板，模块按
`[module].place = "right"` 停靠到外框。终端入口需要 `lazygit` 在目标机器
的 `PATH` 里。

## 启用、授权、卸载

```sh
hc-tui packages disable dev.hctui.pkg.git-board   # 动作、终端、模块一起消失
hc-tui packages enable dev.hctui.pkg.git-board    # 三者恢复，不需要重启
hc-tui mods grant dev.hctui.pkg.git-board --host state --host exec --host notify
hc-tui packages uninstall dev.hctui.pkg.git-board # 记录、代际与模块区域一起清干净
```

`[grants]` 在安装期校验、写入包记录；运行期只读记录，一次安装确认一次。
`mods grant --revoke` 之后模块收到 `grant.changed`，再调用被撤销的方法返回
`forbidden` 且不产生副作用。
