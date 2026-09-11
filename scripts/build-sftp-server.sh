#!/usr/bin/env bash
# Build the static arm64 sftp-server (Go + github.com/pkg/sftp) for the L0
# image — the dropbear sftp subsystem (#319, 2026-09-12). dropbear 2026.94
# has SFTP support compiled in (DROPBEAR_SFTP_SERVER=/usr/libexec/sftp-server)
# and execs this binary per connection with SFTP v3 on stdin/stdout — same
# contract as OpenSSH's sftp-server, so host scp/sftp/GUI clients work
# unmodified. CGO off = fully static, no libc; ~7 MB stripped (Go runtime).
# Cached under out/ (cacert/resize2fs pattern): the module download and
# build happen once per host; build-rootfs.sh installs the product.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_BIN="${ROOT}/out/sftp-server"

command -v go >/dev/null 2>&1 \
  || { echo "go not found (needed for the sftp-server build — brew install go)" >&2; exit 1; }

if [ -s "${OUT_BIN}" ]; then
  echo "sftp-server already built: ${OUT_BIN} ($(stat -f%z "${OUT_BIN}") bytes)"
  exit 0
fi

# Versions are pinned by tools/sftp-server/go.sum (committed); this build is
# reproducible. -trimpath drops host paths, -s -w drops the symbol table.
( cd "${ROOT}/tools/sftp-server" && \
  GOOS=linux GOARCH=arm64 CGO_ENABLED=0 \
  go build -trimpath -ldflags='-s -w' -o "${OUT_BIN}" . )

file "${OUT_BIN}"
echo "built ${OUT_BIN} ($(stat -f%z "${OUT_BIN}") bytes)"
