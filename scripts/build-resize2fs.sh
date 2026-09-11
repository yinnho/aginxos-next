#!/usr/bin/env bash
# Build a static musl resize2fs (e2fsprogs) for the L0 image — first-boot
# root-fs grow (L0 缺口刀1, 2026-09-12). M12 配方重铸: e2fsprogs 1.47.0
# via zig cc, two known traps from the 2026-08-30 session — configure's
# LSEEK64 probe fails under zig cc (forced in config.h afterwards) and the
# uuid test binaries do not build (we never `make all`; only the libs +
# resize/resize2fs are built). Cached under out/ so the download and the
# ~2 min build happen once per host; build-rootfs.sh installs the product.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="${E2FSPROGS_VER:-1.47.0}"
TARBALL="${ROOT}/out/e2fsprogs-${VER}.tar.gz"
OUT_BIN="${ROOT}/out/resize2fs"

ZIG="$(command -v zig || true)"
[ -z "${ZIG}" ] && ZIG=/opt/homebrew/bin/zig
test -x "${ZIG}" || { echo "zig not found (needed for the musl cross build)" >&2; exit 1; }

if [ -s "${OUT_BIN}" ]; then
  echo "resize2fs already built: ${OUT_BIN} ($(stat -f%z "${OUT_BIN}") bytes)"
  exit 0
fi

mkdir -p "${ROOT}/out"
if [ ! -s "${TARBALL}" ]; then
  echo "==> downloading e2fsprogs ${VER}"
  curl -fsSL --max-time 300 -o "${TARBALL}" \
    "https://cdn.kernel.org/pub/linux/kernel/people/tytso/e2fsprogs/v${VER}/e2fsprogs-${VER}.tar.gz"
fi

WORK="$(mktemp -d /tmp/e2fsprogs-build.XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT
tar -xzf "${TARBALL}" -C "${WORK}"
cd "${WORK}/e2fsprogs-${VER}"

echo "==> configure (zig cc musl cross)"
# AR/RANLIB must be zig's llvm tools: macOS ar rejects ELF members silently
# ("not a mach-o file" -> 96-byte empty archives -> every link fails on
# undefined symbols). zig 0.16 ships both as subcommands. LDFLAGS=-s: ship
# stripped (3.6MB debug -> ~1.2MB; nothing on-device reads those symbols).
CC="${ZIG} cc -target aarch64-linux-musl" \
AR="${ZIG} ar" \
RANLIB="${ZIG} ranlib" \
LDFLAGS="-s" \
./configure --build=x86_64-apple-darwin \
            --host=aarch64-unknown-linux-musl \
            --disable-nls \
  > configure.log 2>&1 || { tail -40 configure.log >&2; exit 1; }

# M12 trap 1: the LSEEK64 link probe fails under zig cc — force the defines.
# (e2fsprogs keeps its config.h under lib/, not the top level. Both defines
# are needed: llseek.c takes the lseek64 path only when the PROTOTYPE macro
# is set too, else `my_llseek` goes undeclared.)
patch_lseek64() {
  sed -i '' 's@/\* #undef HAVE_LSEEK64 \*/@#define HAVE_LSEEK64 1@' lib/config.h
  sed -i '' 's@/\* #undef HAVE_LSEEK64_PROTOTYPE \*/@#define HAVE_LSEEK64_PROTOTYPE 1@' lib/config.h
  grep -q '^#define HAVE_LSEEK64 1' lib/config.h \
    || echo '#define HAVE_LSEEK64 1' >> lib/config.h
  grep -q '^#define HAVE_LSEEK64_PROTOTYPE 1' lib/config.h \
    || echo '#define HAVE_LSEEK64_PROTOTYPE 1' >> lib/config.h
}
patch_lseek64

echo "==> build libs + resize2fs (never 'make all'/'make libs' — M12 trap 2:"
echo "    lib/uuid's all:: hardcodes tst_uuid/uuid_time, which fail to link;"
echo "    resize2fs links only e2p+ext2fs+com_err, so build just those archives)"
J="-j$(sysctl -n hw.ncpu)"
# subs FIRST: on a fresh tree its Makefile generation runs a bare config.status
# that rewrites lib/config.h from the template — the patch above would be lost
# if applied before it. Re-apply after, then never let anything regenerate.
make subs > subs.log 2>&1 || { tail -20 subs.log >&2; exit 1; }
patch_lseek64
# uuid/blkid headers+archives only, never their all:: (test progs);
# resize2fs's own LIBS line is e2p+ext2fs+com_err, but blkid (pulled by
# libsupport's plausible.o) drags uuid symbols at the static link.
make -C lib/uuid uuid.h libuuid.a > lib-uuid.log 2>&1 \
  || { patch_lseek64; tail -20 lib-uuid.log >&2; exit 1; }
for d in et e2p blkid support ext2fs; do
  make -C "lib/$d" ${J} > "lib-$d.log" 2>&1 \
    || { patch_lseek64; tail -20 "lib-$d.log" >&2; exit 1; }
done
make -C resize resize2fs > resize.log 2>&1 || { tail -20 resize.log >&2; exit 1; }

install -m 755 resize/resize2fs "${OUT_BIN}"
file "${OUT_BIN}"
echo "built ${OUT_BIN} ($(stat -f%z "${OUT_BIN}") bytes)"
