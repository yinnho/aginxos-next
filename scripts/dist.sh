#!/usr/bin/env bash
# dist.sh — assemble the public device package zip (PACKAGING.md §7 step 3).
#
#   DEVICE=redfin    ./scripts/dist.sh <version>
#   DEVICE=enchilada ./scripts/dist.sh <version>
#
# Inputs (must already exist — bake + pack are separate steps):
#   out/rootfs.img                                    (build-rootfs.sh, DEVICE=<d>)
#   redfin:    .local/device/redfin/boot-out/vendor_boot-test.img
#              .local/device/redfin/stock/stock-vendor_boot.img
#   enchilada: .local/device/enchilada/boot-out/enchilada-boot.img
#              built with RESCUE_PUBKEY=0 — dist refuses otherwise (a public
#              boot.img must not carry the packer's ssh key; the stamp file
#              .rescue_pubkey_stripped is written by pack-boot.sh on 0)
#   devices/<device>/dist/{SKILL.md,flash.sh}         (public templates)
#
# Output: dist/aginxos-<device>-<version>.zip containing the tree named in
# docs/PACKAGING.md §3, with manifest.json rendered from the real payloads
# and SHA256SUMS over every payload file. Before staging, the rootfs.img is
# scanned for the packer's personal secret values (packaging rule: the zip
# carries zero personal information).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
DEVICE="${DEVICE:?DEVICE= required (redfin|enchilada)}"
VERSION="${1:?usage: DEVICE=<redfin|enchilada> ./scripts/dist.sh <version>}"

DEVDIR="${REPO}/devices/${DEVICE}"
DISTTPL="${DEVDIR}/dist"
ROOTFS_IMG="${REPO}/out/rootfs.img"

case "${DEVICE}" in
  redfin)
    GATE_PRODUCT="redfin"
    VB_IMG="${REPO}/.local/device/redfin/boot-out/vendor_boot-test.img"
    STOCK_VB="${REPO}/.local/device/redfin/stock/stock-vendor_boot.img"
    ;;
  enchilada)
    GATE_PRODUCT="sdm845"   # OP6 bootloader reality (2026-09-14 getvar receipt, HARDWARE.md)
    BOOT_IMG="${REPO}/.local/device/enchilada/boot-out/enchilada-boot.img"
    ;;
  *) echo "unknown DEVICE '${DEVICE}' (redfin|enchilada)" >&2; exit 1 ;;
esac

NAME="aginxos-${DEVICE}-${VERSION}"
STAGE="${REPO}/dist/${NAME}"
ZIP="${REPO}/dist/${NAME}.zip"

command -v zip >/dev/null 2>&1 || { echo "zip not on PATH" >&2; exit 1; }
[ -s "${ROOTFS_IMG}" ] || { echo "missing ${ROOTFS_IMG}" >&2; exit 1; }
[ -s "${DISTTPL}/SKILL.md" ] || { echo "missing ${DISTTPL}/SKILL.md" >&2; exit 1; }
[ -s "${DISTTPL}/flash.sh" ] || { echo "missing ${DISTTPL}/flash.sh" >&2; exit 1; }
case "${DEVICE}" in
  redfin)
    [ -s "${VB_IMG}" ]    || { echo "missing ${VB_IMG}" >&2; exit 1; }
    [ -s "${STOCK_VB}" ]  || { echo "missing ${STOCK_VB}" >&2; exit 1; }
    ;;
  enchilada)
    [ -s "${BOOT_IMG}" ]  || { echo "missing ${BOOT_IMG}" >&2; exit 1; }
    [ -f "${REPO}/.local/device/enchilada/boot-out/.rescue_pubkey_stripped" ] || {
      echo "enchilada boot.img lacks .rescue_pubkey_stripped stamp —" >&2
      echo "  repack: RESCUE_PUBKEY=0 ./devices/enchilada/boot/pack-boot.sh" >&2
      exit 1
    }
    ;;
esac

if command -v sha256sum >/dev/null 2>&1; then SHA=sha256sum; else SHA="shasum -a 256"; fi
fsize() { if stat -f %z "$1" >/dev/null 2>&1; then stat -f %z "$1"; else stat -c %s "$1"; fi; }
fhash() { ${SHA} "$1" | awk '{print $1}'; }

# Personal-secret scan: refuse to package a rootfs.img that carries any
# secret value from the packer's environment (brain key, relay secret) or
# the packer's ssh public key (a stale surgically-modified image can carry
# /root/.ssh/authorized_keys — build-rootfs.sh has no injection points, so
# a fresh bake is clean; the scan is the gate that proves it).
# Values never echo — only the key name names the leak.
scan_secrets() { # scan_secrets <target-file> <label>
  local target="$1" label="$2" bad=0
  local envf="${REPO}/.local/aginx-env"
  local line k v rs pk cfg="${HOME}/.aginx/config.toml"
  if [ -f "${envf}" ]; then
    while IFS= read -r line; do
      case "${line}" in ''|\#*) continue ;; esac
      case "${line}" in *=*) ;; *) continue ;; esac
      k="${line%%=*}"; v="${line#*=}"
      [ -n "${v}" ] || continue
      if LC_ALL=C grep -a -qF -- "${v}" "${target}"; then
        echo "LEAK: ${k} value found in ${label} — refusing to package" >&2
        bad=1
      fi
    done < "${envf}"
  fi
  if [ -f "${cfg}" ]; then
    rs="$(sed -n 's/^[[:space:]]*relay_secret[[:space:]]*=[[:space:]]*"\{0,1\}\([^"[:space:]]*\)"\{0,1\}[[:space:]]*$/\1/p' "${cfg}" | head -1)"
    if [ -n "${rs}" ] && LC_ALL=C grep -a -qF -- "${rs}" "${target}"; then
      echo "LEAK: relay_secret value found in ${label} — refusing to package" >&2
      bad=1
    fi
  fi
  for pk in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
    [ -f "${pk}" ] || continue
    if LC_ALL=C grep -a -qF -- "$(cat "${pk}")" "${target}"; then
      echo "LEAK: packer ssh public key found in ${label} — refusing to package" >&2
      bad=1
    fi
  done
  return "${bad}"
}

rm -rf "${STAGE}" "${ZIP}"
mkdir -p "${STAGE}/boot"

echo "==> stage ${NAME}"
cp "${ROOTFS_IMG}" "${STAGE}/rootfs.img"
case "${DEVICE}" in
  redfin)
    cp "${VB_IMG}"   "${STAGE}/boot/vendor_boot.img"
    cp "${STOCK_VB}" "${STAGE}/boot/vendor_boot.stock.img"
    ;;
  enchilada)
    cp "${BOOT_IMG}" "${STAGE}/boot/enchilada-boot.img"
    ;;
