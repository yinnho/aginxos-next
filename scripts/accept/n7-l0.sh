#!/usr/bin/env bash
# n7 acceptance — L0 产品流全链（刀5，2026-09-12）：刷机=刷蛋，配置后置，
# ssh+pkg 自持。「刷完就是一台活的机器，装什么是用户自己的事」的收据线。
#
# 与 n6 分工：n6=镜像形状与整机等价（pre/opt-in/等价/capture 日）；n7=
# 产品流——出厂→配置→接管→安装→稳态。刷机腿是人工的（flash-redfin.sh
# 默认免 capture，刷完跑 pre）。
#
#   pre          刷完 L0：出厂形状速检（详细形状断言在 n6 pre，这里只锁
#                门面事实：l0 戳/svc.d=2/无 wifi.conf/var/bin 空/清单见母体）
#   usbconf      配置后置第一腿（host 驱动 adb）：推 wifi.conf → /etc/，
#                追加 ssh 公钥（套件自生成一次性键），可选 N7_ENV 灌注
#                /etc/aginx/env（brain 键 + AGINX_GATEWAY_ID），reboot
#   netup        等网回来：boot.state wifi/internet ok + 钟到 2026
#   ssh          host 腿真 ssh 往返（Wi-Fi 直连，不走 adb forward——产品
#                故事）：公钥腿 BatchMode 硬断言；密码腿=本相位内闭环
#                （throwaway 密码生成→chpasswd→expect 登录→passwd 锁回→
#                证锁后公钥仍在=双通道不互斥）；值零回显、零落盘
#   optin-mother 母体一句话：opt-in aginx → unit aginx ready → env 键名
#                在 → send 真中文回复（真脑断言——secret.policy 哑弹在
#                此现形）
#   optin-phone  手机形态：opt-in aginx-term / aginx-voice（自动带
#                asr/tts/ocr）/ aginxbrowser（裸上游件，缺席容忍拾取）/
#                aginx-gateway（自动带 secretd，env 已灌注不进断路器）
#                → 单元 ready + term handoff 亮屏 + voice face
#   steady       同像重启：wifi 自动连 / pkg ok 秒落（全 opt 早退）/
#                六单元 / send 仍答 / sync 零 downloading
#
# 用法（相位序即产品序）：
#   N7_WIFI_CONF=~/somewhere/wifi.conf ./scripts/accept/n7-l0.sh usbconf
#   ./scripts/accept/n7-l0.sh netup|ssh|optin-mother|optin-phone|steady
#
# 秘密纪律：wifi.conf/env 走 adb push 推文件（路径进命令行，内容不进）；
# throwaway 密码与 ssh 键对生灭于单次相位进程内（mktemp -d + trap 清理），
# 值零回显；锁回用 sed 直写 shadow（root:! = 确定性锁，不赌 busybox
# passwd -l 旗标面）。
set -euo pipefail

ACCEPT_DEVICE=redfin . "$(dirname "$0")/_serial.sh"  # SERIAL：env 最高，默认读 redfin 档案 [adb]
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
N7_WIFI_CONF="${N7_WIFI_CONF:-${REPO}/.local/wifi.conf}"
N7_ENV="${N7_ENV:-${REPO}/.local/aginx-env}"

PASS=0
FAIL=0

adbx() { adb -s "$SERIAL" "$@"; }

die() { echo "n7: $*" >&2; exit 1; }

drv() {
  local raw
  raw="$(adbx shell "export HOME=/home PATH=/usr/bin:/bin:/sbin:/var/bin; $1; echo __RC=\$?" 2>&1 || true)"
  raw="${raw//$'\r'/}"
  DRV_RC="$(printf '%s\n' "$raw" | sed -n 's/^__RC=//p' | tail -1)"
  DRV_OUT="$(printf '%s\n' "$raw" | grep -vE '^(libc:|linker|WARNING)' | grep -v '^__RC=' || true)"
}

expect_rc()  { [ "${DRV_RC:-}" = "0" ] && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1 (rc=${DRV_RC:-?})"; FAIL=$((FAIL+1)); } }
expect_norc(){ [ "${DRV_RC:-}" != "0" ] && { echo "ok   - $1 (rc=${DRV_RC:-?})"; PASS=$((PASS+1)); } || { echo "FAIL - ${1}（rc=0，不该成功）"; FAIL=$((FAIL+1)); } }
expect_out() { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } }
expect_no()  { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "FAIL - ${1}（不该出现: ${2}）"; FAIL=$((FAIL+1)); } || { echo "ok   - $1"; PASS=$((PASS+1)); } }

