#!/usr/bin/env bash
# build-pkg.sh — aginx 体系包打包线（C8 蛋案）。
#
# usage: scripts/build-pkg.sh <name> [--push <serial>]
#
#   四裸包  aginx-gateway aginx-secretd aginx-voice
#                       — zigbuild musl 件 + pkgs/<name>/ 配方
#   一树包  aginx        — 母体三件（router/server/runtime，exec=bin/aginx，
#                         [service] 指 pkgfiles 真身；刀3 合一）
#   三树包  aginx-asr aginx-tts aginx-ocr
#                       — .local/device/redfin 冻结 bionic 件 + 模型一树
#                         （exec=bin/ag-*，模型与整机烤机逐字节同源：asr
#                          全树；tts=vits-melo 去 133B lfs 指针；ocr=
#                          det/rec/dict 三件）
#
# 产物 out/pkgs/<name>-v<ver>-4pc.tar + .sha256（裸 hex），尾行打一行
# manifest 片段（name url sha core version [deps]）——C10 EGG 清单组装
# 直接收走。`--push <serial>` = adb push + `aginx-pkg install`（显式路径
# = dev 免签通道）。镜像发布保持手工 scp（sync.sh 留服务器端）。
#
# MEMBER_MAX 闸：安装器 stream_member 单成员上限 256MiB（268435456B，
# crates/pkg 的 MEMBER_MAX）。asr 的 model.int8.onnx 239,233,841B 贴顶——
# 单成员 >240,000,000B 报警（预案：提 MEMBER_MAX 或拆包）；≥256MiB 拒打。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

usage() {
  echo "usage: scripts/build-pkg.sh <name> [--push <serial>]" >&2
  exit 2
}

[ $# -ge 1 ] || usage
PKG="$1"; shift
SERIAL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --push) [ $# -ge 2 ] || usage; SERIAL="$2"; shift 2 ;;
    *) usage ;;
  esac
done

RECIPE="pkgs/${PKG}"
[ -f "${RECIPE}/pkg.toml" ] || { echo "FATAL: no recipe ${RECIPE}/pkg.toml" >&2; exit 1; }
[ -f "${RECIPE}/SKILL.md" ] || { echo "FATAL: no ${RECIPE}/SKILL.md（四件套铁律）" >&2; exit 1; }

# pkg.toml 是自家扁平格式，值全带引号无内嵌引号——sed 取串够用
toml_str() {
  sed -n "s/^$1 *= *\"\([^\"]*\)\"/\1/p" "${RECIPE}/pkg.toml" | sed -n '1p'
}

NAME="$(toml_str name)"
VER="$(toml_str version)"
URL="$(toml_str url)"
DEPS="$(toml_str depends)"
[ "${NAME}" = "${PKG}" ] || { echo "FATAL: pkg.toml name='${NAME}' != '${PKG}'" >&2; exit 1; }
[ -n "${VER}" ] || { echo "FATAL: ${RECIPE}/pkg.toml 缺 version" >&2; exit 1; }
[ -n "${URL}" ] || { echo "FATAL: ${RECIPE}/pkg.toml 缺 url（C10 EGG 清单组装要用）" >&2; exit 1; }

TARGET_DIR="${ROOT}/target/aarch64-unknown-linux-musl/release"
VOICE="${ROOT}/.local/device/redfin/voice"
OCR="${ROOT}/.local/device/redfin/ocr"
OUT="${ROOT}/out/pkgs"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/aginx-pkg-${PKG}.XXXXXX")"
trap 'rm -rf "${STAGE}"' EXIT
mkdir -p "${OUT}"
TARNAME="${PKG}-v${VER}-4pc.tar"

