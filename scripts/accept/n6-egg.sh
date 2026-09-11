#!/usr/bin/env bash
# n6 acceptance — L0 底座设备日（C11 蛋案 → 刀5 L0 翻档，2026-09-12）。
#
# L0 的世界：镜像=内核+init+svc+网络+ssh+pkg，其余全是包。收据按相位跑
# （刷机/推配置是人工腿，不在套件里；runbook 见 HARDWARE.md 刀6 节）：
#
#   pre     刷完 L0、配置前：出厂形状预检 + A 哑终端（pkg/svc 活、路由器
#           死、清单可见母体）
#   paired  配网+灌 env 后（adb push wifi.conf / n7 usbconf 腿）：
#           B 配网产物证（wifi.conf/boot.state/env 键名不回显值/钟）+
#           C 显式 opt-in 五连（aginx/aginx-term/aginx-voice/aginx-
#           browser/aginx-gateway——voice 自动带 asr/tts/ocr，gateway 自动
#           带 secretd，刀1 依赖感知的设备面收据；browser 是裸上游二进制
#           opt 行，缺席容忍单元 30s 自拾取=设计用途首次实证）+
#           D 等价（send 真往返/8443/voice local/secretd policy）+
#           E 点击装（grok opt-in，CLI 代行）
#   steady  二启后（同像）：wifi 自动连 / pkg ok（全 opt=秒落）/
#           六单元 / send 仍答 / sync 零 downloading
#   egg2    capture 升级日（CAPTURE=1 刷机后）：wifi.conf 从 state 回来；
#           脸被刷没但全 opt 不再自动重拉（provision 早退）——opt-in 重跑
#           回脸 + 等价复验。与蛋时代的 missing—downloading×8 是两条路。
#
# 前置（刀6 镜像先行闸）：pkgs.aginx.net 已上 8 包新车——opt-in 404 =
# 镜像源没上，不是套件的失败。env 灌注（brain 键 + AGINX_GATEWAY_ID）
# 是运维腿：gateway 缺 id 会裸 exit(1) 进断路器（刀3 必修④），send 缺
# brain 键 401 空答。
#
# 已知缺口（R13 残余，接受）：L0 首启无语音问候（voice 是包，装上时
# uptime 已过闸）——问候收据归真人眼验（#198 同类）。
#
# 纪律同 m42c/n2：钉死 serial；秘密零回显——env 只查键名；busybox
# netstat 禁用（/proc/net/tcp）；设备杀进程 pkill -x 或按 pid。
set -euo pipefail

ACCEPT_DEVICE=redfin . "$(dirname "$0")/_serial.sh"  # SERIAL：env 最高，默认读 redfin 档案 [adb]
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT_HEX=20FB          # 8443 = 0x20FB（busybox netstat 必炸，走 /proc/net/tcp）
# L0 八包（刀4 定档）：五连 opt-in 的落地面——voice 带 asr/tts/ocr、
# gateway 带 secretd 全靠 depends；aginxbrowser 是基础清单裸二进制 opt
# 行（无配方、无依赖，缺席容忍单元 30s 拾取——svc.d=2 里它活着的理由）。
CORE8="aginx aginx-term aginx-voice aginx-asr aginx-tts aginx-ocr aginx-gateway aginx-secretd"
OPTIN5="aginx aginx-term aginx-voice aginxbrowser aginx-gateway"
UNITS6="aginx aginx-voice aginxbrowser net-watch aginx-secretd aginx-gateway"
N6_SCOPE=n6-l0-probe

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
expect_norc(){ [ "${DRV_RC:-}" != "0" ] && { echo "ok   - $1 (rc=${DRV_RC:-?})"; PASS=$((PASS+1)); } || { echo "FAIL - ${1}（rc=0，不该成功）"; FAIL=$((FAIL+1)); } }
expect_out() { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } }
expect_no()  { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "FAIL - ${1}（不该出现: ${2}）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } || { echo "ok   - $1"; PASS=$((PASS+1)); } }

wait_ready() { # name label tries
  local i
  for i in $(seq 1 "${3:-10}"); do
    drv "/usr/bin/aginx-svc status $1"
    printf '%s' "${DRV_OUT:-}" | grep -q "ready" && { echo "ok   - ${2}（ready）"; PASS=$((PASS+1)); return 0; }
    sleep 3
  done
  echo "FAIL - ${2}（未 ready）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -3)"; FAIL=$((FAIL+1)); return 1
}

wait_8443() { # label
  local i
  for i in $(seq 1 20); do
    drv "grep -q ':${PORT_HEX} 01 ' /proc/net/tcp /proc/net/tcp6 2>/dev/null"
    [ "${DRV_RC:-}" = "0" ] && { echo "ok   - ${1}（8443 ESTABLISHED）"; PASS=$((PASS+1)); return 0; }
    sleep 3
  done
  echo "FAIL - ${1}（8443 未 ESTABLISHED）"; FAIL=$((FAIL+1)); return 1
}

# 8 stamps 全落（C 段安装完成信号；opt-in 不写 boot.state，stamps 是唯一
# 落地面）。tries×sleeps 是给首拉 ~700MB 的窗（asr 239MB 贴 MEMBER_MAX）。
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
  echo "FAIL - ${1}（15min 窗内未齐，缺:${missing}）"; FAIL=$((FAIL+1)); return 1
}

