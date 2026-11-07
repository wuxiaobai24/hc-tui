#!/usr/bin/env bash
# M7 插件终端冒烟：远端 placement/cwd/环境/标签、关闭 Pane 不结束程序也不释放代际 pin、
# core 报告终端结束才回收代际、detach 后继续运行、包内相对程序的 unsupported、缺失程序、
# 离线与 target_gone。
#
# 用法：scripts/plugin-terminals-smoke.sh
# 前置：target/debug/hc-tui、tmux、hc（herdr-core）、jq、ssh 到 localhost 免密。
set -uo pipefail
ROOT=${HC_TUI_M7_ROOT:-/tmp/hc-tui-m7}
ROOTDIR=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOTDIR/target/debug/hc-tui"
SOCK="hctui-m7-$$"
SESSION="hctui-m7-$$"
# peer 是 localhost 上的另一个 core session：用独立名字，结束时停掉，不碰默认会话。
PEER_SESSION="hctui-m7-peer-$$"
FAILED=0

rm -rf "$ROOT"
mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui" "$ROOT/cache" "$ROOT/src"
cp -r "$ROOTDIR/examples/plugins/git-tools" "$ROOT/src/git-tools"
cp -r "$ROOTDIR/examples/plugins/review" "$ROOT/src/review"
mkdir -p "$ROOT/src/missing" "$ROOT/repo"
git -C "$ROOT/repo" init -q
git -C "$ROOT/repo" -c user.email=m7@example.com -c user.name=m7 commit -q --allow-empty -m "m7 fixture"
cat > "$ROOT/src/missing/plugin.toml" <<'EOF'
manifest_version = 1
id = "dev.hctui.plugins.missing"
name = "Missing"
version = "0.1.0"
host_api = 1
platforms = ["linux", "macos"]

[[terminals]]
id = "needs-tool"
title = "Needs a missing tool"
argv = ["definitely-missing-hctui-prog"]
placement = "tab"
requires_programs = ["definitely-missing-hctui-prog"]
EOF

export HC_SESSION="$SESSION"
export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"
: > "$ROOT/config/hc-tui/config.toml"

cleanup() {
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.3
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  hc server stop >/dev/null 2>&1 || true
  # peer 的 core 跑在本机默认 XDG 下，只按会话名停掉测试自己创建的那一个。
  env -u XDG_RUNTIME_DIR -u XDG_STATE_HOME -u XDG_CONFIG_HOME -u XDG_CACHE_HOME \
    HC_SESSION="$PEER_SESSION" hc server stop >/dev/null 2>&1 || true
  rm -rf "$HOME/.local/state/herdr-core/$PEER_SESSION"
}
trap cleanup EXIT

step() { echo "== $*"; }
# 经运行中 TUI 的命令：stderr 收集起来，失败时打印，不混进 JSON。
ui() { "$BIN" "$@" 2>>"$ROOT/ui.err"; }
check() {
  if printf '%s' "$2" | grep -qF -- "$3"; then
    echo "ok   $1"
  else
    echo "FAIL $1: $2"
    [ -s "$ROOT/ui.err" ] && { echo "     ui: $(tail -2 "$ROOT/ui.err" | tr '\n' ' ')"; }
    FAILED=1
  fi
}
check_json() {
  local label="$1" json="$2" filter="$3"
  if printf '%s' "$json" | jq -e "$filter" >/dev/null 2>&1; then
    echo "ok   $label"
  else
    echo "FAIL $label: $json"
    [ -s "$ROOT/ui.err" ] && { echo "     ui: $(tail -2 "$ROOT/ui.err" | tr '\n' ' ')"; }
    FAILED=1
  fi
}
store_dir() { echo "$ROOT/state/hc-tui/plugins"; }

step "install the three example plugins"
"$BIN" plugins install "$ROOT/src/git-tools" --yes >/dev/null || FAILED=1
"$BIN" plugins install "$ROOT/src/review" --yes >/dev/null || FAILED=1
"$BIN" plugins install "$ROOT/src/missing" --yes >/dev/null || FAILED=1
"$BIN" plugins list | grep -q "git-ui" && echo "ok   terminal entries listed" || { echo "FAIL terminal entries listed"; FAILED=1; }

