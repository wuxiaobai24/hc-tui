#!/bin/sh
# scripts/reap-cores.sh — 清理泄漏的 herdr-core（按 socket 路径判定会话归属）
#
# 判定方法（与 docs/ops-cores.md 一致）：
#   core 的 cmdline 里没有会话名，会话名只能从它的 unix socket 路径读：
#   `<runtime>/herdr-core/<session>.sock`（holder 是 `<session>.holder.<hash>.sock`）。
#   runtime 就是 core 启动时的 XDG_RUNTIME_DIR。同一个会话名出现在
#   `/tmp/<测试目录>/run/herdr-core/` 下，说明是测试用隔离 runtime 留下的孤儿。
#
# 分类：
#   protect         规范 runtime 下的 default / doc1，以及任何 hcp-* 会话
#   skip-docker     cgroup 属于 docker-*.scope（容器自己的 core，不归宿主管）
#   skip-nosock     拿不到会话 socket（刚启动还没 bind，或扫描时已退出）：不动
#   leak-name       会话名是测试残留（hctui-* / hcw* / probe* / osc-probe* / tmp* / pi-ext-test*）
#   leak-runtime    会话 socket 不在规范 runtime 目录下（测试用隔离 runtime 的孤儿）
#   protect-unknown 规范 runtime 下的其它会话名：不认识就不动，打印出来人工确认
#
# 只有 leak-* 会被动刀；其余一律只报告。
#
# 用法：
#   scripts/reap-cores.sh [--apply] [--evidence-dir DIR] [--wait SECONDS]
#   --apply         真的动手：先 systemctl --user stop 活跃单元，再对剩下的 kill -TERM
#   --evidence-dir  把清单与结果写到 DIR/reap-plan.txt、DIR/reap-result.txt
#   --wait          apply 之后等进程退出的秒数（默认 10）

set -u

APPLY=0
EVID=
WAIT=10

usage() {
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --apply) APPLY=1 ;;
    --evidence-dir) EVID=${2:?--evidence-dir 需要一个目录}; shift ;;
    --wait) WAIT=${2:?--wait 需要秒数}; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
  shift
done

BASE="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
CANON="$BASE/herdr-core"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/reap-cores.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

# 自己的祖先链：绝不 SIGNAL 它们（本终端就挂在某个 core 的 holder 下面）。
ancestors() {
  pid=$$
  while [ -n "$pid" ] && [ "$pid" != 1 ]; do
    echo "$pid"
    pid=$(sed -n 's/^PPid:\s*//p' "/proc/$pid/status" 2>/dev/null)
  done
}
ancestors > "$WORK/ancestors"

is_ancestor() {
  grep -qx "$1" "$WORK/ancestors"
}

# session 名 -> 分类。参数：session XDG_RUNTIME_DIR（core 启动时的值） cgroup
classify() {
  session=$1
  runtime=$2
  cgroup=$3
  case "$cgroup" in
    docker-*.scope) echo skip-docker; return ;;
  esac
  if [ -z "$session" ]; then echo skip-nosock; return; fi
  case "$session" in
    hcp-*) echo protect; return ;;
  esac
  if [ "$runtime" = "$BASE" ] && { [ "$session" = default ] || [ "$session" = doc1 ]; }; then
    echo protect
    return
  fi
  case "$session" in
    hctui-*|hcw*|probe*|osc-probe*|tmp*|pi-ext-test*) echo leak-name; return ;;
  esac
  if [ "$runtime" != "$BASE" ]; then echo leak-runtime; return; fi
  echo protect-unknown
}

