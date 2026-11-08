#!/usr/bin/env bash
# M9 空闲 CPU 基线：无输入 5 秒，从 /proc/<pid>/stat 取 utime+stime 的差值。
# 两态：`--no-mods`（内置外框、不加载模块）与带模块（链接 examples/mods/spaces）。
#
# 用法：scripts/m9-idle-cpu.sh [秒数]
# 依赖：target/release/hc-tui、tmux、hc、nsh（构建示例模块的 bundle）。
# 隔离：独立 XDG 根 + HC_SESSION=hcp-m9a；收尾只停这一个 session。
set -uo pipefail
cd "$(dirname "$0")/.."

SECONDS_IDLE=${1:-5}
BIN=$PWD/target/release/hc-tui
[ -x "$BIN" ] || {
  echo "missing $BIN；先跑 cargo build -p hc-tui --release" >&2
  exit 1
}
ROOT=/tmp/hc-tui-m9-idle
SOCK="hctui-m9-idle-$$"

rm -rf "$ROOT"
mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui" "$ROOT/cache"
: > "$ROOT/config/hc-tui/config.toml"
export HC_SESSION=hcp-m9a
export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"

# 收尾：本脚本起的进程全部认 `XDG_RUNTIME_DIR=$ROOT/run`，按这个环境变量精确回收，
# 不碰别人留下的 cuo 与 holder。hc-tui 收到 SIGTERM 会走 detach 留存，必须 SIGKILL。
cleanup() {
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  local pid
  for pid in $(pgrep -f 'hc-tui|hc connect|herdr-core'); do
    [ "$pid" = "$$" ] && continue
    [ "$pid" = "$PPID" ] && continue
    if cat "/proc/$pid/environ" 2>/dev/null \
      | tr '\0' '\n' | grep -qx "XDG_RUNTIME_DIR=$ROOT/run"; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  sleep 0.2
  # 只停自己这一份；HC_SESSION 已导出。
  hc server stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

# tmux 直接执行二进制时 pane_pid 就是 hc-tui；它的子进程是 `hc connect`。
tui_pid() {
  local pane
  pane=$(tmux -L "$SOCK" list-panes -F '#{pane_pid}' -t tui 2>/dev/null | head -1)
  [ -n "$pane" ] || return 1
  if [ "$(cat "/proc/$pane/comm" 2>/dev/null)" != "hc-tui" ]; then
    pane=$(pgrep -P "$pane" -x hc-tui | head -1)
  fi
  [ -n "$pane" ] || return 1
  echo "$pane"
}

measure() {
  local state=$1
  shift
  tmux -L "$SOCK" new-session -d -s tui -x 110 -y 30 \
    "cd /tmp && TERM=xterm-256color $BIN $*"
  local ready=0
  for _ in $(seq 1 100); do
    if tmux -L "$SOCK" capture-pane -p -t tui 2>/dev/null | grep -q "Spaces"; then
      ready=1
      break
    fi
    sleep 0.05
  done
  if [ "$ready" = 0 ]; then
    echo "state=$state failed: TUI did not draw" >&2
    return 1
  fi
  sleep 2
  local pid ticks start end pct
  pid=$(tui_pid) || {
    echo "state=$state failed: cannot find the TUI process" >&2
    return 1
  }
  ticks=$(getconf CLK_TCK)
  start=$(awk '{print $14+$15}' "/proc/$pid/stat")
  sleep "$SECONDS_IDLE"
  end=$(awk '{print $14+$15}' "/proc/$pid/stat")
  pct=$(awk -v d="$((end - start))" -v t="$ticks" -v s="$SECONDS_IDLE" \
    'BEGIN{printf "%.3f", d*100/(t*s)}')
  printf 'state=%-8s window_s=%s pid=%s idle_cpu_pct=%s\n' "$state" "$SECONDS_IDLE" "$pid" "$pct"
  tmux -L "$SOCK" kill-session -t tui 2>/dev/null || true
  sleep 1
}

# 带模块那一态：示例模块打到 bundle，再链接进隔离配置目录。
SRC=$ROOT/mod-spaces
cp -r examples/mods/spaces "$SRC"
(
  cd "$SRC" || exit 1
  nsh build -o dist >/dev/null 2>&1 || exit 1
) || {
  echo "cannot build examples/mods/spaces" >&2
  exit 1
}
"$BIN" packages link "$SRC" --enable >/dev/null 2>&1 || {
  echo "cannot link the spaces module" >&2
  exit 1
}

measure no-mods --no-mods
measure with-mods
