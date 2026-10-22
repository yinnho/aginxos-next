#!/bin/bash
# redfin 裸 substrate 出件：zig cc freestanding → arm64 Image 壳 → LZ4 帧修块
# → mkbootimg v3。产物 .local/device/redfin/boot-out/boot-heartbeat.img（默认）
# 或 boot-bare.img（--full，研究件）。S0 起另有 Rust 线：--s01（本目录
# 设备 crate + crates/bare 平台库，cargo 出件后走同一打包尾巴）。
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
[ "${1:-}" = "--u4j" ] && MODE="u4j"
[ "${1:-}" = "--u4k" ] && MODE="u4k"
[ "${1:-}" = "--u5a" ] && MODE="u5a"
[ "${1:-}" = "--s01" ] && MODE="s01"
[ "${1:-}" = "--s02" ] && MODE="s02"
[ "${1:-}" = "--s03" ] && MODE="s03"
[ "${1:-}" = "--s04" ] && MODE="s04"
[ "${1:-}" = "--s05" ] && MODE="s05"
[ "${1:-}" = "--s06" ] && MODE="s06"
[ "${1:-}" = "--s06b" ] && MODE="s06b"
[ "${1:-}" = "--s11" ] && MODE="s11"
[ "${1:-}" = "--s12" ] && MODE="s12"
[ "${1:-}" = "--s12b" ] && MODE="s12b"
[ "${1:-}" = "--s13" ] && MODE="s13"
[ "${1:-}" = "--s13b" ] && MODE="s13b"
[ "${1:-}" = "--s13c" ] && MODE="s13c"
[ "${1:-}" = "--s13d" ] && MODE="s13d"
[ "${1:-}" = "--s14" ] && MODE="s14"
[ "${1:-}" = "--s15" ] && MODE="s15"
[ "${1:-}" = "--s16" ] && MODE="s16"
[ "${1:-}" = "--s16b" ] && MODE="s16b"
[ "${1:-}" = "--s17" ] && MODE="s17"
[ "${1:-}" = "--s18" ] && MODE="s18"
[ "${1:-}" = "--s19" ] && MODE="s19"
[ "${1:-}" = "--s20" ] && MODE="s20"
[ "${1:-}" = "--s21" ] && MODE="s21"
[ "${1:-}" = "--s22" ] && MODE="s22"
[ "${1:-}" = "--s23" ] && MODE="s23"
[ "${1:-}" = "--s24" ] && MODE="s24"
[ "${1:-}" = "--s25" ] && MODE="s25"
[ "${1:-}" = "--s26" ] && MODE="s26"
[ "${1:-}" = "--s27" ] && MODE="s27"
[ "${1:-}" = "--s28" ] && MODE="s28"
[ "${1:-}" = "--s29" ] && MODE="s29"
[ "${1:-}" = "--s30" ] && MODE="s30"
[ "${1:-}" = "--s31" ] && MODE="s31"
[ "${1:-}" = "--s32" ] && MODE="s32"
[ "${1:-}" = "--s33" ] && MODE="s33"
[ "${1:-}" = "--s34" ] && MODE="s34"
[ "${1:-}" = "--s35" ] && MODE="s35"
[ "${1:-}" = "--s36" ] && MODE="s36"
[ "${1:-}" = "--s37" ] && MODE="s37"
[ "${1:-}" = "--s38" ] && MODE="s38"
[ "${1:-}" = "--s39" ] && MODE="s39"
[ "${1:-}" = "--s40" ] && MODE="s40"
[ "${1:-}" = "--s41" ] && MODE="s41"
[ "${1:-}" = "--s42" ] && MODE="s42"
[ "${1:-}" = "--s44" ] && MODE="s44"
[ "${1:-}" = "--s45" ] && MODE="s45"
[ "${1:-}" = "--s46" ] && MODE="s46"
[ "${1:-}" = "--s47" ] && MODE="s47"
[ "${1:-}" = "--s48" ] && MODE="s48"
[ "${1:-}" = "--s49" ] && MODE="s49"
[ "${1:-}" = "--s50" ] && MODE="s50"
[ "${1:-}" = "--s51" ] && MODE="s51"
[ "${1:-}" = "--s52" ] && MODE="s52"
[ "${1:-}" = "--s53" ] && MODE="s53"
[ "${1:-}" = "--s54" ] && MODE="s54"
mkdir -p "${OUTDIR}"
ELF="${OUTDIR}/heartbeat.elf"

CFLAGS="-O2 -ffreestanding -nostdlib -fno-builtin -fno-stack-protector \
  -fno-asynchronous-unwind-tables -fomit-frame-pointer -Wl,--build-id=none"

echo "==> 出件 (${MODE})"
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
elif [ "${MODE}" = "u4j" ]; then
  # 阶梯3 刀4-j：u4h 流程 + crc 多项式改 w13（修踩 x3 HCI 基址）。
  cat > "${OUTDIR}/u4j.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4j.ld" -o "${ELF}" "${HERE}/u4j-probe.S"
  BIN="${OUTDIR}/u4j.bin"; IMG="${OUTDIR}/boot-u4j.img"
elif [ "${MODE}" = "u4k" ]; then
  # 阶梯3 刀4-k：u4j + 按 GPT 名找 boot_b、读首块验 ANDROID!。
  cat > "${OUTDIR}/u4k.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u4k.ld" -o "${ELF}" "${HERE}/u4k-probe.S"
  BIN="${OUTDIR}/u4k.bin"; IMG="${OUTDIR}/boot-u4k.img"
elif [ "${MODE}" = "u5a" ]; then
  # 阶梯5 刀5-a：写路径首证——boot_a 尾 scratch 块（名查+界检）WRITE(10) FUA + 回读逐字比。
  cat > "${OUTDIR}/u5a.ld" <<'LD'
