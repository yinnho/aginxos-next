#!/bin/sh
# M0 预验：qemu-aarch64 + edk2 固件 boot 我们的 EFI app。
# 固件来自 brew qemu 自带 share/qemu/edk2-aarch64-code.fd。
set -e
cd "$(dirname "$0")"
QEMU="${QEMU:-$(command -v qemu-system-aarch64 || true)}"
[ -n "$QEMU" ] && [ -x "$QEMU" ] || {
  echo "qemu-system-aarch64 not found (brew install qemu)" >&2; exit 1; }

QDIR=$(dirname "$(command -v "$QEMU" || echo "$QEMU")")
FW="${FW:-$QDIR/../share/qemu/edk2-aarch64-code.fd}"
[ -f "$FW" ] || { echo "edk2 firmware not found at $FW" >&2; exit 1; }
[ -f hello/target/efi-boot.img ] || { echo "no boot img (./mk-fat.sh first)" >&2; exit 1; }

exec "$QEMU" -M virt -cpu cortex-a57 -m 512 -nographic -bios "$FW" \
  -drive if=none,file=hello/target/efi-boot.img,format=raw,id=d0 \
  -device virtio-blk-device,drive=d0 \
  -device virtio-net-device,netdev=n0 -netdev user,id=n0