wait_ready() { # name label tries
  local i
  for i in $(seq 1 "${3:-10}"); do
    drv "/usr/bin/aginx-svc status $1"
    printf '%s' "${DRV_OUT:-}" | grep -q "ready" && { echo "ok   - ${2}（ready）"; PASS=$((PASS+1)); return 0; }
    sleep 3
  done
  echo "FAIL - ${2}（未 ready）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -3)"; FAIL=$((FAIL+1)); return 1
}

wait_stamp() { # name label tries
  local i
  for i in $(seq 1 "${3:-40}"); do
    drv "test -e /var/bin/$1"
    [ "${DRV_RC:-}" = "0" ] && { echo "ok   - $2"; PASS=$((PASS+1)); return 0; }
    sleep 15
  done
  echo "FAIL - $2"; FAIL=$((FAIL+1)); return 1
}

online() { adbx get-state >/dev/null 2>&1 || die "device $SERIAL 不在线"; }

# 设备 Wi-Fi IP（wlan0）——产品腿要真 Wi-Fi 直连；Mac 与设备异网段时的
# adb forward 替代法留在 #142 收据，不在本套件。
dev_ip() {
  adbx shell "ip addr show wlan0 2>/dev/null" | sed -n 's/.*inet \([0-9][0-9.]*\).*/\1/p' | head -1
}

phase_pre() {
  echo "==> 出厂形状速检（详细断言在 n6 pre）"
  online
  drv "grep -q ' l0\$' /etc/aginx-version"
  expect_rc  "版本戳尾 l0（L0 形戳）"
  drv "ls /etc/aginx/svc.d | wc -l"
  expect_out "镜像 svc.d 恰 2（net-watch+aginxbrowser）" '^2$'
  drv "test ! -e /etc/wifi.conf && test ! -e /root/.ssh/authorized_keys"
  expect_rc  "出厂无配置（wifi.conf/authorized_keys 都无——零个人信息实证）"
  drv "ls -A /var/bin 2>/dev/null | wc -l"
  expect_out "var/bin 空（零包）" '^0$'
  drv "pidof dropbear >/dev/null"
  expect_rc  "dropbear 在跑（ssh 是 L0 自持件）"
  drv "aginx-pkg available | grep -qx aginx"
  expect_rc  "清单见母体（available 列 aginx）"
}

phase_usbconf() {
  echo "==> 配置后置第一腿（adb 推文件；内容零回显）"
  online
  test -s "${N7_WIFI_CONF}" \
    || die "wifi.conf 不在 ${N7_WIFI_CONF}（N7_WIFI_CONF= 指到真源；psk 不进命令行铁律）"
  adbx push "${N7_WIFI_CONF}" /etc/wifi.conf >/dev/null
  adbx shell "chmod 600 /etc/wifi.conf"
  drv "test -s /etc/wifi.conf"
  expect_rc  "wifi.conf 已落 /etc（600）"
  if [ -s "${N7_ENV}" ]; then
    adbx shell "mkdir -p /etc/aginx && chmod 700 /etc/aginx"
    adbx push "${N7_ENV}" /etc/aginx/env >/dev/null
    adbx shell "chmod 600 /etc/aginx/env"
    drv "grep -q '^AGINXBRAIN_API_KEY=' /etc/aginx/env && grep -q '^AGINX_GATEWAY_ID=' /etc/aginx/env"
    expect_rc  "env 已灌注（brain/gateway 键名在，值零回显）"
  else
    echo "warn - N7_ENV 未给（${N7_ENV} 缺）——optin-mother 段会 FAIL；这是 send 真回复的前置" >&2
  fi
  # 公钥腿材料：一次性键对（相位内闭环，trap 清理）——authorized_keys 用
  # 追加不覆写（capture 日已有运维键时双键并存=不互斥的另一面）。
  KEYDIR="$(mktemp -d "${TMPDIR:-/tmp}/n7-key.XXXXXX")"
  trap 'rm -rf "${KEYDIR}"' EXIT
  ssh-keygen -q -t ed25519 -N '' -C n7-l0 -f "${KEYDIR}/id" 2>/dev/null
  adbx shell "mkdir -p /root/.ssh && chmod 700 /root/.ssh"
  adbx push "${KEYDIR}/id.pub" /tmp/n7-pub >/dev/null
  drv "cat /tmp/n7-pub >> /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys && rm -f /tmp/n7-pub && wc -l < /root/.ssh/authorized_keys"
  expect_out "公钥已追加 authorized_keys（1 行）" '^1$'
  rm -rf "${KEYDIR}"
  # 重启进配置生效的世界（wifi 接上；usbconf 段到此为止）
  drv "/usr/bin/aginx-reboot"
  echo "ok   - reboot（下一相位 netup 等网）"
  PASS=$((PASS+1))
}

