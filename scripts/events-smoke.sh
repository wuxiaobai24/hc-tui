#!/usr/bin/env bash
# M8 事件流冒烟：两个客户端各收到一次、按 kind 与 plugin 过滤、客户端断开后
# 服务仍可用、队列溢出发出 events.dropped、插件动作消费事件。
#
# 用法：scripts/events-smoke.sh
# 前置：target/debug/hc-tui、tmux、hc（herdr-core）、jq。
set -uo pipefail
ROOT=${HC_TUI_M8_ROOT:-/tmp/hc-tui-m8}
ROOTDIR=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOTDIR/target/debug/hc-tui"
SOCK="hctui-m8-$$"
SESSION="hctui-m8-$$"
FAILED=0

rm -rf "$ROOT"
mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui" "$ROOT/cache" "$ROOT/src"
cp -r "$ROOTDIR/examples/plugins/events" "$ROOT/src/events"

export HC_SESSION="$SESSION"
export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"
: > "$ROOT/config/hc-tui/config.toml"

cleanup() {
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.3
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  hc server stop >/dev/null 2>&1 || true
  pkill -f "$ROOT/events.*\.log" 2>/dev/null || true
}
trap cleanup EXIT

step() { echo "== $*"; }
check() {
  if printf '%s' "$2" | grep -qF -- "$3"; then echo "ok   $1"; else echo "FAIL $1: $2"; FAILED=1; fi
}
check_json() {
  if printf '%s' "$2" | jq -e "$3" >/dev/null 2>&1; then echo "ok   $1"; else echo "FAIL $1: $2"; FAILED=1; fi
}
events_client() {
  local log="$1"
  shift
  "$BIN" events "$@" > "$log" 2>"$log.err" &
  echo $!
}
wait_lines() {
  local file="$1" want="$2"
  for _ in $(seq 1 100); do
    [ -f "$file" ] && [ "$(wc -l < "$file")" -ge "$want" ] && return 0
    sleep 0.05
  done
  return 1
}
trigger_status() {
  local state="$1"
  "$BIN" do terminal.open "{\"argv\":[\"sh\",\"-c\",\"printf '\\\\033]7501;state=$state\\\\a'; sleep 0.2\"]}" >/dev/null
}

step "install the events plugin"
"$BIN" plugins install "$ROOT/src/events" --yes --enable >/dev/null || FAILED=1

step "run the TUI"
tmux -L "$SOCK" new-session -d -s tui -x 110 -y 30 "cd /tmp && TERM=xterm-256color $BIN"
sleep 4

step "two clients each receive one copy"
A=$ROOT/a.log; A2=$ROOT/a2.log
PA=$(events_client "$A" --kinds terminal.status --count 1)
PA2=$(events_client "$A2" --kinds terminal.status --count 1)
sleep 0.6
trigger_status working
wait_lines "$A" 1 && echo "ok   first client got the status" || { echo "FAIL first client got the status"; FAILED=1; }
wait_lines "$A2" 1 && echo "ok   second client got the status" || { echo "FAIL second client got the status"; FAILED=1; }
check "same event payload" "$(cat "$A")" '"kind":"terminal.status"'
check_json "status payload" "$(head -1 "$A")" '.data.status == "working" and .machine == "local"'
[ "$(wc -l < "$A")" = "1" ] && echo "ok   one delivery only" || { echo "FAIL deliveries: $(wc -l < "$A")"; FAILED=1; }
wait "$PA" 2>/dev/null || true
wait "$PA2" 2>/dev/null || true

step "kind filter excludes other events"
B=$ROOT/b.log
PB=$(events_client "$B" --kinds terminal.finished --count 1)
sleep 0.5
trigger_status idle
sleep 0.6
if grep -q 'terminal.status' "$B" 2>/dev/null; then
  echo "FAIL kind filter leaked: $(cat "$B")"
  FAILED=1
else
  echo "ok   status event not delivered to the finished-only client"
fi
"$BIN" do terminal.open '{"argv":["sh","-c","exit 0"]}' >/dev/null
wait_lines "$B" 1 && echo "ok   finished event delivered" || { echo "FAIL finished event delivered"; FAILED=1; }
check "finished payload" "$(head -1 "$B")" '"kind":"terminal.finished"'
wait "$PB" 2>/dev/null || true

step "plugin filter"
C=$ROOT/c.log
PC=$(events_client "$C" --plugin dev.hctui.plugins.events --count 1)
sleep 0.5
"$BIN" plugins pane open dev.hctui.plugins.events.feed >"$ROOT/feed.json" 2>&1
wait_lines "$C" 1 && echo "ok   plugin event delivered" || { echo "FAIL plugin event delivered"; FAILED=1; }
check_json "plugin event carries the package" "$(head -1 "$C")" '.plugin == "dev.hctui.plugins.events"'
wait "$PC" 2>/dev/null || true

step "disconnect stops the subscription and the socket stays usable"
timeout 0.4 "$BIN" events --count 1000 >/dev/null 2>&1
D=$ROOT/d.log
PD=$(events_client "$D" --kinds terminal.status --count 1)
sleep 0.5
trigger_status working
wait_lines "$D" 1 && echo "ok   stream usable after a disconnect" || { echo "FAIL stream usable after a disconnect"; FAILED=1; }
wait "$PD" 2>/dev/null || true

step "queue overflow emits events.dropped"
E=$ROOT/e.log
PE=$(events_client "$E" --count 100000)
sleep 0.5
kill -STOP "$PE" 2>/dev/null
"$BIN" do terminal.open '{"argv":["sh","-c","i=0; while [ $i -lt 3000 ]; do printf \"\\033]7501;state=working\\a\"; printf \"\\033]7501;state=idle\\a\"; i=$((i+1)); done; sleep 4"]}' >/dev/null
sleep 1.5
kill -CONT "$PE" 2>/dev/null
for _ in $(seq 1 200); do
  grep -q 'events.dropped' "$E" 2>/dev/null && break
  sleep 0.05
done
if grep -q 'events.dropped' "$E"; then
  echo "ok   events.dropped sent"
  check_json "dropped count" "$(grep 'events.dropped' "$E" | head -1)" '.count > 0'
  echo "     delivered=$(grep -c 'terminal.status' "$E") dropped=$(grep 'events.dropped' "$E" | head -1)"
else
  echo "FAIL events.dropped missing (delivered=$(grep -c . "$E" 2>/dev/null || echo 0))"
  FAILED=1
fi
kill "$PE" 2>/dev/null; wait "$PE" 2>/dev/null || true

step "plugin action consumes the stream"
REQ=$("$BIN" plugins action invoke dev.hctui.plugins.events.watch '{"count":1}' | jq -r '.request // empty')
sleep 0.5
trigger_status working
STATUS=""
for _ in $(seq 1 200); do
  STATUS=$("$BIN" plugins action status "$REQ" 2>/dev/null)
  printf '%s' "$STATUS" | jq -e '.status == "done"' >/dev/null 2>&1 && break
  sleep 0.1
done
check_json "watch action finished" "$STATUS" '.status == "done"'
check "action printed an event" "$STATUS" 'terminal.status'
LOG=$("$BIN" plugins action log "$REQ" 2>/dev/null)
check "log carries the event line" "$LOG" '"kind":"terminal.status"'

if [ "$FAILED" -ne 0 ]; then
  echo "events smoke: 有失败项"
  exit 1
fi
echo "events smoke: 全部通过"
