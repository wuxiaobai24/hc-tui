# 示例模块

三个随仓库维护的界面模块，用来验收宿主通道、焦点路由、覆盖层与授予：

| 模块 | 形态 | 内容 |
|---|---|---|
| `agents` | 右侧区域 | `host.call("state")` 列出 Agent，点击执行 `agent.focus` |
| `spaces` | 左侧区域 | Space 状态点、Tab 计数与本地折叠，点击执行 `space.select` |
| `prs` | dialog 覆盖层 | `host.call("exec")` 拉取 `gh pr list`，失败时显示原因 |

每个目录的 `hctui.toml` 声明挂载位置与宿主授予，`nsh.json` 的 `capabilities.host`
声明同一份方法名；`dist/` 由构建生成，不进入版本库。

```sh
cd examples/mods/agents && nsh build -o dist
hc-tui mods check examples/mods/agents examples/mods/spaces examples/mods/prs
hc-tui do mods.open '{"name":"agents","place":"dialog","size":[60,16]}'
```

把目录复制到配置目录后即可停靠：

```sh
cp -r examples/mods/agents ~/.config/hc-tui/mods/agents
hc-tui do mods.dock '{"name":"agents","place":"right","size":{"cols":32}}'
```
