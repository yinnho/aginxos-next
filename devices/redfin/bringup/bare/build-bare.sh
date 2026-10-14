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
[ "${1:-}" = "--u1" ] && MODE="u1"
[ "${1:-}" = "--u1a" ] && MODE="u1a"
[ "${1:-}" = "--u1c" ] && MODE="u1c"
[ "${1:-}" = "--u1d" ] && MODE="u1d"
[ "${1:-}" = "--u1e" ] && MODE="u1e"
[ "${1:-}" = "--u1f" ] && MODE="u1f"
[ "${1:-}" = "--u1m" ] && MODE="u1m"
[ "${1:-}" = "--u1n" ] && MODE="u1n"
[ "${1:-}" = "--u1g" ] && MODE="u1g"
[ "${1:-}" = "--u1s" ] && MODE="u1s"
[ "${1:-}" = "--u1t" ] && MODE="u1t"
[ "${1:-}" = "--u1u" ] && MODE="u1u"
[ "${1:-}" = "--u1v" ] && MODE="u1v"
[ "${1:-}" = "--u1w" ] && MODE="u1w"
[ "${1:-}" = "--u1x" ] && MODE="u1x"
[ "${1:-}" = "--u1y" ] && MODE="u1y"
[ "${1:-}" = "--u1aa" ] && MODE="u1aa"
[ "${1:-}" = "--u1ab" ] && MODE="u1ab"
[ "${1:-}" = "--u2" ] && MODE="u2"
[ "${1:-}" = "--u3" ] && MODE="u3"
[ "${1:-}" = "--u4" ] && MODE="u4"
[ "${1:-}" = "--u4b" ] && MODE="u4b"
[ "${1:-}" = "--u3b" ] && MODE="u3b"
[ "${1:-}" = "--u4c" ] && MODE="u4c"
[ "${1:-}" = "--u3c" ] && MODE="u3c"
[ "${1:-}" = "--u4d" ] && MODE="u4d"
[ "${1:-}" = "--u4e" ] && MODE="u4e"
[ "${1:-}" = "--u3d" ] && MODE="u3d"
[ "${1:-}" = "--u3e" ] && MODE="u3e"
[ "${1:-}" = "--u3g" ] && MODE="u3g"
[ "${1:-}" = "--u3h" ] && MODE="u3h"
[ "${1:-}" = "--u3i" ] && MODE="u3i"
[ "${1:-}" = "--u3j" ] && MODE="u3j"
[ "${1:-}" = "--u4f" ] && MODE="u4f"
[ "${1:-}" = "--u4g" ] && MODE="u4g"
[ "${1:-}" = "--u4h" ] && MODE="u4h"
[ "${1:-}" = "--u4i" ] && MODE="u4i"
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
elif [ "${MODE}" = "u1" ]; then
  # 阶梯3 刀1 v2：UFS MMIO 探针（CAP NUTRS 位权；VER 寄存器 qcom 坏已弃用）。
  cat > "${OUTDIR}/u1.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1.ld" -o "${ELF}" "${HERE}/u1-probe.S"
  BIN="${OUTDIR}/u1.bin"; IMG="${OUTDIR}/boot-u1.img"
elif [ "${MODE}" = "u1a" ]; then
  # 阶梯3 刀1b：MMIO 读可行性判别器（读完即闪，拆「读挂」与「值不符」）。
  cat > "${OUTDIR}/u1a.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1a.ld" -o "${ELF}" "${HERE}/u1a-probe.S"
  BIN="${OUTDIR}/u1a.bin"; IMG="${OUTDIR}/boot-u1a.img"
elif [ "${MODE}" = "u1c" ]; then
  # 阶梯3 刀1c：AHB 门钟开+读（U1a 冻死→时钟关，先开 iface 门再读）。
  cat > "${OUTDIR}/u1c.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1c.ld" -o "${ELF}" "${HERE}/u1c-probe.S"
  BIN="${OUTDIR}/u1c.bin"; IMG="${OUTDIR}/boot-u1c.img"
