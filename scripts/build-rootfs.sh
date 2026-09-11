#!/usr/bin/env bash
# Build the N4 AginxOS rootfs image — the new repo owns the bake chain.
#
# Assembles the tree from: the recipe in ./rootfs (etc + aginx-* faces +
# libexec daemons + generic C sources), new-repo musl binaries (zigbuild),
# and DEVICE ASSETS under .local/device/${DEVICE}/ (vendor ramdisk, voice/OCR
# stacks, dropbear, radio blobs, the frozen aginxos trampoline pair — see
# devices/${DEVICE}/boot/assets.md for the layout and regeneration paths);
# N5②: every renamed-at-install CLI is rebuilt here instead; the old `ag`
# router, ag-* shims, carrier daemon and relay do NOT enter the image
# (切净).
#
# Flash with:  fastboot flash userdata out/rootfs.img
# Boot needs a vendor_boot packed with ROOTFS=1
# (devices/${DEVICE}/boot/pack-vendor-boot.sh, on the vendor-boot style).
#
# Note: mke2fs -d records the building user's uid (501 on macOS) as owner.
# rcS chowns everything back to 0:0 on first boot — do not "fix" that here.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# D14 机型是数据：DEVICE 选 devices/<codename>/（device.toml + modules.txt +
# bringup/ + cam/）。默认 redfin 只在「不传就烤首目标」的意义上成立——目录
# 缺失即 die，绝不猜、绝不兜底第二台机的数据。
DEVICE="${DEVICE:-redfin}"
DEVDIR="${ROOT}/devices/${DEVICE}"
test -f "${DEVDIR}/device.toml" \
  || { echo "unknown device '${DEVICE}' — no ${DEVDIR}/device.toml (see devices/README.md)" >&2; exit 1; }
ASSETS="${ROOT}/.local/device/${DEVICE}"
RAMDISK="${ASSETS}/vendor-ramdisk-root"
TRAMP="${ASSETS}/trampoline"
RECIPE="${ROOT}/rootfs"
TARGET="${ROOT}/target/aarch64-unknown-linux-musl/release"
TREE="${TREE:-/tmp/aginxos-n4-rootfs}"
IMG="${IMG:-${ROOT}/out/rootfs.img}"
# 2 GB sparse-ish image (bake #18 data: 651M used; N4 drops carrier+relay).
SIZE="${SIZE:-2g}"
# L0 无头底座（刀4，2026-09-11，蛋案转正）：镜像=内核+init+svc+网络+
# ssh+pkg，刷完就是一台活的机器。母体三件（aginx 树包）、面板
# （aginx-term 包+字体）、网关/密钥/语音/三模型树全是包——8 行 opt 附加
# 在 etc 装配段组装+签名进树（全 opt：provision 默认什么都不装）。
# 裸机=哑终端：显示/触摸/扫码/联网/ssh 在，装什么是用户的 opt-in。

test -x "${RAMDISK}/system/bin/adbd" || { echo "missing ${RAMDISK} — see devices/${DEVICE}/boot/assets.md (run pack-vendor-boot.sh)" >&2; exit 1; }
test -x "${RECIPE}/busybox" || { echo "missing ${RECIPE}/busybox recipe asset" >&2; exit 1; }
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
  test -x "${TRAMP}/${b}" \
    || { echo "missing ${b} — see devices/${DEVICE}/boot/assets.md (frozen trampoline pair)" >&2; exit 1; }
done

# L0（刀4）：voice/ocr 的 bionic 件与模型树不再烤镜像——aginx-asr/
# tts/ocr 三树包随清单走（真源 .local/device/redfin/{voice,ocr}，由
# build-pkg.sh 打包，再生路径 devices/${DEVICE}/boot/assets.md）。

echo "==> zigbuild 新仓 musl 件（缓存则秒过）"
# L0 六件：pkg/svc/download/update/done/secret（刀4：router/server/
# runtime/voice/term/gateway 出镜像走包——build-pkg.sh 烤，不在此列。
# aginx-secret crate 双 bin，secretd 产物本线不装、由 aginx-secretd 包走）。
(cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
  -p aginx-pkg -p aginx-svc \
  -p aginx-download -p aginx-update -p aginx-done -p aginx-secret)