ENTRY(_start)
SECTIONS { . = 0x0; .text : { KEEP(*(.text.boot)) *(.text*) }
  /DISCARD/ : { *(.eh_frame*) *(.eh_frame_hdr) *(.comment) *(.note*) } }
LD
  zig cc -target aarch64-freestanding-none ${CFLAGS} -Wl,-T,"${OUTDIR}/u5a.ld" -o "${ELF}" "${HERE}/u5a-probe.S"
  BIN="${OUTDIR}/u5a.bin"; IMG="${OUTDIR}/boot-u5a.img"
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
elif [ "${MODE}" = "s01" ]; then
  # NOLINUX-PLAN S0-1：Rust 入口复刻 u4j（SSU 唤醒+GPT 双 CRC）。
  # crates/bare 平台库（独立 workspace）+ 本目录设备 crate；机器事实
  # 走 bare.toml→build.rs→facts.rs；rust.ld 链 ABL 落点+0x40。
  # 注：S0-2 起 main.rs 已换日志环正身——本模式出的件即当前 main.rs；
  # 复现 S0-1 行为 checkout 42aa96c。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s01.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s01.bin"; IMG="${OUTDIR}/boot-s01.img"
elif [ "${MODE}" = "s02" ]; then
  # NOLINUX-PLAN S0-2：UFS 日志环（自检链前置 + boot_a scratch 256 块环、
  # 心跳落盘、成功即复位）。同一设备 crate 换 main.rs 内容出件。
  # 注：S0-3 起 main.rs 已换异常向量正身——本模式出的件即当前 main.rs；
  # 复现 S0-2 行为 checkout 83135c7。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s02.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s02.bin"; IMG="${OUTDIR}/boot-s02.img"
elif [ "${MODE}" = "s03" ]; then
  # NOLINUX-PLAN S0-3：异常向量 + EL 定谳（VBAR 装订 → 故意非对齐 ldr →
  # ESR/ELR/FAR/CurrentEL 落环 → 复位）。
  # 注：S0-4 起 main.rs 已换 MMU 正身——本模式出的件即当前 main.rs；
  # 复现 S0-3 行为 checkout 53cb818。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s03.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s03.bin"; IMG="${OUTDIR}/boot-s03.img"
elif [ "${MODE}" = "s04" ]; then
  # NOLINUX-PLAN S0-4：MMU 恒等映射 + cache 使能（.bss 双表 L1/L2、
  # DMA 窗 NC；cache-off/on GPT 双 CRC 计时对比落环 + 256 块扫环计时）。
  # 注：S0-5 起 main.rs 已换中断正身——本模式出的件即当前 main.rs；
  # 复现 S0-4 行为 checkout e5776e8。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s04.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s04.bin"; IMG="${OUTDIR}/boot-s04.img"
elif [ "${MODE}" = "s05" ]; then
  # NOLINUX-PLAN S0-5：GICv3 + 通用定时器中断（RD 唤醒/dist 使能/SRE/
  # PMR/IGRPEN1；虚拟定时器 INTID 19 武装 1s → daifclr → WFI → 槽 5
  # IRQ 入口记 id/dt/ctl 落环复位）。复现 S0-5 行为 checkout 69e6885。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s05.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s05.bin"; IMG="${OUTDIR}/boot-s05.img"
elif [ "${MODE}" = "s06" ]; then
  # NOLINUX-PLAN S0-6：PSCI CPU_ON 拉起 7 副核（version 落环→清
  # mailbox→CPU_ON×7 rc 落环→3s WFI→IRQ 入口收割 7 槽心跳落环）。
  # 复现 S0-6 行为 checkout 2f511f4。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s06.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s06.bin"; IMG="${OUTDIR}/boot-s06.img"
elif [ "${MODE}" = "s06b" ]; then
  # NOLINUX-PLAN S0-6b：RD_TYPER 全表探针（S0-6 链路 + 主核裸 MPIDR
  # 落环 + 8 帧 RD_TYPER 逐帧落环——帧↔真 MPIDR↔DT target 三角定谳）。
  # 复现 S0-6b 行为 checkout d0dcc91。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s06b.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s06b.bin"; IMG="${OUTDIR}/boot-s06b.img"
elif [ "${MODE}" = "s11" ]; then
  # NOLINUX-PLAN S1-1：SPMI v5 运输层只读探针（S0 链路 + ⑩ v5 判定/
  # CHGR+MISC 全副本 owner 落环/充电状态三寄存器 5 采样×4s——喂狗写
  # 路径留 S1-2）。复现 S1-1 行为 checkout 1880663。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s11.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s11.bin"; IMG="${OUTDIR}/boot-s11.img"
elif [ "${MODE}" = "s12" ]; then
  # NOLINUX-PLAN S1-2：USBIN 判别刀（S1-1 链路 + ⑩ 写路径首上 Rust 线：
  # 基线读→解挂 RMW 0x1340 bit0←0+读回→使能 RMW 0x1042 bit0←1+读回→
  # 后采样 5×4s——判 PLUGIN 能否无 TCPM 抬起）。复现 S1-2 行为 checkout 62d7f71。
  # 注：S1-2 自身寻址错（flat 高字节=外设号），判读无效——S1-2b 为修正刀。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s12.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s12.bin"; IMG="${OUTDIR}/boot-s12.img"
elif [ "${MODE}" = "s12b" ]; then
  # NOLINUX-PLAN S1-2b：USBIN 判别修正刀（flat 高字节=外设号纠错：
  # USBIN=PPID 0x213、TYPEC=PPID 0x215；真 0x1310 PLUGIN + Type-C
  # 0x1506/09/0B/0D/0F/0x1544 面 + 解挂写真身 0x213+0x40）。
  # 复现 S1-2b 行为 checkout 6d3c98c。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s12b.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s12b.bin"; IMG="${OUTDIR}/boot-s12b.img"
