#!/usr/bin/env bash
# builtin↔mod 对拍：同一份状态、同一矩形，逐格比较字符格。
# 用法：
#   scripts/mods-parity.sh [区域]      区域默认 status；builtin 区域 vs 该区域的模块实现
#   scripts/mods-parity.sh --negative  负对照：builtin 侧栏 vs builtin Tab 栏（必须非 0）
#
# 两侧的字符网格由 `cargo test --lib m9_parity_dump` 转储到 target/m9/parity/。
# 比较只看字符格：nsh 的 TUI 画布按终端主题着色，颜色不是对拍对象。
set -uo pipefail
cd "$(dirname "$0")/.."

REGIONS="sidebar tabs status picker dialogs settings help toast"
NEGATIVE=0
REGION=status
for arg in "$@"; do
  case "$arg" in
    --negative) NEGATIVE=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) REGION=$arg ;;
  esac
done

if [ "$NEGATIVE" = 1 ]; then
  LEFT=builtin-sidebar
  RIGHT=builtin-tabs
else
  case " $REGIONS " in
    *" $REGION "*) ;;
    *) echo "unknown region: $REGION (one of: $REGIONS)" >&2; exit 2 ;;
  esac
  LEFT="builtin-$REGION"
  RIGHT="mod-$REGION"
fi

HC_TUI_GRAPHICS=kitty cargo test -p hc-tui --lib m9_parity_dump -- --ignored --nocapture \
  > target/m9/parity-run.log 2>&1
if [ ! -s "target/m9/parity/$LEFT.txt" ]; then
  echo "转储失败，见 target/m9/parity-run.log" >&2
  tail -20 target/m9/parity-run.log >&2
  exit 1
fi

/usr/bin/python3 - "$LEFT" "$RIGHT" "$NEGATIVE" <<'PY'
import sys

left_name, right_name, negative = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
left = open(f"target/m9/parity/{left_name}.txt").read().split("\n")
right = open(f"target/m9/parity/{right_name}.txt").read().split("\n")
if left and left[-1] == "":
    left.pop()
if right and right[-1] == "":
    right.pop()
rows = max(len(left), len(right))
cols = max([len(line) for line in left + right] + [0])
total = rows * cols
diff = 0
map_rows = []
for index in range(rows):
    a = left[index] if index < len(left) else ""
    b = right[index] if index < len(right) else ""
    line = []
    for column in range(cols):
        ac = a[column] if column < len(a) else " "
        bc = b[column] if column < len(b) else " "
        if ac == bc:
            line.append(".")
        else:
            diff += 1
            line.append("#")
    map_rows.append("".join(line))

print(f"left={left_name} right={right_name} size={cols}x{rows} total={total} diff={diff}")
if diff:
    print("--- diff map (# = different) ---")
    for line in map_rows:
        print(line)
if negative:
    if diff == 0:
        print("NEGATIVE FAILED: builtin vs builtin 差异为 0，脚本恒真")
        sys.exit(1)
    print(f"NEGATIVE OK diff={diff}")
else:
    if diff:
        print(f"PARITY GAP diff={diff} cells of {total}")
        sys.exit(1)
    print("PARITY OK")
PY