# aginx-qr 是第二次独立调用（--features aginx-qr/jpeg）：特性选择是调用
# 级旗标——并进共享调用会把 quircs+aginx-img 织进任何依赖 aginx-qr 的
# crate（N5② 特性陷阱的出生地，当年受害者 aginx-voice）。L0 名单里没有
# 它的依赖者，但独立调用法保持：解码器必须是自己一个进程。
(cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
  -p aginx-qr --features aginx-qr/jpeg)

# 蛋案 C3/C10：设备面 aginx-pair 走第三次独立调用（--no-default-features
# 是调用级旗标——并进上面任一次调用都会把 mint 的 qrcodegen/jpeg-encoder
# 连带 aginx-qr/jpeg 的 quircs+aginx-img 织进其它包）。设备只要 apply 面
# （stdin payload，C4 voice 依赖）；铸码在 host 跑 default 特性。两档都装。
# <2MB 绊网同律：尺寸变化=feature 折叠事故。
(cd "${ROOT}" && cargo zigbuild --release --target aarch64-unknown-linux-musl \
  -p aginx-pair --no-default-features)
PAIR_SZ="$(stat -f%z "${TARGET}/aginx-pair")"
[ "${PAIR_SZ}" -lt 2097152 ] \
  || { echo "FATAL: aginx-pair is ${PAIR_SZ}B (≥2MiB) — mint feature leaked into the device build" >&2; exit 1; }

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

# Kernel modules for the touch/display + battery chains (M3/M3c) — machine
# data (D14): devices/<codename>/modules.txt holds the ordered list. The
# vendor_boot base loads only the 64-module USB/storage set; the full
# modules.load panics this kernel (observed 2026-08-27, retry counter
# burned), so the chains are loaded from the rootfs world by the device's
# bringup scripts, in the order proven live. Same .ko files as the ramdisk
# holds — copied from the local unpack (never committed, §7).
MODULES="$(grep -Ev '^[[:space:]]*(#|$)' "${DEVDIR}/modules.txt")"
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
  -o "${TREE}/bin/splash" "${RECIPE}/src/splash2.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/binder-init" "${RECIPE}/src/binder-init.c"
# QRTR observability (M3d): qrtr-lookup snapshots/watches the name service,
# qmi-req sends one raw QMI request. radio-bringup starts a qrtr-lookup
# watcher before the modem boot trigger to record the fresh-boot service
# registration order (WLFW 0x45 transient vs never-present).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/qrtr-lookup" "${RECIPE}/src/qrtr-lookup.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/qmi-req" "${RECIPE}/src/qmi-req.c"
# cam-shot (M19) — the IFE/RDI stills capture tool. Vendor sensor register
# tables are decoded into the source; vendor module bins stay local and
# gitignored. N4: the four brain-facing C tools take their D13 /usr/bin
# names AT BUILD TIME (aginx-voice spawns /usr/bin/aginx-cam-shot; net-bringup
# and net-rejoin call /usr/bin/aginx-net-join; net scans go through
# /usr/bin/aginx-net-scan; reboot is /usr/bin/aginx-reboot). Default flags
# for a rear shot: --stream --rear --slowrear --rawvendor [--gain N] [--png].
# M47①: the camera trio (cam-shot.c + campix.h + campix_test.c) lives in
# devices/<codename>/cam/ (D14 — sensor timing/calibration is machine
# data); the old-repo copies are frozen history.
# M47⑤d: encoder = vendored libjpeg-turbo (NEON); the build command (and the
# source lists it mirrors) lives in build-cam.sh — this script just runs it.
"${ROOT}/scripts/build-cam.sh" "${DEVDIR}/cam"
install -m 755 "${ROOT}/out/cam/aginx-cam-shot" "${TREE}/usr/bin/aginx-cam-shot"
# raw2jpg (M19c) — RAW10 dump -> JPEG converter, companion to cam-shot's
# native --jpeg (for converting already-captured dumps).
install -m 755 "${ROOT}/out/cam/raw2jpg" "${TREE}/bin/raw2jpg"

