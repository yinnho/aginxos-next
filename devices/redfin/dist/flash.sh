#!/usr/bin/env bash
# flash.sh — AginxOS device package, redfin. PUBLIC artifact.
#
# Deterministic fresh-install flash. Dry-run by default; GO=1 executes.
# Gates: exactly one fastboot device, and `fastboot getvar product` must be
# "redfin". Order is crash-safe: userdata first, vendor_boot last (the
# commit point), then reboot.
set -euo pipefail

cd "$(dirname "$0")"

DEVICE="redfin"
ROOTFS="rootfs.img"
VENDOR_BOOT="boot/vendor_boot.img"
STOCK_VB="boot/vendor_boot.stock.img"

say() { printf '%s\n' "$*"; }

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
  say "Recovery: fastboot flash vendor_boot ${STOCK_VB} && fastboot reboot"
  say "returns the phone to the stock boot chain. Re-running this script is safe."
}
trap recover_hint ERR

command -v fastboot >/dev/null 2>&1 || { say "fastboot not on PATH (Android platform-tools)" >&2; exit 1; }

if [ -z "${GO:-}" ]; then
  say "dry-run (GO=1 to flash) — plan:"
  say "  verify  : SHA256SUMS over payload"
  say "  gate    : exactly one fastboot device, getvar product == ${DEVICE}"
  say "  flash 1 : userdata    ← ${ROOTFS}"
  say "  flash 2 : vendor_boot ← ${VENDOR_BOOT} (commit point)"
  say "  reboot  : into the new system"
  say "  after   : adb enumerates → /run/boot.state → configure (see SKILL.md)"
  exit 0
fi

say "==> verify payload"
sha_verify

ATTACHED="$(fastboot devices 2>/dev/null || true)"
COUNT="$(printf '%s\n' "${ATTACHED}" | grep -c . || true)"
[ "${COUNT}" = "1" ] || {
  say "refusing: expected exactly 1 fastboot device, found ${COUNT}:" >&2
  printf '%s\n' "${ATTACHED:-  (none)}" >&2
  exit 1
}

PRODUCT="$(fastboot getvar product 2>&1 | sed -n 's/^product: //p' | head -1 | tr -d '[:space:]')"
[ "${PRODUCT}" = "${DEVICE}" ] || {
  say "refusing: device reports product '${PRODUCT}', this package is for '${DEVICE}'" >&2
  exit 1
}
say "gate ok: product=${PRODUCT}"

say "==> flash userdata (${ROOTFS})"
fastboot flash userdata "${ROOTFS}"

say "==> flash vendor_boot (commit point)"
fastboot flash vendor_boot "${VENDOR_BOOT}"

say "==> reboot"
fastboot reboot

trap - ERR
say ""
say "done. Next: wait for adb, then follow SKILL.md §verify and §configure."
say "The screen staying dark after the bootloader stage is CORRECT — this is"
say "a headless OS; proof of life is adb + /run/boot.state, not the panel."
