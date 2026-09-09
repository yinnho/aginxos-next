#!/usr/bin/env bash
# n6-egg acceptance — 蛋案设备日（C11）。
#
# 蛋的收据分四段，对应设备日的四个状态——套件按相位跑（刷机/举码是
# 人工腿，不在套件里；runbook 见 HARDWARE.md 蛋案节）：
#
#   pre     刷完蛋、配网前：出厂形状预检 + A 哑终端（commands 活、
#           母体死、单元 absent）
#   paired  用户举屏扫码配网后：B 配网产物证（wifi.conf/boot.state/
#           env 行数不回显值/钟）+ C 清单自动装（term 自动 sync，15min
#           窗：8 stamps + 8 faces + sidecar + 模型 symlink 落地 +
#           wait_ready×6）+ D 等价（send 真往返/8443/voice local/
#           secretd policy）+ E 点击装（grok opt-in，CLI 代行+注记）
#   steady  二启后（同像）：wifi 自动连 / pkg ok / 六单元 / send 仍答 /
#           sync 零 downloading
#   egg2    第二颗蛋+state 后（capture 正常刷机日）：wifi.conf 从 state
#           回来（出厂本无此件）→ sync missing—downloading×8 重拉 →
#           等价复验
#
# 已知缺口（R13，接受）：首启无语音问候（装完 uptime 已超 #282 闸），
# 问候收据归 steady 段后的真人眼验（#198 同类）。
#
# 纪律同 n5/m42c：钉死 serial；秘密零回显——env 只数行、policy 探针是
# 套件自有 scope；busybox netstat 禁用（/proc/net/tcp）；设备杀进程
# pkill -x 或按 pid。
set -euo pipefail