phase_netup() {
  echo "==> 等网（wifi.conf 生效 + internet + 钟）"
  online
  local UP_OK=0 i
  for i in $(seq 1 90); do
    adbx get-state 2>/dev/null | grep -q device || { sleep 5; continue; }
    drv "grep -q '^wifi ok' /run/boot.state && grep -q '^internet ok' /run/boot.state"
    [ "${DRV_RC:-}" = "0" ] && { UP_OK=1; break; }
    sleep 5
  done
  [ "$UP_OK" = 1 ] && { echo "ok   - wifi ok + internet ok"; PASS=$((PASS+1)); } \
                  || { echo "FAIL - wifi/internet ok 未落（7.5min）"; FAIL=$((FAIL+1)); return 0; }
  local CLK_OK=0
  for i in $(seq 1 60); do
    drv "[ \$(date +%Y) -ge 2026 ]"
    [ "${DRV_RC:-}" = "0" ] && { CLK_OK=1; break; }
    sleep 5
  done
  [ "$CLK_OK" = 1 ] && { echo "ok   - 钟到 2026（TLS/镜像源前置）"; PASS=$((PASS+1)); } \
                  || { echo "FAIL - 钟未到 2026"; FAIL=$((FAIL+1)); }
  local ip
  ip="$(dev_ip)"
  [ -n "${ip}" ] && { echo "ok   - wlan0 IP ${ip}（ssh 腿用）"; PASS=$((PASS+1)); } \
                || { echo "FAIL - wlan0 无 IP"; FAIL=$((FAIL+1)); }
}

phase_ssh() {
  echo "==> host 腿真 ssh 往返（Wi-Fi 直连；公钥+密码双通道）"
  online
  command -v expect >/dev/null 2>&1 || { echo "FAIL - host 无 expect（macOS 自带 /usr/bin/expect）"; FAIL=$((FAIL+1)); return 0; }
  local ip
  ip="$(dev_ip)"
  [ -n "${ip}" ] || { echo "FAIL - wlan0 无 IP，ssh 腿无从下腿"; FAIL=$((FAIL+1)); return 0; }
  drv "pidof dropbear >/dev/null"
  expect_rc  "dropbear 在跑"
  KEYDIR="$(mktemp -d "${TMPDIR:-/tmp}/n7-ssh.XXXXXX")"
  PWLOCKED=0
  cleanup_ssh() {
    [ "${PWLOCKED}" = "0" ] && drv "sed -i 's/^root:[^:]*:/root:!:/' /etc/shadow; chmod 600 /etc/shadow" >/dev/null 2>&1 || true
    rm -rf "${KEYDIR}"
  }
  trap cleanup_ssh EXIT
  ssh-keygen -q -t ed25519 -N '' -C n7-l0 -f "${KEYDIR}/id" 2>/dev/null
  adbx push "${KEYDIR}/id.pub" /tmp/n7-pub >/dev/null
  drv "cat /tmp/n7-pub >> /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys && rm -f /tmp/n7-pub"
  expect_rc  "一次性公钥追加（不覆写——存量键共存）"
  # --- 公钥腿：BatchMode=只许键，密码不可用则硬失败 ---
  if ssh -i "${KEYDIR}/id" -o BatchMode=yes -o IdentitiesOnly=yes \
       -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
       -o ConnectTimeout=15 "root@${ip}" 'echo n7-ssh-ok' 2>/dev/null | grep -q n7-ssh-ok; then
    echo "ok   - 公钥腿真 ssh 往返（${ip}）"; PASS=$((PASS+1))
  else
    echo "FAIL - 公钥腿 ssh 往返（${ip}）"; FAIL=$((FAIL+1))
  fi
  # --- 密码腿：throwaway 生灭于本相位（值零回显零落盘） ---
  N7PASS="$(head -c 9 /dev/urandom | base64)"
  printf 'root:%s\n' "${N7PASS}" > "${KEYDIR}/pw"
  chmod 600 "${KEYDIR}/pw"
  adbx push "${KEYDIR}/pw" /tmp/.n7pw >/dev/null
  adbx shell "/bin/busybox chpasswd -c sha512 < /tmp/.n7pw; rm -f /tmp/.n7pw"
  rm -f "${KEYDIR}/pw"
  # 密码哈希档=sha512（#320，2026-09-12 活体收据：busybox 1.36.1 带 -c ALG，
  # -c sha512 落 $6$，dropbear 静态 musl crypt 验 $6$ 密码腿真通）。裸
  # chpasswd 是 des crypt（13 字符、8 字符截断——bake #22 实测），契约
  # 钉死 sha512：断言第二字段 $6$ 前缀。
  drv "F=\$(cut -d: -f2 /etc/shadow); case \$F in '\$6'*) true;; *) false;; esac"
  expect_rc  "密码已设（shadow \$6\$ sha512 形；值零回显）"
  # expect 走密码路：禁公钥，只许 password
  if PASS="${N7PASS}" IP="${ip}" expect -c '
        set timeout 25
        spawn ssh -o PubkeyAuthentication=no -o PreferredAuthentications=password \
                  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
                  -o ConnectTimeout=15 root@$env(IP) echo n7-pass-ok
        expect {
          -re "(?i)password" { send "$env(PASS)\r"; exp_continue }
          "n7-pass-ok" { exit 0 }
          eof
        }
        exit 1' 2>/dev/null; then
    echo "ok   - 密码腿真 ssh 往返（史上首收据线）"; PASS=$((PASS+1))
  else
    echo "FAIL - 密码腿 ssh 往返"; FAIL=$((FAIL+1))
  fi
  # 锁回 + 证双通道独立：锁死密码不伤公钥
  drv "sed -i 's/^root:[^:]*:/root:!:/' /etc/shadow && chmod 600 /etc/shadow"
  expect_rc  "密码锁回（root:! 确定性锁）"
  PWLOCKED=1
  drv "grep -q '^root:!' /etc/shadow"
  expect_rc  "shadow 锁形在"
  if ssh -i "${KEYDIR}/id" -o BatchMode=yes -o IdentitiesOnly=yes \
       -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
       -o ConnectTimeout=15 "root@${ip}" 'echo n7-ssh-ok' 2>/dev/null | grep -q n7-ssh-ok; then
    echo "ok   - 锁密码后公钥仍通（双通道不互斥实证）"; PASS=$((PASS+1))
  else
    echo "FAIL - 锁密码后公钥不通"; FAIL=$((FAIL+1))
  fi
  rm -rf "${KEYDIR}"
}

