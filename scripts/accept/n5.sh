#!/usr/bin/env bash
# n5 acceptance — 吸收归并 + 远端通道 + 备份线（N5：六冻结件吸收、
# /var/lib/aginx 状态世界、aginx-gateway 六单元、aginx-backup 本地线）。
#
# 前提：N5 镜像已刷（runbook 见 plans/nifty-wandering-sutton.md 刷机日段），
# 设备侧已灌 AGINX_GATEWAY_ID（/etc/aginx/env）与 relay.primary（sidecar）。
#
#   预检  serial 钉死 / 版本戳=N5_STAMP / brain key 键名 / 网关 id 键名
#   H 迁移  老根三处清 / migrate 日志 done / 七成员在 / stamps 存活 /
#          done check / secret store 0600
#   I 吸收  aginx-update status（slot+新戳+boot-ok=活证）/ aginx-qr 定数
#          QR / aginx-secret set/list/rm 回路 / pkg ok
#   J 备份  now/list/verify 全绿 / crontab 定时行
#   K 网关  六单元 ready / registered 日志 / 8443 ESTABLISHED（/proc/net/tcp，
#          busybox netstat 禁用铁律）
#   L 远端  宿主 agc 真往返（本里程碑首条远端收据）+ 化身负例
#   M 二启  aginx-reboot → 六单元自起 → 钟闸门 → send 仍应答 → 网关重连
#          （ESTABLISHED 复现）→ 语音地板 我在
#
# 纪律同 n4：钉死 serial；只读为主（/tmp 标记 + n5.secret 测试 scope 自清）；
# N5_STAMP 必须显式传入（= 烤机时新仓 git 戳，防拿 HEAD 冒充镜像）：
#   N5_STAMP="$(git -C ~/Documents/aginxos-next log -1 --format='aginxos %h %cd' --date=short)" \
#     ./scripts/accept/n5.sh
# L 段密钥两式任一：AGC_RELAY_SECRET 直接 env，或 AGC_SECRET_FILE 指向 0600
# 文件——两式都不回显。缺密钥 = FAIL（这是收据不是选配）。
set -euo pipefail

SERIAL="${ADB_SERIAL:-aginxosredfin}"
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ECO="$HOME/Documents/aginx"
STAMP="${N5_STAMP:-}"

SOCK=/run/aginx.sock
FACE=/run/aginx-voice/face
FACEBAK=/tmp/n5-face.bak
GWLOG=/var/log/aginx-svc/aginx-gateway.log
KEYVAR=AGINXBRAIN_API_KEY
IDVAR=AGINX_GATEWAY_ID
# 8443 = 0x20FB。busybox netstat 必炸——/proc/net/tcp 里 rem 端口十六进制
# 后紧跟状态列（01=ESTABLISHED），sed/positional 而非 awk。
PORT_HEX=20FB

PASS=0
FAIL=0
REBOOTED=0
N5_SCOPE=n5.acceptance-probe

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

# 8443 出向 ESTABLISHED：等网关拨通（TLS 握手+register 后连接长存）。
wait_8443() { # label
  local i
  for i in $(seq 1 20); do
    drv "grep -q ':${PORT_HEX} 01 ' /proc/net/tcp /proc/net/tcp6 2>/dev/null"
    [ "${DRV_RC:-}" = "0" ] && { echo "ok   - $1（8443 ESTABLISHED）"; PASS=$((PASS+1)); return 0; }
    sleep 3
  done
  echo "FAIL - $1（8443 未 ESTABLISHED）"; FAIL=$((FAIL+1)); return 1
}

cleanup() {
  if [ "$REBOOTED" = "0" ]; then
    drv "cp $FACEBAK $FACE 2>/dev/null; true"
  fi
  drv "rm -f $FACEBAK; true"
  # n5 探针 scope 自清（set 过才需要清）
  drv "printf x | /usr/bin/aginx-secret rm $N5_SCOPE >/dev/null 2>&1; true"
}
trap cleanup EXIT

echo "==> 预检（serial 钉死 / N5_STAMP 在 / key+id 键名在）"
adbx get-state >/dev/null 2>&1 || { echo "n5: device $SERIAL 不在线"; exit 1; }
[ -n "$STAMP" ] || { echo "n5: N5_STAMP 未传（防 HEAD 冒充镜像，见文件头）"; exit 1; }
drv "cat /etc/aginx-version"
expect_out "设备版本戳 = 烤机戳（${STAMP}）" "^${STAMP}$"
drv "grep -c \"^$KEYVAR=\" /etc/aginx/env"
expect_out "brain key 键名在 /etc/aginx/env（只数行不回显）" '^1$'
drv "grep -c \"^$IDVAR=\" /etc/aginx/env"
expect_out "网关 id 键名在 /etc/aginx/env（只数行不回显）" '^1$'