phase_pre() {
  echo "==> 预检（出厂形状：底座/无路由器/dangling/无 wifi.conf/零 stamps）"
  adbx get-state >/dev/null 2>&1 || { echo "n6: device $SERIAL 不在线"; exit 1; }
  drv "grep -q ' l0\$' /etc/aginx-version"
  expect_rc  "版本戳尾 l0（L0 形戳，刀4）"
  drv "test ! -e /usr/bin/aginx"
  expect_rc  "路由器不烤（/usr/bin/aginx 无——母体在包里）"
  drv "ls /etc/aginx/svc.d | wc -l"
  expect_out "镜像 svc.d 恰 2 单元（net-watch+aginxbrowser）" '^2$'
  drv "ls /usr/libexec/aginx | wc -l"
  expect_out "libexec 恰三件（svcd/net-watch/net-rejoin）" '^3$'
  for b in aginx-svcd net-watch net-rejoin; do
    drv "test -x /usr/libexec/aginx/$b"; expect_rc "libexec 在：$b"
  done
  drv "test ! -e /usr/bin/aginx-voice && test ! -e /usr/bin/aginx-term && test ! -e /usr/libexec/aginx/aginx-server"
  expect_rc  "剥面不在：/usr/bin/aginx-{voice,term}、libexec/aginx-server"
  drv "ls -A /var/bin 2>/dev/null | wc -l"
  expect_out "var/bin 全空（0 件——face 待包装）" '^0$'
  drv "test -L /var/models/asr && test ! -e /var/models/asr && test -L /var/models/ocr && test ! -e /var/models/ocr"
  expect_rc  "models asr/ocr = dangling symlink（装包前 -e 恒假）"
  drv "test -L /var/models/tts/vits-melo-tts-zh_en && test ! -e /var/models/tts/vits-melo-tts-zh_en"
  expect_rc  "models tts = dangling symlink"
  drv "test ! -e /etc/wifi.conf"
  expect_rc  "出厂形状：无 /etc/wifi.conf（默认免 capture 实证）"
  drv "test ! -e /run/aginx-voice/face"
  expect_rc  "voice 从未起过（face 不存在）"
  drv "ls /var/lib/aginx/stamps 2>/dev/null | wc -l"
  expect_out "stamps 空（fresh L0 零包）" '^0$'

  echo "==> A 哑终端（pkg/svc 活、路由器死、清单见母体）"
  drv "aginx-pkg available | grep -qx aginx"
  expect_rc  "清单见母体（available 列 aginx = 刀6 镜像源闸的设备面）"
  drv "aginx commands >/dev/null 2>&1"
  expect_norc "aginx commands 失败（路由器未装——L0 无母体命令面）"
  drv "aginx agent send me"
  expect_norc "aginx agent send 失败（无母体 = 哑终端本相）"
  drv "/usr/bin/aginx-svc status aginx"
  # svcd 对未知单元的真面相是 'ERR no such unit'（bake #22 首跑实证），
  # absent 语义由此承载——两词都收，防 svcd 措辞再变。
  expect_out "母体单元 absent（未 opt-in）" '(absent|no such unit)'
  drv "test -x /usr/bin/aginx-pair"
  expect_rc  "配网 apply 面在（L0 件：voice 包装上即用）"
}

