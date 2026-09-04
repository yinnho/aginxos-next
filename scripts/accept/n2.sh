#!/usr/bin/env bash
# n2 acceptance — 平台心脏上机并行试跑（N2①，docs/ARCH.md 宪法 D4–D12）。
#
# 在现役设备上以隔离树并行验证新仓三件（aginx / aginx-server /
# aginx-runtime）：不碰老 carrier 的 ~/.aginx（宪法两线并行），不注册
# 单元（那是 N3 agpkg 的事），不进 PATH（避免与老 relay 的 aginx 撞名，
# 一律显式路径调用）。
#
# 全生命周期收据 = host N1⑦ 同款：母体直答 → 进（建化身）→ 点名+真工具
# （D12：spawn 路由器）→ 住（跨轮记性）→ 切 → 退房词回母体 → 杀进程
# 冷重启（光标回 me + 会话账恢复）。真 brain（brain.aginx.net）。
#
# 纪律：adb 钉死实验机 serial；busybox 无 awk（sed/set--）；脚本对设备的
# 写全部限定在 /home/.aginx-n 隔离树 + /tmp/aginx-n.sock；结束杀掉试跑
# server，设备回到进场状态（老线不受影响）。
set -euo pipefail

SERIAL="${ADB_SERIAL:-aginxosredfin}"
TREE=/home/.aginx-n                      # 隔离根：bin/ cmds/ workspaces/
SOCK=/tmp/aginx-n.sock
KEYVAR=AGINXBRAIN_API_KEY
BIN_DIR="$(cd "$(dirname "$0")/../.." && pwd)/target/aarch64-unknown-linux-musl/release"

PASS=0
FAIL=0

adbx() { adb -s "$SERIAL" "$@"; }

# drv：设备上跑一句，回显里剥 bionic linker 噪声；DRV_RC/DRV_OUT 收结果。
drv() {
  local raw
  raw="$(adbx shell "export HOME=/home PATH=/usr/bin:/bin:/sbin:/var/bin; $1; echo __RC=\$?" 2>&1 || true)"
  raw="${raw//$'\r'/}"
  DRV_RC="$(printf '%s\n' "$raw" | sed -n 's/^__RC=//p' | tail -1)"
  DRV_OUT="$(printf '%s\n' "$raw" | grep -vE '^(libc:|linker|WARNING)' | grep -v '^__RC=' || true)"
}

expect_rc()  { [ "${DRV_RC:-}" = "0" ] && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1 (rc=${DRV_RC:-?})"; FAIL=$((FAIL+1)); } }
expect_out() { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } }

cleanup() {
  # 设备回进场状态：杀试跑 server、删 socket；隔离树留着（下轮开跑会清）
  drv "kill \$(cat $TREE/server.pid 2>/dev/null) 2>/dev/null; rm -f $TREE/server.pid $SOCK; true"
}
trap cleanup EXIT

[ -x "$BIN_DIR/aginx" ] && [ -x "$BIN_DIR/aginx-server" ] && [ -x "$BIN_DIR/aginx-runtime" ] \
  || { echo "n2: musl binaries missing under $BIN_DIR — cargo zigbuild first"; exit 1; }

echo "==> push 三件 + 隔离树起手"
drv "rm -rf $TREE && mkdir -p $TREE/bin $TREE/cmds"
adbx push "$BIN_DIR/aginx"         "$TREE/bin/aginx"         >/dev/null
adbx push "$BIN_DIR/aginx-server"  "$TREE/bin/aginx-server"  >/dev/null
adbx push "$BIN_DIR/aginx-runtime" "$TREE/bin/aginx-runtime" >/dev/null
drv "chmod +x $TREE/bin/aginx $TREE/bin/aginx-server $TREE/bin/aginx-runtime"
expect_rc "三件就位（exec 位补上）"

# 试跑工具面：一个 sh 回声命令（真工具派发走它；真包工具 N3 接）
drv "printf '#!/bin/sh\n# aginx:summary=回声（N2 试跑）\nprintf \"echo: %%s\\\\n\" \"\$*\"\n' > $TREE/cmds/aginx-dev-echo && chmod +x $TREE/cmds/aginx-dev-echo"
expect_rc "aginx-dev-echo 工具落位"

# launcher：key 从 /etc/aginx/env 单行取（不整读文件），其余全是显式路径
drv "printf '#!/bin/sh\nset -eu\nexport %s=\$(grep \"^%s=\" /etc/aginx/env | sed \"s/^[^=]*=//\")\nexport AGINX_HOME=/home/.aginx-n AGINX_SOCK=$SOCK\nexport AGINX_BIN=$TREE/bin/aginx AGINX_RUNTIME_BIN=$TREE/bin/aginx-runtime\nexport AGINX_CMD_PATH=$TREE/cmds\nexec $TREE/bin/aginx-server\n' $KEYVAR $KEYVAR > $TREE/bin/n2-launch.sh && chmod 700 $TREE/bin/n2-launch.sh"
expect_rc "launcher 落位（key 只进 env 不进参数表）"

start_server() {
  drv "nohup $TREE/bin/n2-launch.sh > $TREE/server.log 2>&1 & echo \$! > $TREE/server.pid; sleep 1; ls -l $SOCK"
  expect_rc "server 起在 $SOCK"
}
stop_server() {
  drv "kill \$(cat $TREE/server.pid) 2>/dev/null; sleep 1; rm -f $SOCK; true"
}

send() { # send <期望标记> <断言grep> <args…>
  local label="$1" pat="$2"; shift 2
  drv "export AGINX_SOCK=$SOCK AGINX_CMD_PATH=$TREE/cmds; $TREE/bin/aginx agent $*"
  expect_out "$label" "$pat"
}

echo "==> 1 起服 + 母体直答（真 brain）"
start_server
send "母体直答" "me|母体|AginxOS" send me 你好，母体。一句话介绍你自己。

echo "==> 2 进 + 点名 + 真工具（D12 派发）"
drv "export AGINX_SOCK=$SOCK AGINX_CMD_PATH=$TREE/cmds; $TREE/bin/aginx agent create 小满 你是小满，安静细致，说话简短。"
expect_rc "create 小满"
send "点名+工具往返" "平台心脏跳动了" send 小满 请调用 aginx-dev-echo 工具喊一句：平台心脏跳动了。把工具输出原样告诉我。
send "住：跨轮记性" "平台心脏跳动了" send 我刚才让你喊的那句话原话是什么？

echo "==> 3 切 + 退房词"
drv "export AGINX_SOCK=$SOCK AGINX_CMD_PATH=$TREE/cmds; $TREE/bin/aginx agent create 阿澈 你是阿澈，直性子，回答不超过两句。"
expect_rc "create 阿澈"
send "切到阿澈" "阿澈" send 阿澈 一句话自我介绍。
send "退房词回母体" "已回到母体" send 再见

echo "==> 4 账本形状（每轮收口）"
drv "wc -l < $TREE/workspaces/小满/sessions/main.jsonl"
expect_out "小满账本轮次收口（6 帧：req+call+res+done + req+done）" "^6$"

echo "==> 5 冷重启（光标回 me + 会话恢复）"
stop_server
start_server
send "冷启后光标=母体" "前台：母体" status
send "冷启后小满记性（D8 账本重放）" "平台心脏跳动了" send 小满 服务器刚重启过。你还记得你之前用工具喊过的那句话吗？原话告诉我。

echo "==> 6 收场"
stop_server
drv "ls $TREE/workspaces"
expect_out "隔离树双化身在册" "阿澈|小满"

echo
echo "n2: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
