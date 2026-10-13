#!/bin/sh
# M0 预验：把 .efi 装进 FAT 盘（EFI/BOOT/BOOTAA64.EFI），edk2 默认启动路径吃。
# macOS 无 mkfs.vfat/免挂载写 FAT——hdiutil 造裸盘 + mtools(mcopy) 装文件。
set -e
EFI="${1:-target/aarch64-unknown-uefi/debug/aginx-efi-hello.efi}"
OUT="${2:-target/efi-boot.img}"

cd "$(dirname "$0")/hello"
[ -f "$EFI" ] || { echo "no such efi: $EFI (cargo +nightly build first)" >&2; exit 1; }

rm -f "$OUT" "$OUT.dmg"
hdiutil create -size 16m -fs "MS-DOS FAT16" -volname AGINXEFI \
  -layout NONE -o "$OUT.dmg" 2>/dev/null
mv "$OUT.dmg" "$OUT"
mmd -i "$OUT" ::/EFI ::/EFI/BOOT
mcopy -i "$OUT" "$EFI" ::/EFI/BOOT/BOOTAA64.EFI
mdir -i "$OUT" ::/EFI/BOOT
echo "ok: $OUT ($(stat -f%z "$OUT") bytes)"