phase_paired() {
  echo "==> B 配网产物证（adb push wifi.conf + env 灌注后；秘密零回显）"
  adbx get-state >/dev/null 2>&1 || { echo "n6: device $SERIAL 不在线"; exit 1; }
  drv "grep -c '^ssid' /etc/wifi.conf"
  expect_out "wifi.conf 恰一行 ssid" '^1$'
  drv "grep -q '^internet ok' /run/boot.state"
  expect_rc  "boot.state internet ok"
  drv "grep -q '^AGINXBRAIN_API_KEY=' /etc/aginx/env"
  expect_rc  "env brain 键名在（值零回显——send 真答的前置）"
  drv "grep -q '^AGINX_GATEWAY_ID=' /etc/aginx/env"
  expect_rc  "env gateway id 键名在（gateway 缺 id 会裸 exit 断路器）"
  drv "[ \$(date +%Y) -ge 2026 ]"
  expect_rc  "钟到 2026（TLS 前置）"

  echo "==> C 显式 opt-in 五连（刀1 依赖感知设备面；15min 窗）"
  local p
  for p in $OPTIN5; do
    drv "aginx-pkg opt-in $p"
    expect_rc  "opt-in $p rc=0"
  done
  wait_core8_stamps "8 包 stamps 齐（voice 带 3 模型、gateway 带 secretd）" 60 15
  local n
  for n in $CORE8; do
    drv "test -f /var/lib/aginx/stamps/$n.version"
    expect_rc  "version stamp 在：$n"
    drv "test -e /var/bin/$n && test -f /var/bin/$n.aginxmd"
    expect_rc  "face + sidecar 在：${n}（#284 闭环）"
  done
  drv "test -d /var/models/asr && test -d /var/models/ocr"
  expect_rc  "模型树经 symlink 可达（dangling→真身）"
  drv "test -s /var/lib/aginx/pkgfiles/aginx-term/share/fonts/agterm-cjk.otf"
  expect_rc  "term 字体随包落 pkgfiles（cjk.rs 兜底路径）"
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
  drv "aginx commands >/dev/null"
  expect_rc  "路由器命令面活了（母体包装上）"
  drv "aginx agent send 现在几点了"
  expect_rc  "send rc=0（母体应答）"
  # 错误行本身是中文（「母体 brain 调用失败…」），[一-龥] 单查会误过——
  # 先证非错误前缀，再证有字。真中文回复=secret.policy pkgfiles 真身放行
  # 实证（刀3 必修①的活体判据，哑弹在此现形）。
  expect_no  "send 非报错行（无 aginx agent: 前缀）" '^aginx agent:'
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
  adbx get-state >/dev/null 2>&1 || { echo "n6: device $SERIAL 不在线"; exit 1; }
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
  # wifi/internet 行由 net-bringup phase 2 在 done 之后落（首跑实雷：UP_OK
  # 的 done+voice 门早于 phase 2 收笔）——有界等，5min。
  local WIFIOK=0 i
  for i in $(seq 1 60); do
    drv "grep -q '^wifi ok' /run/boot.state && grep -q '^internet ok' /run/boot.state"
    [ "${DRV_RC:-}" = "0" ] && { WIFIOK=1; break; }
    sleep 5
  done
  [ "$WIFIOK" = 1 ] && { echo "ok   - wifi 自动连 + internet ok（wifi.conf 持久实证）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - wifi/internet ok 未落（5min）"; FAIL=$((FAIL+1)); }
  # 全 opt 清单：provision 早退，pkg ok 应秒落（10min 窗是给异常的——
  # 秒级不到=有人把清单翻回了 core）。
  local PKG_OK=0 PKG_T0=$(date +%s)
  for i in $(seq 1 40); do
    drv "grep -q '^pkg ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && { PKG_OK=1; break; }
    sleep 15
  done
  [ "$PKG_OK" = 1 ] && { echo "ok   - pkg ok（全 opt 早退，$(( $(date +%s) - PKG_T0 ))s 落）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - pkg ok 未落（10min）"; FAIL=$((FAIL+1)); }
  drv "/usr/bin/aginx-svc list | grep -c ready"
  expect_out "六单元恰 ready（aginx/voice/browser/net-watch/secretd/gateway）" '^6$'
  drv "aginx agent send 现在几点了"
  expect_rc  "二启后母体仍应答"
  expect_no  "send 非报错行（无 aginx agent: 前缀）" '^aginx agent:'
  expect_out "二启后母体真回复" "[一-龥]"
  drv "aginx-pkg sync"
  expect_rc  "sync rc=0（全 opt=no-op，但签名链照验）"
  expect_no  "稳态零 downloading" 'downloading'
  # voice face 在 /var/bin（包面）——裸名走 PATH
  drv "aginx-voice --inject 你好; sleep 1; aginx-voice --face"
  expect_out "语音地板仍在（我在）" '我在'
}