step "machine peer over ssh to localhost"
if hc machine add localhost --name peer --session "$PEER_SESSION" --install --json \
    >/dev/null 2>"$ROOT/machine.err"; then
  echo "ok   peer reachable"
else
  echo "FAIL peer unreachable: $(cat "$ROOT/machine.err")"
  echo "plugin terminals smoke: 有失败项"
  exit 1
fi

step "run the TUI"
tmux -L "$SOCK" new-session -d -s tui -x 110 -y 30 "cd /tmp && TERM=xterm-256color $BIN"
sleep 4

step "remote space and lazygit terminal"
"$BIN" do space.new '{"name":"m7-remote","folder":"'"$ROOT"'/repo","machine":"peer"}' >/dev/null
OPEN=$(ui plugins pane open dev.hctui.plugins.git-tools.git-ui)
echo "$OPEN" | jq -c '{request,status,pane,terminal,error,error_kind}' 2>/dev/null || echo "$OPEN"
check_json "opened on peer" "$OPEN" '.status == "opened" and (.terminal | startswith("peer/"))'
GIT_REQ=$(printf '%s' "$OPEN" | jq -r '.request // empty')
GIT_ADDR=$(printf '%s' "$OPEN" | jq -r '.terminal // empty')
GIT_PANE=$(printf '%s' "$OPEN" | jq -r '.pane // empty')

if [ -n "$GIT_ADDR" ]; then
  GIT_TERM=$(hc get "$GIT_ADDR" --json)
  check "cwd from the space" "$GIT_TERM" '"cwd":"'"$ROOT"'/repo"'
  check "keep stays on" "$GIT_TERM" '"keep":true'
  check "plugin label" "$GIT_TERM" '"hctui.plugin":"dev.hctui.plugins.git-tools"'
  check "terminal label" "$GIT_TERM" '"hctui.terminal":"git-ui"'
  check "where label" "$GIT_TERM" '"where":"m7-remote / 1"'
  GIT_PID=$(printf '%s' "$GIT_TERM" | jq -r '.foreground.pid // .pid')
  ENVIRON=$(tr '\0' '\n' < "/proc/$GIT_PID/environ" 2>/dev/null)
  check "injected plugin id" "$ENVIRON" "HCTUI_PLUGIN_ID=dev.hctui.plugins.git-tools"
  check "injected placement" "$ENVIRON" "HCTUI_PLACEMENT=right"
  check "injected context" "$ENVIRON" "HCTUI_CONTEXT="
  check "injected socket" "$ENVIRON" "HCTUI_SOCKET="
  SCREEN=$(hc read "$GIT_ADDR" --json | jq -r '.text // .output // ""')
  check "lazygit draws its panels" "$SCREEN" "Status"
  check_json "status reports the opened terminal" "$( \
    ui plugins pane status "$GIT_REQ")" \
    '.status == "opened" and (.terminal | startswith("peer/"))'
fi

step "uninstall keeps the generation while the terminal runs"
GEN_DIR=$(store_dir)/store/dev.hctui.plugins.git-tools
[ -d "$GEN_DIR" ] && echo "ok   generation installed" || { echo "FAIL generation missing"; FAILED=1; }
"$BIN" plugins uninstall dev.hctui.plugins.git-tools >/dev/null 2>&1
[ -d "$GEN_DIR" ] && echo "ok   generation kept for the running terminal" || { echo "FAIL generation removed too early"; FAILED=1; }
if "$BIN" plugins list 2>&1 | grep -q "dev.hctui.plugins.git-tools"; then
  echo "FAIL installed record still listed"
  FAILED=1
else
  echo "ok   installed record is gone"
fi

step "closing the pane keeps the program and its generation"
ui plugins pane close "$GIT_ADDR" >/dev/null
sleep 0.5
[ -d "$GEN_DIR" ] && echo "ok   generation kept after the pane closed" || { echo "FAIL generation removed while the terminal runs"; FAILED=1; }
check_json "request reports the closed pane" "$( \
  ui plugins pane status "$GIT_REQ")" '.status == "closed"'
check "program survives the closed pane" "$(hc get "$GIT_ADDR" --json)" '"state":"live"'
# 视图已关闭、内存里不再有入口记录时，只能靠 core 里的标签定位。
check_json "focus finds the terminal by plugin label" "$( \
  ui plugins pane focus dev.hctui.plugins.git-tools.git-ui)" \
  '.focused == true'
