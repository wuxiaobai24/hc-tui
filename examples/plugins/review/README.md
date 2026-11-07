# Review 插件

终端入口 `review` 用包内相对程序 `./bin/review.sh` 启动一个长运行终端：
先打印启动目录与注入变量，再每秒输出一次心跳。用于验证 cwd、关闭 Pane
不结束程序、detach 后终端继续运行，以及包内相对程序只能在安装所在的客户端
主机启动这一限制。

```sh
cp -r examples/plugins/review ~/.config/hc-tui/plugins/review
hc-tui plugins pane open dev.hctui.plugins.review.review
hc-tui plugins pane open dev.hctui.plugins.review.review --machine peer
# 第二条返回 unsupported：包内相对程序不能在远端机器执行
```
