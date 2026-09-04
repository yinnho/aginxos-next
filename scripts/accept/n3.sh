#!/usr/bin/env bash
# n3 acceptance — aginx-server 并存包上机（N3：新心脏以 agpkg 包形态住进
# 现役镜像，与老 carrier 线同机共存，真设备日常验证）。
#
# 包 = 四件套 files/ 树形态（build-n3-package.sh 产物）：aginx-server /
# aginx / aginx-runtime / voiced + tools/ 四个薄壳。安装面
# /var/bin/aginx-server → 包树；单元 /var/lib/agpkg/units/aginx-server.toml
# 开机自起；AGINX_HOME=/home/.aginx-n（两线并行，老线 ~/.aginx 不碰）。
# 裸名让名：/var/bin/aginx 仍是老 relay（D13 改名归 N4 bake）。
#
#   A 安装自起：agpkg install（本地显式路径）→ 面具/树/单元/socket
#   B 老线无恙：relay/carrier 在跑，/var/bin/aginx 未动
#   C 发现面：commands 见 4 工具；引擎不进命令宇宙（D13）
#   D 母体对话：agent send → 真 brain 回复
#   E 工具真回路：母体调 aginx-sys-status 报电池
#   F voiced 翻面：手写覆盖单元 → 树 voiced + VOICED_FRONT；封闭词表仍本地
#   G 重启自起：reboot2 → aginx-server 自起 + 老线全起 + send 再通
#
# 终态：包驻留（日常形态）。纪律同 n2b：钉死 serial；只碰
# /home/.aginx-n、/var/lib/agpkg、/tmp；/run/voice/face 快照还原
# （重启后归新 voiced 所有，不再还原）。
set -euo pipefail

SERIAL="${ADB_SERIAL:-aginxosredfin}"
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TAR="${N3_TAR:-$NROOT/out/aginx-server/aginx-server-v0.1.0-4pc.tar}"
SHA256="${N3_SHA:-$(shasum -a 256 "$TAR" | cut -d' ' -f1)}"

TREE=/var/lib/agpkg/pkgfiles/aginx-server
SOCK=/run/aginx.sock
FACE=/run/voice/face
FACEBAK=/tmp/n3-face.bak
UNITS=/var/lib/agpkg/units
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

# agsvc 单元 starting→ready 有秒级窗口：轮询等 ready，不赌一枪。
wait_ready() { # name label tries
  local i
  for i in $(seq 1 "${3:-10}"); do
    drv "/usr/bin/agctl status $1"
    printf '%s' "${DRV_OUT:-}" | grep -q "ready" && { echo "ok   - $2（ready）"; PASS=$((PASS+1)); return 0; }
    sleep 1
  done
  echo "FAIL - $2（未 ready）"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -3)"; FAIL=$((FAIL+1)); return 1
}

cleanup() {
  # 终态=包驻留：不卸载。重启前面还原；重启后 face 归新 voiced，不碰。
  if [ "$REBOOTED" = "0" ]; then
    drv "cp $FACEBAK $FACE 2>/dev/null; true"
  fi
  drv "rm -f $FACEBAK /tmp/n3-server.tar; true"
}
trap cleanup EXIT

[ -f "$TAR" ] || { echo "n3: tar missing at $TAR — ./scripts/build-n3-package.sh first"; exit 1; }
tar -tf "$TAR" | grep -q '^files/tools/aginx-sys-status$' \
  || { echo "n3: tar 形状不对（缺 files/tools/）— 重跑 build-n3-package.sh"; exit 1; }

echo "==> 预检（serial 钉死 / agpkg 在 / key 键名在）"
adbx get-state >/dev/null 2>&1 || { echo "n3: device $SERIAL 不在线"; exit 1; }
drv "ls /usr/bin/agpkg"
expect_rc "agpkg 就位（/usr/bin/agpkg）"
drv "grep -c \"^$KEYVAR=\" /etc/aginx/env"
expect_out "brain key 键名在 /etc/aginx/env（只数行不回显）" '^1$'

echo "==> A 安装自起（本地显式路径安装，无 manifest 签名）"
adbx push "$TAR" /tmp/n3-server.tar >/dev/null
drv "/usr/bin/agpkg install aginx-server /tmp/n3-server.tar $SHA256"
expect_rc "agpkg install aginx-server"
drv "sleep 1; ls -l /var/bin/aginx-server"
expect_out "面具 = 符号链接进包树"  "pkgfiles/aginx-server"
drv "ls $TREE/aginx-server $TREE/aginx $TREE/aginx-runtime $TREE/voiced $TREE/tools/aginx-sys-status"
expect_rc "树内五件齐（含 tools/ 壳）"
drv "cat $UNITS/aginx-server.toml"
expect_out "单元落位（cmd=面具）"      "cmd = \"/var/bin/aginx-server\""
expect_out "单元隔离 HOME"            "AGINX_HOME=/home/.aginx-n"
expect_out "单元 env_file 进 brain key" "env_file"
wait_ready aginx-server "install→reload→自起" 15
drv "ls -l $SOCK"
expect_rc "server 起在默认 $SOCK"

echo "==> B 老线无恙（并存不是顶替）"
drv "/usr/bin/agctl list"
expect_out "老 relay（aginx）仍 ready"  "^aginx[[:space:]].*ready"
expect_out "老 carrier 仍 ready"        "aginx-carrier.*ready"
drv "ls -l /var/bin/aginx"
expect_out "/var/bin/aginx 未动（老 relay 二进制，非符号链接）" "^-rwx"