phase_optin_mother() {
  echo "==> 母体一句话（opt-in aginx → 真脑断言）"
  online
  drv "grep -q '^AGINXBRAIN_API_KEY=' /etc/aginx/env"
  expect_rc  "env brain 键名在（缺=N7_ENV 未灌注，send 只会 401 空答）"
  drv "aginx-pkg opt-in aginx"
  expect_rc  "opt-in aginx rc=0"
  wait_stamp aginx "母体 face 落地（/var/bin/aginx）" 40
  wait_ready aginx "单元 aginx ready（刀3：单元名随包名）" 20
  drv "aginx commands >/dev/null"
  expect_rc  "命令面活了"
  drv "aginx agent send 现在几点了"
  expect_rc  "send rc=0"
  expect_no  "send 非报错行（无 aginx agent: 前缀）" '^aginx agent:'
  expect_out "send 真中文回复（secret.policy pkgfiles 真身放行——哑弹现形门）" "[一-龥]"
}

phase_optin_phone() {
  echo "==> 手机形态（term+voice(带 3 模型)+browser+gateway(带 secretd)）"
  online
  local p
  for p in aginx-term aginx-voice aginxbrowser aginx-gateway; do
    drv "aginx-pkg opt-in $p"
    expect_rc  "opt-in $p rc=0"
  done
  local n
  for n in aginx-term aginx-voice aginx-asr aginx-tts aginx-ocr aginxbrowser aginx-gateway aginx-secretd; do
    wait_stamp "$n" "face 落地：$n" 40
  done
  wait_ready aginx-voice   "单元 aginx-voice ready" 20
  wait_ready aginxbrowser  "单元 aginxbrowser ready（缺席容忍 30s 拾取）" 10
  wait_ready aginx-secretd "单元 aginx-secretd ready" 20
  wait_ready aginx-gateway "单元 aginx-gateway ready（env 已灌注→不进断路器）" 20
  local TWAIT=0 i
  for i in $(seq 1 20); do
    drv "pidof aginx-term >/dev/null"
    [ "${DRV_RC:-}" = "0" ] && { TWAIT=1; break; }
    sleep 3
  done
  [ "$TWAIT" = 1 ] && { echo "ok   - term handoff 亮屏（装包即亮）"; PASS=$((PASS+1)); } \
                  || { echo "FAIL - aginx-term 未起（handoff 静默轮询没接住？）"; FAIL=$((FAIL+1)); }
  local FWAIT=0
  for i in $(seq 1 20); do
    drv "test -s /run/aginx-voice/face"
    [ "${DRV_RC:-}" = "0" ] && { FWAIT=1; break; }
    sleep 3
  done
  [ "$FWAIT" = 1 ] && { echo "ok   - voice face 出现"; PASS=$((PASS+1)); } \
                  || { echo "FAIL - voice face 未出现"; FAIL=$((FAIL+1)); }
}

