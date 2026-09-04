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
if [ "${MODE}" != "lint" ]; then
  echo "==> cargo test --workspace"
  cargo test --workspace
fi

# ---- 2. registry lint ------------------------------------------------------
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