# 收集：pid<TAB>kind<TAB>session<TAB>socket<TAB>runtime<TAB>cgroup<TAB>class
scan() {
  ss -xlp 2>/dev/null | grep 'herdr-core' > "$WORK/ss"
  : > "$WORK/cores"
  for pid in $(pgrep -f 'herdr-core serve' | sort -n); do
    [ -d "/proc/$pid" ] || continue
    cgroup=$(sed -n 's/^0:://p' "/proc/$pid/cgroup" 2>/dev/null | sed 's#.*/##')
    # 会话 socket = 该进程监听的非 .holder. socket；拿不到就是容器内的 core
    socket=$(awk -v pid="$pid" 'index($0, "pid=" pid ",") || index($0, "pid=" pid ")") {print $5}' \
      "$WORK/ss" | grep -v '\.holder\.' | head -1)
    if [ -z "$socket" ]; then
      printf '%s\tcore\t-\t-\t-\t%s\t%s\n' "$pid" "$cgroup" "$(classify "" "" "$cgroup")" >> "$WORK/cores"
      continue
    fi
    runtime=$(dirname "$(dirname "$socket")")
    session=$(basename "$socket" .sock)
    printf '%s\tcore\t%s\t%s\t%s\t%s\t%s\n' "$pid" "$session" "$socket" "$runtime" "$cgroup" \
      "$(classify "$session" "$runtime" "$cgroup")" >> "$WORK/cores"
  done

  : > "$WORK/holders"
  for pid in $(pgrep -f 'herdr-core hold' | sort -n); do
    [ -d "/proc/$pid" ] || continue
    cgroup=$(sed -n 's/^0:://p' "/proc/$pid/cgroup" 2>/dev/null | sed 's#.*/##')
    socket=$(tr '\0' '\n' < "/proc/$pid/cmdline" 2>/dev/null | grep -A1 '^--socket$' | tail -1)
    if [ -z "$socket" ]; then
      printf '%s\tholder\t-\t-\t-\t%s\t%s\n' "$pid" "$cgroup" "$(classify "" "" "$cgroup")" >> "$WORK/holders"
      continue
    fi
    runtime=$(dirname "$(dirname "$socket")")
    session=$(basename "$socket" | sed 's/\.holder\..*$//')
    printf '%s\tholder\t%s\t%s\t%s\t%s\t%s\n' "$pid" "$session" "$socket" "$runtime" "$cgroup" \
      "$(classify "$session" "$runtime" "$cgroup")" >> "$WORK/holders"
  done
}

# 需要停的单元：leak 会话进程所在、且仍 active 的 hc-serve-*/hc-hold-* 单元。
# 同一个单元里只要还养着 protect/skip 进程就不停（宁可留一个 leak，也不误伤活会话）。
leak_units() {
  awk -F'\t' '$7 !~ /^leak/ {print $6}' "$WORK/cores" "$WORK/holders" \
    | sort -u > "$WORK/units-keep"
  {
    awk -F'\t' '$7 ~ /^leak/ && $6 ~ /^hc-(serve|hold)-.*\.service$/ {print $6}' "$WORK/cores"
    awk -F'\t' '$7 ~ /^leak/ && $6 ~ /^hc-(serve|hold)-.*\.service$/ {print $6}' "$WORK/holders"
  } | sort -u | grep -vxF -f "$WORK/units-keep" | while read -r unit; do
    [ -n "$unit" ] || continue
    state=$(systemctl --user show -p ActiveState --value "$unit" 2>/dev/null)
    printf '%s\t%s\n' "$unit" "${state:-unknown}"
  done
}

print_table() {
  printf '%-8s %-7s %-24s %-18s %-30s %s\n' PID KIND SESSION CLASS CGROUP SOCKET
  awk -F'\t' '{printf "%-8s %-7s %-24s %-18s %-30s %s\n", $1, $2, substr($3,1,24), $7, substr($6,1,30), $4}' \
    "$WORK/cores" "$WORK/holders"
}

summary() {
  file=$1
  kind=$2
  for c in protect protect-unknown skip-nosock leak-name leak-runtime skip-docker; do
    n=$(awk -F'\t' -v c="$c" -v k="$kind" '$2==k && $7==c' "$WORK/$file" | wc -l | tr -d ' ')
    [ "$n" -gt 0 ] && echo "  $c: $n"
  done
  true
}

core_count()   { n=$(pgrep -c -f 'herdr-core serve' 2>/dev/null); echo "${n:-0}"; }
holder_count() { n=$(pgrep -c -f 'herdr-core hold' 2>/dev/null); echo "${n:-0}"; }

write_plan() {
  out=$1
  {
    echo "# reap plan $(date -Is)"
    echo "# 规范 runtime: $CANON"
    echo "# 分类规则见 scripts/reap-cores.sh 头部"
    echo "## 要停的 systemd 单元（仅 leak 会话）"
    leak_units | awk -F'\t' '$2=="active" {print $1}'
    echo "## 要 kill -TERM 的 core（leak 会话）：pid<TAB>session<TAB>class<TAB>socket"
    awk -F'\t' '$7 ~ /^leak/ && $2=="core" {printf "%s\t%s\t%s\t%s\n", $1, $3, $7, $4}' \
      "$WORK/cores"
    echo "## 要 kill -TERM 的 holder（leak 会话）：pid<TAB>session<TAB>class<TAB>socket"
    awk -F'\t' '$7 ~ /^leak/ && $2=="holder" {printf "%s\t%s\t%s\t%s\n", $1, $3, $7, $4}' \
      "$WORK/holders"
    echo "## 不动：pid<TAB>session<TAB>class<TAB>socket"
    awk -F'\t' '$7 !~ /^leak/ {printf "%s\t%s\t%s\t%s\n", $1, $3, $7, $4}' \
      "$WORK/cores" "$WORK/holders"
  } > "$out"
}

