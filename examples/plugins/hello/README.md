# Hello 插件

M6 冒烟插件：`summary` 打印宿主传入的上下文，`fail` 以退出码 7 结束，
`slow` 用于验证取消与并发队列。

```sh
cp -r examples/plugins/hello ~/.config/hc-tui/plugins/hello
hc-tui plugins action list
hc-tui plugins action invoke dev.hctui.plugins.hello.summary '{"loud":false}'
hc-tui plugins action status p1
hc-tui plugins action log p1
```
