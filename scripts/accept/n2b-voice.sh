#!/usr/bin/env bash
# n2b acceptance — voiced 接新前台（N2②：ASR 自由文本 → aginx-server，
# 封闭词表 = 离线地板）。
#
# voiced 源码在老仓（~/Documents/aginxos，两线并行纪律），但收据是
# 新链的：voiced-n（试跑名，不碰 /var/bin/voiced 在跑件）在
# VOICED_FRONT 指向新路由器时，把封闭词表 miss 的自由文本改投
# `aginx agent send`（母体/光标，AGINX_SOCK 找 server）。四段收据：
#
#   A 封闭词表离线地板：server 不在，你好 → 我在（本地，零 brain 往返）
#   B 前台不可达兜底：server 不在，自由文本 → 「连不上母体」地板话
#   C 自由文本 → 母体：server 在，问题 → 脸上出现母体真回复（真 brain）
#   D 真耳环回：ag-tts 合成 wav → voiced-n --hear（本地 ASR）→
#     ASR 文本喂 --inject → 封闭词表命中（扫描网络）——M42c 同款
#     全流程的 CLI 可达段；完整 PTT 实机收据归用户
#
# 纪律同 n2.sh：钉死 serial；只写 /home/.aginx-n 与 /tmp/aginx-n.sock；
# /run/voice/face 是在跑 voiced 的脸——先快照、收据后原样还原；
# 结束杀试跑 server、验老线 voiced 还活着，设备回进场状态。
set -euo pipefail

SERIAL="${ADB_SERIAL:-aginxosredfin}"
TREE=/home/.aginx-n
SOCK=/tmp/aginx-n.sock
KEYVAR=AGINXBRAIN_API_KEY
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN_DIR="$NROOT/target/aarch64-unknown-linux-musl/release"
# voiced musl 产物在老仓（源码在那边）；可用 env 覆写
VOICED_MUSL="${VOICED_MUSL:-$NROOT/../aginxos/target/aarch64-unknown-linux-musl/release/voiced}"

PASS=0
FAIL=0

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

FACE=/run/voice/face
FACEBAK=/tmp/n2b-face.bak

cleanup() {
  # 设备回进场状态：杀试跑 server、还原在跑 voiced 的脸、删临时 wav
  drv "cp $FACEBAK $FACE 2>/dev/null; rm -f $FACEBAK /tmp/n2b-ear.wav; kill \$(cat $TREE/server.pid 2>/dev/null) 2>/dev/null; rm -f $TREE/server.pid $SOCK; true"
}
trap cleanup EXIT

[ -x "$BIN_DIR/aginx" ] && [ -x "$BIN_DIR/aginx-server" ] && [ -x "$BIN_DIR/aginx-runtime" ] \
  || { echo "n2b: musl trio missing under $BIN_DIR — cargo zigbuild first"; exit 1; }
[ -x "$VOICED_MUSL" ] || { echo "n2b: voiced musl missing at $VOICED_MUSL — 老仓 ./scripts/build-phone.sh musl voiced"; exit 1; }

echo "==> push 四件（三件 + voiced-n 试跑名）+ 隔离树起手"
drv "rm -rf $TREE && mkdir -p $TREE/bin $TREE/cmds"
adbx push "$BIN_DIR/aginx"         "$TREE/bin/aginx"         >/dev/null
adbx push "$BIN_DIR/aginx-server"  "$TREE/bin/aginx-server"  >/dev/null
adbx push "$BIN_DIR/aginx-runtime" "$TREE/bin/aginx-runtime" >/dev/null
adbx push "$VOICED_MUSL"           "$TREE/bin/voiced-n"      >/dev/null
drv "chmod +x $TREE/bin/aginx $TREE/bin/aginx-server $TREE/bin/aginx-runtime $TREE/bin/voiced-n"
expect_rc "四件就位（exec 位补上；voiced 用试跑名，/var/bin/voiced 未动）"