elif [ "${MODE}" = "s13" ]; then
  # NOLINUX-PLAN S1-3：ka v3 狗面刀（WD_CFG 0x1651 基线→武装 RMW→
  # 喂 0x1643×5@4s→停喂观察 12×5s 判 bark/bite→幸存解除武装）。
  # 注：S1-3 短命令只带 5 位地址（0x51→0x11 错位），aw=0x0 悬案——
  # S1-3b 为 EXT 重制刀。复现 S1-3 行为 checkout 8adcc38。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s13.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s13.bin"; IMG="${OUTDIR}/boot-s13.img"
elif [ "${MODE}" = "s13b" ]; then
  # NOLINUX-PLAN S1-3b：狗面刀 EXT 重制（短命令 5 位地址截断案定谳，
  # 运输层全换 EXT_READ=13/EXT_WRITE=2）：真基线 w0→写驻双试验 c42/m53
  # →武装梯 aw2（+0ms/+1s/+5s 回读）→喂×2@4s→停喂观察 8×5s→幸存解除。
  # 注：S1-3b 判读=8 位 EXT 空间在 PM7250B 是死区（写 ACK 全弃/读疑似
  # 低页别名）——S1-3c 为 EXT-LONG 重制刀。复现 S1-3b 行为 checkout 9ee4014。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s13b.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s13b.bin"; IMG="${OUTDIR}/boot-s13b.img"
elif [ "${MODE}" = "s13c" ]; then
  # NOLINUX-PLAN S1-3c：狗面刀 EXT-LONG 重制（8 位 EXT 死区案定谳，
  # 运输层换 WRITEL=op0/READL=op1——内核 charger regmap flat 16 位地址
  # >0xFF 的唯一走法）：梯子同 S1-3b，观察窗 12×5s+解除前补喂。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s13c.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s13c.bin"; IMG="${OUTDIR}/boot-s13c.img"
elif [ "${MODE}" = "s13d" ]; then
  # NOLINUX-PLAN S1-3d：备用位判别刀（S1-3c 定谳——EXT-LONG 写与内核
  # fmt_cmd_v2/write_data 逐位同仍不驻；差别在世界不在命令）：sp54=
  # CHGR 0x1054 官方 CHARGER_SPARE 翻位读回判「页死 vs 寄存器个案」；
  # 对照 c42 + 武装梯 aw3 + 喂一记 + 150s 停喂观察（POR BARK=128s 档，
  # bark 升起截环落 bk）+ 解除武装。复现 S1-3c 行为 checkout 472b071。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s13d.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s13d.bin"; IMG="${OUTDIR}/boot-s13d.img"
elif [ "${MODE}" = "s14" ]; then
  # NOLINUX-PLAN S1-4：充电控制刀（S1-3d 定谳后写已可信）：FV 0x1070 压
  # −100mV 观察 0x1006 bits[2:0] 行为变（近满预期 4 TAPER→5 TERMINATE、
  # 还原回 4）+ ICL 0x1341/FCC 0x1061 旋钮写驻 + QG VBAT 对照读。
  # 编码律=battery-truth §1 单字节 (µ值−min)/step。复现 s13d checkout 1c36568。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s14.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s14.bin"; IMG="${OUTDIR}/boot-s14.img"
elif [ "${MODE}" = "s15" ]; then
  # NOLINUX-PLAN S1-5：电量计读刀（s14 挂账定谳——0x80 读=0x8000 是
  # FIFO 空哨兵非门）：QG 全读集=ID/状态/LAST_ADC_V 0xC0/LAST_ADC_I
  # 0xC2/MSOC 0xBF/双 OCV/S2 平均 + HOLD 协议示范 + SDAM 三读 +
  # 5×2s 自走活性窗。换算 V=raw×194637/1000µV、I=raw×152588/1000µA
  # （subtype 3）。复现 s14 checkout cd7c2cc。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s15.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s15.bin"; IMG="${OUTDIR}/boot-s15.img"
elif [ "${MODE}" = "s16" ]; then
  # NOLINUX-PLAN S1-6：温控读刀（VADC=PPID 0x231 外设 0x3100，
  # qcom,spmi-adc5）：六通道一发读 die_temp 0x02/vph_pwr 0x83/
  # vbat_sns 0x84/chg_temp 0x09/bat_therm 0x4a/smb1390_therm 0x0e，
  # DIG_PARAM 0x28/0x18 + EOC 轮询 + DATA0/1；终态还原基线六寄存器；
  # 4×2s die_temp 活性窗。换算 full_scale=0x70e4（V=raw×1875000/28900
  # ×den；die m°C=raw×…/2−273150；chg=377500−V/4；smb=419400−V×100/356）。
  # 复现 s15 checkout 32f27b1。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s16.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s16.bin"; IMG="${OUTDIR}/boot-s16.img"
elif [ "${MODE}" = "s16b" ]; then
  # NOLINUX-PLAN S1-6 修正刀：s16 首发射 die_temp 误用通道 0x02（dtsi
  # 节名 @2 是旧 VADC 编号残影，真值=qcom,spmi-vadc.h ADC_DIE_TEMP=0x06）
  # 读成浮脚 1.874V；本刀 die→0x06 + ref_gnd 0x00/vref_1p25 0x01 锚点
  # （应 0V/1.25V 免费刻度校准）。复现 s16 checkout 5bd86bc。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s16b.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s16b.bin"; IMG="${OUTDIR}/boot-s16b.img"
elif [ "${MODE}" = "s17" ]; then
  # NOLINUX-PLAN S1-7 常驻 soak 刀：不再三轮自毁——ka.S 同法喂狗（MISC
  # 0x1643←1 每 30s）+ 每 10min 采一包两字落环（QG V/I/MSOC + CHGR
  # 0x1006/0x1042 + VADC die/chg/bat_therm/vbat），永续 loop 不复位。
  # S1「常驻≥1 天电量不降」真考体；环 256 块 ≈ 1.7 天滚动历史；病发
  # PMIC 硬复位烧一命、3 发自动回 b 槽自愈。复现 s16b checkout 20eb767。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s17.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s17.bin"; IMG="${OUTDIR}/boot-s17.img"
