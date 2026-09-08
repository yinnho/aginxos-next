#!/usr/bin/env bash
# Patch vendor_boot so /init is aginxos-init (vendor ramdisk overwrites boot ramdisk /init).
#
# Root cause: Pixel redfin loads boot + vendor_boot ramdisks; vendor's `init` symlink
# replaces anything we put in boot.img's ramdisk.
#
# redfin device pack (D14). Migrated from aginxos@0534ea8 boot/pack-vendor-boot.sh.
# Local inputs/outputs live under .local/device/redfin/ (see assets.md); the boot
# tools are shared under .local/boot-tools/.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
DEVLOCAL="${REPO}/.local/device/redfin"
TOOLS="${REPO}/.local/boot-tools"
OUTDIR="${DEVLOCAL}/boot-out"
INITSRC="${DEVLOCAL}/initramfs"
STOCK_VB="${DEVLOCAL}/stock/stock-vendor_boot.img"
UNPACK_VB="${OUTDIR}/vendor_boot_unpack"
WORK="${DEVLOCAL}/vendor-ramdisk-root"
OUT_VB="${OUTDIR}/vendor_boot-test.img"
HOLD="${HOLD:-0}"

mkdir -p "${OUTDIR}"

if [[ ! -f "${STOCK_VB}" ]]; then
  echo "missing ${STOCK_VB}" >&2
  exit 1
fi
if [[ ! -f "${INITSRC}/aginxos-init" ]]; then
  echo "missing ${INITSRC}/aginxos-init" >&2
  exit 1
fi
if [[ ! -f "${TOOLS}/mkbootimg.py" ]]; then
  echo "run ./scripts/fetch-boot-tools.sh first" >&2
  exit 1
fi

echo "==> unpack stock vendor_boot"
rm -rf "${UNPACK_VB}"
mkdir -p "${UNPACK_VB}"
python3 "${TOOLS}/unpack_bootimg.py" --boot_img "${STOCK_VB}" --out "${UNPACK_VB}" --format=info \
  | tee "${UNPACK_VB}/info.txt"

VR="${UNPACK_VB}/vendor_ramdisk"
DTB="${UNPACK_VB}/dtb"
test -f "${VR}"
test -f "${DTB}"

echo "==> extract vendor ramdisk"
rm -rf "${WORK}"
mkdir -p "${WORK}"
lz4 -dc "${VR}" | (cd "${WORK}" && cpio -idm)

echo "==> inject aginxos-init (rdinit + /init overwrite)"
if [[ ! -f "${WORK}/system/bin/init" ]]; then
  echo "vendor ramdisk missing /system/bin/init" >&2
  exit 1
fi
# Preserve Android init for handoff
cp -f "${WORK}/system/bin/init" "${WORK}/system/bin/init.android"
chmod 755 "${WORK}/system/bin/init.android"
rm -f "${WORK}/init.android"
cp -f "${WORK}/system/bin/init.android" "${WORK}/init.android"

mkdir -p "${WORK}/aginxos"

# C trampoline is the rdinit entry (Rust handoff is unreliable on redfin).
TRAMP="${INITSRC}/trampoline"
if [[ ! -x "${TRAMP}" ]]; then
  echo "==> build C trampoline"
  zig cc -target aarch64-linux-musl -static -O2 \
    -o "${TRAMP}" "${REPO}/devices/redfin/boot/trampoline.c"
fi
cp -f "${TRAMP}" "${WORK}/aginxos/trampoline"
chmod 755 "${WORK}/aginxos/trampoline"

# Optional Rust helper (splash-test child)
if [[ -f "${INITSRC}/aginxos-init" ]]; then
  cp -f "${INITSRC}/aginxos-init" "${WORK}/aginxos/aginxos-init"
  chmod 755 "${WORK}/aginxos/aginxos-init"
fi

# Static first-stage init from stock boot.img
if [[ -f "${INITSRC}/first_stage_init" ]]; then
  cp -f "${INITSRC}/first_stage_init" "${WORK}/aginxos/first_stage_init"
  chmod 755 "${WORK}/aginxos/first_stage_init"
  echo "note: included first_stage_init"