ACCEPT_DEVICE=redfin . "$(dirname "$0")/_serial.sh"  # SERIAL：env 最高，默认读 redfin 档案 [adb]
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT_HEX=20FB          # 8443 = 0x20FB（busybox netstat 必炸，走 /proc/net/tcp）
GWLOG=/var/log/aginx-svc/aginx-gateway.log
CORE8="aginx-runtime aginx-server aginx-gateway aginx-secretd aginx-asr aginx-tts aginx-ocr aginx-voice"
UNITS6="aginx-server aginx-voice aginxbrowser aginx-secretd net-watch aginx-gateway"
N6_SCOPE=n6-egg-probe

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
expect_norc(){ [ "${DRV_RC:-}" != "0" ] && { echo "ok   - $1 (rc=${DRV_RC:-?})"; PASS=$((PASS+1)); } || { echo "FAIL - $1（rc=0，不该成功）"; FAIL=$((FAIL+1)); } }
expect_out() { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } }
expect_no()  { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "FAIL - $1（不该出现: $2）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } || { echo "ok   - $1"; PASS=$((PASS+1)); } }

wait_ready() { # name label tries
  local i
  for i in $(seq 1 "${3:-10}"); do
    drv "/usr/bin/aginx-svc status $1"
    printf '%s' "${DRV_OUT:-}" | grep -q "ready" && { echo "ok   - $2（ready）"; PASS=$((PASS+1)); return 0; }
    sleep 3
  done
  echo "FAIL - $2（未 ready）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -3)"; FAIL=$((FAIL+1)); return 1
}

wait_8443() { # label
  local i
  for i in $(seq 1 20); do
    drv "grep -q ':${PORT_HEX} 01 ' /proc/net/tcp /proc/net/tcp6 2>/dev/null"
    [ "${DRV_RC:-}" = "0" ] && { echo "ok   - $1（8443 ESTABLISHED）"; PASS=$((PASS+1)); return 0; }
    sleep 3
  done
  echo "FAIL - $1（8443 未 ESTABLISHED）"; FAIL=$((FAIL+1)); return 1
}

# 8 stamps 全落（C 段安装完成信号；term 自动 sync 不写 boot.state，
# stamps 是唯一的落地面）。tries×sleeps 是给首拉 ~700MB 的窗。
wait_core8_stamps() { # label tries sleep_secs
  local i n missing
  for i in $(seq 1 "${2:-60}"); do
    missing=""
    for n in $CORE8; do
      drv "test -f /var/lib/aginx/stamps/$n"
      [ "${DRV_RC:-}" = "0" ] || missing="$missing $n"
    done
    [ -z "$missing" ] && { echo "ok   - $1"; PASS=$((PASS+1)); return 0; }
    sleep "${3:-15}"
  done
  echo "FAIL - $1（15min 窗内未齐，缺:$missing）"; FAIL=$((FAIL+1)); return 1
}

phase_pre() {
  echo "==> 预检（出厂形状：剥面/dangling/无 wifi.conf/无 stamps）"
  adbx get-state >/dev/null 2>&1 || { echo "n6-egg: device $SERIAL 不在线"; exit 1; }
  drv "grep -q ' egg\$' /etc/aginx-version"
  expect_rc  "版本戳尾 egg（n6 蛋形戳）"
  drv "ls /usr/libexec/aginx | wc -l"
  expect_out "libexec 恰三件（svcd/net-watch/net-rejoin）" '^3$'
  for b in aginx-svcd net-watch net-rejoin; do
    drv "test -x /usr/libexec/aginx/$b"; expect_rc "libexec 在：$b"
  done
  drv "test ! -e /usr/bin/aginx-voice && test ! -e /usr/libexec/aginx/aginx-server"
  expect_rc  "剥面不在：/usr/bin/aginx-voice、libexec/aginx-server"
  drv "ls /var/bin 2>/dev/null | grep -vc '\\.aginxmd\$'"
  expect_out "var/bin 无二进制（只余 sidecar）" '^0$'
  drv "test -L /var/models/asr && test ! -e /var/models/asr && test -L /var/models/ocr && test ! -e /var/models/ocr"
  expect_rc  "models asr/ocr = dangling symlink（装包前 -e 恒假）"
  drv "test -L /var/models/tts/vits-melo-tts-zh_en && test ! -e /var/models/tts/vits-melo-tts-zh_en"
  expect_rc  "models tts = dangling symlink"
  drv "test ! -e /etc/wifi.conf"
  expect_rc  "出厂形状：无 /etc/wifi.conf（SKIP_STATE 未预武实证）"
  drv "test ! -e /run/aginx-voice/face"
  expect_rc  "voice 从未起过（face 不存在）"
  drv "ls /var/lib/aginx/stamps 2>/dev/null | wc -l"
  expect_out "stamps 空（fresh 蛋零包）" '^0$'

  echo "==> A 哑终端（壳活、母体死）"
  drv "aginx commands >/dev/null"
  expect_rc  "aginx commands rc=0（路由器在蛋里）"
  drv "aginx agent send me"
  expect_norc "aginx agent send 失败（无 server = 哑终端本相）"
  drv "/usr/bin/aginx-svc status aginx-server"
  expect_out "aginx-server 单元 absent（cmd 指未装的 /var/bin）" 'absent'
}

phase_paired() {
  echo "==> B 配网产物证（举屏扫码后；秘密零回显）"
  adbx get-state >/dev/null 2>&1 || { echo "n6-egg: device $SERIAL 不在线"; exit 1; }
  drv "grep -c '^ssid' /etc/wifi.conf"
  expect_out "wifi.conf 恰一行 ssid" '^1$'
  drv "grep -q '^internet ok' /run/boot.state"
  expect_rc  "boot.state internet ok（pair apply 定点刷新）"
  drv "test \$(wc -l < /etc/aginx/env) -ge 3"
  expect_rc  "env 三键在（只数行，值零回显）"
  drv "[ \$(date +%Y) -ge 2026 ]"
  expect_rc  "钟到 2026（quick_clock 腿）"

  echo "==> C 清单自动装（term 自动 sync；15min 窗）"
  wait_core8_stamps "8 包 stamps 齐（首拉完成）" 60 15
  local n
  for n in $CORE8; do
    drv "test -f /var/lib/aginx/stamps/$n.version"
    expect_rc  "version stamp 在：$n"
    drv "test -e /var/bin/$n && test -f /var/bin/$n.aginxmd"
    expect_rc  "face + sidecar 在：$n（#284 闭环）"
  done
  drv "test -d /var/models/asr && test -d /var/models/ocr"
  expect_rc  "模型树经 symlink 可达（dangling→真身）"
  local u
  for u in $UNITS6; do
    wait_ready "$u" "单元 $u" 20
  done
  local FACEWAIT=0 i
  for i in $(seq 1 20); do
    drv "test -s /run/aginx-voice/face"
    [ "${DRV_RC:-}" = "0" ] && { FACEWAIT=1; break; }
    sleep 3
  done
  [ "$FACEWAIT" = 1 ] && { echo "ok   - voice face 出现且非空"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - voice face 未出现"; FAIL=$((FAIL+1)); }

  echo "==> D 等价（与整机同待遇）"
  drv "aginx agent send 现在几点了"
  expect_rc  "send rc=0（母体应答）"
  expect_out "send 真回复（非空有字）" "[一-龥]"
  wait_8443   "网关远端通道"
  drv "grep -rq 'local=true' /var/log/aginx-svc/ 2>/dev/null"
  expect_rc  "voice 日志 local=true（本地语音栈在役）"
  drv "printf n6-probe-value | /usr/bin/aginx-secret set $N6_SCOPE"
  expect_rc  "secret set（探针 scope）rc=0"
  drv "printf x | /usr/bin/aginx-secret get $N6_SCOPE"
  expect_out "get 对未放行 exe 拒读（policy 生证）" '"code":"denied"'
  drv "printf x | /usr/bin/aginx-secret rm $N6_SCOPE"
  expect_rc  "secret rm（探针自清）rc=0"

  echo "==> E 点击装（grok opt-in；CLI 代行——tap 是真人腿，注记收据）"
  drv "aginx-pkg opt-in grok"
  expect_rc  "opt-in grok rc=0"
  local GWAIT=0 i
  for i in $(seq 1 20); do
    drv "test -e /var/bin/grok"
    [ "${DRV_RC:-}" = "0" ] && { GWAIT=1; break; }
    sleep 15
  done
  [ "$GWAIT" = 1 ] && { echo "ok   - /var/bin/grok 落地（点击装同引擎）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - grok 未落地（5min）"; FAIL=$((FAIL+1)); }
}

phase_steady() {
  echo "==> F 同像重启（套件内真重启 + 等回）"
  adbx get-state >/dev/null 2>&1 || { echo "n6-egg: device $SERIAL 不在线"; exit 1; }
  drv "/usr/bin/aginx-reboot || true"
  sleep 5
  local BACK=0 i
  for i in $(seq 1 60); do
    adbx get-state 2>/dev/null | grep -q device && { BACK=1; break; }
    sleep 5
  done
  [ "$BACK" = 1 ] || { echo "FAIL - 5 分钟未回 adb"; FAIL=$((FAIL+1)); return 0; }
  REBOOTED=1
  local UP_OK=0
  for i in $(seq 1 60); do
    drv "grep -q '^done' /run/boot.state 2>/dev/null && pidof aginx-voice >/dev/null"
    [ "${DRV_RC:-}" = "0" ] && { UP_OK=1; break; }
    sleep 5
  done
  [ "$UP_OK" = 1 ] || { echo "FAIL - 未见 boot done + voice up"; FAIL=$((FAIL+1)); return 0; }
  drv "grep -q '^wifi ok' /run/boot.state && grep -q '^internet ok' /run/boot.state"
  expect_rc  "wifi 自动连 + internet ok（wifi.conf 持久实证）"
  # provision 本靴全程跑（wifi ok 不早退）——pkg ok 分钟级，有界等
  local PKG_OK=0
  for i in $(seq 1 40); do
    drv "grep -q '^pkg ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && { PKG_OK=1; break; }
    sleep 15
  done
  [ "$PKG_OK" = 1 ] && { echo "ok   - pkg ok（provision resync 全程跑通）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - pkg ok 未落（10min）"; FAIL=$((FAIL+1)); }
  drv "/usr/bin/aginx-svc list | grep -c ready"
  expect_out "六单元恰 ready" '^6$'
  drv "aginx agent send 现在几点了"
  expect_rc  "二启后母体仍应答"
  expect_out "二启后母体真回复" "[一-龥]"
  drv "aginx-pkg sync"
  expect_rc  "sync rc=0"
  expect_no  "稳态零 downloading" 'downloading'
  drv "/usr/bin/aginx-voice --inject 你好; sleep 1; /usr/bin/aginx-voice --face"
  expect_out "语音地板仍在（我在）" '我在'
}

phase_egg2() {
  echo "==> G 第二颗蛋 + state（capture 正常刷机日后跑）"
  adbx get-state >/dev/null 2>&1 || { echo "n6-egg: device $SERIAL 不在线"; exit 1; }
  drv "grep -q ' egg\$' /etc/aginx-version"
  expect_rc  "还是蛋（版本戳尾 egg）"
  drv "test -e /etc/wifi.conf"
  expect_rc  "wifi.conf 从 state tar 回来（出厂本无此件 = state-restore 实证）"
  drv "grep -q '^wifi ok' /run/boot.state"
  expect_rc  "wifi 自动连（无需再扫）"
  drv "test ! -e /var/bin/aginx-server"
  expect_rc  "var/bin 被刷没（stamps 在而脸不在 = sync 判 missing 的前提）"
  # 重拉 8 包 ~700MB——30min 窗
  local PKG_OK=0 i
  for i in $(seq 1 60); do
    drv "grep -q '^pkg ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && { PKG_OK=1; break; }
    sleep 30
  done
  [ "$PKG_OK" = 1 ] && { echo "ok   - pkg ok（重拉完成）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - pkg ok 未落（30min）"; FAIL=$((FAIL+1)); return 0; }
  drv "grep -c 'downloading' /var/tmp/aginx-pkg-sync.log 2>/dev/null"
  expect_out "sync 日志见 missing—downloading×8（stamp 在脸不在的判据）" '^([89]|1[0-9])$'
  local n
  for n in $CORE8; do
    drv "test -e /var/bin/$n"
    expect_rc  "face 回来：$n"
  done
  drv "test -d /var/models/asr"
  expect_rc  "模型树回来（C9 排除 → 重拉即真身）"
  local u
  for u in $UNITS6; do
    wait_ready "$u" "单元 $u" 20
  done
  drv "aginx agent send 现在几点了"
  expect_rc  "换蛋后母体仍应答"
  expect_out "换蛋后母体真回复" "[一-龥]"
  wait_8443   "换蛋后网关重连"
}

cleanup() {
  drv "printf x | /usr/bin/aginx-secret rm $N6_SCOPE >/dev/null 2>&1; true"
}
trap cleanup EXIT

case "${1:-}" in
  pre)    phase_pre ;;
  paired) phase_paired ;;
  steady) phase_steady ;;
  egg2)   phase_egg2 ;;
  *)
    cat >&2 <<USAGE
usage: n6-egg.sh <pre|paired|steady|egg2>
  pre     刷完蛋配网前（出厂形状 + 哑终端）
  paired  扫码配网后（B 配网证 + C 自动装 + D 等价 + E 点击装）
  steady  二启后（同像稳态：pkg ok / 六单元 / send / 零 downloading）
  egg2    第二颗蛋+state 刷机日后（重拉 + 等价复验）
USAGE
    exit 2 ;;
esac

echo
echo "n6-egg($1): $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
