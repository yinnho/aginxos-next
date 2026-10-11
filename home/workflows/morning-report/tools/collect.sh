#!/bin/sh
# 晨报取数器：机身 + 研究四线 24h 摘要（昨日 08:00 → 现在）。
# busybox 雷区：禁 awk；字符串比较用 test \> 。
# 10-09 晚：预算阀空转火单列（rc=0 不等于落库）；库存面走引擎 status。
# 10-09 深夜：空转阀⚠改当日口径（阀=daily，旧日的击打不该在滚动窗里
# 永远告警）；24h 计数仍喂 落库=rc0-空转 的产量算术（阀退出不是落库，
# #465 假账教训）。
export TZ=CST-8
WIN="$(date -d @$(( $(date +%s) - 86400 )) +%F) 08:00"
TODAY="$(date +%F)"

echo "# 机身 $(date '+%F %T')"
uptime
free -m | sed -n '1,2p'
df -h / | sed -n '2p'
echo "备份日志尾: $(tail -1 /var/log/aginx-backup.log 2>/dev/null)"
echo

echo "# 守夜（无人盯的账，晨报自查段的料）"
svc=$(/usr/bin/aginx-svc list 2>/dev/null)
echo "单元: $(echo "$svc" | grep -c 'ready')/$(echo "$svc" | grep -c '.') ready"
echo "$svc" | grep -v 'ready' | grep '.' | cut -f1,2 | sed 's/^/  ⚠ /'
b=/sys/class/power_supply/battery
if [ -r "$b/capacity" ]; then
  echo "电量: $(cat $b/capacity)% $(cat $b/status) 温度=$(($(cat $b/temp 2>/dev/null || echo 0)/10))°C"
else
  echo "电量: 无电池面（服务器板）"
fi
wl=$(ls /run/aginx-warn/ 2>/dev/null | grep .)
if [ -n "$wl" ]; then echo "$wl" | sed 's/^/  ⚠ 警告: /'; else echo "警告注册表: 清"; fi
echo "重启次数(全期 start 行): $(grep -c 'net-watch start' /var/log/net-watch.log 2>/dev/null)"
echo "守夜日志尾(电量/告警/重启):"
grep -E 'battery:|alert|net-watch start' /var/log/net-watch.log 2>/dev/null | tail -8 | sed 's/^/  /'
echo "日循环(23:47)尾: $(tail -1 /var/log/day-review.log 2>/dev/null)"
echo "知识库最新件: $(ls -lt /home/knowledge 2>/dev/null | sed -n 2p | sed 's/^[-dl][^ ]* *[0-9]* [^ ]* [^ ]* [0-9]* //')"
echo

for n in duanju taishigong qujiu baize; do
  f=/home/research/logs/$n.log
  [ -f "$f" ] || continue
  rounds=0; rc0=0; rcbad=0; skipped=0; vtoday=0; lastts="-"; lastbad=""; badcause=""; titles=""; inwin=0
  while IFS= read -r line; do
    case "$line" in
      "=== 20"*)
        ts=$(echo "$line" | cut -c5-20)
        if [ "$ts" \> "$WIN" ] || [ "$ts" = "$WIN" ]; then
          inwin=1; rounds=$((rounds+1)); lastts=$ts
        else
          inwin=0
        fi
        ;;
      "--- rc="*)
        if [ "$inwin" = "1" ]; then
          case "$line" in
            "--- rc=0") rc0=$((rc0+1)) ;;
            *) rcbad=$((rcbad+1)); lastbad="$lastts $line" ;;
          esac
        fi
        ;;
      "aginxresearch: "*)
        # 失败原因行（搜索零结果/grok 死等）——随账进晨报，不再「原因未记录」
        [ "$inwin" = "1" ] && badcause="$line"
        ;;
      *预算阀*)
        if [ "$inwin" = "1" ]; then
          skipped=$((skipped+1))
          case "$ts" in "$TODAY "*) vtoday=$((vtoday+1)) ;; esac
        fi
        ;;
      "[1] 题"*)
        [ "$inwin" = "1" ] && titles="$titles
$line"
        ;;
    esac
  done < "$f"
  prod=$((rc0-skipped))
  [ $prod -lt 0 ] && prod=0
  echo "## $n 轮数=$rounds 落库=$prod 空转阀=$vtoday 失败=$rcbad 最后活动=$lastts"
  /var/bin/aginxresearch status /home/research/$n 2>/dev/null | sed -n '2,4p' | sed 's/^/  /'
  w=""
  [ "$vtoday" -gt 0 ] && w="$w 空转阀${vtoday}次"
  [ "$rcbad" -ge 3 ] && w="$w 失败${rcbad}次"
  [ -n "$w" ] && echo "⚠ 异常:$w"

  [ -n "$lastbad" ] && echo "最近失败: $lastbad"
  [ -n "$badcause" ] && echo "失败原因: $(echo "$badcause" | cut -c1-120)"
  echo "窗口内题目（末 6 条）:"
  echo "$titles" | grep . | tail -6 | cut -c1-80
  echo
done