else
  echo "error: missing boot/initramfs/first_stage_init" >&2
  exit 1
fi

if [[ -f "${INITSRC}/aginxos-probe" ]]; then
  cp -f "${INITSRC}/aginxos-probe" "${WORK}/aginxos/aginxos-probe"
  chmod 755 "${WORK}/aginxos/aginxos-probe"
fi

# Feature flags (empty files). Safe default: HOLD only, no modules/splash.
HOLD="${HOLD:-0}"
SPLASH="${SPLASH:-0}"
# MODULES: 0 | 1 (small allow-list) | drm (stock modules.load through msm_drm)
MODULES="${MODULES:-0}"
MODULES_FULL="${MODULES_FULL:-0}"
if [[ "${HOLD}" == "1" ]]; then
  : >"${WORK}/aginxos/hold"
  echo "note: HOLD=1"
fi
if [[ "${SPLASH}" == "1" ]]; then
  : >"${WORK}/aginxos/splash"
  echo "note: SPLASH=1 (DRM paint; needs MODULES=drm or MODULES=1)"
fi
if [[ "${MODULES}" == "drm" || "${MODULES}" == "DRM" ]]; then
  # Preferred: same order Android first_stage uses, stop at msm_drm.ko
  : >"${WORK}/aginxos/load-modules-loadfile"
  echo "note: MODULES=drm → load /lib/modules/modules.load through msm_drm.ko"
elif [[ "${MODULES}" == "1" ]]; then
  : >"${WORK}/aginxos/load-modules"
  cat >"${WORK}/aginxos/modules.allow" <<'EOF'
# pinctrl / clocks / bus
pinctrl-msm.ko
pinctrl-spmi-gpio.ko
pinctrl-spmi-mpp.ko
pinctrl-lito.ko
msm_bus.ko
clk-qcom.ko
clk-aop-qmp.ko
cmd-db.ko
msm_ipc_logging.ko
qcom_rpmh.ko
clk-rpmh.ko
dispcc-lito.ko
gcc-lito.ko
llcc-slice.ko
llcc-lito.ko
# memory / iommu / ion / tz
qtee_shm_bridge.ko
secure_buffer.ko
msm_dma_iommu_mapping.ko
ion-alloc.ko
msm_bus_rpmh.ko
iommu-logger.ko
arm-smmu-debug.ko
arm-smmu.ko
# regulators / i2c / panel power
regmap-spmi.ko
qcom-spmi-pmic.ko
qcom-i2c-pmic.ko
qpnp-amoled-regulator.ko
rpmh-regulator.ko
qcom-geni-se.ko
i2c-qcom-geni.ko
# display + drm
fsa4480-i2c.ko
msm_ext_display.ko
qseecom.ko
hdcp_qseecom.ko
msm_hdcp.ko
msm_drm.ko
EOF
  echo "note: MODULES=1 wrote small display modules.allow"
fi
if [[ "${MODULES_FULL}" == "1" ]]; then
  : >"${WORK}/aginxos/load-modules-full"
  echo "note: MODULES_FULL=1 loads entire modules.load (RISKY)"
fi
# USBADB=1: ffs.adb gadget console (adbd is in this ramdisk already; see docs/HARDWARE.md)
# USBDIAG=1: same module chain, but diagnostics instead of gadget — extcon +
#            deferred-probe dumps, drivers_probe replay kick, then Android
#            handoff so the full kernel log can be read via adb+root dmesg.
USBADB="${USBADB:-0}"
USBDIAG="${USBDIAG:-0}"
# USBNOBIND=1 (with USBADB=1): full gadget setup but skip the final UDC bind,
# then hand off - bisects bind vs pre-bind stages and preserves the run's
# kmsg for reading from booted Android. Log-collection mode, not console mode.
USBNOBIND="${USBNOBIND:-0}"
if [[ "${USBADB}" == "1" || "${USBDIAG}" == "1" ]]; then
  cat >"${WORK}/aginxos/modules.usb" <<'EOF'
