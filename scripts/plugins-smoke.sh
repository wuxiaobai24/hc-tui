#!/usr/bin/env bash
# M6 插件冒烟：动作目录、调用、状态、取消、失败日志、禁用与卸载（隔离 XDG 与 hc session）。
# 用法：scripts/plugins-smoke.sh（需要 cargo build 后的 target/debug/hc-tui、tmux、hc、jq）。
set -uo pipefail
ROOT=/tmp/hc-tui-m6
ROOTDIR=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOTDIR/target/debug/hc-tui"
REPO="$ROOTDIR/examples/plugins/hello"
SOCK="hctui-m6-$$"
SESSION="hctui-m6-$$"
FAILED=0
rm -rf "$ROOT"; mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui/plugins" "$ROOT/cache"
: > "$ROOT/config/hc-tui/config.toml"
cp -r "$REPO" "$ROOT/config/hc-tui/plugins/hello"
export HC_SESSION="$SESSION" XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state" XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"
cleanup() {
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.3; tmux -L "$SOCK" kill-server 2>/dev/null || true
  hc server stop >/dev/null 2>&1 || true
  # 全部通过才清理现场；失败时保留 $ROOT 里的日志与状态。
  if [ "$FAILED" -eq 0 ]; then rm -rf "$ROOT"; fi
}
trap cleanup EXIT
step() { echo "== $*"; }
check() { if printf '%s' "$2" | grep -qi -- "$3"; then echo "ok   $1"; else echo "FAIL $1: $2"; FAILED=1; fi; }

step "catalog without a window"
OUT=$("$BIN" plugins action list)
check "three actions listed" "$OUT" "dev.hctui.plugins.hello.summary"
COUNT=$("$BIN" plugins action list --json | jq 'length')
[ "$COUNT" = "3" ] || { echo "FAIL json action count: $COUNT"; FAILED=1; }
"$BIN" plugins action list --json | jq -e '.[0].source | startswith("plugin:")' >/dev/null || FAILED=1

step "install into the store and list"
"$BIN" plugins install "$ROOT/config/hc-tui/plugins/hello" --yes --enable || FAILED=1
"$BIN" plugins list --json | jq -e '.[0].source == "local"' >/dev/null || FAILED=1

step "run the TUI"
tmux -L "$SOCK" new-session -d -s tui -x 110 -y 30 "cd /tmp && TERM=xterm-256color $BIN"
sleep 4

step "invoke summary"
RESP=$("$BIN" plugins action invoke dev.hctui.plugins.hello.summary '{"loud":false}')
REQ=$(printf '%s' "$RESP" | jq -r '.request // empty')
echo "request=$REQ"
for _ in $(seq 1 50); do
  STATUS=$("$BIN" plugins action status "$REQ")
  printf '%s' "$STATUS" | grep -q '"status": "done"' && break
  sleep 0.1
done
check "summary done" "$STATUS" '"status": "done"'
check "context id in stdout" "$STATUS" "$REQ"
OUT=$("$BIN" plugins action log "$REQ")
check "log carries stdout" "$OUT" "request=$REQ"

step "invoke failure"
RESP=$("$BIN" plugins action invoke dev.hctui.plugins.hello.fail)
REQ=$(printf '%s' "$RESP" | jq -r '.request // empty')
for _ in $(seq 1 50); do
  STATUS=$("$BIN" plugins action status "$REQ")
  printf '%s' "$STATUS" | grep -q '"status": "failed"' && break
  sleep 0.1
done
check "failed status" "$STATUS" '"status": "failed"'
check "exit code 7" "$STATUS" '"exit_code": 7'
OUT=$("$BIN" plugins action log "$REQ")
check "stderr in log" "$OUT" "oops"

step "invoke slow then cancel"
RESP=$("$BIN" plugins action invoke dev.hctui.plugins.hello.slow)
REQ=$(printf '%s' "$RESP" | jq -r '.request // empty')
sleep 0.5
"$BIN" plugins action cancel "$REQ" >/dev/null || FAILED=1
for _ in $(seq 1 100); do
  STATUS=$("$BIN" plugins action status "$REQ")
  printf '%s' "$STATUS" | grep -q '"status": "cancelled"' && break
  sleep 0.1
done
check "cancelled status" "$STATUS" '"status": "cancelled"'

step "disable removes actions from the catalog"
"$BIN" plugins disable dev.hctui.plugins.hello || FAILED=1
OUT=$("$BIN" plugins action list)
check "catalog empty" "$OUT" "no plugin actions"
"$BIN" plugins enable dev.hctui.plugins.hello || FAILED=1
OUT=$("$BIN" plugins action list)
check "catalog restored" "$OUT" "summary"

step "uninstall removes the installed generation"
"$BIN" plugins uninstall dev.hctui.plugins.hello || FAILED=1
OUT=$("$BIN" plugins list)
check "no installed packages" "$OUT" "no packages installed"
exit "$FAILED"
