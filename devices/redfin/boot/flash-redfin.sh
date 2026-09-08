#!/usr/bin/env bash
# flash-redfin — the redfin flash day, one command (P3①).
#
# bake → pack → flash, with the multi-machine safety gate the AGENTS
# Device Safety section demands: every fastboot call is pinned to the
# fastboot serial from THIS device's profile (../device.toml [adb]),
# and the script refuses to flash anything else. A second phone on the
# bench (enchilada) is exactly why the gate exists.
#
# Sequence (crash-safe order, commit point last):
#   1. rootfs.img must exist (DEVICE=redfin ./scripts/build-rootfs.sh)
#   2. pack vendor_boot with HOLD=1 USBADB=1 ROOTFS=1 (the working set,
#      observed 2026-09-02 — ROOTFS=1 without USBADB=1 boots unreachable)
#   3. fastboot flash userdata   ← the big payload first; until the
#      vendor_boot lands the OLD system keeps booting untouched
#   4. fastboot flash vendor_boot ← LAST: this is the point where the
#      machine switches to the new world
#   5. fastboot reboot
#
# Dry-run by default: prints the plan and exits. GO=1 executes.
# SKIP_PACK=1 reuses the already-packed vendor_boot (quick re-flash of
# a new rootfs with an unchanged boot side).
#
# Recovery: fastboot flash vendor_boot the stock image
# (.local/device/redfin/stock/stock-vendor_boot.img) returns the slot
# chain to the known-good restore point; userdata re-flash is this
# script again.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEVDIR="$(cd "${HERE}/.." && pwd)"
REPO="$(cd "${DEVDIR}/../.." && pwd)"
PROFILE="${DEVDIR}/device.toml"
STOCK_VB="${REPO}/.local/device/redfin/stock/stock-vendor_boot.img"
ROOTFS_IMG="${ROOTFS:-${REPO}/out/rootfs.img}"
OUT_VB="${REPO}/.local/device/redfin/boot-out/vendor_boot-test.img"

test -f "${PROFILE}" || { echo "missing ${PROFILE}" >&2; exit 1; }

# The serial gate: this script flashes exactly one machine, by its
# profile. Grep'd from the sibling device.toml (sed with [^"]* — a
# greedy .* swallows inline comments on BSD sed).
FB_SERIAL="$(sed -n 's/^fastboot_serial *= *"\([^"]*\)".*/\1/p' "${PROFILE}")"
test -n "${FB_SERIAL}" || { echo "no fastboot_serial in ${PROFILE}" >&2; exit 1; }

say() { printf '%s\n' "$*"; }

# ---- gate: attached device must be THE machine -----------------------------
ATTACHED="$(fastboot devices 2>/dev/null || true)"
if [ -z "${GO:-}" ]; then
  say "dry-run (GO=1 to flash) — plan:"
  say "  serial gate : fastboot devices must list exactly '${FB_SERIAL}'"
  say "  rootfs      : ${ROOTFS_IMG}"
  say "  vendor_boot : HOLD=1 USBADB=1 ROOTFS=1 pack-vendor-boot.sh (SKIP_PACK=1 to reuse)"
  say "  flash order : userdata first, vendor_boot last (commit point), reboot"
  say "  recovery    : fastboot flash vendor_boot ${STOCK_VB}"
  exit 0
fi

echo "${ATTACHED}" | grep -q "${FB_SERIAL}" \
  || { echo "refusing: fastboot does not see '${FB_SERIAL}' (profile serial). Attached:" >&2
      echo "${ATTACHED:-  (nothing)}" >&2; exit 1; }
# Exactly the profile serial may be attached — another fastboot device on
# the bench (wrong cable swapped) must abort, not flash in parallel.
EXTRA="$(echo "${ATTACHED}" | grep -v "${FB_SERIAL}" | grep -v '^[[:space:]]*$' || true)"
if [ -n "${EXTRA}" ]; then
  echo "refusing: other fastboot device(s) attached:" >&2
  echo "${EXTRA}" >&2
  exit 1
fi
say "serial gate ok: ${FB_SERIAL}"

# ---- payload checks ---------------------------------------------------------
test -s "${ROOTFS_IMG}" \
  || { echo "missing ${ROOTFS_IMG} — run DEVICE=redfin ./scripts/build-rootfs.sh first" >&2; exit 1; }
test -f "${STOCK_VB}" \
  || { echo "missing stock restore point ${STOCK_VB}" >&2; exit 1; }

if [ -n "${SKIP_PACK:-}" ]; then
  test -s "${OUT_VB}" || { echo "SKIP_PACK=1 but no packed ${OUT_VB}" >&2; exit 1; }
  say "reusing packed vendor_boot ${OUT_VB}"
else
  say "==> pack vendor_boot (HOLD=1 USBADB=1 ROOTFS=1)"
  HOLD=1 USBADB=1 ROOTFS=1 "${HERE}/pack-vendor-boot.sh"
fi

# ---- flash: payload first, commit point last --------------------------------
say "==> flash userdata (${ROOTFS_IMG})"
fastboot -s "${FB_SERIAL}" flash userdata "${ROOTFS_IMG}"

say "==> flash vendor_boot (commit point)"
fastboot -s "${FB_SERIAL}" flash vendor_boot "${OUT_VB}"

say "==> reboot into the new image"
fastboot -s "${FB_SERIAL}" reboot

say "done. If bring-up fails: fastboot -s ${FB_SERIAL} flash vendor_boot ${STOCK_VB}"
say "restores the stock slot chain (AGENTS Device Safety end-state rule)."
