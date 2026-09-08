#!/usr/bin/env bash
# Pack boot/out/boot-test.img from unpacked stock kernel + AginxOS initramfs.
#
# Modes:
#   HYBRID=1 (default) — stock ramdisk base, wrap Android /init as /init.android
#   HYBRID=0           — minimal AginxOS-only ramdisk (no modules; black screen risk)
#
# Prerequisites:
#   ./scripts/unpack-boot.sh .local/device/redfin/stock/stock-boot.img
#   ./scripts/fetch-boot-tools.sh
set -euo pipefail

# redfin device pack (D14). Migrated from aginxos@0534ea8 boot/pack-boot.sh.
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
DEVLOCAL="${REPO}/.local/device/redfin"
UNPACK="${DEVLOCAL}/unpack"
INITSRC="${DEVLOCAL}/initramfs"
WORK="${DEVLOCAL}/boot-out/initramfs-root"
OUTDIR="${DEVLOCAL}/boot-out"
TOOLS="${REPO}/.local/boot-tools"
OUTIMG="${OUTDIR}/boot-test.img"
HYBRID="${HYBRID:-1}"

mkdir -p "${OUTDIR}"

if [[ ! -d "${UNPACK}" ]] || [[ -z "$(ls -A "${UNPACK}" 2>/dev/null || true)" ]]; then
  echo "unpack dir is empty. Run ./scripts/unpack-boot.sh <boot.img> first." >&2
  exit 1
fi

KERNEL=""
for cand in "${UNPACK}/kernel" "${UNPACK}/Image" "${UNPACK}/Image.gz"; do
  if [[ -f "${cand}" ]]; then
    KERNEL="${cand}"
    break
  fi
done
if [[ -z "${KERNEL}" ]]; then
  echo "No kernel found in ${UNPACK}" >&2
  exit 1
fi

if [[ ! -f "${INITSRC}/aginxos-init" ]]; then
  echo "missing ${INITSRC}/aginxos-init — see devices/redfin/boot/assets.md (regeneration)" >&2
  exit 1
fi

echo "==> building initramfs (HYBRID=${HYBRID})"
rm -rf "${WORK}"
mkdir -p "${WORK}"

if [[ "${HYBRID}" == "1" ]]; then
  STOCK_RD="${UNPACK}/ramdisk"
  if [[ ! -f "${STOCK_RD}" ]]; then
    echo "missing stock ramdisk at ${STOCK_RD}" >&2
    exit 1
  fi
  # Extract stock ramdisk (lz4 or gzip or raw cpio)
  if lz4 -dc "${STOCK_RD}" 2>/dev/null | (cd "${WORK}" && cpio -idm 2>/dev/null); then
    echo "note: extracted stock ramdisk (lz4)"
  elif gzip -dc "${STOCK_RD}" 2>/dev/null | (cd "${WORK}" && cpio -idm 2>/dev/null); then
    echo "note: extracted stock ramdisk (gzip)"
  elif (cd "${WORK}" && cpio -idm <"${STOCK_RD}" 2>/dev/null); then
    echo "note: extracted stock ramdisk (raw cpio)"
  else
    echo "failed to extract stock ramdisk" >&2
    file "${STOCK_RD}" >&2
    exit 1
  fi

  if [[ ! -f "${WORK}/init" ]]; then
    echo "stock ramdisk has no /init" >&2
    exit 1
  fi
  mv -f "${WORK}/init" "${WORK}/init.android"
  chmod 755 "${WORK}/init.android"
  cp -f "${INITSRC}/aginxos-init" "${WORK}/init"
  chmod 755 "${WORK}/init"

  mkdir -p "${WORK}/aginxos" "${WORK}/bin"
  if [[ -f "${INITSRC}/aginxos-probe" ]]; then
    cp -f "${INITSRC}/aginxos-probe" "${WORK}/aginxos/aginxos-probe"
    cp -f "${INITSRC}/aginxos-probe" "${WORK}/bin/aginxos-probe"
    chmod 755 "${WORK}/aginxos/aginxos-probe" "${WORK}/bin/aginxos-probe"
  fi
  # HOLD=1 → do not hand off to Android (proves our init is pid1; long-press power to leave)
  if [[ "${HOLD:-0}" == "1" ]]; then
    : >"${WORK}/aginxos/hold"
    echo "note: HOLD=1 — will NOT hand off to Android"
  fi
  echo "note: hybrid — /init=aginxos-init, /init.android=stock"
else
  mkdir -p "${WORK}"/{bin,dev,proc,sys,tmp}
  cp -f "${INITSRC}/aginxos-init" "${WORK}/init"
  chmod 755 "${WORK}/init"
  if [[ -f "${INITSRC}/aginxos-probe" ]]; then
    cp -f "${INITSRC}/aginxos-probe" "${WORK}/bin/aginxos-probe"
    chmod 755 "${WORK}/bin/aginxos-probe"
  fi
  echo "note: minimal AginxOS-only ramdisk"
fi

