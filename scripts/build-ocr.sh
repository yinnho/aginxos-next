#!/usr/bin/env bash
# M45: ag-ocr 构建入口。
#   scripts/build-ocr.sh           设备版（NDK 静态链，out/ocr/bin/ag-ocr）
#   scripts/build-ocr.sh host      host 版（brew onnxruntime，数值验证用）
# 前置：scripts/fetch-ocr-models.sh 已拉模型到 out/ocr/models/。
# 模型规整名（det.onnx/rec.onnx/dict.txt）由本脚本铺——ag-ocr.c 只认规整名，
# fetch 脚本保存原名（ModelScope 文件名），单一换名点在这里。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
M="${ROOT}/out/ocr/models"

# 1. 模型规整名（幂等：缺失或大小变了才拷）
src_for() {
  case "$1" in
    det.onnx)  echo "ch_PP-OCRv5_det_mobile.onnx" ;;
    rec.onnx)  echo "ch_PP-OCRv5_rec_mobile.onnx" ;;
    dict.txt)  echo "ppocrv5_dict.txt" ;;
    *) return 1 ;;
  esac
}
for dst in det.onnx rec.onnx dict.txt; do
  src="$(src_for "$dst")"
  test -f "${M}/${src}" || { echo "model missing: ${M}/${src} — run scripts/fetch-ocr-models.sh" >&2; exit 1; }
  if [ ! -f "${M}/${dst}" ] || ! cmp -s "${M}/${src}" "${M}/${dst}"; then
    cp -f "${M}/${src}" "${M}/${dst}"
  fi
done

if [ "${1:-}" = "host" ]; then
  # host 数值验证版：共享库直链（shims 是 android 专供，host 不链）
  # brew 的头在 include/onnxruntime/ 嵌套层里
  ORT_INC="/opt/homebrew/opt/onnxruntime/include/onnxruntime"
  test -f "${ORT_INC}/onnxruntime_c_api.h" \
    || { echo "brew onnxruntime 头不在 ${ORT_INC} — brew install onnxruntime" >&2; exit 1; }
  mkdir -p "${ROOT}/out/ocr/bin"
  cc -O2 -I"${ROOT}/tools/ocr" -I"${ORT_INC}" \
    -o "${ROOT}/out/ocr/bin/ag-ocr-host" "$ROOT/tools/ocr/ag-ocr.c" \
    -L/opt/homebrew/opt/onnxruntime/lib -lonnxruntime
  file "${ROOT}/out/ocr/bin/ag-ocr-host"
  echo HOST-OK
  exit 0
fi

# 2. ORT 静态树：复用 voice 工艺链的下载（不在则同源补拉）
V="${ROOT}/out/voice"
S="${V}/sherpa-onnx-src"
if ! find "$S/ort" -name libonnxruntime.a 2>/dev/null | grep -q .; then
  mkdir -p "$S/ort"
  (cd "$S/ort" \
    && gh release download v1.27.1 --repo csukuangfj/onnxruntime-libs \
       --pattern 'onnxruntime-android-arm64-v8a-static_lib-1.27.1.zip' \
    && unzip -q onnxruntime-android-arm64-v8a-static_lib-1.27.1.zip)
fi

# 3. 终链
bash "${ROOT}/tools/ocr/link-ocr.sh"
echo "ocr build complete: ${ROOT}/out/ocr/bin/ag-ocr"
