# Flashing AginxOS on redfin (Pixel 5) — agent manual

You are an agent with shell access on the user's computer. This package
installs AginxOS on a Google Pixel 5 (codename `redfin`). The human's only
role is physical: cable and buttons. Everything else — verification, the
flash itself, acceptance, recovery — is yours. Work through this file top
to bottom; do not improvise steps not listed here.

## 0. Prerequisites (check all, stop on any miss)

- `fastboot` on PATH (Android platform-tools).
- A USB data cable between the computer and the phone.
- The phone's bootloader already **unlocked**. If `fastboot flashing get_unlock_ability`
  or the fastboot screen shows it is locked, STOP and tell the human:
  unlocking wipes the device and is their decision, not yours.
- Payload integrity: run `shasum -a 256 -c SHA256SUMS` (macOS) or
  `sha256sum -c SHA256SUMS` (Linux) in the package directory. All lines OK.

## 1. The one human step

Tell the human:

> Power the phone off. Hold **Power + Volume Down** until the fastboot
> screen appears. Plug the phone into this computer.

Then wait until `fastboot devices` lists exactly one device. If it lists
more than one, STOP — this package refuses to guess which phone it is
talking to.

## 2. Dry run

```bash
./flash.sh
```

Prints the plan, changes nothing. Read it.

## 3. Flash

```bash
GO=1 ./flash.sh
```

The script verifies checksums, checks the device is a redfin, flashes
`userdata` first and `vendor_boot` last (the commit point), then reboots.
If anything fails it prints the recovery command — run it, then start over.

## 4. Verify — receipts, not screen

**The screen staying dark after the bootloader stage is correct.** AginxOS
is a headless OS; proof of life is on the wire, not the panel.

1. Within ~5 minutes of reboot: `adb devices` lists the phone.
2. `adb shell cat /run/boot.state` — wait until every line is an `ok` or
   the file contains `^done`. First boot grows the disk to fill the phone;
   this can take a few minutes.
3. Only then is the flash accepted. Anything else = recovery (§6).

## 5. Configure — over adb, then ssh takes over

The fresh image is factory-shaped: no Wi-Fi credentials, no password, no
keys. Configuration is deliberately post-flash:

```bash
# Wi-Fi — wifi.conf is KEY=VALUE, two lines:
#   ssid=YourNetworkName
#   psk=YourPassphrase
adb push wifi.conf /etc/wifi.conf
adb shell chmod 600 /etc/wifi.conf

# Auth — either set a root password:
adb shell passwd
# or push a public key:
adb push authorized_keys /root/.ssh/authorized_keys

adb shell reboot
```

After reboot, `/run/boot.state` should show `wifi ok` and `internet ok`.
Then `ssh root@<phone-ip>` works and USB may be unplugged. The phone's IP:
`adb shell ip addr show wlan0`.

## 6. Recovery

If the new system does not come up:

```bash
fastboot flash vendor_boot boot/vendor_boot.stock.img
fastboot reboot
```

returns the phone to the stock boot chain. Re-running the flash afterwards
is safe.

## 7. Editions

- **Server edition**: nothing further. What you flashed is complete — a
  headless agent node: ssh in, software via `agpkg`. Keep it on power;
  it answers the network at all times.
- **Touch edition**: once on the network, opt in the touch suite
  (terminal, voice, QR pairing, browser) as packages. No reflash, ever.

## Rules (hard)

- Never flash a device whose `fastboot getvar product` is not `redfin`.
- Never flash with more than one fastboot device attached.
- Never commit or echo Wi-Fi passphrases, passwords, or key material into
  logs, repos, or chat transcripts.
- The only partitions this package touches are `userdata` and
  `vendor_boot`. Do not flash anything else.
