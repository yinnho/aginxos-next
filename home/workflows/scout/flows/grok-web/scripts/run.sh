#!/bin/sh
# run.sh — 后台腿：真跑 grok，结果+退出码落 output/<ts>.md 供斥候取。
set -eu

Q="${1:?任务}"; OUT="${2:?输出路径}"

# 隧道 env 铁律（#407）：漏 NO_PROXY 则 socks5h 吃回环 127.0.0.1:8089，
# 浏览器 MCP 握手死在隧道里（症候：tool_count=27 基线）。
# 同配方住 providers/grok/grok 壳层（母体裸名 spawn 走那条）。
export HTTPS_PROXY=socks5h://127.0.0.1:8800
export ALL_PROXY=socks5h://127.0.0.1:8800
export NO_PROXY=127.0.0.1,localhost
export no_proxy=127.0.0.1,localhost

RC=0
timeout 280 /var/bin/grok -p "$Q" --always-approve >"$OUT.body" 2>/run/grok-web/last.err || RC=$?

{
  echo "# grok 取材 $(date +%Y-%m-%d_%H:%M:%S)"
  echo
  echo "- 任务：$Q"
  echo "- 退出码：$RC（0=成；124=超 280s 被掐；1=本体错，详情 /run/grok-web/last.err）"
  echo
  echo "---"
  echo
  cat "$OUT.body"
} >"$OUT.tmp"
mv "$OUT.tmp" "$OUT"
rm -f "$OUT.body"
