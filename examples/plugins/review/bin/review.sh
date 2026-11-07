#!/bin/sh
# 长运行的审查终端：先报告启动位置与环境，再持续输出心跳。
# 包内相对程序：只在安装所在的客户端主机可用。
printf 'review start\n'
printf 'cwd=%s\n' "$(pwd)"
printf 'plugin=%s\n' "${HCTUI_PLUGIN_ID:-}"
printf 'terminal=%s\n' "${HCTUI_CONTEXT_ID:-}"
printf 'placement=%s\n' "${HCTUI_PLACEMENT:-}"
printf 'ctxtool=%s\n' "$(command -v hc-tui || echo missing)"
tick=0
while [ "$tick" -lt 3600 ]; do
  tick=$((tick + 1))
  printf 'review tick %s\n' "$tick"
  sleep 1
done
