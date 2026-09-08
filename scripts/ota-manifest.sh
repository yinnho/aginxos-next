#!/usr/bin/env bash
# ota-manifest — build + sign an update manifest with the device field (P3①).
#
# The device codename is NOT typed by hand: it is read from the target
# machine's devices/<codename>/device.toml and cross-checked against the
# directory name (the same law the bake and the on-device refuse gate
# enforce), then stamped into the manifest as the mandatory "device"
# field (crates/update: a manifest without it fails to parse,
# fail-closed; a manifest naming another machine is refused at apply).
#
# Usage:
#   scripts/ota-manifest.sh <codename> <version> <outdir> <name>=<path>...
#
#     <name>     partition: boot (required), vendor_boot, dtbo, vbmeta,
#                vbmeta_system, rootfs
#     <path>     local image file; recorded as an absolute url
#   PRE_STAGED=1 marks rootfs pre_staged (host already streamed the body
#                to the userdata swap area — the M22/bake flow)
#
# Output: <outdir>/manifest.json (+ .sig, ed25519 via aginx-sign; the
# private key never leaves .local/keys/). Sign, then verify with the
# compiled-in chain before trusting:
#   cargo run -p aginx-sign -- verify .local/keys/aginx.pub manifest.json
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEV="${1:-}"
VER="${2:-}"
OUTDIR="${3:-}"
if [ -z "${DEV}" ] || [ -z "${VER}" ] || [ -z "${OUTDIR}" ] || [ $# -lt 4 ]; then
  echo "usage: scripts/ota-manifest.sh <codename> <version> <outdir> <name>=<path>... (boot required)" >&2
  exit 1
fi
shift 3

PROFILE="${ROOT}/devices/${DEV}/device.toml"
test -f "${PROFILE}" || { echo "unknown device '${DEV}' — no ${PROFILE}" >&2; exit 1; }
# Dir-name law: [device]name must equal the codename dir it lives in.
# ([^"]* — a greedy .* swallows the line's inline comment on BSD sed.)
PROFILE_NAME="$(sed -n 's/^name *= *"\([^"]*\)".*/\1/p' "${PROFILE}" | head -1)"
[ "${PROFILE_NAME}" = "${DEV}" ] \
  || { echo "device.toml says name='${PROFILE_NAME}' but dir is '${DEV}' — the two must match (D14)" >&2; exit 1; }

KEY="${ROOT}/.local/keys/aginx.key"
test -f "${KEY}" || { echo "missing ${KEY} (signing key stays local)" >&2; exit 1; }

# sha256 + size, host-portable (macOS shasum / Linux sha256sum).
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  else sha256sum "$1" | cut -d' ' -f1; fi
}
size_of() {
  stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

# Partition names the updater knows (crates/update Manifest). Anything
# else is a typo that would silently ride as an unknown JSON key.
KNOWN="boot vendor_boot dtbo vbmeta vbmeta_system rootfs"
case "${1}" in
  boot=*) ;;
  *) echo "first image must be boot=<path> (the only required one)" >&2; exit 1 ;;
esac

mkdir -p "${OUTDIR}"
MF="${OUTDIR}/manifest.json"
JSON_BODY="{\"version\": \"${VER}\", \"device\": \"${DEV}\""
for pair in "$@"; do
  name="${pair%%=*}"
  path="${pair#*=}"
  echo "${KNOWN}" | tr ' ' '\n' | grep -qx "${name}" \
    || { echo "unknown partition '${name}' (known: ${KNOWN})" >&2; exit 1; }
  test -s "${path}" || { echo "missing image ${path}" >&2; exit 1; }
  abs="$(cd "$(dirname "${path}")" && pwd)/$(basename "${path}")"
  extra=""
  if [ "${name}" = "rootfs" ] && [ -n "${PRE_STAGED:-}" ]; then
    extra=", \"pre_staged\": true"
  fi
  JSON_BODY="${JSON_BODY}, \"${name}\": {\"url\": \"${abs}\", \"sha256\": \"$(sha256_of "${abs}")\", \"size\": $(size_of "${abs}")${extra}}"
done
JSON_BODY="${JSON_BODY}}"
printf '%s\n' "${JSON_BODY}" > "${MF}"

echo "==> signing ${MF}"
(cd "${ROOT}" && cargo run -q -p aginx-sign -- sign "${KEY}" "${MF}")
echo "==> verifying (public chain)"
(cd "${ROOT}" && cargo run -q -p aginx-sign -- verify "${ROOT}/.local/keys/aginx.pub" "${MF}")

echo "built ${MF} (+ .sig) for device '${DEV}'"
echo "on device: aginx-update apply ${MF}   # refuses if manifest.device != this machine"
