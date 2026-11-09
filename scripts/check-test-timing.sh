#!/usr/bin/env bash
# 时序写法的提示性检查：固定帧数当时间、`sleep` 后断言、依赖 mtime 先后。
#
# 这三类写法在慢 runner（尤其 macOS CI）上会把「几帧」「几毫秒」和真实进度
# 错开，随机变红。脚本只列出疑似位置，**不作为门禁**：退出码恒为 0。
#
#   scripts/check-test-timing.sh              # 默认查 tests/ 与 src/app
#   scripts/check-test-timing.sh tests/       # 只查给定路径

set -u

roots=("$@")
if [ ${#roots[@]} -eq 0 ]; then
    roots=(tests src/app)
fi

files=()
while IFS= read -r file; do
    files+=("$file")
done < <(find "${roots[@]}" -name '*.rs' -type f 2>/dev/null | sort)

if [ ${#files[@]} -eq 0 ]; then
    echo "check-test-timing: 给定路径下没有 .rs 文件：${roots[*]}"
    exit 0
fi

# 三个独立的近似匹配，命中就打印一行；每一行都能直接跳转去看。
awk '
    FNR == 1 { window = 0 }

    # 一、sleep 之后几行里出现同一缩进的断言：等待与判定被拆成了两件事。
    /sleep[[:space:]]*\(/ {
        sleep_at = FNR
        window = 2
        next
    }
    window > 0 {
        if (/assert/) {
            printf "%s:%d: sleep(...) 之后 %d 行内出现断言\n", FILENAME, sleep_at, FNR - sleep_at
        }
        window -= 1
    }
' "${files[@]}"

awk '
    # 二、固定帧数当时间轴：pump(..., 数字) 之后靠帧数推进到断言。
    /pump\([^)]*,[[:space:]]*[0-9]+[[:space:]]*\)/ {
        printf "%s:%d: 用固定帧数当时间轴\n", FILENAME, FNR
    }
' "${files[@]}"

awk '
    # 三、靠文件 mtime 先后判定顺序，除非显式设置了时间戳（set_modified/FileTime）。
    /modified\(\)/ && !/set_modified|FileTime/ {
        printf "%s:%d: 依赖 mtime 先后\n", FILENAME, FNR
    }
' "${files[@]}"

echo "check-test-timing: 扫过 ${#files[@]} 个文件；命中项只作提示，不阻塞构建"
exit 0
