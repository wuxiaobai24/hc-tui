#!/bin/sh
# 打印若干条事件后退出：演示动作进程消费 hc-tui events。
# 上下文是单行 JSON；args.count 与 args.kinds 缺省 3 / status+turn。
context=$(cat)
field() {
  key=$1
  default=$2
  value=$(printf '%s' "$context" | sed -n "s/.*\"$key\": *\"\([^\"]*\)\".*/\1/p")
  [ -n "$value" ] && printf '%s' "$value" || printf '%s' "$default"
}
count=$(printf '%s' "$context" | sed -n 's/.*"count": *\([0-9][0-9]*\).*/\1/p')
[ -n "$count" ] || count=3
kinds=$(field kinds "terminal.status,terminal.turn")
printf 'watching %s events of %s\n' "$count" "$kinds" >&2
"${HCTUI_BIN:-hc-tui}" events --kinds "$kinds" --count "$count"