echo "== before: cores=$(core_count) holders=$(holder_count)  canonical=$CANON"
scan
echo
print_table
echo
echo "== 分类（core）:"
summary cores core
echo "== 分类（holder）:"
summary holders holder

if [ -n "$EVID" ]; then
  mkdir -p "$EVID"
  write_plan "$EVID/reap-plan.txt"
  echo "== plan: $EVID/reap-plan.txt"
fi

if [ "$APPLY" -eq 0 ]; then
  echo "== 干跑：没有动任何进程。加 --apply 执行上面 plan。"
  exit 0
fi

stopped_units=0
signalled=0
skipped_self=0

# 第一批：活跃单元（leak 会话）交给 systemd 停，连带回收 holder 持有的终端。
for unit in $(leak_units | awk -F'\t' '$2=="active" {print $1}'); do
  echo "stop unit: $unit"
  systemctl --user stop "$unit" || { echo "错：停不掉 $unit，中止" >&2; exit 1; }
  stopped_units=$((stopped_units + 1))
done

# 第二批：剩下的 core 直接 SIGTERM（单元已退的孤儿、scope 里的孤儿）。
scan
for pid in $(awk -F'\t' '$7 ~ /^leak/ && $2=="core" {print $1}' "$WORK/cores"); do
  if is_ancestor "$pid"; then
    echo "skip core $pid（是本进程的祖先）"
    skipped_self=$((skipped_self + 1))
    continue
  fi
  case "$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)" in
    */herdr-core\ serve*) ;;
    *) echo "skip core $pid（已不是 herdr-core serve，pid 可能被复用）"; continue ;;
  esac
  echo "kill -TERM core $pid"
  kill -TERM "$pid" 2>/dev/null || true
  signalled=$((signalled + 1))
done

# 第三批：剩下的 holder（持有终端 pty，core 走了它也该走）。
for pid in $(awk -F'\t' '$7 ~ /^leak/ && $2=="holder" {print $1}' "$WORK/holders"); do
  if is_ancestor "$pid"; then
    echo "skip holder $pid（是本进程的祖先）"
    skipped_self=$((skipped_self + 1))
    continue
  fi
  case "$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)" in
    */herdr-core\ hold*) ;;
    *) echo "skip holder $pid（已不是 herdr-core hold，pid 可能被复用）"; continue ;;
  esac
  echo "kill -TERM holder $pid"
  kill -TERM "$pid" 2>/dev/null || true
  signalled=$((signalled + 1))
done

# 等退出。不用 kill -9 兜底：没退干净就照实报。
sleep "$WAIT"

after_cores=$(core_count)
after_holders=$(holder_count)
scan
left=$(awk -F'\t' '$7 ~ /^leak/' "$WORK/cores" "$WORK/holders" | wc -l | tr -d ' ')

echo
echo "== after: cores=$after_cores holders=$after_holders（停单元 $stopped_units 个，TERM $signalled 个，跳过祖先 $skipped_self 个，未退的 leak 进程 $left 个）"

if [ -n "$EVID" ]; then
  {
    echo "# reap result $(date -Is)"
    echo "before: cores=$(awk -F'\t' 'END {print NR}' "$WORK/cores") holders=$(awk -F'\t' 'END {print NR}' "$WORK/holders")"
    echo "after: cores=$after_cores holders=$after_holders"
    echo "stopped_units=$stopped_units signalled=$signalled skipped_ancestors=$skipped_self leak_left=$left"
    echo "## 剩余 core：pid<TAB>session<TAB>class<TAB>socket"
    awk -F'\t' '{printf "%s\t%s\t%s\t%s\n", $1, $3, $7, $4}' "$WORK/cores"
    echo "## 剩余 holder：pid<TAB>session<TAB>class<TAB>socket"
    awk -F'\t' '{printf "%s\t%s\t%s\t%s\n", $1, $3, $7, $4}' "$WORK/holders"
  } > "$EVID/reap-result.txt"
  echo "== result: $EVID/reap-result.txt"
fi

[ "$left" -eq 0 ] || exit 1
