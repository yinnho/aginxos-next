#!/usr/bin/env bash
# Fetch portable AOSP/Lineage mkbootimg + unpack_bootimg (no GKI Python deps).
# Pinned to lineage-19.1: supports boot header v3/v4, runs as standalone Python.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLS="${ROOT}/.local/boot-tools"
mkdir -p "${TOOLS}"

# Lineage 19.1 scripts are self-contained (later branches import gki.* modules).
BASE="${MKBOOTIMG_RAW_BASE:-https://raw.githubusercontent.com/LineageOS/android_system_tools_mkbootimg/lineage-19.1}"

fetch() {
  local name="$1"
  local url="${BASE}/${name}"
  echo "==> ${name}"
  curl -fsSL "${url}" -o "${TOOLS}/${name}"
  chmod +x "${TOOLS}/${name}"
}

fetch mkbootimg.py
fetch unpack_bootimg.py

ln -sfn mkbootimg.py "${TOOLS}/mkbootimg"
ln -sfn unpack_bootimg.py "${TOOLS}/unpack_bootimg"

echo "OK: tools in ${TOOLS}"
python3 "${TOOLS}/unpack_bootimg.py" --help >/dev/null
python3 "${TOOLS}/mkbootimg.py" --help >/dev/null
echo "Python tools runnable (header v3/v4 capable)."
