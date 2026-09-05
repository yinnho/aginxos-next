#!/usr/bin/env bash
# Build the N4 AginxOS rootfs image — the new repo owns the bake chain.
#
# Assembles the tree from: the recipe in ./rootfs (etc + aginx-* faces +
# libexec daemons), new-repo musl binaries (zigbuild), and FIRST-GEN
# ASSETS referenced in place from the old repo (OLD=, single-source
# discipline — busybox, C tools, vendor ramdisk, voice/OCR stacks,
# dropbear, radio blobs, fonts) plus the frozen aginxos trampoline pair
# (N5②: every renamed-at-install CLI is now rebuilt here instead); the
# old `ag` router, ag-* shims, carrier daemon and relay do NOT enter
# the image (切净).
#
# Flash with:  fastboot flash userdata out/rootfs.img
# Boot needs a vendor_boot packed with ROOTFS=1 (old repo pack-vendor-boot.sh).
#
# Note: mke2fs -d records the building user's uid (501 on macOS) as owner.
# rcS chowns everything back to 0:0 on first boot — do not "fix" that here.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OLD="${OLD:-$HOME/Documents/aginxos}"
RAMDISK="${OLD}/boot/out/vendor-ramdisk-root"
ORECIPE="${OLD}/boot/rootfs"
OTARGET="${OLD}/target/aarch64-unknown-linux-musl/release"
RECIPE="${ROOT}/rootfs"
TARGET="${ROOT}/target/aarch64-unknown-linux-musl/release"
TREE="${TREE:-/tmp/aginxos-n4-rootfs}"
IMG="${IMG:-${ROOT}/out/rootfs.img}"
# 2 GB sparse-ish image (bake #18 data: 651M used; N4 drops carrier+relay).
SIZE="${SIZE:-2g}"

test -x "${RAMDISK}/system/bin/adbd" || { echo "missing ${RAMDISK} — old repo boot/unpack-boot.sh first" >&2; exit 1; }
test -x "${ORECIPE}/busybox" || { echo "missing ${ORECIPE}/busybox (old repo asset)" >&2; exit 1; }
MKE2FS="$(command -v mke2fs || true)"
test -z "${MKE2FS}" && MKE2FS=/opt/homebrew/bin/mke2fs
test -x "${MKE2FS}" || { echo "mke2fs not found (android-platform-tools provides it)" >&2; exit 1; }

# First-gen musl binaries, renamed at install (D13). N5② emptied this
# list down to the trampoline pair: every CLI the new repo has source for
# (download/update/qr/done/secret + the earlier voice/wizard/term/svc/
# pkg/sign wave) is rebuilt here instead; the trampoline stays frozen
# deliberately (aginxos-init owns the userdata rootfs swap — swap the
# swapper and the update flow has no rollback story).
for b in aginxos-init aginxos-agent; do
  test -x "${OTARGET}/${b}" \
    || { echo "missing old ${b} — old repo ./scripts/build-phone.sh musl first" >&2; exit 1; }
done

# Voice stack (M42d) + OCR (M45) — bionic-static CLIs and their models,
# first-gen build products. Voice is the bootstrap human interface (the
# WiFi-join flow speaks before any network exists) and 念读 is the eye
# path that must work offline, so the models ride the baked image rather
# than provision. /var/bin overlaps the provision overlay, but provision
# only fills manifest items (asr/tts/ocr are not in it) and never wipes
# extras. Renamed at install: ag-asr→aginx-asr, ag-tts→aginx-tts,
# ag-ocr→aginx-ocr (spawn paths in crates/voice/src/{audio,main}.rs).
VOICE="${OLD}/out/voice"
test -x "${VOICE}/bin/ag-asr" && test -x "${VOICE}/bin/ag-tts" \
  || { echo "missing old out/voice/bin/ag-{asr,tts} — run scripts/build-voice.sh there" >&2; exit 1; }