elif [ "${MODE}" = "u1d" ]; then
  # 阶梯3 刀1d：GCC 自检（读/RMW/回读粘住——证常开域 config 总线活）。
  cat > "${OUTDIR}/u1d.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1d.ld" -o "${ELF}" "${HERE}/u1d-probe.S"
  BIN="${OUTDIR}/u1d.bin"; IMG="${OUTDIR}/boot-u1d.img"
elif [ "${MODE}" = "u1e" ]; then
  cat > "${OUTDIR}/u1e.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1e.ld" -o "${ELF}" "${HERE}/u1e-probe.S"
  BIN="${OUTDIR}/u1e.bin"; IMG="${OUTDIR}/boot-u1e.img"
elif [ "${MODE}" = "u1f" ]; then
  # 阶梯3 刀1f：VBAR 网+SError 解掩（读挂三解判别：stall/abort/干净）。
  # 链基 0（落点无关）；vectors 定 flat 0x7C0=运行时基+0x800（2KB 对齐律）。
  cat > "${OUTDIR}/u1f.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1f.ld" -o "${ELF}" "${HERE}/u1f-probe.S"
  BIN="${OUTDIR}/u1f.bin"; IMG="${OUTDIR}/boot-u1f.img"
elif [ "${MODE}" = "u1m" ]; then
  # 阶梯3 刀1m：恒等映射 MMU-on（真 stall 后的制度刀，U1f 全套+页表）。
  # vectors 定 flat 0x7C0=运行时基+0x800（2KB 对齐律）。
  cat > "${OUTDIR}/u1m.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1m.ld" -o "${ELF}" "${HERE}/u1m-probe.S"
  BIN="${OUTDIR}/u1m.bin"; IMG="${OUTDIR}/boot-u1m.img"
elif [ "${MODE}" = "u1n" ]; then
  # 阶梯3 刀1n：网正向标定（U1m 骨架+udf，长闪=仪器证毕）。
  cat > "${OUTDIR}/u1n.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1n.ld" -o "${ELF}" "${HERE}/u1n-probe.S"
  BIN="${OUTDIR}/u1n.bin"; IMG="${OUTDIR}/boot-u1n.img"
elif [ "${MODE}" = "u1g" ]; then
  # 阶梯3 刀1g：GICD 读（MMU-on，均匀死 vs config-NoC 死分刀）。
  cat > "${OUTDIR}/u1g.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1g.ld" -o "${ELF}" "${HERE}/u1g-probe.S"
  BIN="${OUTDIR}/u1g.bin"; IMG="${OUTDIR}/boot-u1g.img"
elif [ "${MODE}" = "u1s" ]; then
  # 阶梯3 刀1s：网最小自检（MMU-off 零页表零外域；scratch 全在镜像内）。
  cat > "${OUTDIR}/u1s.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  .scratch 0x1FC0 : { KEEP(*(.scratch)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1s.ld" -o "${ELF}" "${HERE}/u1s-probe.S"
  BIN="${OUTDIR}/u1s.bin"; IMG="${OUTDIR}/boot-u1s.img"
elif [ "${MODE}" = "u1t" ]; then
  # 阶梯3 刀1t：对齐触发网自检（SCTLR.A+未对齐读，零总线零 MMU）。
  cat > "${OUTDIR}/u1t.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  .scratch 0x1FC0 : { KEEP(*(.scratch)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1t.ld" -o "${ELF}" "${HERE}/u1t-probe.S"
  BIN="${OUTDIR}/u1t.bin"; IMG="${OUTDIR}/boot-u1t.img"
elif [ "${MODE}" = "u1u" ]; then
  # 阶梯3 刀1u：无触发判别件（u1t 去 SCTLR.A 触发，判异常进入 vs 触发前毒性）。
  cat > "${OUTDIR}/u1u.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  .scratch 0x1FC0 : { KEEP(*(.scratch)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1u.ld" -o "${ELF}" "${HERE}/u1u-probe.S"
  BIN="${OUTDIR}/u1u.bin"; IMG="${OUTDIR}/boot-u1u.img"
