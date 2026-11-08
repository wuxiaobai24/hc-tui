# 示例模块

四个随仓库维护的界面模块，用来验收宿主通道、焦点路由、覆盖层与授予：

| 模块 | 形态 | 内容 |
|---|---|---|
| `agents` | 右侧区域 | `host.call("state")` 列出 Agent，点击执行 `agent.focus` |
| `spaces` | 左侧区域 | Space 状态点、Tab 计数与本地折叠，点击执行 `space.select` |
| `prs` | dialog 覆盖层 | `host.call("exec")` 拉取 `gh pr list`，失败时显示原因 |
| `sidebar` | 默认侧栏 | 接管 Spaces + Agents + 折叠组 + 状态点，逐格等于内置实现 |
| `tabs` | 默认 Tab 栏 | 接管 Tab 标签、选中底色与右端菜单块 |
| `status` | 默认模式行 | 接管连接状态与右侧提示 |
| `toast` | 提示层 | 读 `props.toasts` 在内容区右下角画盒，点击跑动作再 `dismiss` |

`sidebar` / `tabs` / `status` 是默认外框区域模块：宿主在区域可见时把它们绑到该
区域，模块没装、`mods.disable` 停用或渲染失败时自动回退到内置 Rust 实现，
`--no-mods` 下三块全部由 Rust 绘制。这三个模块只负责绘制，键盘与鼠标仍由 Rust
侧处理。
| `settings` | dialog 覆盖层 | 段、项与按键/鼠标交互；写配置走 `app.theme` 等动作 |
| `picker` | dialog 覆盖层 | 宿主归一的候选行（动作菜单、工作区、Pane、终端菜单、模板、机器、授予、Go to、命令面板）；模块内过滤、`1`-`9` 快选、`j`/`k`、滚轮、`Esc`/`q` |

每个目录的 `hctui.toml` 声明挂载位置与宿主授予，`nsh.json` 的 `capabilities.host`
声明同一份方法名；`dist/` 由构建生成，不进入版本库。

```sh
cd examples/mods/agents && nsh build -o dist
hc-tui mods check examples/mods/agents examples/mods/spaces examples/mods/prs examples/mods/settings
hc-tui do mods.open '{"name":"agents","place":"dialog","size":[60,16]}'
```

把目录复制到配置目录后即可停靠：

```sh
cp -r examples/mods/agents ~/.config/hc-tui/mods/agents
hc-tui do mods.dock '{"name":"agents","place":"right","size":{"cols":32}}'
```
