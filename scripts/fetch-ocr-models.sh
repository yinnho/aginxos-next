#!/usr/bin/env bash
# M45: OCR 模型再取 —— ModelScope RapidAI/RapidOCR tag v3.9.2 直拉
# （中国网络直连快；gh 无此仓库），摆成烤盘形状 out/ocr/models/。
#   det = PP-OCRv5 mobile det（DB，fp onnx ~4.9MB）
#   rec = PP-OCRv5 mobile rec（CTC 多语 zh简繁+en+jp+pinyin，~16.6MB）
#   dict = ppocrv5_dict.txt（18,378 字符，行=类目）
# 模型不入仓；out/ 已 gitignore。幂等：在位即跳过。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
M="${ROOT}/out/ocr/models"
DL="${ROOT}/out/ocr/dl"
BASE="https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2"

# path | sha256
ITEMS=(
  "onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx|4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae"
  "onnx/PP-OCRv5/rec/ch_PP-OCRv5_rec_mobile.onnx|5825fc7ebf84ae7a412be049820b4d86d77620f204a041697b0494669b1742c5"
  "paddle/PP-OCRv5/rec/ch_PP-OCRv5_rec_mobile/ppocrv5_dict.txt|d1979e9f794c464c0d2e0b70a7fe14dd978e9dc644c0e71f14158cdf8342af1b"
)

mkdir -p "${M}" "${DL}"

for item in "${ITEMS[@]}"; do
  path="${item%%|*}"; want="${item##*|}"
  name="$(basename "${path}")"
  dest="${M}/${name}"
  [ -s "${dest}" ] && { echo "have ${name}"; continue; }
  echo "fetch ${name} ..."
  curl -fSL --retry 3 -o "${DL}/${name}" "${BASE}/${path}"
  if [ "${want}" != "SKIP" ]; then
    got="$(shasum -a 256 "${DL}/${name}" | cut -d' ' -f1)"
    [ "${got}" = "${want}" ] || { echo "sha256 mismatch ${name}: ${got}" >&2; exit 1; }
    echo "sha256 ok ${name}"
  fi
  mv "${DL}/${name}" "${dest}"
done

rm -rf "${DL}"
ls -la "${M}"
echo "ocr models ready under ${M}"
