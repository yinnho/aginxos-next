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
[ "${1:-}" = "--probe" ] && MODE="probe"
[ "${1:-}" = "--calib" ] && MODE="calib"
[ "${1:-}" = "--sp" ] && MODE="sp"
[ "${1:-}" = "--pi" ] && MODE="pi"
[ "${1:-}" = "--spmiprobe" ] && MODE="spmiprobe"
[ "${1:-}" = "--scan" ] && MODE="scan"
[ "${1:-}" = "--rd" ] && MODE="rd"
[ "${1:-}" = "--wr" ] && MODE="wr"
[ "${1:-}" = "--vr" ] && MODE="vr"
[ "${1:-}" = "--own" ] && MODE="own"
[ "${1:-}" = "--ow" ] && MODE="ow"
[ "${1:-}" = "--pi2" ] && MODE="pi2"
[ "${1:-}" = "--pon" ] && MODE="pon"
[ "${1:-}" = "--fb" ] && MODE="fb"
[ "${1:-}" = "--fb2" ] && MODE="fb2"
[ "${1:-}" = "--ka" ] && MODE="ka"
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
elif [ "${MODE}" = "calib" ]; then
  # B5 时钟标定桩：L13 壳 + 1e8 纯立即数延时，秒表量周期定钟速。
  cat > "${OUTDIR}/calib.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} \
    -Wl,-T,"${OUTDIR}/calib.ld" \
    -o "${ELF}" "${HERE}/calib.S"
  BIN="${OUTDIR}/calib.bin"; IMG="${OUTDIR}/boot-calib.img"
elif [ "${MODE}" = "sp" ]; then
  # B6 栈写单变量实验：L13 壳+一条写 0x80018ff8（B4 同源地址）。
  cat > "${OUTDIR}/sp.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} \
    -Wl,-T,"${OUTDIR}/sp.ld" \
    -o "${ELF}" "${HERE}/sp.S"
  BIN="${OUTDIR}/sp.bin"; IMG="${OUTDIR}/boot-sp.img"
elif [ "${MODE}" = "pi" ]; then
  # B7 位置无关全量 substrate：main.c 纯寄存器翻译，零字面量零 DRAM 写
  # （ABL 真落点=0xA7E80000，逆向收据见 boot-pi.S 头注）。
  cat > "${OUTDIR}/pi.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} \
    -Wl,-T,"${OUTDIR}/pi.ld" \
    -o "${ELF}" "${HERE}/boot-pi.S"
  BIN="${OUTDIR}/pi.bin"; IMG="${OUTDIR}/boot-pi.img"
elif [ "${MODE}" = "vr" ]; then
  # B13 合法读事务探针：首个掩码合法的 SPMI 读（0x1042→0x42），零后续事务。
  cat > "${OUTDIR}/vr.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS}     -Wl,-T,"${OUTDIR}/vr.ld"     -o "${ELF}" "${HERE}/vr-probe.S"
  BIN="${OUTDIR}/vr.bin"; IMG="${OUTDIR}/boot-vr.img"
elif [ "${MODE}" = "own" ]; then
  # B14 cnfg/owner 窗读探针：owner 表（0x0c40a700+4N）零事务纯读。
  cat > "${OUTDIR}/own.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/own.ld" -o "${ELF}" "${HERE}/own-probe.S"
  BIN="${OUTDIR}/own.bin"; IMG="${OUTDIR}/boot-own.img"
elif [ "${MODE}" = "ka" ]; then
  # RUNG2 在役裸 substrate：充电使能+永续喂狗环，不复位。
  cat > "${OUTDIR}/ka.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/ka.ld" -o "${ELF}" "${HERE}/ka.S"
  BIN="${OUTDIR}/ka.bin"; IMG="${OUTDIR}/boot-ka.img"
elif [ "${MODE}" = "fb2" ]; then
  # B18b 白带判别：0xFFFFFFFF 任何格式皆不透明白。
  cat > "${OUTDIR}/fb2.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/fb2.ld" -o "${ELF}" "${HERE}/fb2-probe.S"
  BIN="${OUTDIR}/fb2.bin"; IMG="${OUTDIR}/boot-fb2.img"