echo "==> C 发现面（D13：引擎不进命令宇宙）"
drv "AGINX_CMD_PATH=$TREE/tools $TREE/aginx commands"
expect_out "4 工具被路由器发现（菜单列去前缀名）" "sys-status"
expect_out "工具组按 aginx:group 落"  "net"
expect_no  "引擎混进命令宇宙"         "aginx-server|aginx-runtime"

echo "==> D 母体对话（默认 socket，真 brain）"
drv "$TREE/aginx agent send 用一句话介绍AginxOS"
expect_rc "agent send rc=0"
expect_out "母体真回复（含 AginxOS）" "AginxOS"

echo "==> E 工具真回路（母体调薄壳工具；多词文本要整体引号，否则首词会被当化身名）"
drv "$TREE/aginx agent send \"用 sys-status 查一下电池还剩多少，把电量数字告诉我\""
expect_rc "send rc=0"
expect_out "回复含电池读数（% 或 电）" "电|%"

echo "==> F voiced 翻面（手写覆盖单元 → 树 voiced + VOICED_FRONT）"
drv "cp $FACE $FACEBAK 2>/dev/null; ls $FACEBAK"
expect_rc "生产 face 快照（翻面收据后、重启前还原）"
drv "printf '%s\n' '[unit]' 'name = voiced' '' '[service]' 'cmd = $TREE/voiced' 'type = simple' 'envs = [\"VOICED_FRONT=$TREE/aginx\"]' 'env_file = /etc/aginx/env' > $UNITS/voiced.toml && cat $UNITS/voiced.toml"
expect_out "覆盖单元写好" "VOICED_FRONT=$TREE/aginx"
drv "/usr/bin/agctl reload >/dev/null; sleep 2"
wait_ready voiced "reload 后 voiced 换树版" 15
drv "pgrep -a voiced"
expect_out "在跑 voiced 已是树内件" "pkgfiles/aginx-server/voiced"
drv "$TREE/voiced --inject 你好; sleep 1"
expect_rc "封闭词表：inject 你好 rc=0"
drv "$TREE/voiced --face"
expect_out "封闭词表仍本地直答（我在）" "我在"
drv "$TREE/voiced --inject 用一句话介绍AginxOS; sleep 3"
expect_rc "自由文本 inject rc=0"
drv "$TREE/voiced --face"
expect_no  "不是兜底话"    "连不上母体"
expect_out "脸上是母体真回复" "AginxOS"
drv "cp $FACEBAK $FACE"
expect_rc "face 还原（重启后归树 voiced 所有）"

echo "==> G 重启自起（reboot2；adb reboot 会挂）"
REBOOTED=1
drv "/bin/reboot2 reboot" || true
echo "    等设备回来…"
for i in $(seq 1 60); do adbx get-state >/dev/null 2>&1 && break; sleep 5; done
adbx get-state >/dev/null 2>&1 || { echo "FAIL - 设备未回来"; FAIL=$((FAIL+1)); }
up=0
for i in $(seq 1 36); do
  drv "/usr/bin/agctl list" && [ "${DRV_RC:-}" = "0" ] && { up=1; break; }
  sleep 5
done
[ "$up" = "1" ] || { echo "FAIL - agsvc 未就绪"; FAIL=$((FAIL+1)); }
# 网络自愈 + 钟闸门：TLS 吃时钟，net-bringup 的一次性 ntpd 跟 Wi-Fi 竞速
# （这次收据实测：net-watch 段错误循环 → Wi-Fi 迟到 → ntpd 输 → 1970 →
# TLS 全死）。等网回来，钟没跟上就补一枪 ntpd——这是老线启动缺口，
# 归 #112/N4，不是新链的病。
for i in $(seq 1 18); do
  drv "ip -4 addr show wlan0 | grep -q inet"
  [ "${DRV_RC:-}" = "0" ] && break
  sleep 5
done
clocked=0
for i in $(seq 1 12); do
  drv "[ \$(date +%Y) -ge 2026 ]"
  [ "${DRV_RC:-}" = "0" ] && { clocked=1; break; }
  sleep 5
done
if [ "$clocked" != "1" ]; then
  drv "ntpd -q -n -p ntp.aliyun.com 2>/dev/null; [ \$(date +%Y) -ge 2026 ]"
  expect_rc "钟闸门补枪后到 2026（老线一次性 ntpd 竞速缺口）"
else
  echo "ok   - 钟闸门（开机自校到位）"; PASS=$((PASS+1))
fi
drv "/usr/bin/agctl list"
expect_out "aginx-server 开机自起"      "aginx-server.*ready"
expect_out "老 relay 自起"              "^aginx[[:space:]].*ready"
expect_out "老 carrier 自起"            "aginx-carrier.*ready"
expect_out "voiced 按覆盖单元自起"      "voiced.*ready"
drv "pgrep -a voiced"
expect_out "重启后 voiced 仍是树内件"   "pkgfiles/aginx-server/voiced"
drv "ls -l $SOCK"
expect_rc "socket 回来了"
drv "$TREE/aginx agent send 现在几点了"
expect_rc "重启后母体仍应答（rc=0）"
expect_out "重启后母体真回复（非空有字）" "[一-龥]"

echo
echo "n3: $PASS passed, $FAIL failed（终态：包驻留，日常形态）"
[ "$FAIL" = 0 ]
