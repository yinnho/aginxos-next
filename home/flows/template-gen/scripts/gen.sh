#!/bin/sh
# gen.sh — 模板派工入口（母体 flow 经 shell_exec 调，立刻返回；活在后头）。
# 用法: gen.sh <模板id> <一句话风格/用途描述>
# 铁律：本文件由 flow 的 shell_allow 放行（sh *flows/template-gen/scripts/*），
# 调用形状必须保持单命令无元字符。
set -eu

ID="${1:-}"
DESC="${2:-}"
[ -n "$ID" ] || { echo "usage: gen.sh <模板id> <描述>"; exit 2; }
[ -n "$DESC" ] || { echo "描述不能为空"; exit 2; }
case "$ID" in
  *[!a-z0-9-]*|'') echo "模板 id 只能小写英文/数字/连字符：$ID"; exit 2 ;;
esac
[ -x /var/bin/codex ] || { echo "codex 未装：先 aginx-pkg opt-in codex"; exit 3; }

mkdir -p /run/template-gen /home/cards
NOW=$(date +%Y%m%d-%H%M%S)
LOG="/run/template-gen/$ID-$NOW.log"
HERE=$(dirname "$0")

# setsid = 新会话：turn 结束/超时杀进程组时后台腿不死（stdin/out 全落日志）。
if command -v setsid >/dev/null 2>&1; then
  setsid sh "$HERE/run.sh" "$ID" "$DESC" >"$LOG" 2>&1 &
else
  sh "$HERE/run.sh" "$ID" "$DESC" >"$LOG" 2>&1 &
fi

echo "已派工：codex 生成模板 $ID（后台约5分钟）。日志 $LOG。完成出新卡。"