# launcher：key 从 /etc/aginx/env 单行取（不整读文件），其余全是显式路径
drv "printf '#!/bin/sh\nset -eu\nexport %s=\$(grep \"^%s=\" /etc/aginx/env | sed \"s/^[^=]*=//\")\nexport AGINX_HOME=/home/.aginx-n AGINX_SOCK=$SOCK\nexport AGINX_BIN=$TREE/bin/aginx AGINX_RUNTIME_BIN=$TREE/bin/aginx-runtime\nexport AGINX_CMD_PATH=$TREE/cmds\nexec $TREE/bin/aginx-server\n' $KEYVAR $KEYVAR > $TREE/bin/n2-launch.sh && chmod 700 $TREE/bin/n2-launch.sh"
expect_rc "launcher 落位（key 只进 env 不进参数表）"

# 在跑 voiced 的脸：快照，收据完原样还原
drv "cp $FACE $FACEBAK 2>/dev/null; ls $FACEBAK"
expect_rc "生产 face 快照（收据后还原）"

start_server() {
  drv "nohup $TREE/bin/n2-launch.sh > $TREE/server.log 2>&1 & echo \$! > $TREE/server.pid; sleep 1; ls -l $SOCK"
  expect_rc "server 起在 $SOCK"
}
stop_server() {
  drv "kill \$(cat $TREE/server.pid 2>/dev/null) 2>/dev/null; sleep 1; rm -f $SOCK; true"
}

# voiced-n 的前台模式开关 + socket 指向，一个 env 前缀复用
VENV="VOICED_FRONT=$TREE/bin/aginx AGINX_SOCK=$SOCK"

# 读脸（用试跑 voiced 自己的 --face，顺带验它的读面）
face_json() { drv "$VENV $TREE/bin/voiced-n --face"; }

echo "==> A 封闭词表离线地板（server 未起，零 brain 往返）"
drv "$VENV $TREE/bin/voiced-n --inject 你好"
expect_rc "inject 你好 rc=0"
face_json
expect_out "你好→我在（本地状态机直答）" "我在"

echo "==> B 前台不可达兜底（server 未起，自由文本落地板话）"
drv "$VENV $TREE/bin/voiced-n --inject 今天北京天气怎么样"
expect_rc "inject 自由文本 rc=0（兜底不崩）"
face_json
expect_out "自由文本→连不上母体地板话" "连不上母体"

echo "==> C 自由文本 → 母体（server 在，真 brain）"
start_server
drv "$VENV $TREE/bin/voiced-n --inject 用一句话介绍AginxOS"
expect_rc "inject 介绍AginxOS rc=0"
face_json
expect_no  "不是兜底话"                     "连不上母体"
expect_no  "不是没听懂地板"                  "没听懂"
expect_out "脸上是母体真回复（含 AginxOS）"   "AginxOS"

echo "==> D 真耳环回（ag-tts 合成 → 本地 ASR → 词表命中）"
drv "/var/bin/ag-tts 连接无线网络 /tmp/n2b-ear.wav"
expect_rc "ag-tts 合成耳环回 wav"
drv "$VENV $TREE/bin/voiced-n --hear /tmp/n2b-ear.wav"
expect_out "本地 ASR 认出网络词" "无线|网络|wi.?fi"
ASR_TEXT="$(printf '%s' "${DRV_OUT:-}" | head -1)"
echo "    ASR 文本: $ASR_TEXT"
drv "$VENV $TREE/bin/voiced-n --inject \\\"$ASR_TEXT\\\""
expect_rc "ASR 文本喂 inject rc=0"
face_json
expect_out "真耳文本命中封闭词表（扫描网络）" "扫描网络"

echo "==> E 收场（老线无恙 + face 还原 + server 停）"
drv "pgrep voiced"
expect_out "在跑 voiced 未被触碰" "[0-9]"
stop_server
cleanup
drv "pgrep voiced"
expect_out "收场后老线 voiced 仍活" "[0-9]"

echo
echo "n2b: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
