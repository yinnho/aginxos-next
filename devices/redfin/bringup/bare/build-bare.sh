#!/bin/bash
# redfin 裸 substrate 出件：zig cc freestanding → arm64 Image 壳 → LZ4 帧修块
# → mkbootimg v3。产物 .local/device/redfin/boot-out/boot-heartbeat.img（默认）
# 或 boot-bare.img（--full，研究件）。
#
# 【验收形状法】2026-10-13 整夜对切钉死（L1–L14），ABL 收自定义内核的充要形状：
#   1. boot.img v3（mkbootimg --os_version 14.0.0 --os_patch_level 2023-11，
#      ramdisk 可空）；
#   2. 内核 = 现代 LZ4 帧（魔数 04 22 4d 18），**BD 字节=0x70（4MB 块）**；
#   3. 解压内容带 arm64 Image 头：text_offset=0x80000、flags=0xa。
# 三者齐 → 发射（冻 logo/跑到代码）；缺一 → ~20s 内拒回 fastboot（不烧 tries）。
#
# 【brew lz4 1.10.0 雷】-B7 打印 4MB 实写 BD=0x40：帧字节[5] 手补 0x70，
# 字节[6] HC 用 lz4 -t 预言机暴力搜（本树图像恒 0xb9）。
#
# 【双入口垫】镜像垫到 0x81000，+0 与 +0x80000 各埋一条回 +0x40 的支——
# 两种 ABL 入口约定都落到 _start。
#
# 【心跳判据】fastboot flash boot_a <img>（刷写自动重臂 tries=3，无需
# set_active）→ reboot → logo 闪灭 ×3 = 裸件温复位 ×3 → tries 3→0 →
# ABL 自动回滚 b 槽进 L0 Linux，adb 回来。冻 logo = 发射但代码没跑到复位；
# ≤20s 回 fastboot = 形状被拒。
set -euo pipefail

REPO="$(cd "$(dirname "$0/")/../../../.." && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUTDIR="${REPO}/.local/device/redfin/boot-out"
TOOLS="${REPO}/.local/boot-tools"
OBJCOPY="${OBJCOPY:-/opt/homebrew/opt/llvm@21/bin/llvm-objcopy}"

command -v zig >/dev/null || { echo "zig not found" >&2; exit 1; }
[ -x "${OBJCOPY}" ] || { echo "${OBJCOPY} not found" >&2; exit 1; }
[ -f "${TOOLS}/mkbootimg.py" ] || { echo "no mkbootimg.py (scripts/fetch-boot-tools.sh)" >&2; exit 1; }
command -v lz4 >/dev/null || { echo "lz4 not found (brew install lz4)" >&2; exit 1; }

MODE="heartbeat"
[ "${1:-}" = "--full" ] && MODE="full"
mkdir -p "${OUTDIR}"
ELF="${OUTDIR}/heartbeat.elf"

CFLAGS="-O2 -ffreestanding -nostdlib -fno-builtin -fno-stack-protector \
  -fno-asynchronous-unwind-tables -fomit-frame-pointer -Wl,--build-id=none"

echo "==> zig cc freestanding (${MODE})"
if [ "${MODE}" = "heartbeat" ]; then
  # 阶梯 1 正身：36B 位置无关 stub，链接基址 0（无需真落点）
  cat > "${OUTDIR}/heartbeat.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} \
    -Wl,-T,"${OUTDIR}/heartbeat.ld" \
    -o "${ELF}" "${HERE}/heartbeat.S"
  BIN="${OUTDIR}/heartbeat.bin"; IMG="${OUTDIR}/boot-heartbeat.img"
else
  # 研究件：SPMI 全量 substrate。警告：start.S 迁移路径 2026-10-13 三验
  # （L10/L11/L12a）全部冻 logo（崩在 kmain 前）——修好前勿当心跳用。
  ELF="${OUTDIR}/bare.elf"
  zig cc -target aarch64-freestanding-none ${CFLAGS} \
    -ffunction-sections -fdata-sections -Wl,--gc-sections \
    -Wl,-T,"${HERE}/linker.ld" \
    -o "${ELF}" "${HERE}/start.S" "${HERE}/main.c"
  BIN="${OUTDIR}/bare.bin"; IMG="${OUTDIR}/boot-bare.img"
fi

"${OBJCOPY}" -O binary "${ELF}" "${BIN}"

echo "==> flat bin $(stat -f%z "${BIN}") bytes"

echo "==> arm64 Image 壳 + 0x81000 垫 + 双入口"
python3 - "${BIN}" <<'PY'
import struct, sys
src = sys.argv[1]
code = open(src, 'rb').read()
hdr = bytearray(64)
struct.pack_into('<II', hdr, 0x00, 0x14000010, 0)          # code_0: b +0x40 → _start
struct.pack_into('<QQQ', hdr, 0x08, 0x80000, 0x81000, 0xa) # stock text_offset/flags
struct.pack_into('<I', hdr, 0x38, 0x644d5241)              # "ARM\x64"
img = bytearray(bytes(hdr) + code)
img += b'\x00' * (0x81000 - len(img))
back = (0x40 - 0x80000) >> 2 & 0x3ffffff
struct.pack_into('<I', img, 0x80000, 0x14000000 | back)    # 第二入口 → +0x40
open(src + '.img', 'wb').write(bytes(img))
print(f'   wrapped {len(img)} bytes (dual entry @0/0x80000)')
PY

echo "==> LZ4 帧 + BD/HC 修块"
lz4 -12 -B7 -f "${BIN}.img" "${BIN}.kl4" >/dev/null
python3 - "${BIN}.kl4" <<'PY'
import subprocess, sys
f = sys.argv[1]
d = bytearray(open(f, 'rb').read())
assert d[:4] == bytes([0x04, 0x22, 0x4d, 0x18]), 'lz4 现代帧魔数缺失'
d[5] = 0x70                                   # BD=4MB（brew lz4 -B7 雷补）
for hc in range(256):                          # HC 暴力搜（lz4 -t 预言机）
    d[6] = hc
    open(f, 'wb').write(bytes(d))
    if subprocess.run(['lz4', '-t', '-f', f], capture_output=True).returncode == 0:
        break
else:
    raise SystemExit('HC 搜索失败')
src = f[:f.rfind('.')] + '.img'                # roundtrip 全等校验
subprocess.run(['lz4', '-d', '-f', f, f + '.chk'], capture_output=True)
if open(f + '.chk', 'rb').read() != open(src, 'rb').read():
    raise SystemExit('roundtrip 不等，帧损坏')
print(f'   frame {len(d)} bytes, BD=0x70, HC=0x{hc:02x}, roundtrip exact')
PY

: > "${OUTDIR}/empty.rd"
python3 "${TOOLS}/mkbootimg.py" \
  --header_version 3 \
  --kernel "${BIN}.kl4" \
  --ramdisk "${OUTDIR}/empty.rd" \
  --os_version 14.0.0 --os_patch_level 2023-11 \
  -o "${IMG}"

ls -la "${IMG}"
echo
echo "上机（串号闸门先过 serials.env）："
echo "  fastboot flash boot_a ${IMG}"
echo "  fastboot reboot"
echo "心跳判据：logo 闪灭×3 → 自动回滚 b 槽 → adb 回来（~90s）。"
