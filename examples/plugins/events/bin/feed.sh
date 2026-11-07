#!/bin/sh
# 长运行的事件查看器：在这个终端里持续打印本客户端的事件。
printf 'following events; close the pane to detach\n'
"${HCTUI_BIN:-hc-tui}" events --kinds terminal.status,terminal.turn,terminal.finished
printf 'event stream closed\n'
