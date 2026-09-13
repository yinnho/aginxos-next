#!/usr/bin/env bash
# dist.sh — assemble the public device package zip (PACKAGING.md §7 step 3).
#
#   DEVICE=redfin ./scripts/dist.sh <version>     e.g. 0.1.0
#
# Inputs (must already exist — bake + pack are separate steps):
#   out/rootfs.img                                        (build-rootfs.sh)
#   .local/device/redfin/boot-out/vendor_boot-test.img    (pack-vendor-boot.sh)
#   .local/device/redfin/stock/stock-vendor_boot.img      (recovery point)
#   devices/redfin/dist/{SKILL.md,flash.sh}               (public templates)
#
# Output: dist/aginxos-<device>-<version>.zip containing the tree named in
# docs/PACKAGING.md §3, with manifest.json rendered from the real payloads
# and SHA256SUMS over every payload file.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
DEVICE="${DEVICE:?DEVICE= required (redfin)}"
VERSION="${1:?usage: DEVICE=redfin ./scripts/dist.sh <version>}"

DEVDIR="${REPO}/devices/${DEVICE}"
DISTTPL="${DEVDIR}/dist"
ROOTFS_IMG="${REPO}/out/rootfs.img"
VB_IMG="${REPO}/.local/device/${DEVICE}/boot-out/vendor_boot-test.img"
STOCK_VB="${REPO}/.local/device/${DEVICE}/stock/stock-vendor_boot.img"

NAME="aginxos-${DEVICE}-${VERSION}"
STAGE="${REPO}/dist/${NAME}"
ZIP="${REPO}/dist/${NAME}.zip"

for f in "${ROOTFS_IMG}" "${VB_IMG}" "${STOCK_VB}" "${DISTTPL}/SKILL.md" "${DISTTPL}/flash.sh"; do
  [ -s "${f}" ] || { echo "missing ${f}" >&2; exit 1; }
done
command -v zip >/dev/null 2>&1 || { echo "zip not on PATH" >&2; exit 1; }

if command -v sha256sum >/dev/null 2>&1; then SHA=sha256sum; else SHA="shasum -a 256"; fi
fsize() { if stat -f %z "$1" >/dev/null 2>&1; then stat -f %z "$1"; else stat -c %s "$1"; fi; }
fhash() { ${SHA} "$1" | awk '{print $1}'; }

rm -rf "${STAGE}" "${ZIP}"
mkdir -p "${STAGE}/boot"

echo "==> stage ${NAME}"
cp "${ROOTFS_IMG}" "${STAGE}/rootfs.img"
cp "${VB_IMG}"     "${STAGE}/boot/vendor_boot.img"
cp "${STOCK_VB}"   "${STAGE}/boot/vendor_boot.stock.img"
cp "${DISTTPL}/SKILL.md" "${STAGE}/SKILL.md"
cp "${DISTTPL}/flash.sh" "${STAGE}/flash.sh"
chmod +x "${STAGE}/flash.sh"

COMMIT="$(git -C "${REPO}" rev-parse --short HEAD 2>/dev/null || echo unknown)"
BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

R_SIZE="$(fsize "${STAGE}/rootfs.img")";   R_SHA="$(fhash "${STAGE}/rootfs.img")"
V_SIZE="$(fsize "${STAGE}/boot/vendor_boot.img")"; V_SHA="$(fhash "${STAGE}/boot/vendor_boot.img")"
S_SIZE="$(fsize "${STAGE}/boot/vendor_boot.stock.img")"; S_SHA="$(fhash "${STAGE}/boot/vendor_boot.stock.img")"

echo "==> render manifest.json (commit ${COMMIT})"
cat > "${STAGE}/manifest.json" <<EOF
{
  "package": "aginxos-${DEVICE}",
  "version": "${VERSION}",
  "device": "${DEVICE}",
  "build": { "commit": "${COMMIT}", "built_at": "${BUILT_AT}" },
  "device_gate": {
    "fastboot_getvar": { "product": "${DEVICE}" },
    "single_device_required": true
  },
  "images": [
    { "path": "rootfs.img", "partition": "userdata", "sha256": "${R_SHA}", "bytes": ${R_SIZE} },
    { "path": "boot/vendor_boot.img", "partition": "vendor_boot", "sha256": "${V_SHA}", "bytes": ${V_SIZE} }
  ],
  "recovery": [
    { "path": "boot/vendor_boot.stock.img", "partition": "vendor_boot", "sha256": "${S_SHA}", "bytes": ${S_SIZE} }
  ],
  "flash_order": ["userdata", "vendor_boot"],
  "human_steps": ["enter fastboot: power off, hold Power+VolumeDown"],
  "verify": [
    { "probe": "adb device appears", "timeout_s": 300 },
    { "probe": "adb shell cat /run/boot.state reports done", "timeout_s": 600 },
    { "probe": "ssh reachable after configuration", "timeout_s": 300 }
  ],
  "configure_after": {
    "wifi": "adb push wifi.conf /etc/wifi.conf",
    "auth": "set a root password over adb, or push an ssh public key",
    "then": "ssh takes over; USB may be unplugged"
  },
  "editions": {
    "server": "nothing further — the flashed image is complete",
    "touch": "opt-in the touch suite once on network"
  }
}
EOF

echo "==> SHA256SUMS"
( cd "${STAGE}" && ${SHA} rootfs.img boot/vendor_boot.img boot/vendor_boot.stock.img flash.sh SKILL.md manifest.json > SHA256SUMS )

echo "==> zip"
( cd "${REPO}/dist" && zip -X -q -r "${NAME}.zip" "${NAME}" )

ls -la "${ZIP}"
echo "done: ${ZIP}"
echo "publish: gh release create ${DEVICE}-v${VERSION} --repo yinnho/aginxos-next '${ZIP}' --title 'AginxOS ${DEVICE} ${VERSION}'"
