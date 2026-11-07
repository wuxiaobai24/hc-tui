#!/usr/bin/env bash
# M8 退化状态验证：--no-mods、禁用全部模块、禁用全部插件三种状态下界面与终端可用。
set -uo pipefail
ROOTDIR=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOTDIR/target/debug/hc-tui"
FAILED=0
ROOTS=()

cleanup() {
  hc server stop >/dev/null 2>&1 || true
  # 全部通过才清理现场；失败时保留 $ROOT 里的日志与状态。
  if [ "$FAILED" -eq 0 ]; then rm -rf "${ROOTS[@]}"; fi
}
trap cleanup EXIT

step() { echo "== $*"; }
check() {
  if printf '%s' "$2" | grep -qF -- "$3"; then echo "ok   $1"; else echo "FAIL $1: $2"; FAILED=1; fi
}

run_mode() {
  local label=$1 flags=$2
  local ROOT=/tmp/hc-tui-degraded-$label
  ROOTS+=("$ROOT")
  local SOCK="hctui-deg-$label-$$"
  local SESSION="hctui-deg-$label-$$"
  rm -rf "$ROOT"
  mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui" "$ROOT/cache"
  : > "$ROOT/config/hc-tui/config.toml"
  export HC_SESSION="$SESSION"
  export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
  export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"

  case "$label" in
    no-mods)
      for mod in agents spaces prs; do
        "$BIN" mods install "$ROOTDIR/examples/mods/$mod" --yes >/dev/null 2>&1 || true
      done
      ;;
    mods-off)
      for mod in agents spaces prs; do
        "$BIN" mods install "$ROOTDIR/examples/mods/$mod" --yes >/dev/null 2>&1 || true
        "$BIN" mods disable "$mod" >/dev/null 2>&1 || true
      done
      local enabled
      enabled=$("$BIN" mods list --json 2>/dev/null | jq '[.[] | select(.enabled == true)] | length' 2>/dev/null)
      check "no module is enabled" "${enabled:-x}" "0"
      ;;
    plugins-off)
      mkdir -p "$ROOT/src"
      cp -r "$ROOTDIR/examples/plugins/hello" "$ROOT/src/hello"
      cp -r "$ROOTDIR/examples/plugins/events" "$ROOT/src/events"
      for plugin in dev.hctui.plugins.hello dev.hctui.plugins.events; do
        "$BIN" plugins install "$ROOT/src/${plugin##*.}" --yes >/dev/null 2>&1 || true
        "$BIN" plugins disable "$plugin" >/dev/null 2>&1 || true
      done
      check "plugin catalog is empty" "$("$BIN" plugins action list 2>&1)" "no plugin actions"
      ;;
  esac

  tmux -L "$SOCK" new-session -d -s tui -x 110 -y 30 "cd /tmp && TERM=xterm-256color $BIN $flags"
  for _ in $(seq 1 100); do
    tmux -L "$SOCK" capture-pane -p -t tui 2>/dev/null | grep -q "Spaces" && break
    sleep 0.05
  done
  sleep 1.5
  local screen token="deg${label}zz"
  screen=$(tmux -L "$SOCK" capture-pane -p -t tui)
  check "sidebar renders" "$screen" "Spaces"
  tmux -L "$SOCK" send-keys -t tui -l "$token"
  sleep 0.6
  check "terminal echoes typing" "$(tmux -L "$SOCK" capture-pane -p -t tui)" "$token"
  tmux -L "$SOCK" send-keys -t tui C-b v
  sleep 0.8
  check "split still works" "$(tmux -L "$SOCK" capture-pane -p -t tui)" "┌"
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.4
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  hc server stop >/dev/null 2>&1 || true
}

step "--no-mods"
run_mode no-mods "--no-mods"
step "all modules disabled"
run_mode mods-off ""
step "all plugins disabled"
run_mode plugins-off ""

if [ "$FAILED" -ne 0 ]; then
  echo "degraded smoke: 有失败项"
  exit 1
fi
echo "degraded smoke: 全部通过"
