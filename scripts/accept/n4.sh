#!/usr/bin/env bash
# n4 acceptance — bake 接管整机切换（N4：新仓出的镜像在役，裸命令 aginx
# 即母体门面；一代线归档为资产库）。
#
# 镜像 = scripts/build-rootfs.sh 产物（配方 rootfs/ + 新仓 zigbuild 十件 +
# 老仓资产改名落位）：引擎住 /usr/libexec/aginx/（D13 不进命令宇宙），
# /usr/bin = aginx 裸名 + aginx-* 面集，无 ag、无 ag-*、无 carrier、无
# relay（切净）。控制面 /usr/bin/aginx-svc；重启 /usr/bin/aginx-reboot。
#
#   预检  serial 钉死 / 版本戳期望值 env 传入 / brain key 键名在
#   D 首启  boot.state 六行全绿 / 恰五单元 / 引擎在 libexec / 面回归
#   E 切净  无老单元 / 无 /var/bin/aginx / 无 ag 无 ag-* / 老遗留清
#   F 收获  裸 aginx 真回复 / 工具回路 / 语音两路（封闭+自由）
#   G 二启  aginx-reboot → 自起 → 钟闸门 → send/face 再通
#
# 纪律同 n3：钉死 serial；只读为主（/tmp 标记 + face 快照还原）；
# N4_STAMP 必须显式传入（= 烤机时新仓 git 戳，防拿 HEAD 冒充镜像）：
#   N4_STAMP="$(git -C ~/Documents/aginxos-next log -1 --format='aginxos %h %cd' --date=short)" \
#     ./scripts/accept/n4.sh
set -euo pipefail

SERIAL="${ADB_SERIAL:-aginxosredfin}"
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="${N4_STAMP:-}"

SOCK=/run/aginx.sock
FACE=/run/voice/face
FACEBAK=/tmp/n4-face.bak
KEYVAR=AGINXBRAIN_API_KEY

PASS=0
FAIL=0
REBOOTED=0

adbx() { adb -s "$SERIAL" "$@"; }

drv() {
  local raw
  raw="$(adbx shell "export HOME=/home PATH=/usr/bin:/bin:/sbin:/var/bin; $1; echo __RC=\$?" 2>&1 || true)"
  raw="${raw//$'\r'/}"
  DRV_RC="$(printf '%s\n' "$raw" | sed -n 's/^__RC=//p' | tail -1)"
  DRV_OUT="$(printf '%s\n' "$raw" | grep -vE '^(libc:|linker|WARNING)' | grep -v '^__RC=' || true)"
}

