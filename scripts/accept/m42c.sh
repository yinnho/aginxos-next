#!/usr/bin/env bash
# m42c acceptance — 命令优先协议 + 一眼自举（M42c④，设备侧可自动化面）。
#
# 真配对收据（fresh boot、无 adb、用户亲手举 AGINXPAIR1 码 → PairApply →
# 母体在线）属 #198 真人产品收据，**不进套件**——套件只管可自动化的腿：
#
#   预检  serial 钉死 / 设备在网（连网判定走 Up 快路）/ 铸码工具可构建
#   A 铸解  host aginx-pair 铸 fixture 配对码（套件自有身份，禁真秘密）
#          → 秘密卫生（stdout 不回显五件套）→ push → 设备 aginx-qr 解
#          → AGINXPAIR1 五段 payload round-trip（JPEG 进——设备只解 JPEG）
#   B 协议  --inject 你好（地板词表）/ --inject 状态 / --inject 连网（已
#          连网 → 「网已连」，不碰 wifi.conf）/ face 新 schema（无
#          list/psk 段、hint 带对码、state=idle）
#
# 纪律同 n5：钉死 serial；只读为主（face 快照 + EXIT 归还原位；/tmp 标记
# 自清）；秘密零回显——套件 fixture 是假身份，真秘密永远不进脚本。
set -euo pipefail

SERIAL="${ADB_SERIAL:-aginxosredfin}"
NROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FACE=/run/aginx-voice/face
FACEBAK=/tmp/m42c-face.bak
WORK="$(mktemp -d "${TMPDIR:-/tmp}/m42c-host.XXXXXX")"

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

cleanup() {
  drv "cp $FACEBAK $FACE 2>/dev/null; rm -f $FACEBAK /tmp/m42c-pair.jpg; true"
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "==> 预检（serial / 在网 / 铸码工具）"
adbx get-state >/dev/null 2>&1 || { echo "m42c: device $SERIAL 不在线"; exit 1; }
PAIR="$NROOT/target/release/aginx-pair"
if [ ! -x "$PAIR" ]; then
  echo "    构建 aginx-pair（release）…"
  (cd "$NROOT" && cargo build --release -p aginx-pair) || { echo "FAIL - aginx-pair 构建失败"; exit 1; }
fi
drv "ip -4 addr show wlan0 | grep -q inet"
expect_rc "设备在网（连网 inject 走 Up 快路，不碰 conf）"

echo "==> A 铸解（fixture 配对码：host 铸 → 设备解 → 五段 round-trip）"
MINT_OUT="$(cd "$WORK" && "$PAIR" \
  --ssid aginx-m42c-fixture \
  --psk fixture-psk-0123456789 \
  --brain-key sk-fixture-0000000000000000000000000000 \
  --gateway-id fixture0deadbeef \
  --relay-secret fixture-relay-secret \
  -o pair.jpg 2>&1 || true)"
printf '%s' "$MINT_OUT" | grep -q "wrote pair.jpg" \
  && { echo "ok   - aginx-pair 铸码（wrote pair.jpg）"; PASS=$((PASS+1)); } \
  || { echo "FAIL - aginx-pair 铸码"; echo "       out=$(printf '%s' "$MINT_OUT" | head -2)"; FAIL=$((FAIL+1)); }
# 秘密卫生生证：五件套任何一件出现在 stdout = 破律
printf '%s' "$MINT_OUT" | grep -Eq 'fixture-psk|sk-fixture|fixture0deadbeef|fixture-relay-secret|aginx-m42c-fixture' \
  && { echo "FAIL - 铸码 stdout 回显了身份字段（秘密卫生破律）"; FAIL=$((FAIL+1)); } \
  || { echo "ok   - 铸码 stdout 零回显五件套"; PASS=$((PASS+1)); }
adbx push "$WORK/pair.jpg" /tmp/m42c-pair.jpg >/dev/null
drv "/usr/bin/aginx-qr /tmp/m42c-pair.jpg"
expect_rc  "设备 aginx-qr 解配对码 rc=0"
expect_out "payload 五段 AGINXPAIR1 round-trip" \
  '^AGINXPAIR1\|aginx-m42c-fixture\|fixture-psk-0123456789\|sk-fixture-0000000000000000000000000000\|fixture0deadbeef\|fixture-relay-secret$'

echo "==> B 协议（命令优先 inject 冒烟：地板/状态/连网/face 新 schema）"
drv "cp $FACE $FACEBAK 2>/dev/null; ls $FACEBAK"
expect_rc "face 快照（EXIT 归还原位）"

drv "/usr/bin/aginx-voice --inject 你好; sleep 1; /usr/bin/aginx-voice --face"
expect_out "地板词表应答（我在）"            '我在'
expect_out "state=idle（无驻留态状态机）"    '"state":"idle"'
expect_no  "face 无 list 段（新 schema）"    '"list"'
expect_no  "face 无 psk 段（新 schema）"     '"psk"'
expect_out "提示语带对码（一眼自举）"        '对码'

drv "/usr/bin/aginx-voice --inject 状态; sleep 1; /usr/bin/aginx-voice --face"
expect_out "状态话术（电池）"                '电池'
expect_out "状态报网已连"                    '网已连'

drv "/usr/bin/aginx-voice --inject 连网; sleep 1; /usr/bin/aginx-voice --face"
expect_out "连网在已连网设备=网已连（不问不扫）" '网已连'
expect_no  "连网未触发表态流程（无对码话）"   '对准配对码'

echo
echo "m42c: $PASS passed, $FAIL failed（真配对收据属 #198：fresh boot 无 adb 举码）"
[ "$FAIL" = 0 ]
