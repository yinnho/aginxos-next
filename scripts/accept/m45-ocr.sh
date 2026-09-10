#!/usr/bin/env bash
# m45 acceptance — 本地 OCR（#148；S0 自老仓迁入，套件样式换新仓约定）。
# fixture：合成页（数值管线回归）+ 设备真盲拍暗房屏照（auto 旋转 + 光学链
# 回归——手机竖握、传感器横向安装，文字在图里转 90° 是产品常态）。光学
# 全流程（拍照→念出）收据在 docs/HARDWARE.md，不在本套（需要人持机）。
set -euo pipefail

ACCEPT_DEVICE=redfin . "$(dirname "$0")/_serial.sh"  # SERIAL：env 最高，默认读 redfin 档案 [adb]

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIXDIR="$ROOT/tools/ocr/fixtures"
MODELS="$ROOT/out/ocr/models"
SCRATCH=/var/tmp/accept-m45

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

expect_rc()  { [ "${DRV_RC:-}" = "$1" ] && { echo "ok   - $2"; PASS=$((PASS+1)); } || { echo "FAIL - $2 (rc=${DRV_RC:-?}, want $1)"; FAIL=$((FAIL+1)); } }
expect_out() { printf '%s' "${DRV_OUT:-}" | grep -Eq -- "$2" && { echo "ok   - $1"; PASS=$((PASS+1)); } || { echo "FAIL - $1"; echo "       out=$(printf '%s' "${DRV_OUT:-}" | head -2)"; FAIL=$((FAIL+1)); } }

# 前置：构建与模型（fetch-ocr-models.sh 产物）。验收不替人跑构建。
for f in "$ROOT/out/ocr/bin/ag-ocr" "$MODELS/det.onnx" "$MODELS/rec.onnx" \
         "$MODELS/dict.txt"; do
  test -f "$f" || { echo "FAIL: missing $f — scripts/build-ocr.sh + fetch-ocr-models.sh" >&2; exit 2; }
done

cleanup() {
  drv "rm -rf $SCRATCH"
}
trap cleanup EXIT

drv "rm -rf $SCRATCH && mkdir -p $SCRATCH/models"
adbx push "$ROOT/out/ocr/bin/ag-ocr" "$SCRATCH/ag-ocr" >/dev/null
adbx shell "chmod +x $SCRATCH/ag-ocr"
for f in det.onnx rec.onnx dict.txt; do
  adbx push "$MODELS/$f" "$SCRATCH/models/$f" >/dev/null
done
for f in page-synthetic.jpg cam-screen-dark.jpg line-zh.jpg plain-gray.jpg; do
  adbx push "$FIXDIR/$f" "$SCRATCH/$f" >/dev/null
done

OCR="AG_OCR_DIR=$SCRATCH/models $SCRATCH/ag-ocr"

# --- 1. 合成页：数值管线回归（det 分行 + rec 中英混排） ----------------------

drv "$OCR $SCRATCH/page-synthetic.jpg"
expect_rc 0 '合成页识别出文字（rc）'
expect_out '第一行机器视觉' '中文行'
expect_out 'Second line OCR test' '英文行'
expect_out '第三行 ?A123 ?B456' '中英混排行（CJK/拉丁界空格容差——quad 裁剪后 CTC 偶发插入）'

# --- 2. 真盲拍暗房屏照：auto 旋转 + 光学链 ------------------------------------
# 2016×1136 竖握实拍（gain16+dgain2 档，2026-09-04 收据）；文字在原图里
# 转 90°——auto 必须自己找到竖排朝向。首字符被屏边裁掉是图内事实。

drv "$OCR $SCRATCH/cam-screen-dark.jpg"
expect_rc 0 '盲拍屏照识别出（rc）'
expect_out '器视觉测试' '中文屏行（auto rot）'
expect_out '0013' '电话号码行'

# --- 3. 手工行条 --rec-only：跳 det 整行过 rec --------------------------------

drv "$OCR --rec-only $SCRATCH/line-zh.jpg"
expect_rc 0 '行条 rec-only（rc）'

# --- 4. 无字/错误语义 ----------------------------------------------------------
# agqr 约定：0=有字 / 1=没字 / 2=错误。

drv "$OCR $SCRATCH/plain-gray.jpg"
expect_rc 1 '无字图 rc=1（非错误）'

drv "$OCR $SCRATCH/missing.jpg"
expect_rc 2 '读图失败 rc=2'

echo "----"
echo "m45: $PASS pass, $FAIL fail"
[ "$FAIL" -eq 0 ]