case "${PKG}" in
  aginx-gateway | aginx-secretd | aginx-voice)
    # crate 名≠包名的唯一例外：secretd 二进制住在 aginx-secret crate（双 bin）
    CRATE="${PKG}"
    [ "${PKG}" = "aginx-secretd" ] && CRATE="aginx-secret"
    echo "==> zigbuild ${PKG}（musl，缓存则秒过）"
    (cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl -p "${CRATE}")
    mkdir -p "${STAGE}/bin"
    install -m 755 "${TARGET_DIR}/${PKG}" "${STAGE}/bin/${PKG}"
    MEMBERS="bin pkg.toml SKILL.md"
    ;;
  aginx)
    # 母体树包（刀3 合一）：三件一树。target 二进制名 aginx（出自
    # aginx-router crate）+ aginx-server + aginx-runtime。exec=bin/aginx
    # → /var/bin/aginx symlink 面；[service] cmd 指 pkgfiles 真身。
    echo "==> zigbuild 母体三件（musl，缓存则秒过）"
    (cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
      -p aginx-router -p aginx-server -p aginx-runtime)
    mkdir -p "${STAGE}/files/bin"
    install -m 755 "${TARGET_DIR}/aginx" "${TARGET_DIR}/aginx-server" \
      "${TARGET_DIR}/aginx-runtime" "${STAGE}/files/bin/"
    MEMBERS="pkg.toml SKILL.md files"
    ;;
  aginx-asr)
    test -x "${VOICE}/bin/ag-asr" || { echo "FATAL: missing ${VOICE}/bin/ag-asr — see devices/redfin/boot/assets.md" >&2; exit 1; }
    test -s "${VOICE}/models/asr/model.int8.onnx" || { echo "FATAL: missing asr models" >&2; exit 1; }
    mkdir -p "${STAGE}/files/bin" "${STAGE}/files/models"
    install -m 755 "${VOICE}/bin/ag-asr" "${STAGE}/files/bin/ag-asr"
    cp -R "${VOICE}/models/asr" "${STAGE}/files/models/asr"
    MEMBERS="pkg.toml SKILL.md files"
    ;;
  aginx-tts)
    test -x "${VOICE}/bin/ag-tts" || { echo "FATAL: missing ${VOICE}/bin/ag-tts" >&2; exit 1; }
    test -s "${VOICE}/models/tts/vits-melo-tts-zh_en/model.onnx" || { echo "FATAL: missing tts models" >&2; exit 1; }
    mkdir -p "${STAGE}/files/bin" "${STAGE}/files/models/tts"
    install -m 755 "${VOICE}/bin/ag-tts" "${STAGE}/files/bin/ag-tts"
    cp -R "${VOICE}/models/tts/vits-melo-tts-zh_en" "${STAGE}/files/models/tts/vits-melo-tts-zh_en"
    # 与整机烤机同律（build-rootfs.sh 同段）：tarball 的 model.int8.onnx
    # 是 133B git-lfs 指针（M42e 收据），真权重是 fp32 model.onnx
    rm -f "${STAGE}/files/models/tts/vits-melo-tts-zh_en/model.int8.onnx"
    MEMBERS="pkg.toml SKILL.md files"
    ;;
  aginx-ocr)
    test -x "${OCR}/bin/ag-ocr" || { echo "FATAL: missing ${OCR}/bin/ag-ocr" >&2; exit 1; }
    test -s "${OCR}/models/det.onnx" && test -s "${OCR}/models/rec.onnx" \
      && test -s "${OCR}/models/dict.txt" || { echo "FATAL: missing ocr models" >&2; exit 1; }
    mkdir -p "${STAGE}/files/bin" "${STAGE}/files/models/ocr"
    install -m 755 "${OCR}/bin/ag-ocr" "${STAGE}/files/bin/ag-ocr"
    cp "${OCR}/models/det.onnx" "${OCR}/models/rec.onnx" \
      "${OCR}/models/dict.txt" "${STAGE}/files/models/ocr/"
    MEMBERS="pkg.toml SKILL.md files"
    ;;
  *)
    echo "FATAL: 未知包名 ${PKG}（四裸包 zigbuild / aginx 母体树包 / 三树包预编译件）" >&2
    exit 1
    ;;
esac

install -m 644 "${RECIPE}/pkg.toml" "${RECIPE}/SKILL.md" "${STAGE}/"

# 模型文件统一 644 / 目录 755——cp -R 的 mode 语义 BSD/GNU 不一，不赌
if [ -d "${STAGE}/files/models" ]; then
  find "${STAGE}/files/models" -type d -exec chmod 755 {} +
  find "${STAGE}/files/models" -type f -exec chmod 644 {} +
fi

# MEMBER_MAX 闸（见头注）
WARN_BYTES=240000000
FATAL_BYTES=268435456
while IFS= read -r f; do
  sz="$(wc -c < "${f}" | tr -d '[:space:]')"
  if [ "${sz}" -ge "${FATAL_BYTES}" ]; then
    echo "FATAL: ${f#"${STAGE}"/} ${sz}B ≥ MEMBER_MAX 256MiB——安装器拒收（提 MEMBER_MAX 或拆包）" >&2
    exit 1
  fi
  if [ "${sz}" -gt "${WARN_BYTES}" ]; then
    echo "WARN: ${f#"${STAGE}"/} ${sz}B >240MB——贴 MEMBER_MAX 顶" >&2
  fi
done < <(find "${STAGE}" -type f)

echo "==> tar ${TARNAME}（成员序 ${MEMBERS}）"
# COPYFILE_DISABLE：macOS 上挡 AppleDouble/扩展属性头进 ustar
# shellcheck disable=SC2086 — MEMBERS 是受控词表
COPYFILE_DISABLE=1 tar --format ustar -cf "${OUT}/${TARNAME}" -C "${STAGE}" ${MEMBERS}

if command -v shasum >/dev/null 2>&1; then
  SHA="$(shasum -a 256 "${OUT}/${TARNAME}" | cut -d' ' -f1)"
else
  SHA="$(sha256sum "${OUT}/${TARNAME}" | cut -d' ' -f1)"
fi
printf '%s\n' "${SHA}" > "${OUT}/${TARNAME}.sha256"

# manifest 片段（deps 是清单第 6 列；C10 EGG 组装收走此行）
echo "${PKG} ${URL} ${SHA} core ${VER}${DEPS:+ ${DEPS}}"

if [ -n "${SERIAL}" ]; then
  echo "==> push ${SERIAL}（dev 免签通道）"
  adb -s "${SERIAL}" push "${OUT}/${TARNAME}" "/data/local/tmp/${TARNAME}"
  adb -s "${SERIAL}" shell "/usr/bin/aginx-pkg install ${PKG} /data/local/tmp/${TARNAME} ${SHA}"
fi