echo "==> H 迁移（/var/lib 归并到 /var/lib/aginx：老根清、七成员在、状态存活）"
drv "test ! -e /var/lib/agpkg && test ! -e /var/lib/ag && test ! -e /var/lib/voiced"
expect_rc  "老根三处不存在（agpkg/ag/voiced）"
drv "grep -q 'varlib-migrate: done' /var/varlib-migrate.log"
expect_rc  "迁移日志有 done 行"
drv "ls /var/lib/aginx/skills /var/lib/aginx/units /var/lib/aginx/stamps /var/lib/aginx/pkgfiles /var/lib/aginx/done /var/lib/aginx/secret /var/lib/aginx/voice"
expect_rc  "七成员齐（busybox tar 缺成员=致命，bake #9 收据）"
drv "ls /var/lib/aginx/stamps/aginxbrowser /var/lib/aginx/stamps/python3"
expect_rc  "stamps 迁移存活（aginxbrowser/python3）"
drv "/usr/bin/aginx-done check python-finalize"
expect_rc  "done 标记存活（python-finalize）"
drv "stat -c '%a' /var/lib/aginx/secret/store"
expect_out "secret store 0600" '^600$'

echo "==> I 吸收件（六冻结件的活证：updater/qr/secret/pkg 四面）"
drv "/usr/bin/aginx-update status"
expect_rc  "aginx-update status rc=0"
expect_out "status 出 slot（A/B 在役）"       'slot _[ab]'
expect_out "status 出新戳（吸收版在役）"       'aginxos [0-9a-f]{7}'
drv "/usr/bin/aginx-boot-ok status"
expect_rc  "aginx-boot-ok status rc=0（updater 不再死路径）"
drv "/usr/bin/aginx-qr /usr/share/aginx/n5-qr.jpg"
expect_rc  "aginx-qr 解码 rc=0"
expect_out "定数 QR 出已知 payload"            'WIFI:T:WPA;S:aginx-n5;P:fixture;;'
drv "printf n5-probe-value | /usr/bin/aginx-secret set $N5_SCOPE"
expect_rc  "secret set（stdin 灌注）rc=0"
drv "/usr/bin/aginx-secret list"
expect_out "secret list 见探针 scope"          "$N5_SCOPE"
drv "printf x | /usr/bin/aginx-secret get $N5_SCOPE"
expect_out "secret get 回读探针值"             'n5-probe-value'
drv "printf x | /usr/bin/aginx-secret rm $N5_SCOPE"
expect_rc  "secret rm rc=0"
drv "grep -q '^pkg ok' /run/boot.state"
expect_rc  "pkg ok（迁移后的 stamps 让 sync 免重下 = aginx-download 活证）"

echo "==> J 备份（aginx-backup v2 本地线）"
drv "/usr/bin/aginx-backup now"
expect_rc  "backup now rc=0"
drv "/usr/bin/aginx-backup list"
expect_rc  "backup list rc=0"
expect_out "list 见新快照"                     'backup-[0-9]\{8\}T[0-9]\{6\}'
BAK="$(printf '%s' "${DRV_OUT:-}" | sed -n 's/.*\(backup-[0-9T]*\.tar\.gz\).*/\1/p' | tail -1)"
drv "/usr/bin/aginx-backup verify /var/backups/aginx/$BAK"
expect_rc  "backup verify rc=0（含 secret 剔除断言）"
drv "grep -q 'aginx-backup now' /etc/crontabs/root"
expect_rc  "crontab 定时行在"

echo "==> K 网关（六单元 / registered / 长连）"
wait_ready aginx-server  "单元 aginx-server"  15
wait_ready aginx-voice   "单元 aginx-voice"   15
wait_ready aginxbrowser  "单元 aginxbrowser"  15
wait_ready aginx-secretd "单元 aginx-secretd" 15
wait_ready net-watch      "单元 net-watch"     15
wait_ready aginx-gateway  "单元 aginx-gateway" 20
drv "/usr/bin/aginx-svc list | grep -c ready"
expect_out "恰六单元 ready（无多余）" '^6$'
drv "pgrep -a aginx-gateway"
expect_out "网关住 libexec（不进命令宇宙，D13）" "/usr/libexec/aginx/aginx-gateway"
drv "grep -h 'registered' $GWLOG | tail -1"
expect_out "网关日志见 registered" 'registered id='
wait_8443   "首验"