# USB gadget console chain. Raw finit_module in listed order.
# ROOT CAUSE (found 2026-08-26 via USBDIAG + bugreport kernel log): the chain
# must be a true modules.dep topological order. Two prior orderings failed:
#   - eud.ko needs qtee_shm_bridge.ko (exports scm_io_read/write) BEFORE it
#   - qpnp_pdphy.ko needs usb-dwc3-msm.ko (exports ext_vbus_register_notify)
#     BEFORE it — dwc3 defers until pdphy's extcon registers, and pdphy's
#     module load is exactly what replays the deferred probe (stock does the
#     same: dwc3-msm loads 1.25s, pdphy 1.30s, dwc3 probe completes 1.63s).
# This order is machine-validated against modules.dep (all edges satisfied).
# NOTE: rmmod eud panics this kernel — load-only, never unload.
# Foundation — clocks, power, pinctrl, bus
msm_ipc_logging.ko
msm_bus.ko
pinctrl-msm.ko
pinctrl-lito.ko
pinctrl-spmi-gpio.ko
pinctrl-spmi-mpp.ko
cmd-db.ko
# hwspinlock BEFORE smem: smem's probe defers on its hwlock supplier — without
# this, smem stays "deferred probe pending" and the reboot chain below fails
# ("Minidump: SMEM is not initialized"). Found 2026-08-27.
qcom_hwspinlock.ko
smem.ko
qcom_rpmh.ko
clk-rpmh.ko
clk-aop-qmp.ko
clk-qcom.ko
gcc-lito.ko
qcom-pdc.ko
msm_bus_rpmh.ko
refgen.ko
spmi-pmic-arb.ko
regmap-spmi.ko
qcom-spmi-pmic.ko
rpmh-regulator.ko
fsa4480-i2c.ko
# qtee + IOMMU/SMMU chain (qtee MUST precede eud: it exports scm_io_*)
qtee_shm_bridge.ko
iommu-logger.ko
secure_buffer.ko
arm-smmu-debug.ko
arm-smmu.ko
msm_dma_iommu_mapping.ko
# extcon supplier: qpnp-smb5 (charger)
# DT-level suppliers smb5 probe waits on (dtbo analysis 2026-08-26):
#   io-channels = pm7250b_vadc ("qcom,spmi-adc5")     -> adc5 + vadc-common
#   ext-vbus-supply = ext_boost ("regulator-tps")     -> tps-regulator.ko
# pdphy's connector node also consumes a vadc channel, and pdphy waits on
# smb5-vbus/vconn (smb5 child regulators) + ext_boost.
# SOURCE-level supplier (qpnp-smb5.c, verified vs android-msm-redbull-4.19):
#   smb5_probe() returns -EPROBE_DEFER SILENTLY unless alarmtimer_get_rtcdev()
#   is non-NULL -> needs rtc-pm8xxx (pm8150_rtc). Confirmed on device
#   2026-08-26: rtc-pm8xxx load at 21.503s -> "logbuffer: id:smblib
#   registered" 1.7ms later -> smb5 probe success at 21.522s.
# pdphy's usbpd_create -517 also unblocks with smb5: it defers on
# power_supply_get_by_name("usb") and find_votable("USB_ICL"), both created
# by smb5_probe. Chain: rtc -> smb5 -> pdphy -> ssphy/ssusb -> dwc3 -> UDC.
# pdphy's usbpd_create also needs the "wireless" power supply (DT has
# goog,wlc-supported): registered by p9221_charger (Qi RX, i2c 1-003b on
# geni i2c). Verified on device 2026-08-26: p9221 "id:wireless registered"
# at 21.4553s -> pdphy usbpd_create OK 2ms later. p9221 probe prints some
# benign errors (pin group 99, one i2c -107) — stock logs the same.
# The geni i2c controller devices themselves defer on their GPI DMA
# supplier (900000.qcom,gpi-dma) — virt-dma + gpi must come first, else
# the buses (and p9221 on 98c000.i2c) only probe on Android's wave.
virt-dma.ko
gpi.ko
qcom-geni-se.ko
i2c-qcom-geni.ko
qcom-vadc-common.ko
qpnp-revid.ko
qcom-spmi-adc5.ko
tps-regulator.ko
rtc-pm8xxx.ko
pmic-voter.ko
logbuffer.ko
p9221_charger.ko
qpnp-battery.ko
of_batterydata.ko
qpnp-smb5-charger.ko
# SCM + extcon supplier: msm-eud (load-only; rmmod panics)
msm_scm.ko
eud.ko
# dwc3 controller — loads BEFORE pdphy: pdphy needs its ext_vbus_* exports,
# and pdphy's later registration is the deferred-probe replay dwc3 waits for
dwc3.ko
usb-dwc3-msm.ko
# PHYs
phy-generic.ko
phy-msm-ssusb-qmp.ko
phy-msm-snps-hs.ko
# typec + extcon supplier: usb-pdphy (LAST — depends on usb-dwc3-msm)
roles.ko
tcpm.ko
qpnp_pdphy.ko
# Reboot-reason chain (2026-08-27): msm-poweroff registers the kernel restart
# handler that translates RESTART2 mode strings ("bootloader") into the PMIC
# PON scratch register — without it reboot(2) mode strings are lost and the
# box always boots normal. Verified live: chain loaded -> `reboot2 bootloader`
# reached fastboot in 6 s (vs button-combo before). Order matters:
# smem(minidump needs it, loaded above) -> smem_state -> minidump -> watchdog
# -> PON (needs SPMI stack, loaded above) -> msm-poweroff (needs all).
smem_state.ko
msm_minidump.ko
watchdog_v2.ko
qpnp-power-on.ko
msm-poweroff.ko
EOF
fi
if [[ "${USBADB}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-adb"
  echo "note: USBADB=1 → ffs.adb console (first test with HOLD=1)"
  # Shell for adbd's shell service: the vendor ramdisk has no /system/bin/sh.
  # Device's own toybox, deps all in the ramdisk lib64 set (boot/out/, local
  # only). Trampoline links it as sh + applets before forking adbd.
  if [[ -x "${OUTDIR}/toybox" ]]; then
    cp -f "${OUTDIR}/toybox" "${WORK}/aginxos/toybox"
    chmod 755 "${WORK}/aginxos/toybox"
  else
    echo "warning: no ${OUTDIR}/toybox — adb shell will have no sh" >&2
  fi
  # Headless adb auth (v32): A14 adbd forces the RSA handshake via
  # __android_log_is_debuggable() (ro.debuggable; "0" without a property
  # area) and this unit's libadbd_auth.so never reads adb_keys files.
  # Instead the trampoline stages a real /dev/__properties__/ area, pulled
  # from a normal Android boot of THIS unit and patched with
  # scripts/patch-prop-area.py (ro.debuggable=1, ro.secure=0). Device-
  # specific dump → boot/out/props{-min}/, never committed.
  PROPSRC="${OUTDIR}/props-min"
  if [[ -f "${PROPSRC}/property_info" ]]; then
    mkdir -p "${WORK}/aginxos/props"
    cp -f "${PROPSRC}"/* "${WORK}/aginxos/props/"
    echo "note: staged property area from ${PROPSRC}"
  else
    echo "warning: no ${PROPSRC}/property_info — adbd will come up unauthorized (run scripts/pull-prop-area.sh + scripts/patch-prop-area.py first)" >&2
  fi
fi
# STORAGE=1: aginxos-init (after the PID 1 takeover) loads the UFS chain and
# creates the /dev block nodes — boot-time storage, no manual insmod.
STORAGE="${STORAGE:-0}"
if [[ "${STORAGE}" == "1" ]]; then
  : >"${WORK}/aginxos/storage"
  echo "note: STORAGE=1 → aginxos-init brings up UFS after takeover"
fi
# SUPER=1: aginxos-init also parses super and mounts its _a sub-partitions
# (system/vendor/product/system_ext) ext4 ro via dm-linear. Implies STORAGE.
SUPER="${SUPER:-0}"
if [[ "${SUPER}" == "1" ]]; then
  : >"${WORK}/aginxos/super"
  echo "note: SUPER=1 → aginxos-init mounts super _a sub-partitions after takeover"
fi
# ROOTFS=1: aginxos-init mounts the ext4 rootfs on userdata and switch_roots
# into busybox init (M2). The rootfs image is flashed separately to userdata.
# Implies STORAGE.
ROOTFS="${ROOTFS:-0}"
if [[ "${ROOTFS}" == "1" ]]; then
  : >"${WORK}/aginxos/rootfs"
  echo "note: ROOTFS=1 → aginxos-init switch_roots into the userdata rootfs"
  # The rootfs world's adbd re-opens the ffs endpoints the trampoline's
  # console set up; without USBADB there is no gadget at all and the
  # booted system is unreachable (observed 2026-09-02: a ROOTFS=1-only
  # image bootlooped slot b to rollback — no modules.usb, no toybox sh,
  # no props). The working set is HOLD=1 USBADB=1 ROOTFS=1.
  if [[ "${USBADB}" != "1" ]]; then
    echo "error: ROOTFS=1 without USBADB=1 produces an unreachable system — refusing (use HOLD=1 USBADB=1 ROOTFS=1)" >&2
    exit 1
  fi
fi
# KEEPADBD=1 (diagnostic): with ROOTFS, do not kill the trampoline's adbd at
# the switch — it survives chroot and keeps the console alive in the new root.
# Pair with an inittab that does not respawn adbd (ep0 is single-open).
KEEPADBD="${KEEPADBD:-0}"
if [[ "${KEEPADBD}" == "1" ]]; then
  : >"${WORK}/aginxos/keep-adbd"
  echo "note: KEEPADBD=1 → old adbd kept alive through the switch (diagnostic)"
fi
# USBCFGONLY=1: mount configfs, create no gadget tree (bisect v13)
USBCFGONLY="${USBCFGONLY:-0}"
# USBG1ONLY=1: create /config/usb_gadget/g1 only, nothing else (bisect v14)
USBG1ONLY="${USBG1ONLY:-0}"
# USBPROPSONLY=1: g1 + property writes, no functions/configs (bisect v15)
USBPROPSONLY="${USBPROPSONLY:-0}"
# USBVIDPIDONLY=1: g1 + idVendor/idProduct writes only (bisect v16)
USBVIDPIDONLY="${USBVIDPIDONLY:-0}"
# USBMKG1ONLY=1: mkdir usb_gadget/g1 only, no writes (bisect v17)
USBMKG1ONLY="${USBMKG1ONLY:-0}"
# USBNOG1=1: usb_gadget dir only, never mkdir g1 (control, bisect v18)
USBNOG1="${USBNOG1:-0}"
# USBNOCLEANUP=1: build gadget, skip teardown before handoff (bisect v19)
USBNOCLEANUP="${USBNOCLEANUP:-0}"
# USBNOMODS=1: usb_console with NO module load, straight to configfs g1 (bisect v21)
USBNOMODS="${USBNOMODS:-0}"
if [[ "${USBNOMODS}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-nomods"
  echo "note: USBNOMODS=1 -> no modules, mkdir g1 only (bisect v21)"
fi
if [[ "${USBNOCLEANUP}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-nocleanup"
  echo "note: USBNOCLEANUP=1 -> no gadget teardown before handoff (bisect v19)"
fi
if [[ "${USBNOG1}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-nog1"
  echo "note: USBNOG1=1 -> usb_gadget dir only, no g1 (control v18)"
fi
if [[ "${USBMKG1ONLY}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-mkg1-only"
  echo "note: USBMKG1ONLY=1 -> mkdir g1 only, no prop writes (bisect v17)"
fi
if [[ "${USBVIDPIDONLY}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-vidpid-only"
  echo "note: USBVIDPIDONLY=1 -> g1 + vid/pid only (bisect v16)"
fi
if [[ "${USBPROPSONLY}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-props-only"
  echo "note: USBPROPSONLY=1 -> g1 + props only (bisect v15)"
fi
if [[ "${USBG1ONLY}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-g1-only"
  echo "note: USBG1ONLY=1 -> g1 dir only (bisect v14)"
fi
if [[ "${USBCFGONLY}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-configfs-only"
  echo "note: USBCFGONLY=1 -> configfs mount only, no gadget tree (bisect v13)"
fi
if [[ "${USBNOBIND}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-nobind"
  echo "note: USBNOBIND=1 -> gadget setup, no UDC bind, Android handoff (log-collection mode)"
fi
# USBNOBIND=1: skip final UDC bind (log-collection bisect)
# USBNOFFS=1: stop after configfs tree, skip ffs mount + adbd (bisect v12)
USBNOBIND="${USBNOBIND:-0}"
USBNOFFS="${USBNOFFS:-0}"
if [[ "${USBNOFFS}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-noffs"
  echo "note: USBNOFFS=1 -> configfs tree only, no ffs/adbd/bind (bisect)"
fi
if [[ "${USBNOBIND}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-nobind"
  echo "note: USBNOBIND=1 -> gadget setup, no UDC bind, Android handoff (log-collection mode)"
fi
if [[ "${USBDIAG}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-diag"
  echo "note: USBDIAG=1 → module load + extcon/deferred dumps + drivers_probe kick, then handoff"
fi
# USBPROBE=1: load modules + check UDC, then HOLD + paint the verdict on
# screen (green = UDC appeared, red = no UDC). ramoops is dead on this unit,
# so in HOLD mode the screen is the only observable channel.
USBPROBE="${USBPROBE:-0}"
if [[ "${USBPROBE}" == "1" ]]; then
  : >"${WORK}/aginxos/usb-probe"
  echo "note: USBPROBE=1 → UDC probe, HOLD + screen verdict (green/red)"
fi

echo "==> repack vendor ramdisk (lz4 -l)"
VRD_OUT="${OUTDIR}/vendor_ramdisk.lz4"
(
  cd "${WORK}"
  find . -print0 | cpio --null --create --format=newc 2>/dev/null | lz4 -l -12 >"${VRD_OUT}" \
    || find . | cpio -o -H newc | lz4 -l -12 >"${VRD_OUT}"
)
echo "vendor ramdisk $(wc -c <"${VRD_OUT}") bytes"

# cmdline from unpack pretty-info
VCMD=$(python3 - <<'PY' "${UNPACK_VB}/info.txt"
import sys,re
text=open(sys.argv[1]).read()
# format: vendor command line args: ...
for line in text.splitlines():
    if "command line" in line.lower() and ":" in line:
        print(line.split(":",1)[1].strip())
        break
PY
)
if [[ -z "${VCMD}" ]]; then
  VCMD="console=ttyMSM0,115200n8 androidboot.console=ttyMSM0 androidboot.hardware=redfin"
fi
# Proven entry on redfin: C trampoline → first_stage_init → Android
if [[ "${VCMD}" != *rdinit=* ]]; then
  VCMD="${VCMD} rdinit=/aginxos/trampoline"
fi
echo "vendor_cmdline: ${VCMD:0:100}..."

echo "==> mkbootimg vendor_boot → ${OUT_VB}"
python3 "${TOOLS}/mkbootimg.py" \
  --header_version 3 \
  --pagesize 4096 \
  --base 0x00000000 \
  --kernel_offset 0x00008000 \
  --ramdisk_offset 0x01000000 \
  --tags_offset 0x00000100 \
  --dtb "${DTB}" \
  --dtb_offset 0x01f00000 \
  --vendor_cmdline "${VCMD}" \
  --vendor_ramdisk "${VRD_OUT}" \
  --vendor_boot "${OUT_VB}"

ls -lh "${OUT_VB}"
file "${OUT_VB}"
echo
echo "Flash temporary (restore later with stock-vendor_boot.img):"
echo "  fastboot flash vendor_boot ${OUT_VB}"
echo "  fastboot reboot"
echo "Restore:"
echo "  fastboot flash vendor_boot ${STOCK_VB} && fastboot reboot"
