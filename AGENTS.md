# AginxOS — Agent Guide

Second-generation AginxOS: the architecture constitution (D1–D14) built
as a fresh workspace. Since N4 this repo owns the bake chain and the
device: `DEVICE=<codename> ./scripts/build-rootfs.sh` bakes a machine's
flashable image. First machine: Pixel 5 (`redfin`), the daily experiment
unit; a second bring-up line (OnePlus 6, `enchilada`) exists to keep the
platform honest — machines are data (D14), living in `devices/<codename>/`.

- `~/Documents/aginxos` — first-generation line, **SEALED 2026-09-08**
  (zero commits; only exception: a disaster-rollback receipt — its
  `.factory` flash-all stays that repo's authority). Its `docs/ARCHIVED.md`
  holds the asset map and history guide. Everything this repo's bake
  consumes now lives HERE under gitignored `.local/device/redfin/`
  (vendor ramdisk, voice/OCR builds+models, dropbear, radio blobs, the
  frozen aginxos trampoline pair — regeneration paths in
  `devices/redfin/boot/assets.md`); busybox, fonts and the C sources are
  in-tree (`rootfs/`). The trampoline pair stays deliberately first-gen:
  the swapper of the rootfs swap is frozen (see assets.md).
- `~/Documents/aginx` — ecosystem (aginx-carrier, aginx daemon,
  aginxbrowser, memory server). Source of import seams, not development.

## Constitution

`docs/ARCH.md` is the architecture constitution: one server per machine
(the mother, aginx), avatars are folders run by a single runtime engine,
display is request semantics, the session log is the truth source,
addressing is front-desk registration (进/住/切/退), externals are
CLI-only (D12), every command carries the aginx surname (D13), and
machines are data, not code (D14).
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
- **N5 absorption + remote channel + backup line**: six frozen
  first-gen binaries rebuilt here (download/update with the three dead
  D13 paths fixed, qr/done/secret), the /var/lib state world unified
  under `/var/lib/aginx` with an idempotent boot migrator (union,
  new-wins, old parked in `.pre-n5/`, never deletes), `aginx-backup`
  local snapshot line (secret store excluded, verified both ways), and
  the sixth unit `aginx-gateway` — persistent register to
  relay.aginx.net, external JSON-RPC collapsed onto the server's UDS
  front (ACP.md wire authority = ecosystem repo). Acceptance gate:
  `scripts/accept/n5.sh`.
- **L0 无头底座 (2026-09-12, 刀1–刀6 翻档)**: the image is the base and
  nothing else — kernel + init + supervisor + network + ssh (dropbear,
  password AND pubkey channels) + pkg + the bootcard lamp; image svc.d
  ships exactly 2 units (net-watch + absent-tolerant aginxbrowser).
  The mother (one `aginx` tree package: router/server/runtime), term,
  voice, gateway, secretd and the three model trees all ride packages
  carrying their own `[service]` units; provision installs NOTHING
  (manifest is all-opt) — `aginx-pkg opt-in <name>` pulls a package
  with its dependency closure (voice brings asr/tts/ocr, gateway brings
  secretd). Flash is zero-prep — one universal image, no personal data
  baked; configuration is post-flash over adb (`/etc/wifi.conf` +
  `busybox chpasswd -c sha512` or authorized_keys, then ssh takes
  over; acceptance = `n7-l0.sh`). `./flash-redfin.sh capture` is the
  upgrade path (pre-arms
  the state tar so /root/.ssh + wifi.conf survive a re-flash); the
  default flash wants the factory shape.

## Ground Rules

- Host green before anything: `./scripts/check.sh` (cargo test over the
  workspace + per-device camera pixel-chain tests + the D14 machine-string
  grep gate + `aginx commands --check` registry lint) must pass before
  every commit.
- Machines are data (D14): platform crates (`crates/` + `rootfs/` +
  `scripts/`) carry zero machine references; every machine fact lives in
  `devices/<codename>/` and is read through `crates/hwd` — the single
  legal source. Machine strings in crates are unconstitutional, there is
  no default machine (a missing profile fails fast at boot), and device
  dirs never import each other.
- The avatar root is `~/.aginx/workspaces` on the device (unit sets
  `AGINX_HOME=/home/.aginx`); `AGINX_HOME` overrides it for host runs.
- Naming law D13: `aginx` is the only bare command (the router); every
  external command is `aginx-<domain>-<object>-<verb>`, verb last.
  Compiled commands in scan dirs (/usr/bin, /var/bin) REQUIRE a
  `<binary>.aginxmd` sidecar (`# aginx:key=value`, summary mandatory);
  shebang scripts carry inline `# aginx:` headers. Daemons live in
  `/usr/libexec/aginx/` — outside the router's scan, no sidecar needed.
- The image is L0 (2026-09-12): anything beyond kernel+init+svc+network+
  ssh+pkg is a package. New engine work lands as `pkgs/<name>/` with
  its `[service]` unit, never as a baked unit — `build-rootfs.sh` dies
  if image svc.d grows past 2. Provision ships an all-opt manifest and
  installs nothing; what runs on a phone is the user's `opt-in`.
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
static; `build-rootfs.sh` zigbuilds everything it needs. The brain-facing
C tools and /bin internals are zig cc musl statics built from the in-tree
`rootfs/src/` sources (moved from the old repo at P1); the camera chain
builds from `devices/<codename>/cam/` via `scripts/build-cam.sh`; the
dropbear sftp subsystem is a Go static (`tools/sftp-server` +
`scripts/build-sftp-server.sh`, deps pinned by go.sum — host needs go).

## Layout

| Path | What |
|------|------|
| `crates/router` | `aginx` — the bare command, mother's face |
| `crates/server` | `aginx-server` — front desk, cursor, routing, ledger |
| `crates/runtime` | `aginx-runtime` — fast-agi engine (avatar runner) |
| `crates/agi` | fast-agi v0 frame types (both ends share) |
| `crates/agio` | D1 output envelope |
| `crates/hwd` | device profile reader — the single legal source of machine facts (D14) |
| `crates/voice` | `aginx-voice` — the voice dialog daemon (voiced, M42 line) |
| `crates/wizard` | `aginx-net-wizard` — first-boot Wi-Fi setup TUI |
| `crates/term` | `aginx-term` — on-device terminal UI (aterm line) |
| `crates/pkg` | `aginx-pkg` — package manager (signed manifest, 四件套) |
| `crates/svc` | `aginx-svcd` + `aginx-svc` + `aginx-boot-ok` — supervisor, control, A/B marker |
| `crates/sign` | `aginx-sign` — host signer/verifier (ed25519; keys in `.local/keys/`) |
| `crates/pair` | `aginx-pair` — host-only pairing-code minter: AGINXPAIR1 bundle → PNG QR (five fields never echoed) |
| `crates/qr`, `crates/img` | `aginx-qr` — QR decode CLI (quircs + jpeg decode face); vendored libjpeg-turbo |
| `crates/download`, `crates/update` | `aginx-download`/`aginx-update` — HTTPS fetch + signed A/B rootfs updater |
| `crates/done` | `aginx-done` — provision done-marker discipline |
| `crates/secret` | `aginx-secretd`/`aginx-secret` — the secret sidecar + its admin face |
| `crates/gateway` | `aginx-gateway` — remote channel daemon: registers to relay.aginx.net, collapses external JSON-RPC onto the server's UDS front (ACP.md wire authority = ecosystem repo) |
| `crates/testkit` | test helpers |
| `rootfs/` | the image recipe — see `rootfs/README.md` (placement matrix, asset split) |
| `devices/` | per-machine data, one dir per codename: `device.toml`, `modules.txt`, `bringup/`, `boot/` (pack line), `cam/` — add-a-machine checklist in `devices/README.md` |
| `scripts/build-rootfs.sh` | the bake, `DEVICE=<codename>`: recipe + zigbuild + that machine's assets (`.local/device/<codename>`) → `out/rootfs.img` |
| `scripts/accept/` | device acceptance suites: `n6-egg.sh` (L0 bake-day shape/equivalence: pre/paired/steady/egg2), `n7-l0.sh` (product flow: usbconf→netup→ssh→opt-in→steady), `m42c.sh` pairing gate; n4/n5 retired 2026-09-12 (headers say why) |
| `shims/` | repo-local `aginx-*` command faces (host trial registry) |
| `docs/ARCH.md` | the constitution (local only, gitignored) |
| `docs/HARDWARE.md` | device experiment log — this repo's receipts from N4 on |

## Device Safety

Two phones on the bench. Pixel 5 redfin (adb serial `aginxosredfin`,
fastboot `13201FDD4001N8`) is the experiment unit; OnePlus 6 enchilada
(`b0d9f7fe`) is the second bring-up machine. The neighboring Huawei
`NAB0220B10025626` and Redmi 7A (`c353ac919`) are NEVER touched. Before
any destructive fastboot command, confirm the attached serial is the
machine you meant — `devices/<codename>/boot/flash-<codename>.sh` gates
this by the profile's serial; a hand-typed fastboot line must gate
itself the same way.
Ground truths inherited from the first-generation receipts (full history
in the old repo's `docs/HARDWARE.md`):

- adb push does not preserve the exec bit — `chmod +x` after push.
- `adb reboot` hangs; reboot via `/usr/bin/aginx-reboot reboot`
  (formerly /bin/reboot2). `aginx-reboot bootloader` lands in fastboot.
- busybox `awk`/`netstat` segfault unconditionally — device scripts use
  sed/`set --` only.
- Never rmmod on this kernel (panic). Restore points:
  `.local/device/redfin/stock/stock-boot.img` and
  `stock-vendor_boot.img`; last-resort recovery is the old repo's
  `.factory/` flash-all.
- End every device session in a known state, logged in
  `docs/HARDWARE.md`. "Confirm on device" is not done until someone saw
  it; never promote an expected result to a recorded one.

## Git

- Atomic commits: one coherent change, imperative message.
- Every commit passes `./scripts/check.sh`.
- Device-behavior changes get a receipt in `docs/HARDWARE.md` (this
  repo's file since N4).