elif [ "${MODE}" = "s18" ]; then
  # NOLINUX-PLAN S2 正题刀：s17 全部面（开机自检+soak 环）不变，⑩-2 加
  # ext4 只读正题——GPT 名查 userdata → UfsDev（read10 弹 DATA NC 窗）
  # → ext4::Fs::open → 三靶 resolve+64KB 分段读+增量 crc32 落环。验收=
  # 同晚 L0 侧 md5/crc 对账「读出与 L0 一致」。复现 s17 checkout b198974。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s18.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s18.bin"; IMG="${OUTDIR}/boot-s18.img"
elif [ "${MODE}" = "s19" ]; then
  # NOLINUX-PLAN S3 首刀：s18 全部面（soak 环+ext4）不变，⑩-3 加 USB
  # HS-only 设备面上电（配方真源 S3-USB.md）——交接残态四读 → GDSC/
  # BCR/五时钟腿/QUSB2 十六步 PHY → dwc3 软复位+GCTL 设备向+HS-only
  # QSCRATCH 三步+事件缓冲+DCTL 上线 → 2s 枚举窗后 DSTS/GEVTCOUNT/首
  # 事件字落环。验收=Mac 端 system_profiler 见新高速设备 + 环读数互证。
  # 复现 s18 checkout 217612b。宿主先 check.sh（crates/bare 平台面）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s19.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s19.bin"; IMG="${OUTDIR}/boot-s19.img"
elif [ "${MODE}" = "s22" ]; then
  # 刀 G23：s21 同面，大包（>512）两件照办——还 intent + tftp 记账
  # （tf_note_ack/tf_note_data），QMI 深解析仍小包专属。#22 定谳：G14
  # 的 512 跳步把写客户端整块 DATA 全弄瞎，WRQ 会话开而 DATA 永不见。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s22.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s22.bin"; IMG="${OUTDIR}/boot-s22.img"
elif [ "${MODE}" = "s23" ]; then
  # 刀 G23b：s22 同面，四件——①非 so RRQ 一律 ERROR-1 新口（#23 定谳：
  # modem_efs 撞 !tf_is_so() 静默 drop，6+ 笔换源口重试全吞→客户端弃线）；
  # ②入站 cmd7 催件应答（cmd5 补一枚+cmd8 granted，lpass 同款；旧哑跳=
  # 对端预算耗尽即哑死）③cmd12 CONT 入步进器（旧落 break 楔死洞）；
  # ④意图经济台账 rg=/c9=/bg=/i7=（入还队/还清/cmd9 liid/大包五元组）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s23.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s23.bin"; IMG="${OUTDIR}/boot-s23.img"
elif [ "${MODE}" = "s24" ]; then
  # 刀 G24：s23 同面，写腿知名口 + 观察三件——#24 阶梯首槽 dst=0x1011
  # 定谳：客户端写方言 WRQ 发 0x1000+0x11（读腿才用广告口 0x11），旧
  # 武装门整段吞 WRQ→写会话无 OACK 无 ERROR 全静。修：武装门/note_data
  # 门收 0x1011，写 OACK/ACK 回程口镜像来程（TF_W_VIA）；.so 已在 #24
  # 完整送达（tb=3+客户端 ERROR-9 收幕）。观察：pr= 请求路径环（非 so
  # 要哪个文件）、bx= 首枚大包源口+载荷头（0x16 IPA 说 vs 写 DATA 说）、
  # wx= 写腿三账。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s24.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s24.bin"; IMG="${OUTDIR}/boot-s24.img"
elif [ "${MODE}" = "s25" ]; then
  # 刀 G25：s24 同面，msgid 0x23 巨 REQ 应答腿——#25 bx= 定谳客户端发
  # 7784B qtype0/msgid0x23 巨 REQ 打 IPA 服务口 0x16，旧无腿只记账、客
  # 户端等回全静（.so 收幕即哑）。0x20/0x27/0x35 三腿先例：小 RESP TLV
  # 0x02 成功。观察加宽：wx= 扩武装源口/撞门数/撞门口（两发 WRQ 判据）、
  # pr= 28→56B（modem_pr/so/549_38_0.mbn 全长可罩）、i2= 0x23 腿三账。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s25.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s25.bin"; IMG="${OUTDIR}/boot-s25.img"
elif [ "${MODE}" = "s26" ]; then
  # 刀 G26：s25 同面，巨 REQ 腿搬家——#26 定谳 i2=0.0.0：0x23 巨 REQ 走
  # cmd9 大包短路（G23 自供「QMI 深解析仍限小包」），II 区全程只见 0x27
  # （im=27.27.1）。修：大包分支就地头四字深解析，0x23 武装 I2、0x35 武
  # 装 IC（ic=0.0 各靴恒零疑同因）。观察：wx= 扩第 7 位 op2 总数、wl=
  # op2 全量环（最近 4 发 dst.sport，#26 三方矛盾 VIA=11/BLV=11/阶梯槽
  # 1011 在 N/BL 两账内无法自洽）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s26.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s26.bin"; IMG="${OUTDIR}/boot-s26.img"
elif [ "${MODE}" = "s27" ]; then
  # 刀 G27：s26 同面，纯观察零协议变化——#27 i2=1.1.20（0x23 首答）但
  # 客户端仍全静（rl/rq/c9/i7/bg 全冻）、ic=0.0（0x35 从未发）。观察：
  # wn= 武装 WRQ 文件名快照（写目标从未被捕——客户端想写什么）；
  # wl= 环 4→6 槽+op 位+tftp 口过滤（#27 假阳=打 0x12 的 QMI REQ 头
  # 00 02 撞 op2）+钳位修。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s27.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s27.bin"; IMG="${OUTDIR}/boot-s27.img"
