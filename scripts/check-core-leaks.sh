#!/bin/sh
# scripts/check-core-leaks.sh — 全套测试前后对比常驻 herdr-core 数量
#
# 用法：
#   scripts/check-core-leaks.sh              # 跑 HC_TUI_GRAPHICS=kitty cargo test
#   scripts/check-core-leaks.sh --lib        # 额外参数原样传给 cargo test
#
# 判定：结束数 - 开始数 > 1 就退出 1，并列出新出现的 core 进程与它监听的
# 会话 socket（会话名只能从 socket 路径读，cmdline 里没有）。
#
# 注意：不要给这个脚本导出 HC_SESSION。测试进程自己用 `support::IsolatedCore`
# 生成 `hctui-<tag>-<pid>` 会话；外部 HC_SESSION 会覆盖它，把测试连到
# 别的会话上。

set -u
cd "$(dirname "$0")/.." || exit 2

WORK=$(mktemp -d "${TMPDIR:-/tmp}/core-leaks.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

pids() { pgrep -f 'herdr-core serve' 2>/dev/null | sort -n; }
count() { n=$(pids | wc -l | tr -d ' '); echo "${n:-0}"; }
# core 的 cmdline 只有 `herdr-core serve`；会话 = 它监听的非 holder socket 路径。
socket_of() {
  ss -xlp 2>/dev/null | grep "pid=$1," | grep -v '\.holder\.' | awk '{print $5}' | head -1
}

pids > "$WORK/before"
before=$(count)
echo "== before: herdr-core serve = $before"
echo "== cargo test $(date -Is)"

HC_TUI_GRAPHICS=kitty cargo test "$@"
status=$?

# core 退出可能要几百毫秒：等一会儿再数，免得把正在退出的算成泄漏。
sleep 3
pids > "$WORK/after"
after=$(count)
comm -13 "$WORK/before" "$WORK/after" > "$WORK/new"

echo
echo "== after: herdr-core serve = $after"
echo "== delta: $((after - before))"
if [ -s "$WORK/new" ]; then
  echo "== 新出现的 core：pid<TAB>session socket"
  while read -r pid; do
    printf '%s\t%s\n' "$pid" "$(socket_of "$pid")"
  done < "$WORK/new"
else
  echo "== 新出现的 core：无"
fi

new=$(wc -l < "$WORK/new" | tr -d ' ')
delta=$((after - before))
if [ "$delta" -gt 1 ] || [ "$new" -gt 1 ]; then
  echo "== 判定：泄漏（新增 $new 个 core，差值 $delta）" >&2
  echo "== 兜底：HC_SESSION=<上面列出的会话名> hc server stop；或 scripts/reap-cores.sh --apply" >&2
  exit 1
fi

echo "== 判定：通过（新增 $new 个 core，差值 $delta ≤ 1）"
if [ "$status" -ne 0 ]; then
  echo "== 注意：cargo test 退出码 $status，测试本身有失败" >&2
  exit "$status"
fi
exit 0