# Compress ramdisk: prefer lz4 legacy (matches stock redfin), else gzip
RAMDISK_BLOB="${OUTDIR}/initramfs.blob"
(
  cd "${WORK}"
  if command -v lz4 >/dev/null 2>&1; then
    # Android uses lz4 legacy frame (-l)
    find . -print0 | cpio --null --create --format=newc 2>/dev/null | lz4 -l -12 >"${RAMDISK_BLOB}" \
      || find . | cpio -o -H newc | lz4 -l -12 >"${RAMDISK_BLOB}"
    echo "note: ramdisk compressed with lz4 -l"
  else
    find . -print0 | cpio --null --create --format=newc 2>/dev/null | gzip -9 >"${RAMDISK_BLOB}" \
      || {
        if command -v bsdtar >/dev/null 2>&1; then
          bsdtar --format=newc -cf - . | gzip -9 >"${RAMDISK_BLOB}"
        else
          find . | cpio -o -H newc | gzip -9 >"${RAMDISK_BLOB}"
        fi
      }
    echo "note: ramdisk compressed with gzip (install lz4 for stock-matching format)"
  fi
)
echo "==> ramdisk: ${RAMDISK_BLOB} ($(wc -c <"${RAMDISK_BLOB}") bytes)"

CMDLINE="${CMDLINE:-}"
if [[ -z "${CMDLINE}" && -f "${UNPACK}/info.txt" ]]; then
  CMDLINE="$(python3 - <<'PY' "${UNPACK}/info.txt"
import shlex, sys
args = shlex.split(open(sys.argv[1]).read())
for i, a in enumerate(args):
    if a == "--cmdline" and i + 1 < len(args):
        print(args[i + 1])
        break
PY
)"
fi
if [[ -z "${CMDLINE}" && -n "${FORCE_CMDLINE:-}" ]]; then
  CMDLINE="${FORCE_CMDLINE}"
elif [[ -z "${CMDLINE}" ]]; then
  echo "note: empty cmdline (normal for redfin header v3)"
fi

HEADER_VERSION="${HEADER_VERSION:-3}"
if [[ -f "${UNPACK}/info.txt" ]]; then
  hv="$(python3 - <<'PY' "${UNPACK}/info.txt"
import shlex, sys
args = shlex.split(open(sys.argv[1]).read())
for i, a in enumerate(args):
    if a == "--header_version" and i + 1 < len(args):
        print(args[i + 1])
        break
PY
)"
  [[ -n "${hv}" ]] && HEADER_VERSION="${hv}"
fi

OS_VERSION="${OS_VERSION:-}"
OS_PATCH_LEVEL="${OS_PATCH_LEVEL:-}"
if [[ -f "${UNPACK}/info.txt" ]]; then
  eval "$(python3 - <<'PY' "${UNPACK}/info.txt"
import shlex, sys
args = shlex.split(open(sys.argv[1]).read())
kv = {}
i = 0
while i < len(args):
    a = args[i]
    if a.startswith("--") and i + 1 < len(args) and not args[i+1].startswith("--"):
        kv[a[2:]] = args[i+1]
        i += 2
    else:
        i += 1
for key in ("os_version", "os_patch_level"):
    if key in kv:
        print(f"{key.upper()}={kv[key]!r}")
PY
)"
fi
if [[ "${OS_VERSION}" == "0.0.0" || "${OS_VERSION}" == "0" ]]; then OS_VERSION=""; fi
if [[ ! "${OS_PATCH_LEVEL}" =~ ^[0-9]{4}-[0-9]{2}$ ]] || [[ "${OS_PATCH_LEVEL}" == *"-00" ]]; then
  OS_PATCH_LEVEL=""
fi

if [[ ! -f "${TOOLS}/mkbootimg.py" ]]; then
  echo "No pack tool. Run ./scripts/fetch-boot-tools.sh" >&2
  exit 1
fi

echo "==> mkbootimg.py → ${OUTIMG}"
args=(
  python3 "${TOOLS}/mkbootimg.py"
  --header_version "${HEADER_VERSION}"
  --kernel "${KERNEL}"
  --ramdisk "${RAMDISK_BLOB}"
  --cmdline "${CMDLINE}"
  -o "${OUTIMG}"
)
if [[ "${HEADER_VERSION}" -lt 3 ]]; then
  args+=(--base 0x00000000 --pagesize 4096)
fi
[[ -n "${OS_VERSION}" ]] && args+=(--os_version "${OS_VERSION}")
[[ -n "${OS_PATCH_LEVEL}" ]] && args+=(--os_patch_level "${OS_PATCH_LEVEL}")
[[ -f "${UNPACK}/dtb" ]] && args+=(--dtb "${UNPACK}/dtb")
"${args[@]}"

ls -la "${OUTIMG}"
echo
echo "Test boot (does not flash):"
echo "  adb reboot bootloader"
echo "  fastboot boot ${OUTIMG}"
echo "Expect: green splash ~4s, then stock Android if HYBRID=1."