test -s "${VOICE}/models/asr/model.int8.onnx" \
  && test -s "${VOICE}/models/tts/vits-melo-tts-zh_en/model.onnx" \
  && test -s "${VOICE}/models/tts/vits-melo-tts-zh_en/lexicon.txt" \
  && test -s "${VOICE}/models/tts/vits-melo-tts-zh_en/tokens.txt" \
  || { echo "missing voice models — old repo scripts/fetch-voice-models.sh" >&2; exit 1; }
OCR="${OLD}/out/ocr"
test -x "${OCR}/bin/ag-ocr" \
  || { echo "missing old out/ocr/bin/ag-ocr — run scripts/build-ocr.sh there" >&2; exit 1; }
test -s "${OCR}/models/det.onnx" && test -s "${OCR}/models/rec.onnx" \
  && test -s "${OCR}/models/dict.txt" \
  || { echo "missing ocr models — old repo scripts/fetch-ocr-models.sh" >&2; exit 1; }

echo "==> zigbuild 新仓 musl 件（缓存则秒过）"
(cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
  -p aginx-router -p aginx-server -p aginx-runtime -p aginx-voice \
  -p aginx-net-wizard -p aginx-term -p aginx-pkg -p aginx-svc \
  -p aginx-download -p aginx-update -p aginx-done -p aginx-secret \
  -p aginx-gateway)

# N5② feature-unification trap: aginx-voice depends aginx-qr with
# default-features=false (it only links parse_wifi_payload). Selecting
# aginx-qr/jpeg in the SAME invocation as aginx-voice would unify
# quircs+aginx-img into the aginx-voice binary — bloat, and the qr
# decoder would no longer be a separate process. So the device aginx-qr
# comes from a SECOND, separate invocation. The size tripwire below
# catches exactly that unification (a relinked voice changes size);
# if it fires, someone folded the two invocations together.
VOICE_SZ_BEFORE="$(stat -f%z "${TARGET}/aginx-voice")"
(cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
  -p aginx-qr --features aginx-qr/jpeg)
VOICE_SZ_AFTER="$(stat -f%z "${TARGET}/aginx-voice")"
[[ "${VOICE_SZ_BEFORE}" == "${VOICE_SZ_AFTER}" ]] \
  || { echo "FATAL: aginx-voice changed size across the aginx-qr build (${VOICE_SZ_BEFORE} → ${VOICE_SZ_AFTER}) — feature unification leak; keep the two zigbuild invocations separate" >&2; exit 1; }

# Package manifest rides SIGNED: the on-device default path requires a
# detached sig or every `aginx-pkg sync` refuses (fail-closed). Content-
# based check (git does not carry mtimes): resign when the sig is missing
# or no longer verifies against the manifest (a machine with the pubkey
# can prove staleness; without it we ship the committed sig as-is — it is
# a public artifact, safe to commit). N4 note: this gate runs BEFORE the
# recipe cp below, so a fresh resign lands in the image the same bake —
# the first-gen script signed after the copy and could ship a stale sig.
AGPKG_KEY=".local/keys/aginx.key"
AGPKG_PUB=".local/keys/aginx.pub"
AGPKG_MF="${RECIPE}/etc/agpkg.manifest"
AGPKG_SIG="${RECIPE}/etc/agpkg.manifest.sig"
AGPKG_NEED_SIGN=0
[[ -f "${AGPKG_SIG}" ]] || AGPKG_NEED_SIGN=1
if [[ "${AGPKG_NEED_SIGN}" -eq 0 && -f "${ROOT}/${AGPKG_PUB}" ]] \
   && ! (cd "${ROOT}" && cargo run -q -p aginx-sign -- verify "${AGPKG_PUB}" "${AGPKG_MF}" >/dev/null 2>&1); then
  AGPKG_NEED_SIGN=1
