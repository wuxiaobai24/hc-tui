#!/usr/bin/env bash
# 真实 core + tmux 的 UI 冒烟测试：在隔离 session 里驱动 TUI，断言关键行为。
#
# 用法：
#   scripts/ui-smoke.sh            # 用 target/debug/hc-tui
#   HC_TUI_BIN=target/release/hc-tui scripts/ui-smoke.sh
#
# 前置：本机有 hc（herdr-core）、tmux、可用的 $SHELL。脚本不碰默认 session。
set -euo pipefail

ROOT=${HC_TUI_SMOKE_ROOT:-/tmp/hc-tui-smoke}
BIN=$(cd "$(dirname "$0")/.." && pwd)/${HC_TUI_BIN:-target/debug/hc-tui}
SOCK="hctui-smoke-$$"
SESSION="hctui-smoke-$$"
WIDTH=110
HEIGHT=30
FAILED=0

cleanup() {
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.3
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  (
    export HC_SESSION="$SESSION"
    export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
    export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"
    hc server stop >/dev/null 2>&1 || true
  )
}
trap cleanup EXIT

rm -rf "$ROOT"
mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config" "$ROOT/cache"

export HC_SESSION="$SESSION"
export XDG_RUNTIME_DIR="$ROOT/run"
export XDG_STATE_HOME="$ROOT/state"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_CACHE_HOME="$ROOT/cache"

start() {
  tmux -L "$SOCK" new-session -d -s tui -x "$WIDTH" -y "$HEIGHT" \
    "cd /tmp && TERM=xterm-256color $BIN ${2:-}"
  sleep "${1:-3.5}"
}

key() { tmux -L "$SOCK" send-keys -t tui "$@"; }
type_text() { tmux -L "$SOCK" send-keys -t tui -l "$1"; }
screen() { tmux -L "$SOCK" capture-pane -p -t tui; }

expect() {
  local label="$1" needle="$2"
  if screen | grep -qF -- "$needle"; then
    echo "ok   $label"
  else
    echo "FAIL $label: 屏幕上找不到 '$needle'"
    screen | tail -20
    FAILED=1
  fi
}

expect_missing() {
  local label="$1" needle="$2"
  if screen | grep -qF -- "$needle"; then
    echo "FAIL $label: 屏幕上仍出现 '$needle'"
    FAILED=1
  else
    echo "ok   $label"
  fi
}

expect_tabs() {
  local label="$1" want="$2"
  if [[ "$(screen | head -1 | grep -o '[0-9]' | wc -l)" -ge "$want" ]]; then
    echo "ok   $label"
  else
    echo "FAIL $label: 第一行找不到 $want 个 Tab 编号"
    screen | head -3
    FAILED=1
  fi
}

start
expect "首次绘制" "Spaces"
expect "侧栏 agents 段" "Agents"

# Machine headers collapse the entire Spaces group and expand on a second click.
key -l $'\e[<0;2;3M\e[<0;2;3m'; sleep 0.3
expect "折叠机器空间" "▸ This computer"
key -l $'\e[<0;2;3M\e[<0;2;3m'; sleep 0.3
expect "展开机器空间" "▾ This computer"

# 新 Tab（默认先问名字）与左右分屏
key C-b c; sleep 0.5
key Enter; sleep 0.5
key C-b c; sleep 0.5
key Enter; sleep 0.5
key C-b v; sleep 0.8
expect_tabs "三个 Tab" 3
expect "分屏后的边框" "┌"

# Stack：当前 Tab 全部收成堆叠，折叠项只留标题行
key C-b t; sleep 1
expect "Stack 提示" "stacked 2 panes"
if [[ "$(screen | grep -c '^ *│ *[^│ ]' || true)" -ge 1 ]]; then
  echo "ok   Stack 标题行"
else
  echo "FAIL Stack 没有画出折叠标题行"
  screen | tail -20
  FAILED=1
fi

# 布局预设：按顺序重排分屏
key C-b L; sleep 1
expect "布局预设提示" "layout ·"

# 浮动层：浮起、收起、落回分屏
key C-b f; sleep 1
expect "浮起后边框" "┌"
if screen | grep -qF "pin"; then
  echo "FAIL 未 pin 的浮动 Pane 标题里出现了 pin"
  FAILED=1
else
  echo "ok   浮动 Pane 未 pin"
fi
key C-b F; sleep 0.8
expect "pin 标记" "pin"
key C-b H; sleep 0.8
key C-b H; sleep 0.8
key C-b f; sleep 1
expect_missing "落回分屏后没有 pin 标记" "pin"

# 布局写入 core doc 并跨重启恢复
sleep 1.2
key C-b q; sleep 0.8
tmux -L "$SOCK" kill-server 2>/dev/null || true
start 4.5
expect_tabs "重启后恢复 Tab 数" 3

# Copy Mode：进入、选择、复制
type_text "printf 'alpha beta\\n'"; key Enter; sleep 0.6
key C-b '['; sleep 0.4
expect "Copy Mode 徽标" "COPY"
key y; sleep 0.6
expect "复制回执" "copied"

# reader：terminal.read 的文本浮层
key C-b /; sleep 1.2
expect "reader 打开" "read ·"
key Escape; sleep 0.4
expect_missing "reader 关闭" "read ·"

# 自定义命令：popup / 临时 pane / shell
mkdir -p "$ROOT/config/hc-tui"
cat > "$ROOT/config/hc-tui/config.toml" <<'EOF'
[session]
restore = true

[[keys.command]]
key = "prefix+g"
type = "popup"
command = "less /etc/hostname"
width = "60%"
height = "50%"

[[keys.command]]
key = "prefix+shift+g"
type = "pane"
command = "echo smoke-pane; sleep 2"

[[keys.command]]
key = "prefix+shift+h"
type = "shell"
command = "date > /tmp/hc-tui-smoke-shell"
EOF
key C-b q; sleep 0.6
tmux -L "$SOCK" kill-server 2>/dev/null || true
start 4 --no-restore
key C-b g; sleep 1.2
expect "popup 打开" "popup"
key Escape; sleep 0.5
expect_missing "popup 关闭" "popup ·"
key C-b G; sleep 1.2
expect "临时 Pane 运行" "smoke-pane"
sleep 2.5
expect_missing "临时 Pane 自动消失" "smoke-pane"
key C-b H; sleep 1
if [[ -s /tmp/hc-tui-smoke-shell ]]; then
  echo "ok   shell 自定义命令"
else
  echo "FAIL shell 自定义命令没有写出标记文件"
  FAILED=1
fi
rm -f /tmp/hc-tui-smoke-shell

if [[ "$FAILED" -ne 0 ]]; then
  echo "ui-smoke: 有失败项"
  exit 1
fi
echo "ui-smoke: 全部通过"