check_json "close finds the recreated view by label" "$( \
  ui plugins pane close dev.hctui.plugins.git-tools.git-ui)" \
  '.closed == true'

step "core ends the terminal and the generation is reclaimed"
hc kill "$GIT_ADDR" >/dev/null 2>&1 || true
sleep 1
[ -d "$GEN_DIR" ] && { echo "FAIL generation kept after the terminal ended"; FAILED=1; } || echo "ok   generation reclaimed after core reported the terminal ended"

step "package-relative program runs on the client host"
"$BIN" do space.new '{"name":"m7-local","folder":"/tmp","machine":""}' >/dev/null
REVIEW=$(ui plugins pane open dev.hctui.plugins.review.review)
echo "$REVIEW" | jq -c '{request,status,pane,terminal}' 2>/dev/null || echo "$REVIEW"
check_json "review opened locally" "$REVIEW" '.status == "opened" and (.terminal != null)'
REVIEW_ADDR=$(printf '%s' "$REVIEW" | jq -r '.terminal // empty')
REVIEW_PANE=$(printf '%s' "$REVIEW" | jq -r '.pane // empty')
if [ -n "$REVIEW_ADDR" ]; then
  REVIEW_SCREEN=$(hc read "$REVIEW_ADDR" --json | jq -r '.text // ""')
  check "relative program started" "$REVIEW_SCREEN" "review start"
  check "cwd from the space" "$REVIEW_SCREEN" "cwd=/tmp"
  check "plugin id in env" "$REVIEW_SCREEN" "plugin=dev.hctui.plugins.review"
  check "placement in env" "$REVIEW_SCREEN" "placement=tab"
fi

step "package-relative program is unsupported on a remote machine"
BEFORE=$(hc ls --json | jq '[.terminals[] | select(.address | startswith("peer/"))] | length')
REL=$(ui plugins pane open dev.hctui.plugins.review.review --machine peer)
echo "$REL" | jq -c '{status,error_kind,error}' 2>/dev/null || echo "$REL"
check "kind is unsupported" "$REL" '"error_kind": "unsupported"'
check "explains the machine" "$REL" "peer"
AFTER=$(hc ls --json | jq '[.terminals[] | select(.address | startswith("peer/"))] | length')
[ "$BEFORE" = "$AFTER" ] && echo "ok   no fallback to this machine" || { echo "FAIL terminals appeared: $BEFORE -> $AFTER"; FAILED=1; }
REL_REQ=$(printf '%s' "$REL" | jq -r '.request // empty')

step "missing program on the target machine"
MISSING=$(ui plugins pane open dev.hctui.plugins.missing.needs-tool --machine peer)
echo "$MISSING" | jq -c '{status,error_kind,error}' 2>/dev/null || echo "$MISSING"
check "kind is missing_program" "$MISSING" '"error_kind": "missing_program"'
check "lists the missing program" "$MISSING" "definitely-missing-hctui-prog"
check "checks the target machine" "$MISSING" "on peer"

step "stale targets"
check "unknown space" "$(ui plugins pane open dev.hctui.plugins.git-tools.git-ui --space sp999)" '"error_kind": "target_gone"'
check "unknown terminal" "$(ui plugins pane focus peer/t999)" '"error_kind": "target_gone"'

step "offline machine"
hc machine rm peer >/dev/null 2>&1
OFFLINE=$(ui plugins pane open dev.hctui.plugins.git-tools.git-ui --machine peer)
echo "$OFFLINE" | jq -c '{status,error_kind,error}' 2>/dev/null || echo "$OFFLINE"
check "kind is offline" "$OFFLINE" '"error_kind": "offline"'

hc machine add localhost --name peer --session "$PEER_SESSION" --install >/dev/null 2>&1

step "detach keeps both terminals running"
tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
sleep 1
tmux -L "$SOCK" kill-server 2>/dev/null || true
sleep 1
check "review terminal survives detach" "$(hc get "$REVIEW_ADDR" --json)" '"state":"live"'

if [ "$FAILED" -ne 0 ]; then
  [ -s "$ROOT/ui.err" ] && { echo "== control errors"; cat "$ROOT/ui.err"; }
  echo "plugin terminals smoke: 有失败项"
  exit 1
fi
echo "plugin terminals smoke: 全部通过"