elif [ "${MODE}" = "u1v" ]; then
  # 阶梯3 刀1v：物理层判别件（砍全部系统寄存器块，布局+scratch 原样）。
  cat > "${OUTDIR}/u1v.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  .scratch 0x1FC0 : { KEEP(*(.scratch)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1v.ld" -o "${ELF}" "${HERE}/u1v-probe.S"
  BIN="${OUTDIR}/u1v.bin"; IMG="${OUTDIR}/boot-u1v.img"

elif [ "${MODE}" = "u1w" ]; then
  # 阶梯3 刀1w：零 DRAM 写网标定（寄存器旗标，udf 触发，纯取指向量表）。
  cat > "${OUTDIR}/u1w.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1w.ld" -o "${ELF}" "${HERE}/u1w-probe.S"
  BIN="${OUTDIR}/u1w.bin"; IMG="${OUTDIR}/boot-u1w.img"
elif [ "${MODE}" = "u1x" ]; then
  # 阶梯3 刀1x：烤表 MMU 刀（页表=镜像常量，零运行时写；DRAM 往返同址对照 u1v）。
  # 段窗恰好铺满：.vectors [0x7C0,0xFC0) | .pgtable [0xFC0,0x1FC0) | .scratch [0x1FC0,0x1FC4)。
  cat > "${OUTDIR}/u1x.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  .pgtable 0xFC0 : { KEEP(*(.pgtable)) }
  .scratch 0x1FC0 : { KEEP(*(.scratch)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1x.ld" -o "${ELF}" "${HERE}/u1x-probe.S"
  BIN="${OUTDIR}/u1x.bin"; IMG="${OUTDIR}/boot-u1x.img"
elif [ "${MODE}" = "u1y" ]; then
  # 阶梯3 刀1y：sysreg 块判别（矩阵缺格：CurrentEL+VBAR、无触发、无 DRAM 写；
  # 闪长=EL 编码 1s/2s/3s）。
  cat > "${OUTDIR}/u1y.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  .vectors 0x7C0 : { KEEP(*(.vectors)) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1y.ld" -o "${ELF}" "${HERE}/u1y-probe.S"
  BIN="${OUTDIR}/u1y.bin"; IMG="${OUTDIR}/boot-u1y.img"
elif [ "${MODE}" = "u1aa" ]; then
  # 阶梯3 刀1-aa：mrs CurrentEL 单件（calib 骨架+唯一增量 mrs，闪长=EL
  # 编码 1s/2s/3s；刀0 对照后 sysreg 增量二分的第一步）。
  cat > "${OUTDIR}/u1aa.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1aa.ld" -o "${ELF}" "${HERE}/u1aa-probe.S"
  BIN="${OUTDIR}/u1aa.bin"; IMG="${OUTDIR}/boot-u1aa.img"
elif [ "${MODE}" = "u1ab" ]; then
  # 阶梯3 刀1-ab：mrs 隔离审（calib 原文+唯一增量 mrs CurrentEL 读而
  # 不用；u1aa 冻死后拆增量集二分的头号嫌疑单审）。
  cat > "${OUTDIR}/u1ab.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0;
  .text : { KEEP(*(.text.boot)) *(.text*) *(.rodata*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u1ab.ld" -o "${ELF}" "${HERE}/u1ab-probe.S"
  BIN="${OUTDIR}/u1ab.bin"; IMG="${OUTDIR}/boot-u1ab.img"
elif [ "${MODE}" = "u2" ]; then
  # 阶梯3 刀2：UFS 控制器活态（HCE/RSR 判 ABL 留态）。
  cat > "${OUTDIR}/u2.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u2.ld" -o "${ELF}" "${HERE}/u2-probe.S"
  BIN="${OUTDIR}/u2.bin"; IMG="${OUTDIR}/boot-u2.img"
elif [ "${MODE}" = "u3" ]; then
  # 阶梯3 刀3：UTR 往返（READ CAPACITY，OCS==0 判全链路通）。
  cat > "${OUTDIR}/u3.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3.ld" -o "${ELF}" "${HERE}/u3-probe.S"
  BIN="${OUTDIR}/u3.bin"; IMG="${OUTDIR}/boot-u3.img"
elif [ "${MODE}" = "u4" ]; then
  # 阶梯3 刀4：首块读（READ(10) LBA1 == "EFI PART" 判无内核读盘）。
  cat > "${OUTDIR}/u4.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4.ld" -o "${ELF}" "${HERE}/u4-probe.S"
  BIN="${OUTDIR}/u4.bin"; IMG="${OUTDIR}/boot-u4.img"
elif [ "${MODE}" = "u4b" ]; then
  # 阶梯3 刀4-b：512 字节世界问法（u4 唯改 DW1/exp_len/PRDT 三处尺寸）。
  cat > "${OUTDIR}/u4b.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4b.ld" -o "${ELF}" "${HERE}/u4b-probe.S"
  BIN="${OUTDIR}/u4b.bin"; IMG="${OUTDIR}/boot-u4b.img"
elif [ "${MODE}" = "u3b" ]; then
  # 阶梯3 刀3-b：READ CAPACITY 送达审（头字节序+PRDT@+0x100 对账修 + 非零门）。
  cat > "${OUTDIR}/u3b.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3b.ld" -o "${ELF}" "${HERE}/u3b-probe.S"
  BIN="${OUTDIR}/u3b.bin"; IMG="${OUTDIR}/boot-u3b.img"
elif [ "${MODE}" = "u4c" ]; then
  # 阶梯3 刀4-c：READ(10) 首证冲线（u4 + PRDT@+0x100 对账修）。
  cat > "${OUTDIR}/u4c.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4c.ld" -o "${ELF}" "${HERE}/u4c-probe.S"
  BIN="${OUTDIR}/u4c.bin"; IMG="${OUTDIR}/boot-u4c.img"
elif [ "${MODE}" = "u3c" ]; then
  # 阶梯3 刀3-c：响应 UPIU 审（resp@+0x80 transcode+SCSI 状态双门）。
  cat > "${OUTDIR}/u3c.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3c.ld" -o "${ELF}" "${HERE}/u3c-probe.S"
  BIN="${OUTDIR}/u3c.bin"; IMG="${OUTDIR}/boot-u3c.img"
elif [ "${MODE}" = "u4d" ]; then
  # 阶梯3 刀4-d：全契约源码版（DW7 半修 0x00010100 + PRDT 归位 +0x400）。
  cat > "${OUTDIR}/u4d.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4d.ld" -o "${ELF}" "${HERE}/u4d-probe.S"
  BIN="${OUTDIR}/u4d.bin"; IMG="${OUTDIR}/boot-u4d.img"
elif [ "${MODE}" = "u4e" ]; then
  # 阶梯3 刀4-e：512 世界版（u4d 唯改 DW1/exp_len/PRDT 三处尺寸）。
  cat > "${OUTDIR}/u4e.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4e.ld" -o "${ELF}" "${HERE}/u4e-probe.S"
  BIN="${OUTDIR}/u4e.bin"; IMG="${OUTDIR}/boot-u4e.img"
elif [ "${MODE}" = "u3d" ]; then
  # 阶梯3 刀3-d：READ CAPACITY 分诊（DW0 代际修 0x14000000 + exp_len rev 通式 + tag=槽位）。
  cat > "${OUTDIR}/u3d.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3d.ld" -o "${ELF}" "${HERE}/u3d-probe.S"
  BIN="${OUTDIR}/u3d.bin"; IMG="${OUTDIR}/boot-u3d.img"
elif [ "${MODE}" = "u3e" ]; then
  # 阶梯3 刀3-e：u3d 单变量——UTRLBA 64 位写→32 位写（UFSDxe 全 32 位访问）。
  cat > "${OUTDIR}/u3e.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3e.ld" -o "${ELF}" "${HERE}/u3e-probe.S"
  BIN="${OUTDIR}/u3e.bin"; IMG="${OUTDIR}/boot-u3e.img"
elif [ "${MODE}" = "u3g" ]; then
  # 阶梯3 刀3-g：时延编码仪器——任何结局复位，阶段编码进等待秒数（CNTVCT）。
  cat > "${OUTDIR}/u3g.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3g.ld" -o "${ELF}" "${HERE}/u3g-probe.S"
  BIN="${OUTDIR}/u3g.bin"; IMG="${OUTDIR}/boot-u3g.img"
elif [ "${MODE}" = "u3h" ]; then
  # 阶梯3 刀3-h：stage3 细分——响应 UPIU 头四分类，等待秒数直编。
  cat > "${OUTDIR}/u3h.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3h.ld" -o "${ELF}" "${HERE}/u3h-probe.S"
  BIN="${OUTDIR}/u3h.bin"; IMG="${OUTDIR}/boot-u3h.img"
elif [ "${MODE}" = "u3i" ]; then
  # 阶梯3 刀3-i：读 sense key + UNIT ATTENTION 重试一次，等待秒数直编。
  cat > "${OUTDIR}/u3i.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3i.ld" -o "${ELF}" "${HERE}/u3i-probe.S"
  BIN="${OUTDIR}/u3i.bin"; IMG="${OUTDIR}/boot-u3i.img"
elif [ "${MODE}" = "u3j" ]; then
  # 阶梯3 刀3-j：SSU ACTIVE→W-LUN 0xD0 唤醒，再 READ CAPACITY（UA 重试），等待秒数直编。
  cat > "${OUTDIR}/u3j.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u3j.ld" -o "${ELF}" "${HERE}/u3j-probe.S"
  BIN="${OUTDIR}/u3j.bin"; IMG="${OUTDIR}/boot-u3j.img"
elif [ "${MODE}" = "u4f" ]; then
  # 阶梯3 刀4-f：SSU 唤醒后 READ(10) LUN0 LBA1 → "EFI PART" magic 门（读盘首证）。
  cat > "${OUTDIR}/u4f.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4f.ld" -o "${ELF}" "${HERE}/u4f-probe.S"
  BIN="${OUTDIR}/u4f.bin"; IMG="${OUTDIR}/boot-u4f.img"
elif [ "${MODE}" = "u4g" ]; then
  # 阶梯3 刀4-g：GPT 分区表自校验读——头 CRC32+条目数组多块读 CRC32。
  cat > "${OUTDIR}/u4g.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4g.ld" -o "${ELF}" "${HERE}/u4g-probe.S"
  BIN="${OUTDIR}/u4g.bin"; IMG="${OUTDIR}/boot-u4g.img"
elif [ "${MODE}" = "u4h" ]; then
  # 阶梯3 刀4-h：u4g 读 #2 缓冲改 0xB0003000 + 分命令超时码（x29）。
  cat > "${OUTDIR}/u4h.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4h.ld" -o "${ELF}" "${HERE}/u4h-probe.S"
  BIN="${OUTDIR}/u4h.bin"; IMG="${OUTDIR}/boot-u4h.img"
elif [ "${MODE}" = "u4i" ]; then
  # 阶梯3 刀4-i：SSU→读 LBA2→读 LBA1 分诊第三条命令，敲铃前旁读 RSR/IS。
  cat > "${OUTDIR}/u4i.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4i.ld" -o "${ELF}" "${HERE}/u4i-probe.S"
  BIN="${OUTDIR}/u4i.bin"; IMG="${OUTDIR}/boot-u4i.img"
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