elif [ "${MODE}" = "s28" ]; then
  # 刀 G28：写腿会话口对齐 stock——#28 破案 wn=/readwrite/server_check.txt
  # + wl=11.1f.2 单发零重试零 DATA；0x1011 悬案定谳 rl 渲染拼接伪影
  # （N 直接拼首槽 dst）。修：写 OACK/ACK 从新会话口（stock handle_wrq
  # qrtr_open(0)，tf_new_port 0x1000+）、客户端回会话口全闸收（on_wx）、
  # tsize 键在场即回、wsize 去 min(16)。观察：wo= WRQ 选项段快照（五靴
  # 零观测盲区）+ rl N/首槽补分隔。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s28.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s28.bin"; IMG="${OUTDIR}/boot-s28.img"
elif [ "${MODE}" = "s29" ]; then
  # 刀 G29（纯观察，零行为变更，保 #29 基线净）：#29 判决 wo= 首见
  # (blksize7680/tmo1000/wsize1)、写腿 OACK 两种源口客户端同样全静零
  # 重试、+595s 第九证、狗属主翻案 modem 侧 tmr_slave3（APSS 喂狗刀
  # 作废）。观察三件：po= 全口 RX 直方图（QMI-14/servreg 静默丢弃的
  # 探测口有账）、oe= 写 OACK 字节回显（对 stock 逐字节审计）、
  # ws= 会话口闩值直读。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s29.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s29.bin"; IMG="${OUTDIR}/boot-s29.img"
elif [ "${MODE}" = "s30" ]; then
  # 刀 G30（纯观察，零行为变更，保 #34 基线净）：#34 三首数=oe= 与 stock
  # 39B 逐字节全同仍被客户端无视、ws=1000 会话口、po= 五未应答口
  # (10/12/14/15/16)+底噪 fffffffe 爬升。本刀 pc=：未应答口最近一包
  # 抄 64B（口.半字节hex.64）——「敲门」升格「递信」，答什么看完信再裁。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s30.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s30.bin"; IMG="${OUTDIR}/boot-s30.img"
elif [ "${MODE}" = "s31" ]; then
  # 刀 G30b（纯观察，零行为变更，叠在 G30 pc= 之上）：#34 po=fffffffe.38
  # 定谳=QRTR_PORT_CTRL(0xfffffffe) 控制报文非底噪——56 封信整条线只计
  # 数没读过。三件：ch= 控制包 typ 直方图（HELLO/NEW_SERVER/LOOKUP…）、
  # cc= 最近一枚五元组 typ.svc.inst.node.port、lk= NEW_LOOKUP 目击闩
  # SMEM_LOOK 上屏（G3 起只写不读的暗闩——modem 在找什么服务从此可见）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s31.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s31.bin"; IMG="${OUTDIR}/boot-s31.img"
elif [ "${MODE}" = "s32" ]; then
  # 刀 G31（纯观察，零行为变更，叠在 G30/G30b 之上）：#36 三盲一次补齐——
  # ① sm 行稳超 CTRL_REPLY(3072)，q= 服务图自 ~#27 起被截尾（40 枚
  # NEW_SERVER 锁在暗闛）；② cc=4.0.0.0.0 末枚全零（真空通告 vs 短包守卫
  # 两解）——ccz= 计零组、cc= 只存末枚有肉的；③ DEL_CLIENT×15 是谁在关
  # ——dc= client{node.port}；④ lk= 撞名改 ml=；⑤ SMEM_SVC 8→40 对全量。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s32.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s32.bin"; IMG="${OUTDIR}/boot-s32.img"
elif [ "${MODE}" = "s33" ]; then
  # 刀 G32（一行刀，零行为变更）：#37 现场 LEN=3072 整截——G31 只扩了
  # CTRL_REPLY 忘了 TCP_TX（G21 配对尺寸律：send_slice 装不下的静默截尾，
  # partial send 仍 Ok）。TCP_TX 3072→8192 同步；q=/iq= 才真上屏。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s33.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s33.bin"; IMG="${OUTDIR}/boot-s33.img"
elif [ "${MODE}" = "s34" ]; then
  # 刀 G33（观察域两修，零协议行为变更）：#38 现场 q= 空 + iq= 闩上同窗
  # 矛盾定谳——scan_new_servers 每轮清零 SMEM_SVC/LOOK=窗口域，宣告风暴
  # 在开机头几秒、渲染时早翻页。① q=/ml= 改累计+去重闩；② 守卫
  # 56/60/64/68 全降 52（iq= 以 52 读 +64 拿到真 port 旁证），仅 union
  # 型别（4/5/6/10/11）读元组——cc=/dc= 首读真值，15 连闭身份解锁。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s34.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s34.bin"; IMG="${OUTDIR}/boot-s34.img"
elif [ "${MODE}" = "s35" ]; then
  # 刀 G34（观察刀，零协议行为变更）：#39 假说——modem PD 机制拿不到
  # PD map（svc 64@0x12 公告无应答 po= 0x12×4、rmtfs 0x10×8）→ wlan_pd
  # 不生 → 无 0x45。定向包环：0x12/0x10 各末 2 包 ×96B；pd= 解码
  # n.tf.msg.txn.service_name（参照=redbull service-locator*.c：
  # GET_DOMAIN_LIST_REQ=0x0021 TLV 0x01=名字串）、pdh= 96B 全hex
  # （G35 应答刀逐字节参照）、rm= 0x10 末包头解码。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s35.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s35.bin"; IMG="${OUTDIR}/boot-s35.img"
elif [ "${MODE}" = "s36" ]; then
  # 刀 G34b（G34 致盲自修）：#40 靴实证 pdh= 恒空而 po=12.4 有包——96 门
  # 对 ~76B 的 GET_DOMAIN_LIST 永不真（G33 短包守卫律复发，自家法自家撞）。
  # 修=捕获门 52+off 界守卫（union 全在 fifo）；pd= 名解改 @58 无前缀可打印
  # 串（pe=1 定谳的 modem 方言，与 pd_take_name 同位）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s36.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s36.bin"; IMG="${OUTDIR}/boot-s36.img"
