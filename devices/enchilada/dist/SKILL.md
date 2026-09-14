# Flashing AginxOS on enchilada (OnePlus 6) — agent manual

You are an agent with shell access on the user's computer. This package
installs AginxOS on a OnePlus 6 (codename `enchilada`). The human's only
role is physical: cable and buttons. Everything else — verification, the
flash itself, configuration, recovery — is yours. Work through this file
top to bottom; do not improvise steps not listed here.

Key difference from most Android packages: **enchilada has no adb.** The
verification and configuration channel is a USB network (NCM) plus ssh.
Everything needed for that channel is decided BEFORE the flash.

## 0. Prerequisites (check all, stop on any miss)

- `fastboot` on PATH (Android platform-tools).
- A USB data cable between the computer and the phone.
- The phone's bootloader already **unlocked**. If the fastboot screen
  shows it is locked, STOP and tell the human: unlocking wipes the device
  and is their decision, not theirs to skip.
- If you will inject configuration (§3 — recommended): `debugfs`
  (e2fsprogs). macOS: `brew install e2fsprogs` (the binary lands outside
  PATH at `/opt/homebrew/opt/e2fsprogs/sbin/debugfs`; flash.sh finds it
  there). Linux: usually preinstalled.
- Payload integrity: `shasum -a 256 -c SHA256SUMS` (macOS) or
  `sha256sum -c SHA256SUMS` (Linux). All lines OK. Run this BEFORE any
  injection — the injections in §3 modify `rootfs.img` afterwards by
  design.

## 1. The one human step

Tell the human:

> Power the phone off. Hold **Power + Volume Up** until the fastboot
> screen appears. Plug the phone into this computer.

(OnePlus 6 uses Volume Up here — a Pixel would be Volume Down; do not
mix them up.) Then wait until `fastboot devices` lists exactly one
device. More than one = STOP; this package refuses to guess.

## 2. Dry run

```bash
./flash.sh
```

Prints the plan, changes nothing. Read it.

## 3. Flash

Prepare (both recommended):

- `wifi.conf` next to the package, two lines, `KEY=VALUE`:
  `ssid=YourNetworkName` and `psk=YourPassphrase`.
- Your ssh public key (the `.pub` file).

```bash
GO=1 ./flash.sh --pubkey ~/.ssh/id_ed25519.pub --wifi ./wifi.conf
```

The script verifies checksums, injects the two files into `rootfs.img`
(debugfs), fsck-repairs the image and refuses to flash if the filesystem
is not clean afterwards (a debugfs write session leaves the fs dirty, and
first-boot resize2fs refuses dirty images), checks the device reports
`product: sdm845`, reads the current A/B slot, flashes `userdata` first
and `boot_<slot>` last, commits with `set_active <slot>`, and reboots.
The OTHER slot is never touched.

Without `--pubkey` the device boots fine but **no channel can ever
authenticate** (no adb, password lane inert, no keys baked) — recovery
would then be re-flash. Do not skip the pubkey.

## 4. Verify — receipts, not screen

**The screen staying dark is correct.** enchilada is a headless node.

1. Keep the USB cable plugged. Within ~90 s the phone enumerates a USB
   network. If the host interface does not come up after ~2 min, unplug
   and replug the cable once (the USB data plane occasionally wedges
   across a reboot; a replug always clears it).
2. `ssh root@10.9.8.1` (works because of the pubkey you injected). First
   connect only: ssh will report the host key as unknown or **changed** —
   expected, every fresh install generates new server keys; accept the new
   fingerprint (this is a direct USB link, not a network you could be
   spoofed on). If ssh offers no keys at all, the injection failed — the
   flash script verifies it and would have refused to flash.
3. `cat /run/boot.state` — wait until every line is an `ok` and the file
   contains `done ok`. First boot grows the filesystem to fill the phone
   and syncs the package index; this can take a few minutes.
4. Wi-Fi: with `--wifi` injected, `wifi ok` appears on this first boot.
   Without it, configure in §5 first, then reboot and re-check.

Anything else = recovery (§6).

## 5. Configure — over ssh

The fresh image is factory-shaped: no password hash, no keys (unless you
injected), no agent software. Over `ssh root@10.9.8.1`:

```sh
# Auth hardening (password lane; keep typing the value, never log it):
echo 'root:CHOOSE-A-PASSPHRASE' | busybox chpasswd -c sha512

# Wi-Fi, if not injected at flash time (two lines, ssid= / psk=):
umask 077; printf 'ssid=%s\npsk=%s\n' 'NET' 'KEY' > /etc/wifi.conf
reboot
```

To make it an agent node (server edition, §7): append to
`/etc/aginx/env` the gateway identity and brain key, then opt in the
mother package — `aginx-gateway` depends on `aginx-secretd`, and
`aginx-pkg opt-in` pulls the dependency closure:

```sh
cat >> /etc/aginx/env <<'EOF'
AGINX_GATEWAY_ID=<the node's name>
AGINXBRAIN_API_KEY=<its brain key>
AGINX_RELAY_SECRET=<the relay secret>
EOF
aginx-pkg opt-in aginx aginx-gateway
aginx-svc restart aginx aginx-gateway
```

## 6. Recovery

From fastboot, boot the untouched other slot (the script printed which
slot it flashed; use the other one):

```bash
fastboot set_active a   # or b — the slot this run did NOT flash
fastboot reboot
```

That returns the phone to its previous OS. Re-running the flash
afterwards is safe.

## 7. Editions

- **Server edition**: nothing further. What you flashed is complete — a
  headless agent node: ssh in, software via `agpkg`. Keep it on power.
- There is no touch edition for this device yet (no display/touch
  bring-up ships for enchilada).

## Known hardware hazard

The OnePlus 6 Wi-Fi firmware (WCN3990) can assert when the access point
changes channel bandwidth (20↔40 MHz flips). A single event self-heals;
repeated events can wedge the box into Qualcomm CrashDump Mode, which
needs a human: hold **Power + Volume Up + Volume Down** ~10 s to force a
restart (it reboots into fastboot; `fastboot reboot` then boots AginxOS
again). Mitigation is on the AP side: pin its bandwidth or disable
HT40/coexistence auto-flipping.

## Rules (hard)

- Never flash a device whose `fastboot getvar product` is not `sdm845`.
- Never flash with more than one fastboot device attached.
- Never commit or echo Wi-Fi passphrases, passwords, or key material into
  logs, repos, or chat transcripts.
- The only partitions this package touches are `userdata` and the
  current slot's `boot_<slot>`, plus `set_active <slot>`. Never flash or
  set_active the other slot — it is the recovery lane.
