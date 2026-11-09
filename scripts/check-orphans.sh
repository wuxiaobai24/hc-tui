#!/bin/sh
# scripts/check-orphans.sh — 全套测试后检查「父进程已经没了却还在烧 CPU」的 hc-tui
#
# 用法：
#   scripts/check-orphans.sh              # 跑 HC_TUI_GRAPHICS=kitty cargo test 再查
#   scripts/check-orphans.sh --lib        # 额外参数原样传给 cargo test
#
# 判定：收尾后仍存在 `ppid=1` 且 `%CPU>50` 的 hc-tui 进程就退出 1。
# 这类进程的共同点是终端（pty）已经被关掉，事件循环在紧循环里反复失败却不退出。
#
# 注意：和 check-core-leaks.sh 一样，不要从这里导出 HC_SESSION；测试进程自己
# 生成隔离会话，外部 HC_SESSION 会把它指到别的会话上。

set -u
cd "$(dirname "$0")/.." || exit 2

WORK=$(mktemp -d "${TMPDIR:-/tmp}/orphans.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

# ppid、%CPU、命令行三列；匹配 hc-tui 本体（含 target/debug 下的构建产物）。
orphans() {
  ps -eo ppid=,pcpu=,args= 2>/dev/null \
    | awk '$1 == 1 && $2 + 0 > 50 && $3 ~ /(^|\/)hc-tui$/ { print }'
}

echo "== before: ppid=1 且 %CPU>50 的 hc-tui"
orphans | tee "$WORK/before"
echo "== cargo test $(date -Is)"

HC_TUI_GRAPHICS=kitty cargo test "$@"
status=$?
if [ "$status" -ne 0 ]; then
  echo "== 判定：cargo test 自身失败（$status），先修测试" >&2
  exit "$status"
fi

# pty 关闭到进程退出最多等 5 秒（见 tests/pty_gone.rs 的 EXIT_BUDGET）。
sleep 6
orphans > "$WORK/after"
comm -13 "$WORK/before" "$WORK/after" > "$WORK/new"

echo
echo "== after: ppid=1 且 %CPU>50 的 hc-tui"
cat "$WORK/after"
if [ -s "$WORK/new" ]; then
  echo "== 判定：留下孤儿（新增 $(wc -l < "$WORK/new" | tr -d ' ') 个，手工 kill -9 后重跑）" >&2
  exit 1
fi
echo "== 判定：没有孤儿"
