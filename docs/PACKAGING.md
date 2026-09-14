# AginxOS device packages — the release contract

Status: **v1.1 (2026-09-14)** — covers redfin (0.1.0–0.1.2) and enchilada
(0.1.0), both release-proven by device self-tests. This file is the single
authority on what a published AginxOS device package is. Changes to the
package shape land here first, then in the pipeline.

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

 enchilada variant (v0.1.0): one `boot/enchilada-boot.img` (mainline
kernel + trampoline initramfs, header v1, Image.gz+dtb appended) replaces
the vendor_boot pair — enchilada flashes its own kernel and has no stock
vendor_boot restore point. Recovery is the **other A/B slot**: the flash
never touches it, so `fastboot set_active <other-slot> && fastboot reboot`
boots whatever OS lived there (typically stock LineageOS).

```text
aginxos-enchilada-0.1.0/
  manifest.json               — same schema; "recovery": [], plus
                              "recovery_procedure" string (§4)
  SKILL.md                    — enchilada manual (no adb: NCM usb net + ssh)
  flash.sh                    — adds --pubkey/--wifi injection (§6.1)
  boot/enchilada-boot.img     — flashed to boot_<current-slot>
  rootfs.img                  — ext4 userdata payload
  SHA256SUMS
```

## 4. manifest.json

One schema for all devices; device deltas are data, not forks:

- `device_gate.fastboot_getvar.product` is what the bootloader REALLY
  reports — `redfin` for Pixel 5, **`sdm845`** for OnePlus 6 (measured;
  guessing the marketing name would reject every consumer).
- enchilada names the boot partition `boot_<current-slot>` (the slot is
  read at flash time) and carries `"recovery": []` plus a
  `"recovery_procedure"` string (other-slot boot) instead of a stock image.
- `flash_order` includes `"set_active <current-slot>"` for enchilada —
  there the commit point is the slot switch, not the last flash.

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
5. **Recovery** — redfin: flash the bundled stock vendor_boot, re-run.
   enchilada: boot the untouched other slot (`set_active <other-slot>`),
   re-run. Exact commands, no factory-image fetching.
6. **Configuration** — wifi.conf, password or pubkey, ssh handover; the
   touch-suite opt-in for the touch edition.

Device deltas live in each package's SKILL.md, not in this contract; the
big one is the channel: redfin verifies over adb, enchilada has **no adb**
and verifies over a USB NCM network plus ssh (10.9.8.1) — which is why the
enchilada flash must be given the pubkey BEFORE flashing (§6.1).

## 6. flash.sh discipline

- `#!/usr/bin/env bash`, `set -euo pipefail`; dry-run by default, `GO=1`
  executes.
- Gate: `fastboot getvar product` must equal the manifest's device;
  refuse to run if more than one fastboot device is attached.
- Order: `userdata` first, then the boot artifact. The commit point is
  device-shaped: redfin = `vendor_boot` flashed LAST; enchilada =
  `set_active <slot>` after both flashes. Then reboot.
- Verifies `SHA256SUMS` before flashing anything.
- Fresh installs only — no state capture (the upgrade path stays internal
  tooling for now).
- On failure, prints the exact recovery command.

### 6.1 Pre-flash injection (devices without adb)

A package may inject consumer files into `rootfs.img` before flashing
(currently: `--pubkey`, `--wifi`). This is load-bearing for enchilada —
without the pubkey there is NO authentication channel at all. Hard
lessons from the 0.1.0 self-test (HARDWARE.md 2026-09-14, #350):

- **Never trust debugfs exit codes.** debugfs exits 0 even when individual
  commands fail (e.g. a `write` into a missing directory). Every injection
  is post-verified with `debugfs stat <path>` expecting `Type: regular`;
  missing = refuse to flash.
- **A pristine fresh bake has no `/root/.ssh`** — the pubkey script must
  `mkdir` it (redundant `mkdir`/`rm` line errors are tolerated; they don't
  matter, only the post-verify does).
- **debugfs marks the filesystem dirty**; run `e2fsck -fp` after
  injection and refuse to flash unless a following `-fn` check is clean —
  first-boot resize2fs refuses dirty images.
- Host-side dry-runs must exercise the REAL input class (a pristine fresh
  bake), not a convenient stale copy — the stale copy is what let the
  bug through the first time.

## 7. Build & publish pipeline

1. Fresh bake: `DEVICE=redfin ./scripts/build-rootfs.sh` → `out/rootfs.img`.
2. Pack: `HOLD=1 USBADB=1 ROOTFS=1 ./devices/redfin/boot/pack-vendor-boot.sh`
   → `vendor_boot-test.img`.
3. Assemble: `DEVICE=<device> ./scripts/dist.sh <version>` →
   `dist/aginxos-<device>-<version>.zip` (manifest rendered with real
   hashes/sizes/commits; SHA256SUMS over the payload; a secret scan
   refuses to package a rootfs.img carrying any packer credential, and
   the enchilada boot.img additionally needs the
   `.rescue_pubkey_stripped` stamp from `RESCUE_PUBKEY=0` packing —
   a public boot.img carries zero packer keys).
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

Track record: enchilada 0.1.0 needed two rounds (round 1 caught the
injection bug §6.1 exists for); redfin 0.1.2 surfaced the agc credential
truth — the packaged gateway has no device pairing, auth is the relay
secret single gate, so a stale `agc` keychain token is cleared with
`agc --logout`, never `--bind`. Receipts: HARDWARE.md 2026-09-14 (#350,
#351, local).

## 9. Non-goals (v1)

- No dual images, no touch-suite preinstall.
- No OTA payload in the package (the agupd update line is unchanged).
- No bootloader relock guidance; an unlocked bootloader is a prerequisite.
- No touch edition for enchilada (no display/touch bring-up ships for it).