fi
if [[ "${AGPKG_NEED_SIGN}" -eq 1 ]]; then
  [[ -f "${ROOT}/${AGPKG_KEY}" ]] || { echo "FATAL: ${AGPKG_MF} needs signing but ${AGPKG_KEY} is missing" >&2; exit 1; }
  (cd "${ROOT}" && cargo run -q -p aginx-sign -- sign "${AGPKG_KEY}" "${AGPKG_MF}")
  echo "==> signed ${AGPKG_MF} (commit the refreshed .sig)"
fi

rm -rf "${TREE}"
mkdir -p "${TREE}"

# Mountpoints (and /var/log — the only place boot evidence survives; the
# kernel has no pstore, so /var/adbd.log is our cross-boot record).
# /var/power + the seven /var/lib/aginx members (N5③: skills, units,
# stamps, pkgfiles, done, secret, voice — the single state home; the old
# agpkg/ag/voiced roots fold in via /etc/init.d/varlib-migrate): state-tar
# members that must exist on a fresh image — busybox tar exits 1 on a
# missing member, which the hardened aginx-update rightly treats as fatal
# (observed 2026-09-03, bake #9). provision seeds them at runtime too;
# varlib-migrate mkdir's on every boot as the belt to this pair of braces.
# /var/tmp — NOT tmpfs (only /tmp is), yet nothing created it: bake #10's
# fresh image shipped without it, provision's `>$LOG` redirect failed and
# resync reported pkg-fail-with-no-log (observed 2026-09-03). aginx-update
# also stage-builds its state tar there (M22 note).
mkdir -p "${TREE}"/{dev,proc,sys,etc,home,media,mnt,opt,root,run,srv,tmp,var/log,var/power,var/tmp}
mkdir -p "${TREE}"/var/lib/aginx/{skills,units,stamps,pkgfiles,done,secret,voice}

# Android pieces: /system (adbd + linker config + lib64) and the root-level
# property/SELinux files adbd reads at startup.
cp -R "${RAMDISK}/system" "${TREE}/system"
for f in default.prop prop.default *_contexts; do
  cp "${RAMDISK}"/${f} "${TREE}/" 2>/dev/null || true
done

# Kernel modules for the touch/display chain (M3) — the ramdisk half. The
# vendor_boot base loads only the 64-module USB/storage set (modules.usb);
# the full modules.load load panics this kernel (observed 2026-08-27, retry
# counter burned), so the touch chain is loaded from the rootfs world by
# /etc/init.d/touch-bringup, in the order proven live. Same .ko files as
# the ramdisk holds — copied from the local unpack (never committed, §7).
MODULES="spi-geni-qcom rpmsg_core qrtr qrtr-smd ion-alloc qseecom \
hdcp_qseecom msm_hdcp msm_ext_display llcc-slice dispcc-lito \
qpnp-amoled-regulator msm_drm"
# Battery chain (M3c) — loaded by /etc/init.d/battery-bringup. Order
# matters: google-bms provides gbms_storage, at24 registers the
# batt_eeprom entry qpnp-qgauge's probe reads, qpnp-qgauge registers the
# "bms" psy google-battery waits on. qti_qmi_sensor rides along last
# (charge mitigation; needs qmi_helpers from the vendor half).
MODULES="${MODULES} google-bms at24 qpnp-qgauge sm7250_bms google-battery \
google_charger qti_qmi_sensor"
mkdir -p "${TREE}/lib/modules"
for m in ${MODULES}; do
  cp "${RAMDISK}/lib/modules/${m}.ko" "${TREE}/lib/modules/"
done