# Bionic LD_PRELOAD helpers (M3d). These load into vendor binaries, so they
# must be NDK/bionic shared objects, not musl. trace_open.so mirrors file
# access AND logcat output (__android_log_print & co) onto stderr — it is
# our only window into cnss-daemon/pd-mapper, which log exclusively through
# liblog and we run no logd. fake-props.so fakes the servicemanager
# properties pm-service blocks on and logs every other property read.
NDK_CC="${HOME}/Library/Android/sdk/ndk/27.0.12077973/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android24-clang"
test -x "${NDK_CC}" || { echo "NDK clang not found (needed for preload .so)" >&2; exit 1; }
mkdir -p "${TREE}/lib"
"${NDK_CC}" -shared -fPIC -O2 -o "${TREE}/lib/trace_open.so" "${RECIPE}/src/trace_open.c"
"${NDK_CC}" -shared -fPIC -O2 -o "${TREE}/lib/fake-props.so" "${RECIPE}/src/fake-props.c"
echo "built preload helpers (trace_open.so, fake-props.so)"
# fake-sm: minimal binder context manager (musl-static) answering every
# transaction with Status-ok. Without a CM on /dev/binder, vendor libbinder
# clients (cnss-daemon, pm-service) spin forever in "Waiting 1s on context
# object" before ever reaching their QMI work.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/fake-sm" "${RECIPE}/src/fake-sm.c"
# aginx-reboot (原 reboot2): raw reboot(LINUX_REBOOT_CMD_RESTART2) — toybox
# reboot signals init (we run none) and adb reboot needs adbd's sys.powerctl
# handling. With no args it plain-reboots; "bootloader" lands in fastboot
# for re-flashing.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-reboot" "${RECIPE}/src/reboot2.c"
# wdt (M20c): watchdog probe/arm/starve for /dev/watchdog. The dog itself
# is armed and petted by aginx-svcd (crates/svc); this is the diagnostics
# tool that proved the platform story (softdog behind msm_watchdog,
# hardware bark resources absent) and the live-fire starve reset.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/wdt" "${RECIPE}/src/wdt.c"
# rtcal (M23b): pm8xxx RTC alarm arm/read + `sync` — the suspend probes' wake
# path ("set <epoch>" semantics kept from the /tmp zig one-off). net-bringup
# runs `rtcal sync` after ntpd to fix the RTC's -53y offset, which also makes
# early-boot wall time true on the next HCTOSYS pass.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/rtcal" "${RECIPE}/src/rtcal.c"
# Ops channel sshd (#142, 2026-09-04) — the maintenance face: ssh in over
# Wi-Fi or `adb forward tcp:2222 tcp:22` when the subnets differ. Static
# musl dropbear triplet (first-gen build product, zig cc; the AR must be
# LLVM's — macOS BSD ar archives break lld member resolution; regeneration
# in devices/redfin/boot/assets.md). rcS starts the daemon key-only with
# the host key under /root/.ssh.
DROPBEAR="${ASSETS}/dropbear/bin"
for b in dropbear dbclient dropbearkey; do
  test -x "${DROPBEAR}/${b}" || { echo "missing ${b} — see devices/redfin/boot/assets.md" >&2; exit 1; }
  cp "${DROPBEAR}/${b}" "${TREE}/bin/${b}"
  chmod 755 "${TREE}/bin/${b}"
