#!/usr/bin/env bash
# Unpack an Android boot.img (generic AOSP tool; output default lands in the
# redfin device pack's work area). Migrated from aginxos@0534ea8 boot/unpack-boot.sh.
# Usage: ./scripts/unpack-boot.sh path/to/boot.img [outdir]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="${1:-}"
OUT="${2:-${ROOT}/.local/device/redfin/unpack}"
TOOLS="${ROOT}/.local/boot-tools"

if [[ -z "${IMG}" || ! -f "${IMG}" ]]; then
  echo "usage: $0 path/to/boot.img [outdir]" >&2
  exit 1
fi

mkdir -p "${OUT}"
rm -rf "${OUT:?}/"*

if [[ -x "${TOOLS}/unpack_bootimg.py" ]] || [[ -f "${TOOLS}/unpack_bootimg.py" ]]; then
  echo "==> unpack_bootimg.py → ${OUT}"
  python3 "${TOOLS}/unpack_bootimg.py" --boot_img "${IMG}" --out "${OUT}" --format=mkbootimg | tee "${OUT}/info.txt"
elif command -v magiskboot >/dev/null 2>&1; then
  echo "==> magiskboot unpack"
  cp "${IMG}" "${OUT}/boot.img"
  (
    cd "${OUT}"
    magiskboot unpack boot.img
    {
      echo "tool=magiskboot"
      echo "note=see kernel, ramdisk.cpio, header in this directory"
      ls -la
    } | tee info.txt
  )
else
  echo "No unpack tool. Run: ./scripts/fetch-boot-tools.sh" >&2
  echo "Or install magiskboot and re-run." >&2
  exit 1
fi

# Normalize common filenames for pack-boot.sh
if [[ -f "${OUT}/kernel" && ! -f "${OUT}/Image" ]]; then
  ln -sfn kernel "${OUT}/Image" 2>/dev/null || cp "${OUT}/kernel" "${OUT}/Image"
fi

echo
echo "Unpacked. Next:"
echo "  cat ${OUT}/info.txt"
echo "  ./devices/redfin/boot/pack-boot.sh"
