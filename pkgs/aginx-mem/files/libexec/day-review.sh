#!/bin/sh
# 日循环腿 wrapper（docs/日循环.md，aginx-mem v0.2.0 随包）：
# 23:47 避整点跑三段式——
#   Leg A  aginx-mem day        材料化（只读；空跑日幂等收工不烧引擎）
#   Leg B  codex exec 900s      分析 JSON（prompt=day --prompt 现组：
#                                材料清单+已知索引+lifecycle 契约原核）
#   Leg C  aginx-mem digest     落账 knowledge/+版本+索引+缺口件
#                                （坏 JSON 拒收标 degraded，也要出件）
# v0.2.2：+高优直推腿——digest degraded（safety 类）即时推微信，
# 策略真源=/home/policies/reporting.md（wrapper 只机械执行分级）。
# 纪律：day 材料/分析/缺口件绝不出设备；本脚本只在本机转。
export HOME=/home
export PATH=/sbin:/bin:/usr/sbin:/usr/bin:/var/bin
export TZ=CST-8
WS=/home
TS=$(date +%Y%m%d-%H%M%S)
LOG=/var/log/day-review.log
MAT=/tmp/day-materials-$TS.json
OUT=/tmp/day-analysis-$TS.txt

echo "=== day-review $TS (pid $$) ===" >> "$LOG"
cd "$WS" || exit 1

# Leg A：材料化。activity=false → 空跑日（无会话无晨报无研究轮）直接收工。
if ! /var/bin/aginx-mem day > "$MAT" 2>>"$LOG"; then
  echo "--- day rc=$? 材料化失败，本日不整理" >> "$LOG"
  rm -f "$MAT"
  exit 1
fi
if grep -q '"activity": *false' "$MAT"; then
  echo "--- 空跑日（无活动）幂等收工 $(date '+%F %T')" >> "$LOG"
  rm -f "$MAT"
  exit 0
fi

# Leg B：codex 一回合（900s 预算；超时/失败=走 degraded 路）。
PROMPT=$(/var/bin/aginx-mem day --prompt 2>>"$LOG")
timeout 900 codex exec --skip-git-repo-check -o "$OUT" "$PROMPT" >> "$LOG" 2>&1
echo "=== codex rc=$? ===" >> "$LOG"
[ -s "$OUT" ] || echo "codex-no-output（超时或空回合）" > "$OUT"

# Leg C：落账。rc=1=degraded（缺口件已带标记），rc=0=落账/空跑皆正常。
/var/bin/aginx-mem digest --workspace "$WS" < "$OUT" >> "$LOG" 2>&1
rc=$?
rm -f "$MAT" "$OUT"
if [ "$rc" = "0" ]; then
  echo "--- digest ok ($(date '+%F %T'))" >> "$LOG"
else
  echo "--- digest degraded rc=$rc（引擎坏，等人看） ($(date '+%F %T'))" >> "$LOG"
fi

# 高优直推（v0.2.2，汇报策略地板）：只 safety 类——digest degraded
# 就是这一类的现役形状；其余一切攒晨报（08:00 微信直达）。策略行
# `  - safety` 被摘除即静默停用（策略文件是行为真源）。会话没绑=
# SKIP 照常收工；best-effort，不碰落账真源（morning-report 同 idiom）。
if [ "$rc" != "0" ] && grep -q '^  - safety' /home/policies/reporting.md 2>/dev/null; then
  WUID=$(aginx-channels status weixin 2>/dev/null \
    | sed -n 's/.*"user_id": "\([^"]*\)".*/\1/p' | head -1)
  if [ -z "$WUID" ]; then
    echo "--- weixin push SKIPPED (no bound session)" >> "$LOG"
  elif aginx-channels send weixin "$WUID" \
    "[日循环告警·safety] $(date '+%F %T') digest degraded rc=$rc——引擎坏等人看，详 /var/log/day-review.log（其余一切照常攒晨报）" >> "$LOG" 2>&1; then
    echo "--- weixin push ok (safety alert)" >> "$LOG"
  else
    echo "--- weixin push FAILED (safety alert)" >> "$LOG"
  fi
fi
exit "$rc"
