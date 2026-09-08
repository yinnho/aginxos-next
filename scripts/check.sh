#!/usr/bin/env bash
# Host gate (N1) — everything that must pass before a commit.
#
#   ./scripts/check.sh          # cargo test workspace + registry lint
#   ./scripts/check.sh lint     # registry lint only (skip cargo test)
#
# Layer 2 runs `aginx commands --check` over a scratch copy of shims/
# (the repo-local command faces for host trials). git carries shims
# 100644; the router only registers executables, so the copy is chmod'ed
# (the device bake would do the same).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

MODE="${1:-all}"

# ---- 1. host tests ---------------------------------------------------------
# aginx-svc's supervisor bin (aginx-svcd) is Linux-only (prctl/ucred/SO_PEERCRED),
# same discipline as agupd/agsvc in the first-gen repo: on macOS the bin is
# excluded and only its lib (host-testable, also what aginx-term links) runs.
if [ "${MODE}" != "lint" ]; then
  if [ "$(uname -s)" = "Linux" ]; then
    echo "==> cargo test --workspace"
    cargo test --workspace
  else
    echo "==> cargo test --workspace --exclude aginx-svc (+ its lib)"
    cargo test --workspace --exclude aginx-svc
    cargo test -p aginx-svc --lib
  fi
fi

# ---- 2. campix host tests (M47②) --------------------------------------------
# cam-shot's pixel-chain math (black level / gamma LUTs, crop geometry,
# debayer-rotate-scale) lives in each device's cam/campix.h as pure
# functions so it is testable without a device — same zig that builds the
# device binary. D14: one campix_test.c per devices/<codename>/cam/ — every
# machine's math runs, not just the first target's.
if [ "${MODE}" != "lint" ]; then
  ZIG="$(command -v zig || true)"
  test -z "${ZIG}" && ZIG=/opt/homebrew/bin/zig
  test -x "${ZIG}" || { echo "zig not found (needed for campix_test)" >&2; exit 1; }
  found=0
  for ct in "${ROOT}"/devices/*/cam/campix_test.c; do
    test -f "${ct}" || continue
    found=$((found + 1))
    dev="$(basename "$(dirname "$(dirname "${ct}")")")"
    echo "==> campix_test (${dev})"
    CBIN="$(mktemp -d)/campix_test"
    "${ZIG}" cc -O1 -Wall -Wextra -o "${CBIN}" "${ct}" -lm
    "${CBIN}"
    rm -f "${CBIN}"
  done
  test "${found}" -gt 0 || { echo "no devices/*/cam/campix_test.c found" >&2; exit 1; }
fi

# ---- 3. D14 grep gate -------------------------------------------------------
# D14 law 1: no machine strings in platform crates. Every hit must carry
# an inline `// D14-exempt` marker ON THE SAME LINE (a marker on the line
# above does not count — per-line filter), each reviewed by hand:
#   - hwd/img test fixtures asserting the REAL committed device.toml
#     (schema-truth tests) and their helper fns
#   - provenance comments in svc/boot_ok.rs pointing at [slots]/[update]
#     registrations in devices/redfin/device.toml
#   - first-gen frozen-offset exemptions noted in ARCH.md D14
if [ "${MODE}" != "lint" ]; then
  BAD="$(grep -rnE '1080|2340|event[0-9]|qpnp_pon|sm7250|redfin' \
      "${ROOT}"/crates/*/src --include='*.rs' | grep -v 'D14-exempt' || true)"
  if [ -n "${BAD}" ]; then
    echo "D14 gate: machine strings in platform crates (mark reviewed lines" >&2
    echo "with an inline '// D14-exempt', or move the data to devices/):" >&2
    echo "${BAD}" >&2
    exit 1
  fi
fi

# ---- 4. registry lint ------------------------------------------------------
cargo build -p aginx-router --release >/dev/null
SCRATCH="$(mktemp -d)"
trap 'rm -rf "${SCRATCH}"' EXIT

mkdir -p "${SCRATCH}/usr/bin"
cp -R "${ROOT}/shims/." "${SCRATCH}/usr/bin/"
chmod 755 "${SCRATCH}"/usr/bin/aginx-* 2>/dev/null || true

echo "==> aginx commands --check (scratch shim tree)"
AGINX_CMD_PATH="${SCRATCH}/usr/bin" \
AGINX_GROUPS_DESC="${ROOT}/shims/groups.desc" \
  "${ROOT}/target/release/aginx" commands --check \
  || { echo "aginx commands --check failed — fix the shims" >&2; exit 1; }

echo "check: all green"