expect_rc()  { [ "${DRV_RC:-}" = "0" ] && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1 (rc=${DRV_RC:-?})"; FAIL=$((FAIL+1)); } }
expect_out() { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } }
expect_no()  { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "FAIL - $1（不该出现: $2）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } || { echo "ok   - $1"; PASS=$((PASS+1)); } }

wait_ready() { # name label tries
  local i
  for i in $(seq 1 "${3:-10}"); do
    drv "/usr/bin/aginx-svc status $1"
    printf '%s' "${DRV_OUT:-}" | grep -q "ready" && { echo "ok   - $2（ready）"; PASS=$((PASS+1)); return 0; }
    sleep 1
  done
  echo "FAIL - $2（未 ready）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -3)"; FAIL=$((FAIL+1)); return 1
}

cleanup() {
  # 终态 = N4 在役：不动镜像件。重启前面还原；重启后 face 归 aginx-voice。
  if [ "$REBOOTED" = "0" ]; then
    drv "cp $FACEBAK $FACE 2>/dev/null; true"
  fi
  drv "rm -f $FACEBAK; true"
}
trap cleanup EXIT

echo "==> 预检（serial 钉死 / N4_STAMP 在 / key 键名在）"
adbx get-state >/dev/null 2>&1 || { echo "n4: device $SERIAL 不在线"; exit 1; }
[ -n "$STAMP" ] || { echo "n4: N4_STAMP 未传（防 HEAD 冒充镜像，见文件头）"; exit 1; }
drv "cat /etc/aginx-version"
expect_out "设备版本戳 = 烤机戳（$STAMP）" "^${STAMP}$"
drv "grep -c \"^$KEYVAR=\" /etc/aginx/env"
expect_out "brain key 键名在 /etc/aginx/env（只数行不回显）" '^1$'

echo "==> D 首启（boot.state 六行 → 恰五单元 → 引擎落位 → provision 回面）"
# done ok：net-bringup 收口（含有界 ntpd 钟闸）。冷首启 Wi-Fi+校时走完
# 才轮到 provision，给足预算。
for i in $(seq 1 24); do
  drv "grep -q '^done ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && break; sleep 15
done
for k in wifi dhcp internet time done; do
  drv "grep -q '^$k ok' /run/boot.state"
  expect_rc "boot.state: $k ok"
done
# pkg ok：provision 重同步（mirror 拉 7 core 件，分钟级）。
for i in $(seq 1 40); do
  drv "grep -q '^pkg ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && break; sleep 15
done
drv "grep -q '^pkg ok' /run/boot.state"
expect_rc "boot.state: pkg ok（provision 重同步完成）"
wait_ready aginx-server  "单元 aginx-server"  15
wait_ready aginx-voice   "单元 aginx-voice"   15
wait_ready aginxbrowser  "单元 aginxbrowser"  15
wait_ready aginx-secretd "单元 aginx-secretd" 15
wait_ready net-watch      "单元 net-watch"     15
drv "/usr/bin/aginx-svc list | grep -c ready"
expect_out "恰五单元 ready（无多余）" '^5$'
drv "ls -l $SOCK"
expect_rc "server 起在默认 $SOCK"
drv "pgrep -a aginx-server"
expect_out "引擎住 libexec（不进命令宇宙，D13）" "/usr/libexec/aginx/aginx-server"
drv "pgrep -a aginx-voice"
expect_out "voiced 即 /usr/bin/aginx-voice" "/usr/bin/aginx-voice"
drv "ls /usr/libexec/aginx/aginx-runtime /usr/libexec/aginx/aginx-svcd /usr/libexec/aginx/aginx-secretd"
expect_rc "libexec 四件齐（runtime/svcd/secretd）"
drv "ls /var/bin/aginxbrowser /var/bin/python3 /var/bin/codex /var/bin/dup /var/bin/agb /var/bin/agf /var/bin/agmem"
expect_rc "provision 回面：7 core 件全在（codex 含）"
drv "ls /var/models/tts/vits-melo-tts-zh_en/model.onnx /var/models/asr/model.int8.onnx /var/models/ocr/rec.onnx"
expect_rc "语音/OCR 模型入盘（不走红毯 provision）"
drv "ls /var/lib/agpkg/stamps/aginxbrowser /var/lib/agpkg/stamps/python3 /var/lib/agpkg/stamps/agb"
expect_rc "stamps 记账在"

echo "==> E 切净（无老线：单元/relay 面/ag 路由器/ag-* 壳/老遗留）"
drv "/usr/bin/aginx-svc list"
expect_no  "无老 relay 单元（^aginx 裸名）"  '^aginx[[:space:]]'
expect_no  "无 carrier 单元"                 "aginx-carrier"
drv "test ! -e /var/bin/aginx"
expect_rc  "/var/bin/aginx 不存在（sync 后仍不存在）"
drv "ls /usr/bin | grep -c '^ag-\|^ag\$'"
expect_out "/usr/bin 无 ag 无 ag-*（D13 不留过渡别名）" '^0$'
drv "test ! -e /var/lib/agpkg/stamps/aginx && test ! -e /var/lib/agpkg/stamps/aginx-carrier"
expect_rc  "老包 stamps 不在（切净不是遮住）"
drv "test ! -e /home/.aginx-n"
expect_rc  "N 并行试验 HOME（.aginx-n）已清"
drv "test ! -e /home/.aginx/agents/binding.json && test ! -e /home/.aginx/carrier && test ! -e /home/.aginx/sessions.json"
expect_rc  "/home/.aginx 老遗留（relay 身份件）已清"
drv "ls /home/photos/*.jpg 2>/dev/null | wc -l"
expect_out "/home/photos 完好（照片还在）" '^[1-9]'

echo "==> F 收获（裸命令即母体；工具回路；语音两路）"
drv "aginx agent send 用一句话介绍AginxOS"
expect_rc  "裸 aginx agent send rc=0"
expect_out "母体真回复（含 AginxOS）" "AginxOS"
drv "aginx agent send \"用 sys-status 查一下电池还剩多少，把电量数字告诉我\""
expect_rc  "send rc=0"
expect_out "工具真回路（回复含电池读数）" "电|%"
drv "aginx commands"
expect_out "命令宇宙见 sys-status" "sys-status"
expect_no  "引擎不混进命令宇宙"     "aginx-server|aginx-runtime"
drv "aginx commands --json"
expect_rc  "commands --json（tools 查询面）rc=0"
drv "cp $FACE $FACEBAK 2>/dev/null; ls $FACEBAK"
expect_rc  "face 快照（收据后还原）"
drv "/usr/bin/aginx-voice --inject 你好; sleep 1"
expect_rc  "封闭词表：inject 你好 rc=0"
drv "/usr/bin/aginx-voice --face"
expect_out "封闭词表仍本地直答（我在）" "我在"
drv "/usr/bin/aginx-voice --inject 用一句话介绍AginxOS; sleep 3"
expect_rc  "自由文本 inject rc=0"
drv "/usr/bin/aginx-voice --face"
expect_no  "不是兜底话"    "连不上母体"
expect_out "脸上是母体真回复" "AginxOS"
drv "cp $FACEBAK $FACE"
expect_rc  "face 还原"

echo "==> G 二启（aginx-reboot；新线自己的重启面）"
REBOOTED=1
drv "/usr/bin/aginx-reboot reboot" || true
echo "    等设备回来…"
for i in $(seq 1 60); do adbx get-state >/dev/null 2>&1 && break; sleep 5; done
adbx get-state >/dev/null 2>&1 || { echo "FAIL - 设备未回来"; FAIL=$((FAIL+1)); }
up=0
for i in $(seq 1 36); do
  drv "/usr/bin/aginx-svc list" && [ "${DRV_RC:-}" = "0" ] && { up=1; break; }
  sleep 5
done
[ "$up" = "1" ] || { echo "FAIL - aginx-svcd 未就绪"; FAIL=$((FAIL+1)); }
# 网络自愈 + 钟闸门：net-bringup 现在是有界 ntpd 重试（12×10s，N4 真修
# #112），正常应自愈；保留补枪兜底断言——补不回来才是病。
for i in $(seq 1 18); do
  drv "ip -4 addr show wlan0 | grep -q inet"
  [ "${DRV_RC:-}" = "0" ] && break
  sleep 5
done
clocked=0
for i in $(seq 1 24); do
  drv "[ \$(date +%Y) -ge 2026 ]"
  [ "${DRV_RC:-}" = "0" ] && { clocked=1; break; }
  sleep 5
done
if [ "$clocked" != "1" ]; then
  drv "ntpd -q -n -p ntp.aliyun.com 2>/dev/null; [ \$(date +%Y) -ge 2026 ]"
  expect_rc "钟闸门补枪后到 2026（有界重试未覆盖的缺口）"
else
  echo "ok   - 钟闸门（有界 ntpd 自愈到位）"; PASS=$((PASS+1))
fi
drv "/usr/bin/aginx-svc list"
expect_out "aginx-server 二启自起"   "aginx-server.*ready"
expect_out "aginx-voice 二启自起"    "aginx-voice.*ready"
expect_out "aginxbrowser 二启自起"   "aginxbrowser.*ready"
expect_out "aginx-secretd 二启自起"  "aginx-secretd.*ready"
expect_out "net-watch 二启自起"      "net-watch.*ready"
drv "ls -l $SOCK"
expect_rc "socket 回来了"
drv "aginx agent send 现在几点了"
expect_rc  "二启后母体仍应答（rc=0）"
expect_out "二启后母体真回复（非空有字）" "[一-龥]"
drv "/usr/bin/aginx-voice --inject 你好; sleep 1; /usr/bin/aginx-voice --face"
expect_out "二启后语音地板仍在（我在）" "我在"

echo
echo "n4: $PASS passed, $FAIL failed（终态：N4 在役，一代线归档）"
[ "$FAIL" = 0 ]
