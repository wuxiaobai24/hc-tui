# Git tools 插件

动作 `summary` 在 Space 所属机器上读取 Git 分支与改动；终端入口 `git-ui`
在同一个 Space 的机器上用 `lazygit` 打开交互界面。

```sh
cp -r examples/plugins/git-tools ~/.config/hc-tui/plugins/git-tools
hc-tui plugins list
hc-tui plugins action invoke dev.hctui.plugins.git-tools.summary
hc-tui plugins pane open dev.hctui.plugins.git-tools.git-ui
```

`lazygit` 必须在目标机器 PATH 上（`requires_programs`）；缺少时报
`missing_program`，不换成本机执行。