echo "==> L 远端收据（宿主 agc 经骨干真往返——本里程碑首条）"
# 密钥解析：env 直给或 0600 文件；两式都不回显。
AGC_SECRET="${AGC_RELAY_SECRET:-}"
if [ -z "$AGC_SECRET" ] && [ -n "${AGC_SECRET_FILE:-}" ]; then
  [ "$(stat -f '%Lp' "$AGC_SECRET_FILE" 2>/dev/null || stat -c '%a' "$AGC_SECRET_FILE")" = "600" ] \
    || { echo "FAIL - AGC_SECRET_FILE 非 0600（$AGC_SECRET_FILE）"; FAIL=$((FAIL+1)); }
  AGC_SECRET="$(cat "$AGC_SECRET_FILE")"
fi
[ -n "$AGC_SECRET" ] || { echo "FAIL - L 段需要 AGC_RELAY_SECRET（或 AGC_SECRET_FILE=0600 文件）——这是收据不是选配"; FAIL=$((FAIL+1)); }
if [ -n "$AGC_SECRET" ]; then
  # 设备 id：从 /etc/aginx/env 读进 shell 变量，不回显（键名计数后 sed 取值）。
  DEV_ID="$(adbx shell "sed -n 's/^${IDVAR}=//p' /etc/aginx/env" | tr -d '\r\n')"
  [ -n "$DEV_ID" ] || { echo "FAIL - 设备侧 $IDVAR 为空"; FAIL=$((FAIL+1)); }
  # agc 是生态仓里的独立 crate（包名 aginx-cli、bin 名 agc，无 workspace 根）
  AGC_BIN="$ECO/agc/target/release/agc"
  if [ ! -x "$AGC_BIN" ]; then
    echo "    构建 agc（生态仓 release）…"
    (cd "$ECO/agc" && cargo build --release) || { echo "FAIL - agc 构建失败"; FAIL=$((FAIL+1)); }
  fi
  if [ -x "$AGC_BIN" ] && [ -n "$DEV_ID" ]; then
    AGC_OUT="$(AGC_RELAY_SECRET="$AGC_SECRET" "$AGC_BIN" "agent://${DEV_ID}.relay.aginx.net/me" "用一句话介绍AginxOS" 2>&1 || true)"
    printf '%s' "$AGC_OUT" | grep -q "AginxOS" \
      && { echo "ok   - 远端真往返（回复含 AginxOS）"; PASS=$((PASS+1)); } \
      || { echo "FAIL - 远端往返（回复缺 AginxOS）"; echo "       out=$(printf '%s' "$AGC_OUT" | head -3)"; FAIL=$((FAIL+1)); }
    AGC_ERR="$(AGC_RELAY_SECRET="$AGC_SECRET" "$AGC_BIN" "agent://${DEV_ID}.relay.aginx.net/不存在的化身" "hi" 2>&1 || true)"
    printf '%s' "$AGC_ERR" | grep -q "不存在的化身" \
      && { echo "ok   - 化身负例（错误提及化身名）"; PASS=$((PASS+1)); } \
      || { echo "FAIL - 化身负例（错误未提及化身名）"; echo "       out=$(printf '%s' "$AGC_ERR" | head -3)"; FAIL=$((FAIL+1)); }
  fi
fi

echo "==> M 二启（aginx-reboot → 六单元自起 → 网关重连）"
drv "cp $FACE $FACEBAK 2>/dev/null; ls $FACEBAK"
expect_rc  "face 快照（二启后归 aginx-voice）"
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
drv "/usr/bin/aginx-svc list | grep -c ready"
expect_out "二启后恰六单元 ready" '^6$'
drv "aginx agent send 现在几点了"
expect_rc  "二启后母体仍应答（rc=0）"
expect_out "二启后母体真回复（非空有字）" "[一-龥]"
drv "grep -h 'registered' $GWLOG | tail -1"
expect_out "网关重注册（registered 复现）" 'registered id='
wait_8443   "二启后重连"
drv "/usr/bin/aginx-voice --inject 你好; sleep 1; /usr/bin/aginx-voice --face"
expect_out "二启后语音地板仍在（我在）" "我在"

echo
echo "n5: $PASS passed, $FAIL failed（终态：N5 在役，六单元+远端通道）"
[ "$FAIL" = 0 ]