# DRM splash painter — the panel stays black without an explicit mode set
# (the bootloader logo is cont-splash scanout, not KMS; connector sits at
# enabled=disabled). touch-bringup paints green when touch is up. Built with
# the same zig toolchain as the trampoline. binder-init mounts binderfs and
# allocates the binder/hwbinder/vndsbinder devices cnss-daemon needs (this
# kernel's backport ioctl struct — see the source header).
ZIG="$(command -v zig || true)"
test -z "${ZIG}" && ZIG=/opt/homebrew/bin/zig
test -x "${ZIG}" || { echo "zig not found (needed for splash2/binder-init)" >&2; exit 1; }
# usr/bin now, not at the recipe step below: the four D13-renamed C tools
# (aginx-cam-shot/net-scan/net-join/reboot) are zig-built straight into it.
mkdir -p "${TREE}/bin" "${TREE}/usr/bin"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/splash" "${ORECIPE}/src/splash2.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/binder-init" "${ORECIPE}/src/binder-init.c"
# QRTR observability (M3d): qrtr-lookup snapshots/watches the name service,
# qmi-req sends one raw QMI request. radio-bringup starts a qrtr-lookup
# watcher before the modem boot trigger to record the fresh-boot service
# registration order (WLFW 0x45 transient vs never-present).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/qrtr-lookup" "${ORECIPE}/src/qrtr-lookup.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/qmi-req" "${ORECIPE}/src/qmi-req.c"
# cam-shot (M19) — the IFE/RDI stills capture tool. Vendor sensor register
# tables are decoded into the source; vendor module bins stay local and
# gitignored. N4: the four brain-facing C tools take their D13 /usr/bin
# names AT BUILD TIME (aginx-voice spawns /usr/bin/aginx-cam-shot; net-bringup
# and net-rejoin call /usr/bin/aginx-net-join; wizard scans through
# /usr/bin/aginx-net-scan; reboot is /usr/bin/aginx-reboot). Default flags
# for a rear shot: --stream --rear --slowrear --rawvendor [--gain N] [--png].
# M47①: the camera trio (cam-shot.c + jpegenc.h + raw2jpg.c) moved into
# THIS repo's rootfs/src — the camera line is owned here now; the old-repo
# copies are frozen history.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-cam-shot" "${RECIPE}/src/cam-shot.c"
# raw2jpg (M19c) — RAW10 dump -> JPEG converter, companion to cam-shot's
# native --jpeg (for converting already-captured dumps).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/raw2jpg" "${RECIPE}/src/raw2jpg.c"

# Bionic LD_PRELOAD helpers (M3d). These load into vendor binaries, so they
# must be NDK/bionic shared objects, not musl. trace_open.so mirrors file
# access AND logcat output (__android_log_print & co) onto stderr — it is
# our only window into cnss-daemon/pd-mapper, which log exclusively through
# liblog and we run no logd. fake-props.so fakes the servicemanager
# properties pm-service blocks on and logs every other property read.
NDK_CC="${HOME}/Library/Android/sdk/ndk/27.0.12077973/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android24-clang"
test -x "${NDK_CC}" || { echo "NDK clang not found (needed for preload .so)" >&2; exit 1; }
mkdir -p "${TREE}/lib"
"${NDK_CC}" -shared -fPIC -O2 -o "${TREE}/lib/trace_open.so" "${ORECIPE}/src/trace_open.c"
"${NDK_CC}" -shared -fPIC -O2 -o "${TREE}/lib/fake-props.so" "${ORECIPE}/src/fake-props.c"
echo "built preload helpers (trace_open.so, fake-props.so)"
# fake-sm: minimal binder context manager (musl-static) answering every
# transaction with Status-ok. Without a CM on /dev/binder, vendor libbinder
# clients (cnss-daemon, pm-service) spin forever in "Waiting 1s on context
# object" before ever reaching their QMI work.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/fake-sm" "${ORECIPE}/src/fake-sm.c"
# aginx-reboot (原 reboot2): raw reboot(LINUX_REBOOT_CMD_RESTART2) — toybox
# reboot signals init (we run none) and adb reboot needs adbd's sys.powerctl
# handling. With no args it plain-reboots; "bootloader" lands in fastboot
# for re-flashing.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-reboot" "${ORECIPE}/src/reboot2.c"
# wdt (M20c): watchdog probe/arm/starve for /dev/watchdog. The dog itself
# is armed and petted by aginx-svcd (crates/svc); this is the diagnostics
# tool that proved the platform story (softdog behind msm_watchdog,
# hardware bark resources absent) and the live-fire starve reset.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/wdt" "${ORECIPE}/src/wdt.c"
# rtcal (M23b): pm8xxx RTC alarm arm/read + `sync` — the suspend probes' wake
# path ("set <epoch>" semantics kept from the /tmp zig one-off). net-bringup
# runs `rtcal sync` after ntpd to fix the RTC's -53y offset, which also makes
# early-boot wall time true on the next HCTOSYS pass.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/rtcal" "${ORECIPE}/src/rtcal.c"
# Ops channel sshd (#142, 2026-09-04) — the maintenance face: ssh in over
# Wi-Fi or `adb forward tcp:2222 tcp:22` when the subnets differ. Static
# musl dropbear triplet from the old repo's scripts/build-dropbear.sh
# (zig cc; the AR must be LLVM's — macOS BSD ar archives break lld member
# resolution). rcS starts the daemon key-only with the host key under
# /root/.ssh.
DROPBEAR="${OLD}/.local/dropbear/bin"
for b in dropbear dbclient dropbearkey; do
  test -x "${DROPBEAR}/${b}" || { echo "missing ${b} — old repo scripts/build-dropbear.sh" >&2; exit 1; }
  cp "${DROPBEAR}/${b}" "${TREE}/bin/${b}"
  chmod 755 "${TREE}/bin/${b}"