esac
cp "${DISTTPL}/SKILL.md" "${STAGE}/SKILL.md"
cp "${DISTTPL}/flash.sh" "${STAGE}/flash.sh"
chmod +x "${STAGE}/flash.sh"

echo "==> secret scan (rootfs.img)"
scan_secrets "${STAGE}/rootfs.img" "rootfs.img"

COMMIT="$(git -C "${REPO}" rev-parse --short HEAD 2>/dev/null || echo unknown)"
BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

R_SIZE="$(fsize "${STAGE}/rootfs.img")"; R_SHA="$(fhash "${STAGE}/rootfs.img")"

echo "==> render manifest.json (commit ${COMMIT})"
case "${DEVICE}" in
  redfin)
    V_SIZE="$(fsize "${STAGE}/boot/vendor_boot.img")"; V_SHA="$(fhash "${STAGE}/boot/vendor_boot.img")"
    S_SIZE="$(fsize "${STAGE}/boot/vendor_boot.stock.img")"; S_SHA="$(fhash "${STAGE}/boot/vendor_boot.stock.img")"
    cat > "${STAGE}/manifest.json" <<EOF
{
  "package": "aginxos-${DEVICE}",
  "version": "${VERSION}",
  "device": "${DEVICE}",
  "build": { "commit": "${COMMIT}", "built_at": "${BUILT_AT}" },
  "device_gate": {
    "fastboot_getvar": { "product": "${GATE_PRODUCT}" },
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
    SUMS_LIST="rootfs.img boot/vendor_boot.img boot/vendor_boot.stock.img flash.sh SKILL.md manifest.json"
    ;;
  enchilada)
    B_SIZE="$(fsize "${STAGE}/boot/enchilada-boot.img")"; B_SHA="$(fhash "${STAGE}/boot/enchilada-boot.img")"
    cat > "${STAGE}/manifest.json" <<EOF
{
  "package": "aginxos-${DEVICE}",
  "version": "${VERSION}",
  "device": "${DEVICE}",
  "build": { "commit": "${COMMIT}", "built_at": "${BUILT_AT}" },
  "device_gate": {
    "fastboot_getvar": { "product": "${GATE_PRODUCT}" },
    "single_device_required": true
  },
  "images": [
    { "path": "rootfs.img", "partition": "userdata", "sha256": "${R_SHA}", "bytes": ${R_SIZE} },
    { "path": "boot/enchilada-boot.img", "partition": "boot_<current-slot>", "sha256": "${B_SHA}", "bytes": ${B_SIZE} }
  ],
  "recovery": [],
  "recovery_procedure": "the slot NOT flashed still boots the previous OS: fastboot set_active <other-slot> && fastboot reboot",
  "flash_order": ["userdata", "boot_<current-slot>", "set_active <current-slot>"],
  "human_steps": ["enter fastboot: power off, hold Power+VolumeUp"],
  "verify": [
    { "probe": "usb NCM net up (replug cable once if wedged)", "timeout_s": 300 },
    { "probe": "ssh root@10.9.8.1 (requires pre-flash --pubkey injection)", "timeout_s": 300 },
    { "probe": "ssh cat /run/boot.state reports done", "timeout_s": 600 }
  ],
  "configure_after": {
    "pre_flash": "./flash.sh --pubkey <public-key> [--wifi wifi.conf] — debugfs injection; this device has no adb",
    "auth": "the injected pubkey opens ssh; set a password with 'echo root:... | busybox chpasswd -c sha512' if wanted",
    "then": "ssh root@10.9.8.1; env keys -> /etc/aginx/env; aginx-pkg opt-in aginx aginx-gateway"
  },
  "editions": {
    "server": "nothing further — the flashed image is complete"
  }
}
EOF
    SUMS_LIST="rootfs.img boot/enchilada-boot.img flash.sh SKILL.md manifest.json"
    ;;
esac

echo "==> SHA256SUMS"
( cd "${STAGE}" && ${SHA} ${SUMS_LIST} > SHA256SUMS )

echo "==> zip"
( cd "${REPO}/dist" && zip -X -q -r "${NAME}.zip" "${NAME}" )

ls -la "${ZIP}"
echo "done: ${ZIP}"
echo "publish: gh release create ${DEVICE}-v${VERSION} --repo yinnho/aginxos-next '${ZIP}' --title 'AginxOS ${DEVICE} ${VERSION}'"
