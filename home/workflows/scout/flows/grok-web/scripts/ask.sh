#!/bin/sh
# ask.sh — grok×aginxbrowser 检索异步派工（斥候 flow 经 shell_exec 调，立刻返回）。
# 用法: ask.sh '<要查的任务一句话>'
# 铁律：由 flow 的 shell_allow 放行（sh *flows/grok-web/scripts/*），
# 调用形状保持单命令无元字符。
set -eu

Q="${1:-}"
[ -n "$Q" ] || { echo "usage: ask.sh '<任务>'"; exit 2; }
[ -x /var/bin/grok ] || { echo "grok 未装：先 aginx-pkg opt-in grok"; exit 3; }

WS=/home/workflows/scout
mkdir -p /run/grok-web "$WS/output"
NOW=$(date +%Y%m%d-%H%M%S)
OUT="$WS/output/$NOW.md"
HERE=$(dirname "$0")

# setsid = 新会话：turn 结束/超时杀进程组时后台腿不死（relay turn gate
# 110s 撑不下 grok 的 1–4 分钟，必须异步派工、回头取果）。
if command -v setsid >/dev/null 2>&1; then
  setsid sh "$HERE/run.sh" "$Q" "$OUT" >"/run/grok-web/$NOW.log" 2>&1 &
else
  sh "$HERE/run.sh" "$Q" "$OUT" >"/run/grok-web/$NOW.log" 2>&1 &
fi

echo "已派工：grok 取材中（约 1–4 分钟）。结果落 $OUT——完成不通知，回来问时读最新结果文件交原文。"