phase_steady() {
  echo "==> 同像重启稳态（pkg ok 秒落/六单元/send 仍答）"
  online
  drv "/usr/bin/aginx-reboot || true"
  sleep 5
  local BACK=0 i
  for i in $(seq 1 60); do
    adbx get-state 2>/dev/null | grep -q device && { BACK=1; break; }
    sleep 5
  done
  [ "$BACK" = 1 ] || { echo "FAIL - 5 分钟未回 adb"; FAIL=$((FAIL+1)); return 0; }
  local WIFIOK=0
  for i in $(seq 1 60); do
    drv "grep -q '^wifi ok' /run/boot.state && grep -q '^internet ok' /run/boot.state"
    [ "${DRV_RC:-}" = "0" ] && { WIFIOK=1; break; }
    sleep 5
  done
  [ "$WIFIOK" = 1 ] && { echo "ok   - wifi 自动连 + internet ok（配置持久实证）"; PASS=$((PASS+1)); } \
                  || { echo "FAIL - wifi/internet ok 未落"; FAIL=$((FAIL+1)); }
  local PKG_OK=0 PKG_T0=$(date +%s)
  for i in $(seq 1 40); do
    drv "grep -q '^pkg ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && { PKG_OK=1; break; }
    sleep 15
  done
  [ "$PKG_OK" = 1 ] && { echo "ok   - pkg ok（全 opt 早退，$(( $(date +%s) - PKG_T0 ))s 落）"; PASS=$((PASS+1)); } \
                  || { echo "FAIL - pkg ok 未落"; FAIL=$((FAIL+1)); }
  # 六单元=svc.d 两件(net-watch/aginxbrowser)+包四单元(aginx/voice/
  # secretd/gateway)；刚重启单元还在起，有界等。
  local READY_OK=0 i
  for i in $(seq 1 20); do
    drv "/usr/bin/aginx-svc list | grep -c ready"
    printf '%s' "${DRV_OUT:-}" | grep -q '^6$' && { READY_OK=1; break; }
    sleep 5
  done
  [ "$READY_OK" = 1 ] && { echo "ok   - 六单元恰 ready（aginx/voice/browser/net-watch/secretd/gateway）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - 六单元未齐（out=$(printf '%s' "${DRV_OUT:-}" | head -2)）"; FAIL=$((FAIL+1)); }
  drv "aginx agent send 现在几点了"
  expect_rc  "二启后母体仍应答"
  expect_no  "send 非报错行" '^aginx agent:'
  expect_out "二启后真回复" "[一-龥]"
  drv "aginx-pkg sync"
  expect_rc  "sync rc=0（no-op 但签名链照验）"
  expect_no  "稳态零 downloading" 'downloading'
}

case "${1:-}" in
  pre)          phase_pre ;;
  usbconf)      phase_usbconf ;;
  netup)        phase_netup ;;
  ssh)          phase_ssh ;;
  optin-mother) phase_optin_mother ;;
  optin-phone)  phase_optin_phone ;;
  steady)       phase_steady ;;
  *)
    cat >&2 <<USAGE
usage: n7-l0.sh <pre|usbconf|netup|ssh|optin-mother|optin-phone|steady>
  pre          刷完 L0 出厂速检
  usbconf      推 wifi.conf(+可选 N7_ENV)+公钥 → reboot（N7_WIFI_CONF= 指真源）
  netup        等网回来（wifi/internet ok + 钟 + IP）
  ssh          真 ssh 往返：公钥腿 + 密码腿（throwaway 闭环）+ 锁回
  optin-mother opt-in aginx → 真脑断言
  optin-phone  opt-in term/voice/browser/gateway → 手机形态（steady 六单元的地基）
  steady       重启稳态（pkg ok 秒落 / 六单元 / send / 零 downloading）
USAGE
    exit 2 ;;
esac

echo
echo "n7-l0($1): $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
