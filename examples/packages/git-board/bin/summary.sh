#!/bin/sh
# 读取宿主传入的上下文，在 Space 所属机器上打印 Git 摘要。
# 上下文是单行 JSON：workspace 是机器名，folder 是 Space 目录。
context=$(cat)
field() {
  printf '%s' "$context" | sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p"
}
machine=$(field workspace)
folder=$(field folder)
short=$(printf '%s' "$context" | sed -n 's/.*"short": *\(true\|false\).*/\1/p')

if [ -z "$folder" ]; then
  echo "no space folder in this context" >&2
  exit 2
fi
args='--short --branch'
[ "$short" = "false" ] && args='--branch'
if [ "$machine" = "local" ] || [ -z "$machine" ]; then
  git -C "$folder" status $args
else
  hc exec --on "$machine" "git -C '$folder' status $args"
fi