elif [ "${MODE}" = "s37" ]; then
  # 刀 G36（#41 三缺口修）：①PD_SEEN 去重键 (sport<<16)|txn（0x10 腿实证
  # 多客户端各自 txn=1 起算，裸 txn 撞号错杀）②0x20/0x22 应答腿（此前被
  # 无声吞；canon=service-locator-private.h 两 resp 皆仅 TLV02，15B 整包）
  # ③一深二队列（PD_PEND 占位期同圈双敲不丢，先存 slot1 名再解析再对换）
  # +pd0=/pr20=/pr22= 渲染 + 0x12 武装门 56→55（裸 0x20=48+7 也进门）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s37.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s37.bin"; IMG="${OUTDIR}/boot-s37.img"
elif [ "${MODE}" = "s38" ]; then
  # 刀 G38（#43 定谳后的四观察一解耦一杆）：①CFG_ARM 与 sm 解耦（旧 sm
  # 每发暗臂 IPA CONFIG 应答=观察者效应；新 cf 命令显式臂+cfa= 渲染）
  # ②DB_UPDATED ind 探针（第 3 笔 0x21 答成后 0x23 tf=4 一发→末敲者，
  # 客户端重查否=PD 框架 ind 道生死）③wd= 写腿三时刻（WRQ/OACK/末
  # DATA——全静案的「何时」盲区）④pk14= 0x14 定向抄盘（po= 0x14×1
  # 唯一未解口，52 门非 96——G33 短包律）⑥rp 手动杆（RESTART_PD 绕
  # DOWN 门，UNINIT 人为踢；rpm= 计数+SR_RP_ERR 果）。观察器白名单
  # 同步扩（白名单盲区律）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s38.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s38.bin"; IMG="${OUTDIR}/boot-s38.img"
elif [ "${MODE}" = "s39" ]; then
  # 刀 G39（#45 两命同形铁证后的拔毒+可见性刀）：①ind 探针降级 pi 显式臂
  # （G38② 自动臂实锤毒死 TX 消费：xl= 尾笔两命同 12.38.cb8、TXK_N ~1/s
  # 常踢=环非空 tail 永驻 → sr_tick 空环门永早退 → rp 发不出——观察者效应
  # 病族第二刀）②w45= 提前到 sm 头部（pk14 192 字 hex 洪峰把尾部 w45=
  # 挤过 TCP_TX 帽，本线头条指标整靴不可见=盲区律第四犯）③mh= MPSS TX
  # 环现场读数（sm 头部+tx 命令各一份；环卡死是 sr_tick 全部后续的总闸
  # 却从无一栏可见）④pk14 瘦身（sm 只留计数，96B 全hex 移 pk 命令）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s39.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s39.bin"; IMG="${OUTDIR}/boot-s39.img"
elif [ "${MODE}" = "s40" ]; then
  # 刀 G40（#46 后 EFS 挂载死亡组·四查·纯观察）：rmtfs 应答链已全绿
  # （ik=4.0/rd=0/8 请求全 drain），modem 在 +51s 停发 rfs 后静默 9 分钟
  # 等狗杀——本刀把「它到底在等谁」照亮：①r8= 0x10 口末 4 包摘要环
  # （末包形状≠全集教训）②tl= 每口最后包到达秒（51s 停点全景）③ro=
  # OPEN 流水（sport.pathlen.matched.cid ≤4 笔——两笔 OPEN 各是谁）
  # ④sd= seed 抽块保真对账（首/尾块重读 vs 影子；seed 搬坏=EFS 挂载
  # 死第一嫌疑）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s40.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s40.bin"; IMG="${OUTDIR}/boot-s40.img"
elif [ "${MODE}" = "s41" ]; then
  # 刀 G41+G42（#47 后·纯观察·合弹）：G41=GET_DOMAIN_LIST 问句名环
  # dq=（6 槽×24B 旧→新+总数；pn= 只闩 elf_loader 类的盲区补洞）。
  # G42=SO 头保真窗：th= ELF 魔数+e_type/e_machine（采集已久从未渲染），
  # sh=/sh2= elf_loader 实读偏移 0(64B)/0xa0(52B) 的原字节——#47 实锤
  # elf_loader 头两读后静默弃单（17 笔 RRQ 全自家 mbn、tb=3.1），送出的
  # 字节是否合法 ELF/MBN 一翻便知。零行为变化。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s41.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s41.bin"; IMG="${OUTDIR}/boot-s41.img"
elif [ "${MODE}" = "s42" ]; then
  # 刀 G43（#48 命6 黄金帧后·生存刀）：环写/验失败不再 report_reset 自杀
  # ——暖启 UFS 写卡滞（读全活写必死、疑 FUA 滞态跨暖复位遗传）曾让每条
  # 暖命死于自己的第一笔环写（命6=首 sm 后 ~5s、命2-5=~133s），362s 自旋
  # 把 600s 观察窗烧成 11s。失败改记 rw= 台账（计数+末4码）；写败留原槽。
  # 其余与 s41 同面（G41 dq= + G42 th=/sh=/sh2= 合弹）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s42.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s42.bin"; IMG="${OUTDIR}/boot-s42.img"
elif [ "${MODE}" = "s44" ]; then
  # 刀 G44（#49 后·竞态刀）：SO 服死于「OACK 后 1s 无 DATA」调度饿死——
  # 服务轮 ~0.86/s 赶不上客户端 timeoutms=1000 等待窗，EOT 由 walker 即
  # 时置 st=4，泵永无开火（dv=0/nopend=0/BLKNO=0 全零互证，ox=2=第二笔
  # OACK 落进空场）。本刀把 G19 窗口泵抽成 tf_pump_window，OACK 落环成
  # 功那一拍就地启泵送首窗 DATA；ox= 加第三格 kick 计数。其余同 s42
  # （G41 dq= + G42 th=/sh=/sh2= + G43 rw= 台账全保留）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s44.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s44.bin"; IMG="${OUTDIR}/boot-s44.img"
