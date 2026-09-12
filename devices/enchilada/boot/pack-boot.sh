#!/usr/bin/env bash
# E2: enchilada (OnePlus 6) boot.img 打包 — AginxOS #335 节点机
#
# 依赖 (.local, 不入库):
#   .local/device/enchilada/boot/{Image, sdm845-oneplus-enchilada.dtb,
#     mod/*.ko, dropbear, dropbearkey}   ← 86quan 内核构建产物 + redfin 同款 dropbear
#   rootfs/busybox                        ← redfin L0 同款静态 aarch64
#   .local/boot-tools/mkbootimg.py        ← AOSP 官方
#
# 参数权威源: pmaports device-oneplus-enchilada deviceinfo (2026-09-12 取回):
#   base=0x0 kernel=0x8000 ramdisk=0x1000000 second=0xf00000 tags=0x100
#   pagesize=4096 append_dtb=true cmdline="console=ttyMSM0,115200 -quiet"
#   header_version: v1 (2026-09-12 双向实测: v0 内存靴与刷入靴均被 ABL 拒
#     "Load Error" 直落 fastboot; v1 过载荷检查。TWRP 在 OP6 内存靴同为 v1。)
#   cmdline 无 -quiet: pmOS deviceinfo 里 "-quiet" 是删参标记, 非字面量。
#
# 产出: .local/device/enchilada/boot-out/enchilada-boot.img
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "${HERE}/../../.." && pwd)"
DEV="${ROOT}/.local/device/enchilada"
B="${DEV}/boot"
OUT="${DEV}/boot-out"
MKBOOT="${ROOT}/.local/boot-tools/mkbootimg.py"
RAM="${OUT}/initramfs"

# tty0 在前 = 屏显也是观察通道 (2026-09-13 实测: 8 企鹅 = 内核活了; klog 回灌
# 上屏, 无头调试时真人眼即收据); ttyMSM0 保留串口位
CMDLINE="console=tty0 console=ttyMSM0,115200"

fail() { echo "pack-boot: $*" >&2; exit 1; }

[ -f "${B}/Image" ] || fail "missing ${B}/Image (86quan e2-bundle)"
[ -f "${B}/sdm845-oneplus-enchilada.dtb" ] || fail "missing dtb"
[ -x "${B}/dropbear" ] || fail "missing dropbear"
[ -x "${ROOT}/rootfs/busybox" ] || fail "missing rootfs/busybox"
[ -f "${MKBOOT}" ] || fail "missing mkbootimg.py"

rm -rf "${RAM}"
mkdir -p "${OUT}" "${RAM}"/{bin,modules,etc/dropbear,root/.ssh,dev,proc,sys,tmp,sysroot}

# --- busybox + dropbear (静态 musl aarch64) ---
install -m 755 "${ROOT}/rootfs/busybox" "${RAM}/bin/busybox"
ln -sf busybox "${RAM}/bin/sh"       # PID1 exec /bin/sh 之前就位
install -m 755 "${B}/dropbear" "${B}/dropbearkey" "${RAM}/bin/"

# --- gadget 模块 (6.11 生产 config: CONFIGFS/LIBCOMPOSITE/U_ETHER/F_NCM 全 =y,
#     此目录为空也合法 — init 的 insmod 行静默跳过) ---
for _ko in "${B}"/mod/*.ko; do
  [ -e "$_ko" ] && cp "$_ko" "${RAM}/modules/"
done
true

# --- 救援公钥: 打包机 id_ed25519 (个人配置, .local 不入库) ---
if [ -f "${HOME}/.ssh/id_ed25519.pub" ]; then
  cp "${HOME}/.ssh/id_ed25519.pub" "${RAM}/root/.ssh/authorized_keys"
  chmod 600 "${RAM}/root/.ssh/authorized_keys"
else
  echo "pack-boot: WARN 无 ~/.ssh/id_ed25519.pub — 救援 ssh 将不可登" >&2
fi

# --- 用户数据库: dropbear 认证前 getpwnam("root") 必须成立 (2026-09-13 实测:
#     无 /etc/passwd = 一律 Permission denied, 公钥对了也没用) ---
printf 'root:x:0:0:root:/root:/bin/sh\n' > "${RAM}/etc/passwd"
printf 'root:x:0:\n'                      > "${RAM}/etc/group"
printf '/bin/sh\n'                        > "${RAM}/etc/shells"

# --- /init ---
install -m 755 "${HERE}/initramfs-init" "${RAM}/init"

# --- cpio (macOS cpio 支持 -H newc, 2026-09-12 实测) + gzip ---
# -R 0:0 属主修正是必须的: 不带则 uid=501 落地, dropbear checkfileperm 拒
# authorized_keys 链 (2026-09-13 实测: ssh offer 密钥服务端仍 Permission denied)。
# init 里另有运行时 chown 兜底。
( cd "${RAM}" && find . | cpio -o -H newc -R 0:0 --quiet | gzip -9 ) \
  > "${OUT}/initramfs.cpio.gz"

# --- 载荷: Image.gz + dtb 追加 (2026-09-12 实裁定档) ---
# pmOS APKBUILD 装 Image.gz 当 vmlinuz; deviceinfo 偏移 kernel@0x8000/second@15MiB/
# ramdisk@16MiB 只容 <15MB 载荷 — 裸 Image 44MB 会碾过 ramdisk/tags 区, 内核跳转即死
# (两次真机死 + QEMU 全绿的根因; gz 后 13.8MiB 收在 15MiB 内)。ABL 嗅 1f8b 解压后跳。
gzip -9 -n -c "${B}/Image" > "${OUT}/Image.gz"
cat "${OUT}/Image.gz" "${B}/sdm845-oneplus-enchilada.dtb" > "${OUT}/kernel-dtb"

# --- mkbootimg (header v1) ---
python3 "${MKBOOT}" \
  --kernel "${OUT}/kernel-dtb" \
  --ramdisk "${OUT}/initramfs.cpio.gz" \
  --cmdline "${CMDLINE}" \
  --base 0x00000000 \
  --kernel_offset 0x00008000 \
  --ramdisk_offset 0x01000000 \
  --second_offset 0x00f00000 \
  --tags_offset 0x00000100 \
  --pagesize 4096 \
  --header_version 1 \
  -o "${OUT}/enchilada-boot.img"

echo "--- built ---"
ls -la "${OUT}/enchilada-boot.img"
shasum -a 256 "${OUT}/enchilada-boot.img" | tee "${OUT}/enchilada-boot.img.sha256"
echo "next: E3 — fastboot flash boot_<slot> (只刷当前槽, 另一槽留 LOS 作回退)"
