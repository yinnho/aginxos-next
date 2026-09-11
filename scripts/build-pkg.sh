#!/usr/bin/env bash
# build-pkg.sh — aginx 体系包打包线（C8 蛋案）。
#
# usage: scripts/build-pkg.sh <name> [--push <serial>]
#
#   四裸包  aginx-gateway aginx-secretd aginx-voice
#                       — zigbuild musl 件 + pkgs/<name>/ 配方
#   两树包  aginx aginx-term
#                       — 母体三件（router/server/runtime，刀3 合一）+
#                         终端面板（term+字体，刀4 出镜像）
#   三树包  aginx-asr aginx-tts aginx-ocr
#                       — .local/device/redfin 冻结 bionic 件 + 模型一树
#                         （exec=bin/ag-*，模型与整机烤机逐字节同源：asr
#                          全树；tts=vits-melo 去 133B lfs 指针；ocr=
#                          det/rec/dict 三件）
#   上游树包  git
#                       — Alpine v3.22 aarch64 apk 闭包 15 件（sha256 逐件
#                         钉死）+ wrapper 面 bin/git（L0 刀C：https 传输
#                         整树自持，apk 缓存 out/apk-cache 复用）
#
# 产物 out/pkgs/<name>-v<ver>-4pc.tar + .sha256（裸 hex），尾行打一行
# manifest 片段（name url sha opt version [deps]）——L0 清单组装（刀4）
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
  aginx-term)
    # 终端树包（刀4 出镜像）：term + CJK 字体一树。字体真源是 rootfs
    # 配方里的冻结资产（M38a，同 n5-qr.jpg 先例——文件不搬家）。
    echo "==> zigbuild aginx-term（musl，缓存则秒过）"
    (cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
      -p aginx-term)
    mkdir -p "${STAGE}/files/bin" "${STAGE}/files/share/fonts"
    install -m 755 "${TARGET_DIR}/aginx-term" "${STAGE}/files/bin/aginx-term"
    install -m 644 "${ROOT}/rootfs/usr/share/fonts/agterm-cjk.otf" \
      "${STAGE}/files/share/fonts/agterm-cjk.otf"
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
  git)
    # 上游树包（L0 缺口刀C，2026-09-12）：Alpine v3.22 main/aarch64 依赖
    # 闭包 15 apk，sha256 逐件钉死（TOFU 于下载日——APKINDEX 无 apk 文件
    # 哈希，钉表即此）。缓存 out/apk-cache 复用（sha 合则不拉）。apk 内
    # 全是目录正确的相对 symlink（git-core 命令族 -> ../../bin/git、
    # lib*.so.N -> 实文件），安装器按 symlink 原样重建（lib.rs :428-439
    # 收 tree_symlinks，文件成员全部落地后才建链——相对目标必在）。
    # ca-certificates-bundle 故意不在闭包：libcurl 编译期默认 CA 路径
    # /etc/ssl/certs/ca-certificates.crt 镜像已烤入（M12，L0 继承）。
    APK_DIR="${ROOT}/out/apk-cache"
    ALPINE_BASE="https://dl-cdn.alpinelinux.org/alpine/v3.22/main/aarch64"
    GIT_APKS="
      musl-1.2.5-r12:ac281d1e7f9e9c447c51e309317b975f48be6edaf3ab91ae73b959cf86703782
      git-2.49.1-r0:9d27f0e5e57b234e6d6a8a2b635ba82286e1dd441ea26c280aba4c8e416b4f60
      zlib-1.3.2-r0:7a39a917e4dab3c7a45537210ee5b5f17bf75f5e7777809a20cddd0afe074187
      pcre2-10.46-r0:62fdc4a3d6b48ca211cf6480c5da55664b489ec2b192ca8942e5b1d60ebe9496
      libexpat-2.8.4-r0:a67a6a5e55a9e2716197d5bd4d22d980a41d611166d8bcdd71acb69286eaadee
      libcurl-8.14.1-r3:a811d6f0370383461ed4993434f84b812f29a48009332f517d8a373613797532
      zstd-libs-1.5.7-r0:a0e92d2225941a514eb0b2325b137fe6444ef9171627aae8129b74a6ad934ac4
      libssl3-3.5.8-r0:2b175c982f9ff9a80fc88fa587f6db0ae1c58eef4b3f7fe69e8f066be9ff1090
      libcrypto3-3.5.8-r0:094c5816644ade889f74387e8d91aad89dc9a05eda150494e3f91b80ffc15460
      libpsl-0.21.5-r3:b9f7270cb2980876f57360f38ca11093aefaf7f5eb4b777dfbc4e5738449e438
      libunistring-1.3-r0:6b6631284b25fa28bb9e63f9d423145e96d0f7aeb55a592f5cd5d54fb39380f7
      libidn2-2.3.7-r0:9ddc248988707da96752077d05bacbda751d46cc1f7aa1460b3a40c4fbf66a6e
      nghttp2-libs-1.69.0-r0:19db967a36f1e041e96240484d94063354f5c837e36d50daa017c2e96393a5e7
      c-ares-1.34.8-r0:e293f056615fff3cf6050a423c090179daf48516317912daf1e133a1095a2f65
      brotli-libs-1.1.0-r2:b05f9d2839bb89f28325890ffbd7d94025af6b301344ae0255f08617a6036c65
    "
    mkdir -p "${APK_DIR}" "${STAGE}/files"
    for pair in ${GIT_APKS}; do
      apk="${pair%%:*}"; want="${pair##*:}"
      f="${APK_DIR}/${apk}.apk"
      if [ ! -s "${f}" ] || [ "$(shasum -a 256 "${f}" | cut -d' ' -f1)" != "${want}" ]; then
        echo "  fetch ${apk}"
        curl -fsSL -o "${f}" "${ALPINE_BASE}/${apk}.apk" \
          || { echo "FATAL: fetch ${apk} 失败" >&2; exit 1; }
      fi
      got="$(shasum -a 256 "${f}" | cut -d' ' -f1)"
      [ "${got}" = "${want}" ] \
        || { echo "FATAL: ${apk} sha256 不符（钉 ${want} 得 ${got}）" >&2; exit 1; }
      tar -xzf "${f}" -C "${STAGE}/files"
    done
    # apk 家务件 + 赘重裁掉：man/doc/locale、openssl 引擎/模块目录（TLS
    # 默认 provider 内建于 libcrypto）、libcrypto 带的 etc/ssl 配置副本
    # （libcurl 走镜像烤入的绝对 CA 路径，树内副本无人读）。
    rm -f "${STAGE}/files/.PKGINFO" "${STAGE}/files"/.SIGN.* \
          "${STAGE}/files"/.pre-* "${STAGE}/files"/.post-* "${STAGE}/files"/.trigger*
    rm -rf "${STAGE}/files"/usr/share/man "${STAGE}/files"/usr/share/doc \
           "${STAGE}/files"/usr/share/locale "${STAGE}/files"/etc \
           "${STAGE}/files"/usr/lib/engines-3 "${STAGE}/files"/usr/lib/ossl-modules
    # wrapper 面（/lib loader 首跑自链 + GIT_EXEC_PATH/TEMPLATE +
    # LD_LIBRARY_PATH——细节见 pkgs/git/bin/git 头注）。apk 树只有
    # usr/bin，bin/ 是 wrapper 专属层。
    mkdir -p "${STAGE}/files/bin"
    install -m 755 "${RECIPE}/bin/git" "${STAGE}/files/bin/git"
    find "${STAGE}/files" -type d -exec chmod 755 {} +
    # 闭包钉死门：本体 + loader + git-core multicall 面 + 模板 + 全部
    # DT_NEEDED soname（闭包声明与树内容不一致时这里现形）
    test -x "${STAGE}/files/usr/bin/git" || { echo "FATAL: 树里无 usr/bin/git" >&2; exit 1; }
    test -f "${STAGE}/files/lib/ld-musl-aarch64.so.1" || { echo "FATAL: 树里无 musl loader" >&2; exit 1; }
    test -e "${STAGE}/files/usr/libexec/git-core/git-remote-https" \
      || { echo "FATAL: 树里无 git-remote-https（https 传输面）" >&2; exit 1; }
    test -d "${STAGE}/files/usr/share/git-core/templates" \
      || { echo "FATAL: 树里无 git 模板（init/clone 要用）" >&2; exit 1; }
    for so in libc.musl-aarch64.so.1 libcurl.so.4 libexpat.so.1 libpcre2-8.so.0 \
              libz.so.1 libzstd.so.1 libbrotlicommon.so.1 libbrotlidec.so.1 \
              libcares.so.2 libcrypto.so.3 libssl.so.3 libidn2.so.0 \
              libnghttp2.so.14 libpsl.so.5 libunistring.so.5; do
      find "${STAGE}/files/lib" "${STAGE}/files/usr/lib" -maxdepth 1 -name "${so}" | grep -q . \
        || { echo "FATAL: 闭包缺 soname ${so}" >&2; exit 1; }
    done
    MEMBERS="pkg.toml SKILL.md files"
    ;;
  *)
    echo "FATAL: 未知包名 ${PKG}（四裸包 zigbuild / aginx·aginx-term 树包 / 三树包预编译件）" >&2
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

# manifest 片段（deps 是清单第 6 列；L0 全 opt——刀4 翻档，镜像组装段收走）
echo "${PKG} ${URL} ${SHA} opt ${VER}${DEPS:+ ${DEPS}}"

if [ -n "${SERIAL}" ]; then
  echo "==> push ${SERIAL}（dev 免签通道）"
  adb -s "${SERIAL}" push "${OUT}/${TARNAME}" "/data/local/tmp/${TARNAME}"
  adb -s "${SERIAL}" shell "/usr/bin/aginx-pkg install ${PKG} /data/local/tmp/${TARNAME} ${SHA}"
fi