elif [ "${MODE}" = "s45" ]; then
  # 刀 G45（#50 后·so_read 双刀）：①跨扇区单命令连读——#50 首验实证
  # blksize 7680 首块死在双笔背靠背 read10 第二笔（tb=3.1 短块收口），
  # 单槽 UTRD/UCD 复用无完成同步；且老数学只罩 2 扇区（bo+7680 最大
  # 11775>8192，第三扇区从来是陈旧读）。连续 LBA 一笔 read10 读
  # ceil((bo+n)/4096)≤3 扇区进 DATA..DATA+0x2fff（ARRAY=boot 死件、
  # STAGING 单线程时序安全），从 DATA+bo 平切。②so= 新渲染
  # （单命令连读数.逐扇区退路数，工厂 FAT 连续分配下退路应恒 0）。
  # 其余同 s44（G44 OACK 落环原地启泵全保留）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s45.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s45.bin"; IMG="${OUTDIR}/boot-s45.img"
elif [ "${MODE}" = "s46" ]; then
  # 刀 G46（#50 方法论翻案后第一刀·纯插桩零行为改动）：#50 悬案
  # tb=4.0(ST=1 态收 EOT) 与 N=5/so=4/DV=2/DX=2 合不拢——6~12s 粒度
  # 标量计数器无法分辨 1~4s 协议交互。本刀只加毫秒事件时间线：
  # TF_EV 24 槽滚动环（cntvct→ms），记 RRQ/WRQ 到、ACK 到、EOT 到、
  # OACK 发、DATA 发/发拒、ST 迁、泵进（a=进泵 ST）、so_read 返回
  # （a=n，短读直证）、环闸挡、读零、stat 门退；sm= 帧内 rx= 后渲染
  # tl=ms.k.a×24。泵/臂/读写路径逻辑零改动（s45 连读刀全保留）。
  # 纪律：无时间线不定谳，插桩先于行为刀。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s46.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s46.bin"; IMG="${OUTDIR}/boot-s46.img"
elif [ "${MODE}" = "s47" ]; then
  # 刀 G47（#50 附录一悬案判刀·纯插桩零行为改动）：BUG-A 定谳到
  # glink_tx_payload 拒发 7680B DATA 块（小包活大包死），但三拒绝点
  # （caps/回卷闸/池干）二选一未定。本刀在三拒绝点当场抓快照：
  # GX 4 槽滚动（ms.site.a0..a3；site=1 caps:n.chunk.isz.total、
  # 2 回卷闸:head.tail.total.sz479、3 池干:chunk.freeN.freeMax.usedN），
  # 池干另 dump 全池 16 槽（iid.size.in_use）；sm= 渲染 gx=/ipn=/ipd=。
  # site=3 时 free 槽尺寸谱直接分「无 free 可挑」vs「free 全 < chunk
  # （对��只批小的）」。附带 G46 tl= 改名 tev=（根治与 #47 停点全景
  # tl= 撞名——22:14 那波整文件重试的 TL 解码即死于此）。
  # 泵/臂/读写路径逻辑零改动。纪律：插桩先于行为刀。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s47.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s47.bin"; IMG="${OUTDIR}/boot-s47.img"
elif [ "${MODE}" = "s48" ]; then
  # 刀 G48（#51 判刀后果·计数+腾挪捆绑刀）：s47 首帧定谳 DATA拒=②池干
  # （freeMax=0x200<chunk=0x1e24，对端只批 15×0x80+1×0x200）；次悬案=
  # 「cmd7 无人理 vs 满池丢件」——IPD 现场全池 16/16 小件占满、iid2 缺席，
  # iip_add 只填空槽，对端 cmd7 应答的大件广告若带新 iid 即被静默丢。
  # 本刀：iip_add 满池腾最小 free 槽（全 in_use 才丢）+三计（SEEN_MAX=
  # 见过最大批件尺寸/EVICT/DROP）尾接 ii=——计数器保归因，捆绑省一轮闪。
  # 其余同 s47（G46 tev= / G47 gx=ipn=ipd= 全保留）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s48.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s48.bin"; IMG="${OUTDIR}/boot-s48.img"
elif [ "${MODE}" = "s49" ]; then
  # 刀 G49（#51 首块 DATA 后新墙=窗口饿死，三刀捆绑·证据各有出处）：
  # ①泵在途闸 4096 是小 blksize 时代尺寸——px= 实证客户端要 blksize7680/
  # wsize10 突发，7736B 一块即触闸停泵；改随环容量（sz479−4096 预留小
  # 包，下限 8192）。②retry 臂只投 OACK 不 kick（G9c 游标纪律）——
  # op5/9 后客户端两次重 RRQ 全饿死在 52464 OACK；终态（3/7/8/9）重
  # RRQ=新读按首服重置+kick，st=2 半程保游标也 kick。③泵进度（TXGEN）
  # 计入服务内环活性——否则突发被 BUG-B 外圈 2s 拍掐死（旧判据只看
  # RX 游标）。s47/s48 插桩（tev/gx/ipn/ipd/ii 三计）全保留。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s49.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s49.bin"; IMG="${OUTDIR}/boot-s49.img"
