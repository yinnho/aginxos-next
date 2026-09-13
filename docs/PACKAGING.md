# AginxOS device packages — the release contract

Status: **v1 (2026-09-13)**. This file is the single authority on what a
published AginxOS device package is. Changes to the package shape land here
first, then in the pipeline.

## 0. Audience: agents first

An AginxOS device package is built to be consumed by an **agent**, not by a
human following a tutorial. The human's only irreducible role is physical:
plugging the cable and holding buttons. Everything else — reading the
instructions, running the flash, verifying the result, recovering from a bad
flash — is written for a machine to execute. The package therefore carries a
machine-readable manifest and an agent manual (`SKILL.md`), not prose aimed
at a person.

## 1. Editions: one image per device, edition = install state

- **One zip per device model.** The image inside IS the server edition:
  headless L0 — kernel + init + svcd + network + ssh + pkg — an always-on
  network responder with no screen software.
- **The touch edition is the same image plus the touch suite** installed
  after first boot (terminal, voice, QR pairing, browser — one aggregated
  opt-in). Editions never fork the image.
- Rationale: everything-is-a-package. Features ride package updates, never
  reflashes; one image keeps one test matrix and one truth.

## 2. Naming and versioning

`aginxos-<device>-<version>.zip` — device ∈ {`redfin`, `enchilada`};
version is semver per device line, starting `0.1.0`. The manifest records
the build commit(s). Releases live on GitHub (`yinnho/aginxos-next`,
public), one tag per package: `<device>-v<version>`.

## 3. Zip layout (redfin v1)

```text
aginxos-redfin-0.1.0/
  manifest.json               — machine-readable contract (§4)
  SKILL.md                    — agent manual (§5)
  flash.sh                    — deterministic flash script (§6)
  boot/vendor_boot.img        — patched vendor_boot (trampoline + initramfs)
  boot/vendor_boot.stock.img  — stock restore point (recovery)
  rootfs.img                  — 2 GiB ext4 userdata payload
  SHA256SUMS                  — over every file above except itself
```

Facts that shape this (redfin):

- **The kernel is never flashed.** AginxOS rides the stock `boot` partition
  and takes over via the vendor_boot trampoline — there is no `boot.img` in
  the package, and the consumer's boot partition is never touched.
- `fastboot flash userdata` writes the front 2 GiB only; the rest of the
  disk is grown by the image itself on first boot.
- Vendor pieces are bundled deliberately (first-gen DECISIONS §7 superseded
  2026-09-13): the consumer never fetches a factory image, never patches
  anything. One zip is the whole job.

## 4. manifest.json

```json
{
  "package": "aginxos-redfin",
  "version": "0.1.0",
  "device": "redfin",
  "build": { "commit": "…", "built_at": "…" },
  "device_gate": {
    "fastboot_getvar": { "product": "redfin" },
    "single_device_required": true
  },
  "images": [
    { "path": "rootfs.img", "partition": "userdata", "sha256": "…", "bytes": 2147483648 },
    { "path": "boot/vendor_boot.img", "partition": "vendor_boot", "sha256": "…", "bytes": 35774464 }
  ],
  "recovery": [
    { "path": "boot/vendor_boot.stock.img", "partition": "vendor_boot", "sha256": "…" }
  ],
  "flash_order": ["userdata", "vendor_boot"],
  "human_steps": ["enter fastboot: power off, hold Power+VolumeDown"],
  "verify": [
    { "probe": "adb device appears", "timeout_s": 300 },
    { "probe": "adb shell cat /run/boot.state reports done", "timeout_s": 600 },
    { "probe": "ssh reachable after configuration", "timeout_s": 300 }
  ],
  "configure_after": {
    "wifi": "adb push wifi.conf /etc/wifi.conf",
    "auth": "set a root password over adb, or push an ssh public key",
    "then": "ssh takes over; USB may be unplugged"
  },
  "editions": {
    "server": "nothing further — the flashed image is complete",
    "touch": "opt-in the touch suite once on network"
  }
}
```

## 5. SKILL.md (in-package agent manual)

Follows the repo's `agents/skills/` style, written for an agent with shell
access on the host:

1. **Prerequisites** — unlocked bootloader, `fastboot` on PATH, a USB data
   cable, this zip's SHA256SUMS verified.
2. **The one human step** — power off, hold Power+VolumeDown to enter
   fastboot. Explicitly marked: a machine cannot do this.
3. **Execution** — `GO=1 ./flash.sh` (dry-run without `GO=1`).
4. **Acceptance as assertable receipts** — never "the screen lights up":
   adb enumerates, `/run/boot.state` reaches `done`, ssh answers after
   configuration.
5. **Recovery** — flash the bundled stock vendor_boot, re-run. Exact
   commands, no factory-image fetching.
6. **Configuration** — wifi.conf, password or pubkey, ssh handover; the
   touch-suite opt-in for the touch edition.

## 6. flash.sh discipline

- `#!/usr/bin/env bash`, `set -euo pipefail`; dry-run by default, `GO=1`
  executes.
- Gate: `fastboot getvar product` must equal the manifest's device;
  refuse to run if more than one fastboot device is attached.
- Order: `userdata` first, `vendor_boot` LAST (the commit point), then
  reboot.
- Verifies `SHA256SUMS` before flashing anything.
- Fresh installs only — no state capture (the upgrade path stays internal
  tooling for now).
- On failure, prints the exact recovery command.

## 7. Build & publish pipeline

1. Fresh bake: `DEVICE=redfin ./scripts/build-rootfs.sh` → `out/rootfs.img`.
2. Pack: `HOLD=1 USBADB=1 ROOTFS=1 ./devices/redfin/boot/pack-vendor-boot.sh`
   → `vendor_boot-test.img`.
3. Assemble: `DEVICE=redfin ./scripts/dist.sh <version>` →
   `dist/aginxos-redfin-<version>.zip` (manifest rendered with real
   hashes/sizes/commits; SHA256SUMS over the payload).
4. Publish: `gh release create <device>-v<version> …`. The agpkg package
   mirror stays at pkgs.aginx.net — GitHub carries device packages only.

## 8. Self-test protocol (every release, before announcing)

1. From a clean directory, download the release asset exactly as an
   external consumer would.
2. Follow **only** the in-package SKILL.md to flash the experiment unit.
3. Acceptance = the manifest's verify probes pass, plus the n7-l0
   pre/netup/ssh phases green.
4. Log the receipt in the local experiment log; only then is the release
   announced.

## 9. Non-goals (v1)

- No dual images, no touch-suite preinstall.
- No OTA payload in the package (the agupd update line is unchanged).
- No enchilada package until its L0 parity is observed on hardware (#335).
- No bootloader relock guidance; an unlocked bootloader is a prerequisite.
