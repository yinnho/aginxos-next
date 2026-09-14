#!/usr/bin/env bash
# flash.sh — AginxOS device package, enchilada (OnePlus 6). PUBLIC artifact.
#
# Deterministic fresh-install flash. Dry-run by default; GO=1 executes.
# Gates: exactly one fastboot device, and `fastboot getvar product` must be
# "sdm845" (this is what an OP6 bootloader reports — verified on device).
# Order is crash-safe: userdata first, boot_<current slot> last, then
# `set_active <slot>` as the commit point. The OTHER slot keeps whatever OS
# it had (typically the stock LineageOS boot) and stays untouched — that is
# the recovery lane (§recovery in SKILL.md).
#
# enchilada has NO adb. The fresh system accepts nobody: password lane is
# inert (no hash), no keys are baked. The only way in is what you inject
# BEFORE flashing (recommended — do both):
#
#   ./flash.sh --pubkey ~/.ssh/id_ed25519.pub   # your ssh key -> root
#   ./flash.sh --wifi   ./wifi.conf             # ssid=/psk= -> /etc/wifi.conf
#
# Injection uses debugfs (e2fsprogs); afterwards the script runs `e2fsck -fp`
# because a debugfs write session marks the filesystem dirty, and flashing a
# dirty image would make the first-boot resize2fs refuse to run. macOS:
# `brew install e2fsprogs` (both tools). Linux: e2fsprogs (usually
# preinstalled). Without a pubkey the device boots fine but no channel can
# ever authenticate — plan for §5 of SKILL.md accordingly.
set -euo pipefail

cd "$(dirname "$0")"

DEVICE="enchilada"
GATE_PRODUCT="sdm845"
ROOTFS="rootfs.img"
BOOTIMG="boot/enchilada-boot.img"

say() { printf '%s\n' "$*"; }

PUBKEY=""; WIFI_CONF=""
while [ $# -gt 0 ]; do
  case "$1" in
    --pubkey) [ -n "${2:-}" ] || { say "--pubkey needs a file" >&2; exit 1; }; PUBKEY="$2"; shift 2 ;;
    --wifi)   [ -n "${2:-}" ] || { say "--wifi needs a file" >&2; exit 1; };   WIFI_CONF="$2"; shift 2 ;;
    *) say "unknown argument: $1 (supported: --pubkey FILE, --wifi FILE)" >&2; exit 1 ;;
  esac
done

find_e2fstool() { # find_e2fstool <name> — PATH first, then homebrew e2fsprogs sbin
  local n="$1" p
  if command -v "${n}" >/dev/null 2>&1; then command -v "${n}"; return 0; fi
  for p in /opt/homebrew/opt/e2fsprogs/sbin /usr/local/opt/e2fsprogs/sbin; do
    [ -x "${p}/${n}" ] && { printf '%s\n' "${p}/${n}"; return 0; }
  done
  return 1
}

inject() { # inject <fs-image> <debugfs-script-text> <label>
  local img="$1" script="$2" label="$3" df tmp
  df="$(find_e2fstool debugfs)" || { say "debugfs not found — install e2fsprogs (macOS: brew install e2fsprogs)" >&2; exit 1; }
  tmp="$(mktemp)"
  printf '%s\n' "$script" > "$tmp"
  "$df" -w -f "$tmp" "$img" >/dev/null || { say "debugfs injection failed (${label})" >&2; rm -f "$tmp"; exit 1; }
  rm -f "$tmp"
  say "injected ${label}"
}

fsck_repair() { # fsck_repair <fs-image> — clear the dirty state a debugfs
  # session leaves behind and refuse to ship anything still inconsistent
  # (first-boot resize2fs refuses to run on an image with the error flag set)
  local img="$1" ec
  ec="$(find_e2fstool e2fsck)" || { say "e2fsck not found — install e2fsprogs (macOS: brew install e2fsprogs)" >&2; exit 1; }
  "$ec" -fp "${img}" >/dev/null 2>&1 || { say "e2fsck -fp could not repair ${img}" >&2; exit 1; }
  if ! "$ec" -fn "${img}" >/dev/null 2>&1; then
    say "refusing: ${img} still shows filesystem errors after repair" >&2
    exit 1
  fi
  say "rootfs.img filesystem clean after injection (e2fsck fp+fn)"
}

sha_verify() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c SHA256SUMS
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c SHA256SUMS
  else
    say "no sha256sum/shasum on PATH — cannot verify payload integrity" >&2
    return 1
  fi
}

recover_hint() {
  say ""
  say "Recovery: this package never touches the OTHER boot slot. From fastboot,"
  say "  fastboot set_active <other-slot>   (a<->b; the slot this run did NOT flash)"
  say "  fastboot reboot"
  say "boots the previous OS again. Re-running this script is safe."
}
trap recover_hint ERR

