#!/bin/sh
# 日循环腿 wrapper（docs/日循环.md，aginx-mem v0.2.0 随包）：
# 23:47 避整点跑三段式——
#   Leg A  aginx-mem day        材料化（只读；空跑日幂等收工不烧引擎）
#   Leg B  codex exec 900s      分析 JSON（prompt=day --prompt 现组：
#                                材料清单+已知索引+lifecycle 契约原核）
#   Leg C  aginx-mem digest     落账 knowledge/+版本+索引+缺口件
#                                （坏 JSON 拒收标 degraded，也要出件）
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
exit "$rc"
