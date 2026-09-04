#!/usr/bin/env bash
# build-n3-package — 打 N3 并存包 aginx-server（四件套 files/ 树形态）。
#
# 树里装：aginx-server / aginx / aginx-runtime（新仓 musl 三件）+
# voiced（老仓 N2② 前台版 musl）+ 4 个老镜像工具薄壳。face =
# /var/bin/aginx-server 符号链接（pkg.toml exec）；单元/化身树/env 语义
# 见 pkg/aginx-server/SKILL.md。
#
# tar 纪律（M26/M30 收据）：未压缩 ustar；macOS 必须 COPYFILE_DISABLE=1
# 防 AppleDouble；成员根相对（pkg.toml SKILL.md files/…）。
set -euo pipefail

NROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$NROOT/target/aarch64-unknown-linux-musl/release"
VOICED_MUSL="${VOICED_MUSL:-$NROOT/../aginxos/target/aarch64-unknown-linux-musl/release/voiced}"
PKG="$NROOT/pkg/aginx-server"
VER="$(sed -n 's/^version = "\(.*\)"/\1/p' "$PKG/pkg.toml")"
OUT="$NROOT/out/aginx-server"
TAR="$OUT/aginx-server-v$VER-4pc.tar"

for b in aginx aginx-server aginx-runtime; do
  [ -x "$BIN/$b" ] || { echo "n3pkg: $BIN/$b missing — cargo zigbuild first"; exit 1; }
done
[ -x "$VOICED_MUSL" ] || { echo "n3pkg: voiced musl missing at $VOICED_MUSL — 老仓 ./scripts/build-phone.sh musl voiced"; exit 1; }

echo "==> zigbuild 三件（缓存则秒过）"
(cd "$NROOT" && cargo zigbuild --release --target aarch64-unknown-linux-musl -p aginx-router -p aginx-server -p aginx-runtime)

rm -rf "$OUT/staging"
mkdir -p "$OUT/staging/files/tools"

install -m 755 "$BIN/aginx-server"  "$OUT/staging/files/aginx-server"
install -m 755 "$BIN/aginx"         "$OUT/staging/files/aginx"
install -m 755 "$BIN/aginx-runtime" "$OUT/staging/files/aginx-runtime"
install -m 755 "$VOICED_MUSL"       "$OUT/staging/files/voiced"
# 工具壳单独住 tools/：AGINX_CMD_PATH 只指它，常驻引擎名不进命令宇宙（D13）。
install -m 755 "$PKG"/files/tools/aginx-* "$OUT/staging/files/tools/"
install -m 644 "$PKG/pkg.toml" "$PKG/SKILL.md" "$OUT/staging/"

COPYFILE_DISABLE=1 tar --format=ustar -cf "$TAR" -C "$OUT/staging" pkg.toml SKILL.md files

echo "==> $TAR"
tar -tf "$TAR"
echo "sha256  $(shasum -a 256 "$TAR" | cut -d' ' -f1)"
echo "install: adb push $TAR /tmp/ && agpkg install aginx-server /tmp/$(basename "$TAR") <sha256>"