command -v fastboot >/dev/null 2>&1 || { say "fastboot not on PATH (Android platform-tools)" >&2; exit 1; }

if [ -z "${GO:-}" ]; then
  say "dry-run (GO=1 to flash) — plan:"
  say "  verify  : SHA256SUMS over payload"
  [ -n "${PUBKEY}" ]    && say "  inject  : your ssh pubkey -> /root/.ssh/authorized_keys (debugfs, in rootfs.img)"
  [ -n "${WIFI_CONF}" ] && say "  inject  : wifi.conf -> /etc/wifi.conf mode 0600 (debugfs, in rootfs.img)"
  [ -n "${PUBKEY}${WIFI_CONF}" ] && say "  repair  : e2fsck fp+fn — debugfs leaves the fs dirty; refuse if not clean"
  say "  gate    : exactly one fastboot device, getvar product == ${GATE_PRODUCT}"
  say "  slot    : read current slot, flash only boot_<slot>"
  say "  flash 1 : userdata    <- ${ROOTFS}"
  say "  flash 2 : boot_<slot> <- ${BOOTIMG}"
  say "  commit  : set_active <slot>"
  say "  reboot  : into the new system"
  say "  after   : NCM usb net -> ssh root@10.9.8.1 -> /run/boot.state (SKILL.md)"
  exit 0
fi

say "==> verify payload (pristine)"
sha_verify
[ -n "${PUBKEY}${WIFI_CONF}" ] && say "    (injections below will change rootfs.img afterwards — expected)"

if [ -n "${PUBKEY}" ]; then
  [ -f "${PUBKEY}" ] || { say "pubkey file not found: ${PUBKEY}" >&2; exit 1; }
  if grep -q "PRIVATE KEY" "${PUBKEY}"; then
    say "refusing: ${PUBKEY} contains a PRIVATE key — pass the .pub PUBLIC key" >&2
    exit 1
  fi
  PKABS="$(cd "$(dirname "${PUBKEY}")" && pwd)/$(basename "${PUBKEY}")"
  inject "${ROOTFS}" "rm /root/.ssh/authorized_keys
write ${PKABS} /root/.ssh/authorized_keys
sif /root/.ssh/authorized_keys mode 0100600" "ssh pubkey"
fi

if [ -n "${WIFI_CONF}" ]; then
  [ -f "${WIFI_CONF}" ] || { say "wifi.conf not found: ${WIFI_CONF}" >&2; exit 1; }
  WCABS="$(cd "$(dirname "${WIFI_CONF}")" && pwd)/$(basename "${WIFI_CONF}")"
  inject "${ROOTFS}" "rm /etc/wifi.conf
write ${WCABS} /etc/wifi.conf
sif /etc/wifi.conf mode 0100600" "wifi.conf"
fi

if [ -n "${PUBKEY}${WIFI_CONF}" ]; then
  fsck_repair "${ROOTFS}"
fi

ATTACHED="$(fastboot devices 2>/dev/null || true)"
COUNT="$(printf '%s\n' "${ATTACHED}" | grep -c . || true)"
[ "${COUNT}" = "1" ] || {
  say "refusing: expected exactly 1 fastboot device, found ${COUNT}:" >&2
  printf '%s\n' "${ATTACHED:-  (none)}" >&2
  exit 1
}

PRODUCT="$(fastboot getvar product 2>&1 | sed -n 's/^product: //p' | head -1 | tr -d '[:space:]')"
[ "${PRODUCT}" = "${GATE_PRODUCT}" ] || {
  say "refusing: device reports product '${PRODUCT}', this package is for '${GATE_PRODUCT}' (${DEVICE})" >&2
  exit 1
}
say "gate ok: product=${PRODUCT}"

SLOT="$(fastboot getvar current-slot 2>&1 | sed -n 's/^current-slot: //p' | head -1 | tr -d '[:space:]')"
case "${SLOT}" in
  a|b) : ;;
  *) say "refusing: could not read current slot (got '${SLOT:-<empty>}')" >&2; exit 1 ;;
esac
OTHER="b"; [ "${SLOT}" = "b" ] && OTHER="a"
say "slot ok: current=${SLOT} (other slot '${OTHER}' is NOT touched — it is the recovery lane)"

say "==> flash userdata (${ROOTFS})"
fastboot flash userdata "${ROOTFS}"

say "==> flash boot_${SLOT}"
fastboot flash "boot_${SLOT}" "${BOOTIMG}"

say "==> commit: set_active ${SLOT}"
fastboot set_active "${SLOT}"

say "==> reboot"
fastboot reboot

trap - ERR
say ""
say "done. Next: keep the USB cable plugged, wait ~90 s, then"
say "  ping 10.9.8.1 && ssh root@10.9.8.1"
say "and follow SKILL.md §verify. The screen staying dark is CORRECT —"
say "enchilada is a headless node; proof of life is the NCM usb net and"
say "/run/boot.state, not a panel."
