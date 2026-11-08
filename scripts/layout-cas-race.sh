#!/usr/bin/env bash
# 布局文档并发 CAS 端到端：窗口进程与无窗口 `hc-tui do` 同时写 hctui-layout-v1。
#
# 每轮让两批写入者错开几十毫秒同时动手，制造「读到同一版本、先后 doc.set」的重叠：
#   - 无窗口写入者：`do space.new`（控制 socket 指向不存在的路径，强制走 winless
#     路径，读文档 → 改 → 带版本 CAS 写）；
#   - 窗口写入者：`do tab.new`（窗口自己的控制 socket，窗口 App 读文档 → 改 →
#     带版本 CAS 写）。
# 两边都改了「当前选中位置」这类视图字段，结构变更又互不相交；正确实现必须靠
# 冲突重读与三方合并让双方意图都留下。
# 断言：轮末文档既不丢任何原有 pane，也包含两边新增的全部 pane（6 个），
# schema_version 仍为 3；默认跑 10 轮并打印每轮的版本号序列。
#
# 用法：scripts/layout-cas-race.sh [轮数]
# 需要：tmux、hc（herdr-core）、jq；脚本只用隔离 session 与隔离 XDG 目录。
set -uo pipefail

ROOT=${HC_TUI_RACE_ROOT:-/tmp/hc-tui-cas-race-$$}
SESSION=${HC_TUI_RACE_SESSION:-hcp-debt-race-$$}
SOCK="hcp-debt-race-$$"
BIN=$(cd "$(dirname "$0")/.." && pwd)/${HC_TUI_BIN:-target/debug/hc-tui}
ROUNDS=${1:-10}
# 每轮每边的写入次数。
SPACES=3
TABS=3
FAILED=0

export HC_SESSION=$SESSION
export XDG_RUNTIME_DIR=$ROOT/run XDG_STATE_HOME=$ROOT/state
export XDG_CONFIG_HOME=$ROOT/config XDG_CACHE_HOME=$ROOT/cache
# 外部写入者永远走无窗口路径：控制 socket 指向一个不存在的路径。
NO_SOCKET=$ROOT/no-window.sock

cleanup() {
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  for pid in $(pgrep -x hc-tui 2>/dev/null); do
    tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -qx "HC_SESSION=$SESSION" &&
      kill -9 "$pid" 2>/dev/null
  done
  hc server stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

doc() {
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"doc.get","params":{"name":"hctui-layout-v1"}}' |
    hc connect 2>/dev/null
}
doc_version() { doc | jq -r '.result.version // 0'; }
doc_schema() { doc | jq -r '.result.value.schema_version // empty'; }
doc_origins() {
  doc | jq -r '.result.value.panes // {} | to_entries[] | .value.origin // empty' | sort
}

# 轮末收敛：原有 pane 一个不少，且恰好新增 SPACES+TABS 个。
converged() {
  local before=$1 now new
  now=$(doc_origins)
  [[ $(comm -23 <(echo "$before") <(echo "$now") | grep -c .) -eq 0 ]] || return 1
  new=$(comm -13 <(echo "$before") <(echo "$now") | grep -c .)
  [[ $new -eq $((SPACES + TABS)) ]]
}

# 释放文件：无窗口写入者先放行，窗口写入者晚 30ms 放行，让两边的读-写重叠。
barrier() {
  while [[ ! -e "$1" ]]; do sleep 0.002; done
}

cleanup
rm -rf "$ROOT"
mkdir -p "$ROOT"/{run,state,config,cache}

hc server start >/dev/null 2>&1 || { echo "FAIL cannot start core"; exit 1; }
sleep 1

tmux -L "$SOCK" new-session -d -s tui -x 400 -y 100 \
  "cd /tmp && TERM=xterm-256color $BIN ${HC_TUI_RACE_ARGS:-}"

# 等窗口自己的控制 socket 就绪；种子只让窗口写。
ready=0
for _ in $(seq 200); do
  if "$BIN" ls >/dev/null 2>&1; then ready=1; break; fi
  sleep 0.05
done
if [[ $ready -ne 1 ]]; then
  echo "FAIL window control socket never came up"
  exit 1
fi

"$BIN" do tab.new '{"name":"seed"}' >/dev/null 2>&1
for _ in $(seq 100); do
  [[ $(doc_origins | grep -c .) -ge 1 ]] && break
  sleep 0.05
done
if [[ $(doc_origins | grep -c .) -lt 1 ]]; then
  echo "FAIL seeding produced no layout document"
  exit 1
fi
echo "seeded: panes=$(doc_origins | grep -c .) version=$(doc_version) schema=$(doc_schema)"

printf 'round\tversion_before\tversion_after\tdelta\tpanes_before\tpanes_after\tfailed_do\n'
for round in $(seq "$ROUNDS"); do
  before=$(doc_version)
  origins_before=$(doc_origins)
  n=$(echo "$origins_before" | grep -c .)
  release1=$ROOT/release-space-$round
  release2=$ROOT/release-tab-$round
  rm -f "$release1" "$release2"
  outdir=$ROOT/round-$round
  mkdir -p "$outdir"

  for i in $(seq "$SPACES"); do
    (
      barrier "$release1"
      HCTUI_SOCKET=$NO_SOCKET "$BIN" do space.new "{\"name\":\"space-$round-$i\"}" \
        >"$outdir/space-$i.out" 2>&1
      echo $? >"$outdir/space-$i.code"
    ) &
  done
  sleep 0.03
  for i in $(seq "$TABS"); do
    (
      barrier "$release2"
      "$BIN" do tab.new "{\"name\":\"tab-$round-$i\"}" >"$outdir/tab-$i.out" 2>&1
      echo $? >"$outdir/tab-$i.code"
    ) &
  done
  touch "$release1"
  sleep 0.02
  touch "$release2"
  wait

  failed=0
  for file in "$outdir"/*.code; do
    [[ $(<"$file") == 0 ]] || failed=$((failed + 1))
  done
  for file in "$outdir"/*.out; do
    grep -q '"accepted":true' "$file" || failed=$((failed + 1))
  done

  ok=0
  for _ in $(seq 300); do
    if converged "$origins_before"; then ok=1; break; fi
    sleep 0.05
  done
  after=$(doc_version)
  schema=$(doc_schema)
  panes_after=$(doc_origins | grep -c .)
  if [[ $ok -ne 1 ]]; then
    echo "FAIL round $round: doc did not converge (panes $n -> $panes_after)"
    echo "  space outputs: $(cat "$outdir"/space-*.out | tr '\n' ' ')"
    echo "  tab outputs:   $(cat "$outdir"/tab-*.out | tr '\n' ' ')"
    FAILED=1
  fi
  if [[ $schema != 3 ]]; then
    echo "FAIL round $round: schema_version=$schema"
    FAILED=1
  fi
  if [[ $after -le $before ]]; then
    echo "FAIL round $round: version did not advance ($before -> $after)"
    FAILED=1
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$round" "$before" "$after" \
    "$((after - before))" "$n" "$panes_after" "$failed"
done

if [[ $FAILED -ne 0 ]]; then
  echo "RESULT FAIL"
  exit 1
fi
echo "RESULT OK: $ROUNDS rounds, no lost writes, schema_version 3"
