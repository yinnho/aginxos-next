#!/bin/sh
# #68 刀1：晨报 codex 化——busybox crond 08:00 拉起 codex（晨报官
# AGENTS.md 工作目录，三段式：机身/业务/外面）。交付=微信端直推
# （channels weixin，bind_agent=codex）；card 信封 2026-10-09 退役。
# 产物核验=workflow 自归档 output/晨报-*.src.md 本回合内新鲜落盘。
# 10-09 晚：brain 流超时会整回合烧掉不落 -o——加一次重火（重火即活）。
# v0.1.0 (#472 运维件归包)：本件随 morning-report 包入树，cron 线随包
# [cron] 块落位（face /var/bin/morning-report），手拷的
# /usr/libexec/aginx/morning-codex.sh + 手写 crontab 行退役。
export HOME=/home
export PATH=/sbin:/bin:/usr/sbin:/usr/bin:/var/bin
export TZ=CST-8
WS=/home/workflows/morning-report
TS=$(date +%Y%m%d-%H%M%S)
LOG="$WS/logs/codex-$TS.log"
LAST=/tmp/morning-last-$TS.txt

cd "$WS" || exit 1
rc=1
attempt=1
while [ "$attempt" -le 2 ]; do
  {
    echo "=== morning-codex $TS attempt $attempt (pid $$) ==="
    timeout 1800 codex exec --skip-git-repo-check \
      -o "$LAST" "给我今天的晨报"
    echo "=== codex rc=$? ==="
  } >> "$LOG" 2>&1
  rc=$(tail -1 "$LOG" | sed -n 's/=== codex rc=\([0-9]*\) ===/\1/p')
  [ -n "$rc" ] || rc=1
  [ -s "$LAST" ] && rc=0
  [ "$rc" = "0" ] && break
  echo "attempt $attempt failed rc=$rc, refire after 120s" >> "$LOG"
  rm -f "$LAST"
  attempt=$((attempt+1))
  sleep 120
done
if [ "$rc" != "0" ] || [ ! -s "$LAST" ]; then
  echo "morning-codex failed rc=$rc after $((attempt-1)) attempts ($LOG)" >&2
  rm -f "$LAST"
  exit 1
fi

# 产物核验：自归档 .src.md 须在本回合 40 分钟窗内新鲜落盘——rc=0 也要
# 见文件，不听汇报。
if ! find "$WS/output" -name "晨报-*.src.md" -mmin -40 2>/dev/null | grep -q .; then
  echo "morning-codex: 归档 .src.md 未落盘（rc=0 但 output/ 无新件）($LOG)" >&2
  rm -f "$LAST"
  exit 1
fi
rm -f "$LAST"

# 微信直推（best-effort，不碰归档真源）：uid 从频道绑定态取，不硬编码。
# 会话没绑=记 SKIP 照常收工——报告在 output/，人可自救；发了要留痕，
# self-patrol 按日志尾巴抓 FAILED/SKIPPED。
WUID=$(aginx-channels status weixin 2>/dev/null \
  | sed -n 's/.*"user_id": "\([^"]*\)".*/\1/p' | head -1)
SRC=$(ls -t "$WS"/output/晨报-*.src.md | head -1)
if [ -z "$WUID" ]; then
  echo "weixin push SKIPPED (no bound session) — report archived: $SRC" >> "$LOG"
elif aginx-channels send weixin "$WUID" "$(cat "$SRC")" >>"$LOG" 2>&1; then
  echo "weixin push ok -> $WUID ($(wc -c <"$SRC") bytes)" >> "$LOG"
else
  echo "weixin push FAILED — report archived but NOT delivered: $SRC" >> "$LOG"
fi
echo "morning-report done ($(date '+%F %T') 归档已核)"