elif [ "${MODE}" = "fb" ]; then
  # B18 splash fb 判活通道探针：双格式判读带+展示窗+复位（RUNG2 D2）。
  cat > "${OUTDIR}/fb.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/fb.ld" -o "${ELF}" "${HERE}/fb-probe.S"
  BIN="${OUTDIR}/fb.bin"; IMG="${OUTDIR}/boot-fb.img"
elif [ "${MODE}" = "pon" ]; then
  # B17 PON 所有权判别器：找到 owner==0 副本→闪灭；没有→故意冻死当信号。
  cat > "${OUTDIR}/pon.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/pon.ld" -o "${ELF}" "${HERE}/pon-probe.S"
  BIN="${OUTDIR}/pon.bin"; IMG="${OUTDIR}/boot-pon.img"
elif [ "${MODE}" = "pi2" ]; then
  # B16 全量 substrate v2：owner-gated 选择（TRUTH.md §2 铁律正解姿势）。
  cat > "${OUTDIR}/pi2.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/pi2.ld" -o "${ELF}" "${HERE}/pi2.S"
  BIN="${OUTDIR}/pi2.bin"; IMG="${OUTDIR}/boot-pi2.img"
elif [ "${MODE}" = "ow" ]; then
  # B15 owner-gated 单写探针：选 owner==0 的 CHGR 副本发唯一合法写。
  cat > "${OUTDIR}/ow.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/ow.ld" -o "${ELF}" "${HERE}/ow-probe.S"
  BIN="${OUTDIR}/ow.bin"; IMG="${OUTDIR}/boot-ow.img"
elif [ "${MODE}" = "wr" ]; then
  # B11 写事务探针：B10 扫描+读活 + 唯一一个 CHNLS 写事务（喂狗同源）。
  cat > "${OUTDIR}/wr.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS}     -Wl,-T,"${OUTDIR}/wr.ld"     -o "${ELF}" "${HERE}/wr-probe.S"
  BIN="${OUTDIR}/wr.bin"; IMG="${OUTDIR}/boot-wr.img"
elif [ "${MODE}" = "rd" ]; then
  # B10 读事务探针：B9 扫描+唯一一个 OBSRVR 读事务（B7 首事务同源）。
  cat > "${OUTDIR}/rd.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS}     -Wl,-T,"${OUTDIR}/rd.ld"     -o "${ELF}" "${HERE}/rd-probe.S"
  BIN="${OUTDIR}/rd.bin"; IMG="${OUTDIR}/boot-rd.img"
elif [ "${MODE}" = "scan" ]; then
  # B9 纯扫描探针：512 项读表+复位，零事务零写（二分读空间/事务空间）。
  cat > "${OUTDIR}/scan.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS}     -Wl,-T,"${OUTDIR}/scan.ld"     -o "${ELF}" "${HERE}/scan-probe.S"
  BIN="${OUTDIR}/scan.bin"; IMG="${OUTDIR}/boot-scan.img"
elif [ "${MODE}" = "spmiprobe" ]; then
  # B8 SPMI_CORE 读探针：B5 壳+一条 ldr 0x0c440900（单变量定位 B7 冻点）。
  cat > "${OUTDIR}/spmiprobe.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS}     -Wl,-T,"${OUTDIR}/spmiprobe.ld"     -o "${ELF}" "${HERE}/spmi-probe.S"
  BIN="${OUTDIR}/spmiprobe.bin"; IMG="${OUTDIR}/boot-spmiprobe.img"
elif [ "${MODE}" = "probe" ]; then
  # B3 C 入口证：start.S(迁移+VBAR 网)+纯 C 延时+PS_HOLD，零 SPMI。
  ELF="${OUTDIR}/probe.elf"
  zig cc -target aarch64-freestanding-none ${CFLAGS} \
    -ffunction-sections -fdata-sections -Wl,--gc-sections \
    -Wl,-T,"${HERE}/linker.ld" \
    -o "${ELF}" "${HERE}/start.S" "${HERE}/probe.c"
  BIN="${OUTDIR}/probe.bin"; IMG="${OUTDIR}/boot-probe.img"
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