done
# aginx-net-scan (原 nlscan): nl80211 trigger-scan + dump client — busybox
# has no wireless tools and we ship no libnl. Our WLAN operability check
# (M3f); the wizard scans through it.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-net-scan" "${ORECIPE}/src/nlscan.c"
# aginx-net-join (原 wifi-join, M4): self-contained WPA2-PSK supplicant —
# CONNECT, EAPOL 4-way handshake over an AF_PACKET socket, NEW_KEY installs;
# then udhcpc owns IP provisioning. wifi-trace flips QCA vendor dp-trace
# levels for TX/RX logs (internal, /bin).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-net-join" "${ORECIPE}/src/wifi-join.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/wifi-trace" "${ORECIPE}/src/wifi-trace.c"
# M18 audio I/O: bare-ioctl PCM pair (no alsa-lib) — capture is the
# agent's "listen" path, playback its "speak" path. Shared uapi header.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/snd-cap" "${ORECIPE}/src/snd-cap.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/snd-play" "${ORECIPE}/src/snd-play.c"
# snd-mixer: ctl get/set (no alsa-lib) — audio-bringup's whole routing
# recipe runs through it. i2c-reg: rt5514 register peek/poke over
# /dev/i2c-N (kernel has no debugfs here — see audio-bringup notes).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/snd-mixer" "${ORECIPE}/src/snd-mixer.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/i2c-reg" "${ORECIPE}/src/i2c-reg.c"
# Boot card (M5): DRM boot-status renderer — polls /run/boot.state and
# paints the AginxOS bring-up checklist on the panel. Holds DRM master
# for its whole life (it replaces the M3 green splash). Same zig static
# build; host-side layout check via `bootcard --ppm out.ppm [state]`.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/bootcard" "${ORECIPE}/src/bootcard.c"
# httpget: minimal HTTP fetch for the boot internet check — busybox's wget
# applet segfaults in this build (2026-08-28), so net-bringup uses ours.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/httpget" "${ORECIPE}/src/httpget.c"
# udhcpc event hook (compiled-in default path) — without it udhcpc wins a
# lease but nothing applies it to the interface.
mkdir -p "${TREE}/usr/share/udhcpc"
cp "${ORECIPE}/usr/share/udhcpc/default.script" "${TREE}/usr/share/udhcpc/"
chmod 755 "${TREE}/usr/share/udhcpc/default.script"

