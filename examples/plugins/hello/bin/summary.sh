#!/bin/sh
# 打印宿主传入的上下文，供 M6 冒烟与文档示例使用。
cat
if [ "${HCTUI_CONTEXT_ID:-}" != "" ]; then
  printf '\nrequest=%s package=%s\n' "$HCTUI_CONTEXT_ID" "$HCTUI_PLUGIN_ID"
fi
