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
#   0. NO state pre-arm by default (刀5, 2026-09-12): the L0 image is a
#      universal zero-prep base — a fresh flash WANTS the factory shape
#      (no wifi.conf / env / secret / stamps; you configure AFTER, over
#      adb: push /etc/wifi.conf + set a root password or ssh pubkey).
#      CAPTURE=1 opts into the upgrade path (re-flash that keeps the
#      running system's state — /root/.ssh, wifi.conf, secrets): run
#      `./flash-redfin.sh capture` while the OLD system is still on adb,
#      THEN enter fastboot (manual Power+VolDown) — after the device
#      leaves adb it is too late for this boot. bake #20 receipt
#      (2026-09-09): state-restore is a one-shot handshake (marker
#      consumed by the boot that restores it) and `fastboot flash
#      userdata` rewrites the front 2 GiB only, so nothing re-arms state.
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
# a new rootfs with an unchanged boot side). CAPTURE=1 additionally
# pre-arms the state tar (upgrade path — see step 0; fails hard if the
# capture does not verify).
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
ADB_SERIAL="$(sed -n 's/^serial *= *"\([^"]*\)".*/\1/p' "${PROFILE}")"

say() { printf '%s\n' "$*"; }

# ---- state pre-arm: bake #20 trap ------------------------------------------
# Stage the state tar + one-shot AGXSTATE marker from the RUNNING system
# (block 16777216 on userdata; fastboot flashes the front 2 GiB only, so
# the marker+body survive the reflash and the new image's state-restore
# consumes them). Returns 0 armed, 1 not (absent device / old binary /
# readback mismatch) — callers decide whether that is fatal.
capture_state() {
  adb devices 2>/dev/null | grep -q "${ADB_SERIAL}" || { say "no adb device '${ADB_SERIAL}' — cannot capture now"; return 1; }
  say "==> state pre-arm: aginx-update capture on ${ADB_SERIAL}"
  # absolute path: the adb shell PATH does not include /usr/bin (rc=127
  # observed 2026-09-09 with the bare name). 刀F: update is a package —
  # /var/bin face when the aginx family is opted in; /usr/bin only on
  # pre-刀F images. Bare L0 has neither — fail-open (nothing to capture).
  UPD=""
  for p in /var/bin/aginx-update /usr/bin/aginx-update; do
    if adb -s "${ADB_SERIAL}" shell "test -x $p" >/dev/null 2>&1; then UPD="$p"; break; fi
  done
  if [ -z "${UPD}" ]; then
    say "no on-device aginx-update (bare L0 or not opted in) — state not captured"
    return 1
  fi
  if ! adb -s "${ADB_SERIAL}" shell "${UPD} capture"; then
    say "WARNING: on-device aginx-update capture failed (binary predates the verb?)" >&2
    say "  after flashing, re-arm manually — HARDWARE.md bake #20 receipt" >&2
    return 1
  fi
  local magic
  magic="$(adb -s "${ADB_SERIAL}" shell \
    'dd if=/dev/block/by-name/userdata bs=4096 skip=16777216 count=1 2>/dev/null | head -c 8' \
    | tr -d '\r')"
  if [ "${magic}" != "AGXSTATE" ]; then
    say "WARNING: state marker readback '${magic}' != AGXSTATE" >&2
    return 1
  fi
  say "state marker armed (AGXSTATE at block 16777216, readback ok)"
}

if [ "${1:-}" = "capture" ]; then
  test -n "${ADB_SERIAL}" || { echo "no adb serial in ${PROFILE}" >&2; exit 1; }
  capture_state || { echo "state pre-arm FAILED — fix before flashing (or knowingly skip)" >&2; exit 1; }
  say "next: enter fastboot (manual Power+VolDown through a reboot), then GO=1 this script"
  exit 0
fi

# ---- gate: attached device must be THE machine -----------------------------
# L0 default: NO state pre-arm — the fresh image is factory-shaped by
# design (configure after, over adb/ssh). CAPTURE=1 arms it now and
# fails hard on any capture defect (the operator asked for the upgrade
# path; a silent skip would flash away /root/.ssh + wifi.conf).
if [ -z "${GO:-}" ]; then
  say "dry-run (GO=1 to flash) — plan:"
  say "  state       : none (L0 出厂形状 — CAPTURE=1 升级路径，先 './flash-redfin.sh capture')"
  say "  serial gate : fastboot devices must list exactly '${FB_SERIAL}'"
  say "  rootfs      : ${ROOTFS_IMG}"
  say "  vendor_boot : HOLD=1 USBADB=1 ROOTFS=1 pack-vendor-boot.sh (SKIP_PACK=1 to reuse)"
  say "  flash order : userdata first, vendor_boot last (commit point), reboot"
  say "  after boot  : adb push wifi.conf → /etc/, passwd 或 authorized_keys → ssh 接管"
  say "  recovery    : fastboot flash vendor_boot ${STOCK_VB}"
  exit 0
fi

if [ -n "${CAPTURE:-}" ]; then
  capture_state \
    || { echo "CAPTURE=1 but state pre-arm FAILED — fix before flashing" >&2; exit 1; }
else
  say "state pre-arm skipped（L0 出厂形状：无 marker，首启无 state-restore；CAPTURE=1 走升级路径）"
fi

ATTACHED="$(fastboot devices 2>/dev/null || true)"
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