# Radio bring-up payload (M3d). libnl.so is the bionic build cnss-daemon
# dlopens (LD_LIBRARY_PATH=/lib/...); rmt_storage is the PATCHED stock
# binary (erase call sites NOPed); cdsp-cdsp-loader.ko is stock
# cdsp-loader.ko with module/driver/sysfs names renamed (compat + code
# untouched — it binds soc:qcom,msm-cdsp-loader and boots the CDSP) and
# modem-npucc-loader.ko is the modem variant re-anchored to the npucc
# node. See the old repo's .local/radio/README.md. All are vendor-derived
# blobs: they live only in gitignored .local/radio/ and are copied in when
# present. scripts/build-radio-blobs.sh (old repo) regenerates them.
RADIO="${OLD}/.local/radio"
if [ -f "${RADIO}/libnl.so" ] && [ -x "${RADIO}/rmt_storage" ] \
   && [ -f "${RADIO}/cdsp-cdsp-loader.ko" ] \
   && [ -f "${RADIO}/modem-npucc-loader.ko" ]; then
  mkdir -p "${TREE}/lib" "${TREE}/lib/modules"
  cp "${RADIO}/libnl.so" "${TREE}/lib/libnl.so"
  cp "${RADIO}/rmt_storage" "${TREE}/bin/rmt_storage"
  chmod 755 "${TREE}/bin/rmt_storage"
  cp "${RADIO}/cdsp-cdsp-loader.ko" "${RADIO}/modem-npucc-loader.ko" \
     "${TREE}/lib/modules/"
  echo "staged radio payload (libnl.so + patched rmt_storage + cdsp/modem loaders)"
else
  echo "NOTE: .local/radio incomplete — radio-bringup will fail; run scripts/build-radio-blobs.sh" >&2
fi