done
# sftp subsystem (#319, L0 缺口刀B) — dropbear 2026.94 编入 SFTP 支持，
# 每连接 exec DROPBEAR_SFTP_SERVER=/usr/libexec/sftp-server（无参、
# stdin/stdout 说 SFTP v3）。Go+pkg/sftp 静态件（build 脚本带 out/ 缓存
# 早退，cacert/resize2fs 同款）；烤进去 = host scp/sftp/GUI 客户端直连。
# usr/libexec 在下面 320 行 mkdir 才出现，这里自带。
"${ROOT}/scripts/build-sftp-server.sh" >/dev/null
mkdir -p "${TREE}/usr/libexec"
install -m 755 "${ROOT}/out/sftp-server" "${TREE}/usr/libexec/sftp-server"
# aginx-net-scan (原 nlscan): nl80211 trigger-scan + dump client — busybox
# has no wireless tools and we ship no libnl. Our WLAN operability check
# (M3f).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-net-scan" "${RECIPE}/src/nlscan.c"
# aginx-net-join (原 wifi-join, M4): self-contained WPA2-PSK supplicant —
# CONNECT, EAPOL 4-way handshake over an AF_PACKET socket, NEW_KEY installs;
# then udhcpc owns IP provisioning. wifi-trace flips QCA vendor dp-trace
# levels for TX/RX logs (internal, /bin).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/usr/bin/aginx-net-join" "${RECIPE}/src/wifi-join.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/wifi-trace" "${RECIPE}/src/wifi-trace.c"
# M18 audio I/O: bare-ioctl PCM pair (no alsa-lib) — capture is the
# agent's "listen" path, playback its "speak" path. Shared uapi header.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/snd-cap" "${RECIPE}/src/snd-cap.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/snd-play" "${RECIPE}/src/snd-play.c"
# snd-mixer: ctl get/set (no alsa-lib) — audio-bringup's whole routing
# recipe runs through it. i2c-reg: rt5514 register peek/poke over
# /dev/i2c-N (kernel has no debugfs here — see audio-bringup notes).
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/snd-mixer" "${RECIPE}/src/snd-mixer.c"
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/i2c-reg" "${RECIPE}/src/i2c-reg.c"
# Boot card (v4⑤): DRM boot console — paints the AginxOS wordmark only
# (checklist retired 09-08) and exits on the net-verdict ladder. Holds DRM
# master for its whole life (it replaces the M3 green splash). Same zig
# static build; host-side check via `bootcard --ppm out.ppm`.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/bootcard" "${RECIPE}/src/bootcard.c"
# Patched vendor ko override (boot-wedge defense, #228): camera-bringup
# prefers /lib/modules.aginx over vendor. Blob stays out of git (.local) —
# regenerate with scripts/patch-vsync-ko.sh.
if [ -d "${ROOT}/.local/modules.aginx" ]; then
  mkdir -p "${TREE}/lib/modules.aginx"
  cp "${ROOT}"/.local/modules.aginx/*.ko "${TREE}/lib/modules.aginx/"
fi
# httpget: minimal HTTP fetch for the boot internet check — busybox's wget
# applet segfaults in this build (2026-08-28), so net-bringup uses ours.
"${ZIG}" cc -target aarch64-linux-musl -static -O2 \
  -o "${TREE}/bin/httpget" "${RECIPE}/src/httpget.c"
# udhcpc event hook (compiled-in default path) — without it udhcpc wins a
# lease but nothing applies it to the interface.
mkdir -p "${TREE}/usr/share/udhcpc"
cp "${RECIPE}/usr/share/udhcpc/default.script" "${TREE}/usr/share/udhcpc/"
chmod 755 "${TREE}/usr/share/udhcpc/default.script"

# Radio bring-up payload (M3d). libnl.so is the bionic build cnss-daemon
# dlopens (LD_LIBRARY_PATH=/lib/...); rmt_storage is the PATCHED stock
# binary (erase call sites NOPed); cdsp-cdsp-loader.ko is stock
# cdsp-loader.ko with module/driver/sysfs names renamed (compat + code
# untouched — it binds soc:qcom,msm-cdsp-loader and boots the CDSP) and
# modem-npucc-loader.ko is the modem variant re-anchored to the npucc
# node. See radio/README.md in the asset dir. All are vendor-derived
# blobs: they live only in gitignored .local/device/redfin/radio/ and are
# copied in when present. The sealed first-gen repo's
# scripts/build-radio-blobs.sh regenerates them.
RADIO="${ASSETS}/radio"
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
  echo "NOTE: ${RADIO} incomplete — radio-bringup will fail; see devices/${DEVICE}/boot/assets.md" >&2
fi

# Recipe: etc (init.d/aginx/svc.d units/aginx conf/crontabs + manifest+sig),
# usr/bin (bridge sh faces + .aginxmd sidecars), libexec/aginx
# (net-watch/net-rejoin). All D13 knowledge lives here.
mkdir -p "${TREE}/bin" "${TREE}/sbin" "${TREE}/aginxos" "${TREE}/usr/libexec/aginx" "${TREE}/var/bin"
cp "${RECIPE}/busybox" "${TREE}/bin/busybox"
cp -R "${RECIPE}/etc/." "${TREE}/etc/"
# shadow is baked root-locked (* = inert until `passwd`); git carries no
# file mode beyond the exec bit, so pin 0600 here — never world-readable
chmod 600 "${TREE}/etc/shadow"
# 机型数据注入（D14）：bringup 脚本与 device.toml 都来自 devices/<codename>/。
# bringup 内容 verbatim 搬运（211 行 mixer recipe 那种收据流不重排）；
# device.toml 落 /etc/aginx/（hwd::load_or_exit 的读点——烤错档案=开机
# fail-fast，正脸拒绝）。
mkdir -p "${TREE}/etc/aginx"
install -m 644 "${DEVDIR}/device.toml" "${TREE}/etc/aginx/device.toml"
for b in "${DEVDIR}"/bringup/*; do
  test -f "${b}" || { echo "missing bringup scripts in ${DEVDIR}/bringup/" >&2; exit 1; }
  install -m 755 "${b}" "${TREE}/etc/init.d/$(basename "${b}")"
done
# 首启 root-fs 扩容件（#318，L0 缺口刀1）：rcS 同步跑 disk-grow（上面的
# bringup 件），它要 resize2fs。落 /usr/bin——镜像 usr 下只有 bin/libexec/
# share 一个工具目录的惯例位（usr/sbin 不存在，别开新目录）。
# build-resize2fs.sh 自己有缓存早退（out/ 已建即秒回，cacert 同款模式），
# 首次才下载+约 2 分钟构建。
"${ROOT}/scripts/build-resize2fs.sh" >/dev/null
install -m 755 "${ROOT}/out/resize2fs" "${TREE}/usr/bin/resize2fs"
# ---- L0 清单组装（刀4；只改 TREE 副本，配方不动）--------------------------
# 镜像 svc.d 必须恰好 2 单元（net-watch + aginxbrowser——后者是缺席容忍
# 单元：裸上游二进制无配方可装，30s 自拾取先例，不能删）。母体/UI/网关/
# 密钥/语音的单元全在包里带 [service]——svc.d 多出来=有人把引擎单元烤回
# 镜像，die。
L0_SVC_COUNT="$(ls "${TREE}/etc/aginx/svc.d/" | wc -l | tr -d ' ')"
[ "${L0_SVC_COUNT}" = "2" ] \
  || { echo "FATAL: L0 svc.d has ${L0_SVC_COUNT} units (want 2: net-watch + aginxbrowser) — engine units ride packages, not the image" >&2; exit 1; }
# 基础 manifest（全 opt 目录）+ 8 行 opt 附加（aginx 家族包）。sha 取
# out/pkgs 产物（不手维护）；url/version/deps 取 pkgs/<name>/pkg.toml——
# 配方 bump 了 version 没重跑 build-pkg → sha 文件名对不上 → die（宁死
# 不烤错清单）。组装进树后签名（.sig 是构建产物，不回写配方；签的是树里
# 的组装件，覆写 cp 进来的基础件签名）。
OPT_ADD="${TMPDIR:-/tmp}/agpkg-opt-add.$$"
: > "${OPT_ADD}"
for p in aginx aginx-term aginx-gateway aginx-secretd \
         aginx-asr aginx-tts aginx-ocr aginx-voice; do
  R="pkgs/${p}"
  p_ver="$(sed -n 's/^version *= *"\([^"]*\)"/\1/p' "${R}/pkg.toml" | sed -n '1p')"
  p_url="$(sed -n 's/^url *= *"\([^"]*\)"/\1/p' "${R}/pkg.toml" | sed -n '1p')"
  p_dep="$(sed -n 's/^depends *= *"\([^"]*\)"/\1/p' "${R}/pkg.toml" | sed -n '1p')"
  sha_file="${ROOT}/out/pkgs/${p}-v${p_ver}-4pc.tar.sha256"
  [ -n "${p_ver}" ] && [ -n "${p_url}" ] \
    || { echo "FATAL: ${R}/pkg.toml 缺 version/url" >&2; exit 1; }
  [ -s "${sha_file}" ] \
    || { echo "FATAL: L0 manifest needs ${sha_file} — run ./scripts/build-pkg.sh ${p} first" >&2; exit 1; }
  p_sha="$(sed -n '1p' "${sha_file}")"
  case "${p_sha}" in
    ''|*[!0-9a-f]*) echo "FATAL: bad sha in ${sha_file}: '${p_sha}'" >&2; exit 1 ;;
  esac
  [ "${#p_sha}" -eq 64 ] \
    || { echo "FATAL: sha not 64 hex chars in ${sha_file}" >&2; exit 1; }
  printf '%s %s %s opt %s%s\n' "${p}" "${p_url}" "${p_sha}" "${p_ver}" "${p_dep:+ ${p_dep}}" >> "${OPT_ADD}"
done
cat "${RECIPE}/etc/agpkg.manifest" "${OPT_ADD}" > "${TREE}/etc/agpkg.manifest"
cp "${OPT_ADD}" "${ROOT}/out/pkgs/agpkg.opt.add"
rm -f "${OPT_ADD}"
[ -f "${ROOT}/.local/keys/aginx.key" ] \
  || { echo "FATAL: L0 manifest signing needs .local/keys/aginx.key" >&2; exit 1; }
(cd "${ROOT}" && cargo run -q -p aginx-sign -- sign .local/keys/aginx.key "${TREE}/etc/agpkg.manifest")
(cd "${ROOT}" && cargo run -q -p aginx-sign -- verify .local/keys/aginx.pub "${TREE}/etc/agpkg.manifest") \
  || { echo "FATAL: L0 manifest sig does not verify" >&2; exit 1; }
echo "==> L0 manifest: 基础清单（全 opt）+ 8 行 opt 附加已签名进树"
cp -R "${RECIPE}/usr/bin/." "${TREE}/usr/bin/"
cp -R "${RECIPE}/libexec/aginx/." "${TREE}/usr/libexec/aginx/"
# 批② C1（09-10）：包管件的 sidecar 一律由安装器从 pkg.toml 生成（安装
# 即覆写）。L0 下 asr/tts/ocr/voice/term 不再烤镜像，usr/bin 里剩下的
# .aginxmd 全属本线直装件（download/update/qr/done/secret）——无安装器
# 接管，sidecar 由配方自带，`aginx commands` 摘要走这里。
# version stamp (M14): what the running image is, for aginx-update
# status/compare. N4: stamped from THIS repo's git; D14: the device rides
# the stamp — 版本串自证出自哪台机的烤机线。行尾 ` l0`（刀4 起的无头
# 底座形戳，n6 预检读它）。
STAMP="$(git -C "${ROOT}" log -1 --format="aginxos ${DEVICE} %h %cd" --date=short 2>/dev/null || echo "aginxos ${DEVICE} unknown") l0"
echo "${STAMP}" > "${TREE}/etc/aginx-version"

# L0（刀4）：router/server/runtime 三件不烤——母体=`aginx` 树包
# （bin/{aginx,aginx-server,aginx-runtime}，exec=bin/aginx → face
# /var/bin/aginx，[service] 单元随包走——pkgs/aginx/pkg.toml）。
# Platform CLIs (new-repo builds; N4③b 改姓四件)。
# aginx-pair 留 L0（C4 起配网 apply 面；voice 包装上后 spawn /usr/bin/
# aginx-pair apply）。term 不烤（aginx-term 包；rcS 的 aginx-term-handoff
# 缺席静默轮询 /var/bin/aginx-term，装包即亮屏）。批③ (09-10): wizard
# 出烤——装机流程是扫码/语音，wizard 无入口。
install -m 755 "${TARGET}/aginx-pkg" "${TARGET}/aginx-pair" "${TREE}/usr/bin/"
install -m 755 "${TARGET}/aginx-svc" "${TARGET}/aginx-boot-ok" "${TREE}/usr/bin/"
install -m 755 "${TARGET}/aginx-svcd" "${TREE}/usr/libexec/aginx/"
# N5① 吸收件：updater/download 改由本仓重编（修了三死路径的活版本），
# 落位与老资产同名同位（sidecar 已在 usr/bin）。
install -m 755 "${TARGET}/aginx-download" "${TARGET}/aginx-update" "${TREE}/usr/bin/"
# N5② 吸收件：qr/done/secret 全由本仓重编。aginx-qr 是第二次 zigbuild
# 的产物（feature 陷阱，见上）；L0 只装 /usr/bin 的人面 aginx-secret
# ——secretd 引擎走 aginx-secretd 包（[service] 单元随包）。
install -m 755 "${TARGET}/aginx-qr" "${TARGET}/aginx-done" "${TARGET}/aginx-secret" \
  "${TREE}/usr/bin/"
# N5⑨ QR fixture（n5-qr.jpg）出镜像（刀4：n5 套件随 L0 退役，设备面
# 不再读；配方文件保留——host 侧 qr 测试仍以它为真源）。
# N5⑥ 网关不烤（刀4）：aginx-gateway 包（[service] 随包，depends=
# aginx-secretd）；id/secret 与 env 灌注是装机后置事，见 pkgs/*/SKILL.md。
# 模型树三口（刀4 L0）：asr/tts/ocr 全走包（bionic 件+模型一树，M42e
# 收据：tts 真权重是 fp32 model.onnx，tarball 里的 int8 是 133B lfs 指针
# ——build-pkg.sh 已折），镜像只种三条 dangling symlink 指向 pkgfiles
# 未来真身；装包前 test -e 恒假（哑终端无害），provision 的
# ensure_model_link 每靴幂等对账（D15 边界律：大件可重下物既不烤也不进
# state tar）。
mkdir -p "${TREE}/var/models/tts"
ln -s /var/lib/aginx/pkgfiles/aginx-asr/models/asr "${TREE}/var/models/asr"
ln -s /var/lib/aginx/pkgfiles/aginx-tts/models/tts/vits-melo-tts-zh_en \
  "${TREE}/var/models/tts/vits-melo-tts-zh_en"
ln -s /var/lib/aginx/pkgfiles/aginx-ocr/models/ocr "${TREE}/var/models/ocr"
# CJK 字体（M38a 冻结资产）不烤镜像（刀4）：随 aginx-term 包走——
# build-pkg.sh 从配方原文件打包，term cjk.rs font_path() 兜底探测
# pkgfiles 路径（老全量镜像的烤入路径优先级在其前，兼容在役机）。
# Trampoline (M2/M22) — frozen first-gen pair (assets.md); aginxos-init
# performs the userdata rootfs swap the update flow relies on.
cp "${TRAMP}/aginxos-init" "${TRAMP}/aginxos-agent" "${TREE}/aginxos/"

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
# credential is re-provisioned through voice/QR), never committed.

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
# staged bundle in .local/assets before hitting the network.
CACERT="${ROOT}/out/cacert.pem"
if [ ! -s "${CACERT}" ] && [ -s "${ROOT}/.local/assets/cacert.pem" ]; then
  cp "${ROOT}/.local/assets/cacert.pem" "${CACERT}"
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