elif [ "${MODE}" = "s50" ]; then
  # 刀 G50（s49 定谳：大包回卷自造协议写坏）：TX 环界改内核式分段写
  # ——旧「放不下且 head==tail 则 head=0 重写」让对端在 tail..end 吞
  # 4952B 陈旧字节（DATA1 total=7736 从 head≈0x2ca8 起 100% 跨界，环
  # 0x4000），modem 解包错乱→客户端 +1ms op5/9 弃单；小块不跨界全活。
  # 新法=内核 glink_smem_tx 同律：包跨界两段写（woff 绕环助手）、
  # head=(head+total)%sz、free 闸按 used 算不再要求环空；TEV 新事件
  # 13=跨界写发生（arg=旧 head）。s47-s49 插桩全保留。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s50.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s50.bin"; IMG="${OUTDIR}/boot-s50.img"
elif [ "${MODE}" = "s51" ]; then
  # 刀 G51（源码定谳后双刀）：读了真客户端源码（MPSS.HA.1.1
  # tftp_client.c）——①stat= rsize 键缺席（populate :417 只有
  # read_size!=0 才带），旧判式「键在值 0」永不命中，synth-ACK0
  # kick 照泵 7680B 进 stat 会话，孤儿 intent 0x2080 30 分钟不释
  # （#51 IPD 实证）；stat 门改「无键或值 0」→ 只回 OACK 等 code 9
  # （stock 同款零 DATA）。②空闲 listen 节拍 200→5ms：#51 现场
  # S3(160B phdr 读) RRQ 落空闲拍应答 +200ms（BUG-B 主峰），S1/S2
  # 同拍全活唯 S3 死——装载器层读窗容不下 200ms。s47-s50 全保留。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s51.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s51.bin"; IMG="${OUTDIR}/boot-s51.img"
elif [ "${MODE}" = "s52" ]; then
  # 刀 G52（纯插桩，零行为改动）：#52 现场服务端全链洗清（内���
  # SH1/SH2 与宿主件字节全等、offset 台账 160@0x34 对、216B DATA1
  # 无跨界、TX 环 head==tail 全排空、同拍应答），客户端仍在 S3 RRQ
  # 后零 ACK 零 code9 永静默；而 xl= 尾窗实锤 DATA1 后紧跟两笔非
  # tftp 发射（104B@0x12 假 pd-mapper 口 + 64B@0x16 假 IPA 口），
  # 但无时刻无法与静默窗对读。三插桩：TXL 4→8 槽加毫秒戳、入站
  # qrtr 环 rqr=（ms.源口.宿口.尺寸×16）、pd2=（G38 ind 探针
  # arm.n——探针是否落进装载窗一靴可见）。s44-s51 全保留。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s52.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s52.bin"; IMG="${OUTDIR}/boot-s52.img"
elif [ "${MODE}" = "s53" ]; then
  # 刀 G53（纯插桩双证）：#52 三问全答——0x12/0x16 在 DATA1+11.1s
  # （谋杀假说死）、0x12 前零入站（自发）、死点=客户端收 216B DATA1
  # 后暴死（连最终 ACK 都无=死在 cb_func 内=dlopen 放弃，rtld_main
  # 官方静默退出路径 QDI_OS_PROCESS_EXIT）。两证收口：① s3= 端到端
  # 环回读校验——SH1/SH2 只证块缓冲对（boot 期文件直读），没证
  # woff 写环后的字节；大 DATA 写完从环读回 xor32 与源比，A≠C 即
  # TX 写坏实锤（数据假说终审）。② rqr 槽加 type 字（qrtr payload
  # 首 word，控制包=ctrl type）——DATA1+11.1s 的 7 笔 52B 控制包
  # 若是 DEL_SERVER(4)=elf_loader PD 死亡讣告，dlopen 放弃实锤。
  # s44-s52 全保留。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s53.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s53.bin"; IMG="${OUTDIR}/boot-s53.img"
elif [ "${MODE}" = "s54" ]; then
  # 刀 G54（行为刀·p_memsz 阶梯探针）：靴 #54 逆向离线定谳死点=hash
  # 段 alloc（FUN_8c5f0f98 walk→alloc(p_memsz) 失败→rtld 静默退场，
  # 0x1000 偏移读从未上线=alloc 从未成功）。行为证法：送数时覆写 SO
  # phdr1.p_memsz(偏移104)/p_filesz(偏移100) 成阶梯值（rung0=0x2000
  # 原值=对照靴）——若 alloc 是堵点且更小尺寸能过，hash 段读 RRQ
  # （seek=1000）即上线现形=实锤+池尺寸二分。跨 modem SSR 存活：
  # 讣告簇（chunk=52/type=4）+8s 门推档，兜底=距上次首服≥25s 推档；
  # s54= 档位.档值.补丁数.推档数.源，ob= 讣告末2笔 52B 全保真。
  # s44-s53 全保留。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s54.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s54.bin"; IMG="${OUTDIR}/boot-s54.img"
elif [ "${MODE}" = "s21" ]; then
  # 刀 G22：s20 同面，intent 池+归还追踪（cmd5 全量入池、TX 池挑、
  # RX_DONE/W_REUSE 归还、池干 RX_INTENT_REQ、cmd3/11 入 8B 集、ii= 台账）。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s21.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s21.bin"; IMG="${OUTDIR}/boot-s21.img"
elif [ "${MODE}" = "s20" ]; then
  # NOLINUX-PLAN S3 第二刀：s19 上电面不变，⑩-4 加 EP0 + CDC-ECM 枚举
  # （配方 S3-USB.md「EP0 → CDC-ECM/NCM」）。先断线再武装 SETUP，清
  # SFTDISCONNECT 后枚举在 soak 的 30s 窗里轮询。未刷：收线窗仍先 s18。
  (cd "${HERE}" && cargo build --release --target aarch64-unknown-none-softfloat) || { echo "cargo build FAILED (${MODE}) -- 不拷陈旧件" >&2; exit 1; }
  ELF="${OUTDIR}/s20.elf"
  cp "${HERE}/target/aarch64-unknown-none-softfloat/release/redfin-bare" "${ELF}"
  BIN="${OUTDIR}/s20.bin"; IMG="${OUTDIR}/boot-s20.img"
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