# Recipe: etc (init.d/aginx/svc.d units/aginx conf/apps.d/crontabs + manifest+sig),
# usr/bin (bridge sh faces + .aginxmd sidecars), libexec/aginx
# (net-watch/net-rejoin), var/bin sidecars. All D13 knowledge lives here.
mkdir -p "${TREE}/bin" "${TREE}/sbin" "${TREE}/aginxos" "${TREE}/usr/libexec/aginx" "${TREE}/var/bin"
cp "${ORECIPE}/busybox" "${TREE}/bin/busybox"
cp -R "${RECIPE}/etc/." "${TREE}/etc/"
cp -R "${RECIPE}/usr/bin/." "${TREE}/usr/bin/"
cp -R "${RECIPE}/libexec/aginx/." "${TREE}/usr/libexec/aginx/"
cp "${RECIPE}"/var/bin/*.aginxmd "${TREE}/var/bin/"
# version stamp (M14): what the running image is, for aginx-update
# status/compare. N4: stamped from THIS repo's git.
{ git -C "${ROOT}" log -1 --format="aginxos %h %cd" --date=short 2>/dev/null || echo "aginxos unknown"; } > "${TREE}/etc/aginx-version"

# Router (N1④) — the bare `aginx` mother face. Engines stay OUT of the
# command universe in /usr/libexec/aginx (D13: libexec 是引擎的家).
install -m 755 "${TARGET}/aginx" "${TREE}/usr/bin/aginx"
install -m 755 "${TARGET}/aginx-server" "${TARGET}/aginx-runtime" "${TREE}/usr/libexec/aginx/"
# Platform CLIs (new-repo builds; N4③b 改姓四件 + voice + wizard).
install -m 755 "${TARGET}/aginx-voice" "${TARGET}/aginx-net-wizard" \
  "${TARGET}/aginx-term" "${TARGET}/aginx-pkg" "${TREE}/usr/bin/"
install -m 755 "${TARGET}/aginx-svc" "${TARGET}/aginx-boot-ok" "${TREE}/usr/bin/"
install -m 755 "${TARGET}/aginx-svcd" "${TREE}/usr/libexec/aginx/"
# N5① 吸收件：updater/download 改由本仓重编（修了三死路径的活版本），
# 落位与老资产同名同位（sidecar 已在 usr/bin）。
install -m 755 "${TARGET}/aginx-download" "${TARGET}/aginx-update" "${TREE}/usr/bin/"
# N5② 吸收件：qr/done/secret 全由本仓重编。aginx-qr 是第二次 zigbuild
# 的产物（feature 陷阱，见上）；aginx-secretd 落 libexec（引擎的家），
# aginx-secret 是 /usr/bin 的人面。
install -m 755 "${TARGET}/aginx-qr" "${TARGET}/aginx-done" "${TARGET}/aginx-secret" \
  "${TREE}/usr/bin/"
# N5⑨ 定数收据件：QR fixture 进镜像（首烤漏装——套件 I 段靠它出定数）。
mkdir -p "${TREE}/usr/share/aginx"
install -m 644 "${RECIPE}/usr/share/aginx/n5-qr.jpg" "${TREE}/usr/share/aginx/"
install -m 755 "${TARGET}/aginx-secretd" "${TREE}/usr/libexec/aginx/"
# N5⑥ 网关：远端通道守护落 libexec（引擎的家）；id/secret 都不进镜像——
# env_file 与 sidecar 在刷机日灌注（runbook 步 11）。
install -m 755 "${TARGET}/aginx-gateway" "${TREE}/usr/libexec/aginx/"
# Voice/OCR CLIs (bionic-static) + models. TTS: melo (vits-melo-tts-zh_en)
# is the product mouth; the 170MB fp32 model.onnx is the real weights — the
# tarball's model.int8.onnx is a 133B git-lfs pointer (release packaging
# accident, M42e receipt); bake the fp32 only. kokoro dropped: nothing
# references it by default. NOT in the aginx-update state tar: models ride
# every baked image instead (voice is the bootstrap interface — see above).
install -m 755 "${VOICE}/bin/ag-asr" "${TREE}/var/bin/aginx-asr"
install -m 755 "${VOICE}/bin/ag-tts" "${TREE}/var/bin/aginx-tts"
install -m 755 "${OCR}/bin/ag-ocr" "${TREE}/var/bin/aginx-ocr"
mkdir -p "${TREE}/var/models/ocr"
cp "${OCR}/models/det.onnx" "${OCR}/models/rec.onnx" \
   "${OCR}/models/dict.txt" "${TREE}/var/models/ocr/"
mkdir -p "${TREE}/var/models/tts"
cp -R "${VOICE}/models/asr" "${TREE}/var/models/asr"
cp -R "${VOICE}/models/tts/vits-melo-tts-zh_en" \
  "${TREE}/var/models/tts/vits-melo-tts-zh_en"
rm -f "${TREE}/var/models/tts/vits-melo-tts-zh_en/model.int8.onnx"
# CJK font subset (M38a) — aginx-term cjk.rs rasterizes through ab_glyph;
# GB2312 full + ASCII + punct rows, ~1.5MB (old repo subset-cjk-font.sh).
cp -R "${ORECIPE}/usr/share/fonts" "${TREE}/usr/share/fonts"
# Trampoline (M2/M22) — unmodified first-gen pair; aginxos-init performs
# the userdata rootfs swap the update flow relies on.
cp "${OTARGET}/aginxos-init" "${OTARGET}/aginxos-agent" "${TREE}/aginxos/"

# Exec bits: git may not carry them through cp for every recipe file, and a
# non-executable init script or shim is invisible at boot. Sidecars (.aginxmd)
# stay 644 — they are data read next to the binary.
chmod 755 "${TREE}"/etc/init.d/*
chmod 755 "${TREE}"/usr/bin/aginx-web "${TREE}"/usr/bin/aginx-file \
  "${TREE}"/usr/bin/aginx-mem "${TREE}"/usr/bin/aginx-sys-status \
  "${TREE}"/usr/bin/aginx-backup
chmod 755 "${TREE}"/usr/libexec/aginx/net-watch "${TREE}"/usr/libexec/aginx/net-rejoin
# NB: wifi.conf.example rides along in ${RECIPE}/etc — the real
# /etc/wifi.conf (with the passphrase) rides the aginx-update state tar
# from the running device (N4 从零开始 keeps only /home/photos; the join
# credential is re-provisioned through voice/QR or wizard), never committed.

# Curated applet symlinks — enough for init and debugging; rcS runs
# `busybox --install -s /bin` to fill in the full set on first boot.
APPLETS="[ awk blkid cat chmod chown clear cp cut date dd df dmesg echo env \
expr false fdisk find free getty grep gunzip gzip head hostname id insmod ip \
kill less ln ls lsmod mkdir mknod more mount mv netcat netstat nice passwd \
pidof ping printf ps renice rm rmdir route sed setsid sh sleep sort \
start-stop-daemon stat su switch_root sync tail tar telnet test top touch tr \
true umount uname uniq uptime vi wc wget which whoami xargs zcat"
for a in ${APPLETS}; do ln -sf busybox "${TREE}/bin/${a}"; done
ln -sf ../bin/busybox "${TREE}/sbin/init"
ln -sf ../bin/busybox "${TREE}/sbin/reboot"
ln -sf ../bin/busybox "${TREE}/sbin/poweroff"
ln -sf ../bin/busybox "${TREE}/sbin/ifconfig"

# TLS trust store: codex (and anything using system-native cert roots)
# fails with "waiting for network" without it. Cached under out/ so the
# download happens once per host, not once per build — falls back to the
# old repo's cache before hitting the network.
CACERT="${ROOT}/out/cacert.pem"
if [ ! -s "${CACERT}" ] && [ -s "${OLD}/out/cacert.pem" ]; then
  cp "${OLD}/out/cacert.pem" "${CACERT}"
fi
if [ ! -s "${CACERT}" ]; then
  curl -sL --max-time 120 -o "${CACERT}" https://curl.se/ca/cacert.pem
fi
mkdir -p "${TREE}/etc/ssl/certs"
cp "${CACERT}" "${TREE}/etc/ssl/certs/ca-certificates.crt"
ln -sf certs/ca-certificates.crt "${TREE}/etc/ssl/cert.pem"

# Registry gate (N4): lint the assembled command set with a host-built
# router before the image is packed. AGINX_CMD_PATH mirrors the device
# (/var/bin first) plus /bin+/sbin so future aginx:exec targets into the
# internals resolve. Bridge shims (aginx-web/file/mem) declare no
# aginx:exec — their targets exist only after provision syncs the
# packages (legal absence, warning-only). Fails the build on collisions,
# missing summaries, bad metadata, or missing exec targets.
cargo build -p aginx-router --release >/dev/null
AGINX_CMD_PATH="${TREE}/var/bin:${TREE}/usr/bin:${TREE}/bin:${TREE}/sbin" \
AGINX_GROUPS_DESC="${TREE}/etc/aginx/groups.desc" \
  "${ROOT}/target/release/aginx" commands --check \
  || { echo "aginx commands --check failed — fix the faces" >&2; exit 1; }

mkdir -p "${ROOT}/out"
# rm first: mke2fs never truncates an existing output file, so a SIZE
# change leaves stale bytes past the new fs end (a 2g image stayed 2 GiB
# after re-baking at 1g — the tail was the old image, 2026-09-02).
rm -f "${IMG}"
"${MKE2FS}" -t ext4 -b 4096 -F -d "${TREE}" "${IMG}" "${SIZE}"
echo "built ${IMG} ($(du -h "${IMG}" | cut -f1)) from ${TREE}"
