#!/usr/bin/env bash
# M11b 统一包冒烟：一个包一次安装三类入口，停用后三类一起消失，启用恢复，
# 卸载后记录、代际与模块区域一起清干净。
#
# 用法：scripts/packages-smoke.sh
# 前置：target/debug/hc-tui、tmux、hc（herdr-core）、jq、lazygit、git。
set -uo pipefail
ROOT=/tmp/hc-tui-packages-smoke
ROOTDIR=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOTDIR/target/debug/hc-tui"
PACKAGE=dev.hctui.pkg.git-board
SOCK="hctui-packages-$$"
SESSION="hctui-packages-$$"
FAILED=0

rm -rf "$ROOT"
mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui" "$ROOT/cache" "$ROOT/src" "$ROOT/repo"
cp -r "$ROOTDIR/examples/packages/git-board" "$ROOT/src/git-board"
: > "$ROOT/config/hc-tui/config.toml"
git -C "$ROOT/repo" init -q
git -C "$ROOT/repo" -c user.email=m11b@example.com -c user.name=m11b commit -q --allow-empty -m "smoke fixture"

export HC_SESSION="$SESSION"
export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"

cleanup() {
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.3
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  hc server stop >/dev/null 2>&1 || true
  # 全部通过才清理现场；失败时保留 $ROOT 里的日志与状态。
  if [ "$FAILED" -eq 0 ]; then rm -rf "$ROOT"; fi
}
trap cleanup EXIT

step() { echo "== $*"; }
check() { if printf '%s' "$2" | grep -qF -- "$3"; then echo "ok   $1"; else echo "FAIL $1: $2"; FAILED=1; fi; }
check_not() { if printf '%s' "$2" | grep -qF -- "$3"; then echo "FAIL $1: $2"; FAILED=1; else echo "ok   $1"; fi; }
screen() { tmux -L "$SOCK" capture-pane -p -t tui 2>/dev/null || true; }

step "install one package"
"$BIN" packages install "$ROOT/src/git-board" --yes --enable || FAILED=1
LIST=$("$BIN" packages list)
check "one package with A T M badges" "$LIST" "$PACKAGE"
check "capabilities badge" "$LIST" "ATM"
check "module摘要" "$LIST" "module · Git board"
check "action catalog" "$("$BIN" plugins action list)" "$PACKAGE.summary"

step "run the TUI"
tmux -L "$SOCK" new-session -d -s tui -x 120 -y 34 "cd $ROOT/repo && TERM=xterm-256color $BIN"
for _ in $(seq 1 100); do
  screen | grep -q "Spaces" && break
  sleep 0.05
done
sleep 2
check "module surface renders" "$(screen)" "recent commits"

step "action runs in the Space folder"
"$BIN" do space.new "{\"name\":\"smoke-repo\",\"folder\":\"$ROOT/repo\"}" >/dev/null 2>&1 || true
sleep 0.3
RESP=$("$BIN" plugins action invoke "$PACKAGE.summary" '{"short":true}' 2>&1)
REQUEST=$(printf '%s' "$RESP" | jq -r '.request // empty')
[ -n "$REQUEST" ] || { echo "FAIL no action request: $RESP"; FAILED=1; }
STATUS=''
for _ in $(seq 1 60); do
  STATUS=$("$BIN" plugins action status "$REQUEST" 2>&1)
  printf '%s' "$STATUS" | grep -q '"status": "done"' && break
  printf '%s' "$STATUS" | grep -q '"status": "failed"' && break
  sleep 0.1
done
check "action finished done" "$STATUS" '"status": "done"'
check "action read the branch" "$STATUS" "## master"

step "terminal entry opens lazygit"
PANE=$("$BIN" plugins pane open "$PACKAGE.git-ui" --placement right 2>&1)
check "terminal opened" "$PANE" '"status": "opened"'
TERMINAL=$(printf '%s' "$PANE" | jq -r '.terminal // empty')
[ -n "$TERMINAL" ] || { echo "FAIL no terminal: $PANE"; FAILED=1; }
"$BIN" plugins pane close "$TERMINAL" >/dev/null 2>&1 || true
# 视图关闭不等于程序结束：只有 core 报告终端结束，卸载时才允许回收代际。
hc kill "$TERMINAL" >/dev/null 2>&1 || true
sleep 1

step "disable removes all three"
"$BIN" packages disable "$PACKAGE" >/dev/null || FAILED=1
sleep 1.2
check_not "module surface gone" "$(screen)" "recent commits"
check "action catalog is empty" "$("$BIN" plugins action list 2>&1)" "no plugin actions"
DISABLED=$("$BIN" plugins pane open "$PACKAGE.git-ui" 2>&1)
check "terminal entry refuses" "$DISABLED" "disabled"

step "enable restores all three without restart"
"$BIN" packages enable "$PACKAGE" >/dev/null || FAILED=1
sleep 1.5
check "module surface back" "$(screen)" "recent commits"
check "action catalog back" "$("$BIN" plugins action list)" "$PACKAGE.summary"

step "uninstall cleans the store"
"$BIN" packages uninstall "$PACKAGE" >/dev/null || FAILED=1
sleep 1.2
check "package list empty" "$("$BIN" packages list 2>&1)" "no plugins installed"
[ -e "$ROOT/state/hc-tui/plugins/records/$PACKAGE.json" ] \
  && { echo "FAIL record left behind"; FAILED=1; } || echo "ok   record removed"
[ -e "$ROOT/state/hc-tui/plugins/store/$PACKAGE" ] \
  && { echo "FAIL generation left behind"; FAILED=1; } || echo "ok   generation removed"
check_not "module surface gone after uninstall" "$(screen)" "recent commits"

if [ "$FAILED" -ne 0 ]; then
  echo "packages smoke: 有失败项（现场保留在 $ROOT）"
  exit 1
fi
echo "packages smoke: 全部通过"
exit 0
