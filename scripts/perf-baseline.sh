#!/usr/bin/env bash
# 性能基线：同一台机器、同一套隔离环境测量二进制体积、常驻内存、空闲 CPU 与
# 按键到屏幕可见的延迟，供里程碑之间对比。脚本不写用户目录，也不碰默认 session。
#
# 用法：scripts/perf-baseline.sh <label> <path-to-hc-tui> [samples]
set -uo pipefail
LABEL=${1:?usage: perf-baseline.sh <label> <binary> [samples]}
BIN=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
SAMPLES=${3:-20}
# 隔离 XDG 之后 mise 的 python3 shim 读不到配置：一开始就解析成真实路径。
PYTHON=/usr/bin/python3
[ -x "$PYTHON" ] || PYTHON=$(command -v python3)
ROOT=/tmp/hc-tui-perf-$LABEL
SOCK="hctui-perf-$LABEL-$$"
SESSION="hctui-perf-$LABEL-$$"

rm -rf "$ROOT"
mkdir -p "$ROOT/run" "$ROOT/state" "$ROOT/config/hc-tui" "$ROOT/cache"
: > "$ROOT/config/hc-tui/config.toml"
export HC_SESSION="$SESSION"
export XDG_RUNTIME_DIR="$ROOT/run" XDG_STATE_HOME="$ROOT/state"
export XDG_CONFIG_HOME="$ROOT/config" XDG_CACHE_HOME="$ROOT/cache"

cleanup() {
  tmux -L "$SOCK" send-keys -t tui C-b q 2>/dev/null || true
  sleep 0.3
  tmux -L "$SOCK" kill-server 2>/dev/null || true
  (
    export HC_SESSION="$SESSION"
    hc server stop >/dev/null 2>&1 || true
  )
}
trap cleanup EXIT

size=$(stat -c%s "$BIN")
tmux -L "$SOCK" new-session -d -s tui -x 110 -y 30 "cd /tmp && TERM=xterm-256color $BIN"
for _ in $(seq 1 100); do
  tmux -L "$SOCK" capture-pane -p -t tui 2>/dev/null | grep -q "Spaces" && break
  sleep 0.05
done
sleep 2

# tmux 直接执行二进制时 pane_pid 就是 hc-tui；它的子进程是 `hc connect`。
tui_pid=$(tmux -L "$SOCK" list-panes -F '#{pane_pid}' -t tui | head -1)
if [ "$(cat "/proc/$tui_pid/comm" 2>/dev/null)" != "hc-tui" ]; then
  tui_pid=$(pgrep -P "$tui_pid" -x hc-tui | head -1)
fi
[ -n "$tui_pid" ] || { echo "cannot find the TUI process"; exit 1; }

rss_kb=$(awk '/VmRSS/{print $2}' "/proc/$tui_pid/status")
ticks=$(getconf CLK_TCK)
cpu_start=$(awk '{print $14+$15}' "/proc/$tui_pid/stat")
sleep 10
cpu_end=$(awk '{print $14+$15}' "/proc/$tui_pid/stat")
idle_pct=$(awk -v d="$((cpu_end - cpu_start))" -v h="$ticks" 'BEGIN{printf "%.3f", d*100/(h*10)}')

# 延迟：把 pane 输出直接接进文件，按键后轮询文件出现回显的时间。
# 去掉 ANSI 序列后匹配 token，不含 tmux capture 的进程启动开销。
tmux -L "$SOCK" pipe-pane -t tui -O "cat >> $ROOT/out.log"
sleep 0.5
read -r p50 p90 <<EOF
$("$PYTHON" - "$SOCK" "$ROOT/out.log" "$SAMPLES" <<'PY'
import re, subprocess, sys, time

sock, log, samples = sys.argv[1], sys.argv[2], int(sys.argv[3])
ansi = re.compile(
    rb"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Za-z0-9]|\x1b[=>]"
)
latencies = []
for index in range(1, samples + 1):
    token = f"lat{index}zq".encode()
    open(log, "wb").close()
    started = time.perf_counter()
    subprocess.run(
        ["tmux", "-L", sock, "send-keys", "-t", "tui", "-l", token.decode()],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = started + 2
    while time.perf_counter() < deadline:
        try:
            with open(log, "rb") as handle:
                data = handle.read()
        except FileNotFoundError:
            data = b""
        if token in ansi.sub(b"", data):
            break
    latencies.append((time.perf_counter() - started) * 1e6)
    subprocess.run(
        ["tmux", "-L", sock, "send-keys", "-t", "tui", "C-u"],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
latencies.sort()
print(latencies[len(latencies) // 2], latencies[int(len(latencies) * 0.9) - 1])
PY
)
EOF
p50=${p50%.*}
p90=${p90%.*}

printf 'label=%s size_bytes=%s rss_kb=%s idle_cpu_pct=%s input_us_p50=%s input_us_p90=%s samples=%s\n' \
  "$LABEL" "$size" "$rss_kb" "$idle_pct" "$p50" "$p90" "$SAMPLES"