phase_egg2() {
  echo "==> G capture 升级日（CAPTURE=1 刷机后；先跑 './flash-redfin.sh capture'）"
  adbx get-state >/dev/null 2>&1 || { echo "n6: device $SERIAL 不在线"; exit 1; }
  drv "grep -q ' l0\$' /etc/aginx-version"
  expect_rc  "还是 L0（版本戳尾 l0）"
  drv "test -e /etc/wifi.conf"
  expect_rc  "wifi.conf 从 state tar 回来（出厂本无此件 = state-restore 实证）"
  drv "grep -q '^wifi ok' /run/boot.state"
  expect_rc  "wifi 自动连（无需再推）"
  drv "test ! -e /var/bin/aginx"
  expect_rc  "母体脸被刷没（state tar 不带 pkgfiles——C9 排除集）"
  # 全 opt：provision 早退，不再 missing—downloading 重拉（蛋时代路径已死）
  local PKG_OK=0 i
  for i in $(seq 1 40); do
    drv "grep -q '^pkg ok' /run/boot.state"; [ "${DRV_RC:-}" = "0" ] && { PKG_OK=1; break; }
    sleep 15
  done
  [ "$PKG_OK" = 1 ] && { echo "ok   - pkg ok（全 opt 早退——不自动重拉）"; PASS=$((PASS+1)); } \
                    || { echo "FAIL - pkg ok 未落"; FAIL=$((FAIL+1)); return 0; }
  drv "grep -c 'downloading' /var/tmp/aginx-pkg-sync.log 2>/dev/null || echo 0"
  expect_out "sync 日志零 downloading（stamp 在脸不在也不拉）" '^0$'
  # 回脸=显式重跑（satisfied 判 stamp+face，脸缺即重下）；browser 裸件
  # 同法（stamp 在、脸缺=重拉 55MB）。
  local p
  for p in $OPTIN5; do
    drv "aginx-pkg opt-in $p"
    expect_rc  "opt-in 重跑 $p rc=0（回脸）"
  done
  wait_core8_stamps "8 包脸全回" 60 15
  local u
  for u in $UNITS6; do
    wait_ready "$u" "单元 $u" 20
  done
  drv "aginx agent send 现在几点了"
  expect_rc  "重灌后母体仍应答"
  expect_no  "send 非报错行（无 aginx agent: 前缀）" '^aginx agent:'
  expect_out "重灌后母体真回复" "[一-龥]"
  wait_8443   "重灌后网关重连"
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
  pre     刷完 L0 配置前（出厂形状 + 哑终端 + 清单见母体）
  paired  配网+灌 env 后（B 配网证 + C 显式 opt-in 五连 + D 等价 + E 点击装）
  steady  二启后（同像稳态：pkg ok 秒落 / 六单元 / send / 零 downloading）
  egg2    capture 升级日后（wifi.conf 回来 + opt-in 重跑回脸 + 等价复验）
USAGE
    exit 2 ;;
esac

echo
echo "n6-l0($1): $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
