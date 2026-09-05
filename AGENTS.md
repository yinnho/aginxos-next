# AginxOS — Agent Guide

Second-generation AginxOS: the architecture constitution (D1–D13) built
as a fresh workspace. Since N4 this repo owns the bake chain and the
device: `scripts/build-rootfs.sh` produces the flashable image, and the
running Pixel 5 (redfin) is this line's hardware.

- `~/Documents/aginxos` — first-generation line, now an ASSET LIBRARY.
  `build-rootfs.sh` references it read-only via `OLD=` (default
  `~/Documents/aginxos`) for: busybox, `rootfs/src/*.c`, the unpacked
  vendor ramdisk, voice/OCR builds+models, dropbear, radio blobs, fonts,
  and the frozen aginxos trampoline pair (aginxos-init/aginxos-agent —
  deliberately not absorbed: the swapper of the rootfs swap stays
  first-gen). Do not build on its code; its `docs/HARDWARE.md` holds
  every device receipt before N4.
- `~/Documents/aginx` — ecosystem (aginx-carrier, aginx daemon,
  aginxbrowser, memory server). Source of import seams, not development.

## Constitution

`docs/ARCH.md` is the architecture constitution: one server per machine
(the mother, aginx), avatars are folders run by a single runtime engine,
display is request semantics, the session log is the truth source,
addressing is front-desk registration (进/住/切/退), externals are
CLI-only (D12), and every command carries the aginx surname (D13).
**ARCH.md is LOCAL ONLY — never commit or push it** (same treatment as
the old repo's ARCH/CARRIER/SYSTEM docs; `.gitignore` enforces it).

## Milestones (N series)

- **N1 platform heart, host loop**: `aginx` router + `aginx-server`
  (front desk, cursor, request routing, session ledger) +
  `aginx-runtime` (fast-agi engine) — closed loop with the real brain on
  the host.
- **N2 trial on device**: adb push, isolated HOME `~/.aginx-n`, the old
  carrier data untouched.
- **N3 coexisting package**: agpkg on the live image, daily-driver
  verification.
- **N4 bake takeover**: this repo bakes the whole image (rootfs recipe +
  renamed first-gen assets), fresh-flash cutover, old repo archived.
  The N3 coexistence package is retired; the image is the product.

## Ground Rules

- Host green before anything: `./scripts/check.sh` (cargo test over the
  workspace + `aginx commands --check` registry lint) must pass before
  every commit.
- The avatar root is `~/.aginx/workspaces` on the device (unit sets
  `AGINX_HOME=/home/.aginx`); `AGINX_HOME` overrides it for host runs.
- Naming law D13: `aginx` is the only bare command (the router); every
  external command is `aginx-<domain>-<object>-<verb>`, verb last.
  Compiled commands in scan dirs (/usr/bin, /var/bin) REQUIRE a
  `<binary>.aginxmd` sidecar (`# aginx:key=value`, summary mandatory);
  shebang scripts carry inline `# aginx:` headers. Daemons live in
  `/usr/libexec/aginx/` — outside the router's scan, no sidecar needed.
- Secrets never enter the repo or docs. Brain access is
  `AGINXBRAIN_API_KEY` in `/etc/aginx/env` at runtime (unit env_file) —
  never committed, never echoed; on host it rides the environment only.
- Import, don't entangle: code brought in from the asset libraries gets
  re-housed on fast-agi frames and D13 names in the same commit it
  arrives; no compatibility shims to the old kernel types.

## Hosts & Toolchains

Host builds/tests run on macOS and Linux with stable Rust (the aginx-svc
daemon bin is Linux-only — check.sh runs its lib on darwin). The device
target is `aarch64-unknown-linux-musl` via zig / cargo-zigbuild, fully
static; `build-rootfs.sh` zigbuilds everything it needs. The four
brain-facing C tools and /bin internals are zig cc musl statics built
from the old repo's sources at bake.

## Layout

| Path | What |
|------|------|
| `crates/router` | `aginx` — the bare command, mother's face |
| `crates/server` | `aginx-server` — front desk, cursor, routing, ledger |
| `crates/runtime` | `aginx-runtime` — fast-agi engine (avatar runner) |
| `crates/agi` | fast-agi v0 frame types (both ends share) |
| `crates/agio` | D1 output envelope |
| `crates/voice` | `aginx-voice` — the voice dialog daemon (voiced, M42 line) |
| `crates/wizard` | `aginx-net-wizard` — first-boot Wi-Fi setup TUI |
| `crates/term` | `aginx-term` — on-device terminal UI (aterm line) |
| `crates/pkg` | `aginx-pkg` — package manager (signed manifest, 四件套) |
| `crates/svc` | `aginx-svcd` + `aginx-svc` + `aginx-boot-ok` — supervisor, control, A/B marker |
| `crates/sign` | `aginx-sign` — host signer/verifier (ed25519; keys in `.local/keys/`) |
| `crates/qr`, `crates/img` | `aginx-qr` — QR decode CLI (quircs + jpeg decode face); vendored libjpeg-turbo |
| `crates/download`, `crates/update` | `aginx-download`/`aginx-update` — HTTPS fetch + signed A/B rootfs updater |
| `crates/done` | `aginx-done` — provision done-marker discipline |
| `crates/secret` | `aginx-secretd`/`aginx-secret` — the secret sidecar + its admin face |
| `crates/gateway` | `aginx-gateway` — remote channel daemon: registers to relay.aginx.net, collapses external JSON-RPC onto the server's UDS front (ACP.md wire authority = ecosystem repo) |
| `crates/testkit` | test helpers |
| `rootfs/` | the image recipe — see `rootfs/README.md` (placement matrix, asset split) |
| `scripts/build-rootfs.sh` | the bake: recipe + zigbuild + OLD= assets → `out/rootfs.img` |
| `scripts/accept/` | device acceptance suites (n4.sh is the switch gate) |
| `shims/` | repo-local `aginx-*` command faces (host trial registry) |
| `docs/ARCH.md` | the constitution (local only, gitignored) |
| `docs/HARDWARE.md` | device experiment log — this repo's receipts from N4 on |

## Device Safety

One real device: Pixel 5 redfin, adb serial `aginxosredfin`, fastboot
`13201FDD4001N8` (the neighboring Huawei `NAB0220B10025626` is NEVER
touched). Before any destructive fastboot command, confirm the serial.
Ground truths inherited from the first-generation receipts (full history
in the old repo's `docs/HARDWARE.md`):

- adb push does not preserve the exec bit — `chmod +x` after push.
- `adb reboot` hangs; reboot via `/usr/bin/aginx-reboot reboot`
  (formerly /bin/reboot2). `aginx-reboot bootloader` lands in fastboot.
- busybox `awk`/`netstat` segfault unconditionally — device scripts use
  sed/`set --` only.
- Never rmmod on this kernel (panic). Old-repo restore points:
  `boot/stock-boot.img`, `boot/stock-vendor_boot.img`; last-resort
  recovery is the old repo's `.factory/` flash-all.
- End every device session in a known state, logged in
  `docs/HARDWARE.md`. "Confirm on device" is not done until someone saw
  it; never promote an expected result to a recorded one.

## Git

- Atomic commits: one coherent change, imperative message.
- Every commit passes `./scripts/check.sh`.
- Device-behavior changes get a receipt in `docs/HARDWARE.md` (this
  repo's file since N4).
