# HARDWARE — AginxOS second-generation device log (N4 line)

Device: Google Pixel 5 (redfin, SM7250) — same physical unit the first
generation brought up. adb serial `aginxosredfin`, fastboot
`13201FDD4001N8`.

**Everything before N4 lives in the first-generation ledger**:
`~/Documents/aginxos/docs/HARDWARE.md` (M2 boot through M45, bake #1–#18,
the N1–N3 parallel-heart receipts). This file starts at the N4 cutover —
append observed results only, same discipline: never promote an expected
result to a recorded one; "confirm on device" is not done until someone
saw it.

## N4 — bake takeover cutover (2026-09-05, observed)

**Artifacts.** Image `aginxos 54bf8c3 2026-09-05` (commit 54bf8c3 = the
D13 interior sweep), 2147483648 B, tree 658 M used, rootfs sha256
`aa4c99875cef7c89151b8e92f1223087a6fc9a8afe31842470de98a80da27a88`.
boot/vendor_boot reused the in-service pair unchanged
(`e2ce2f17…` / `d80b8098…`). Bundle in `out/update-n4/` (manifest signed
with `.local/keys/aginx.key` — pub verified byte-identical to the old
repo's `agupd.pub`, one pair for agpkg+agupd).

**Stage A (on the running N3 form).**
- A1 insurance: three tars (`/etc`, `/home/.aginx`, `/home/.aginx-n`)
  sha256-verified on both sides → `.local/backup-n4/` (gitignored).
- A2 清场: `agctl stop aginx-server`; removed units/{aginx-server,voiced}
  .toml, pkgfiles/aginx-server, skills/aginx-server,
  stamps/{aginx-server,aginx,aginx-carrier}, /var/bin/aginx-server;
  `agctl reload` → voiced fell back to the old baked `/usr/bin/voiced`
  (ready). Old line held 6 units until the swap.
- A3 从零开始: `/home/.aginx-n` removed after backup. The live carrier
  re-created `/home/.aginx/carrier` within seconds of an early rm —
  final dirt-clear was therefore sequenced pre-apply: relay+carrier
  stopped, `/home/.aginx` emptied, state tar captured immediately after.
- A4 policy: `/etc/aginx/secret.policy` overwritten with the N4-baked
  file verbatim (env-file injection is a plain svcd read, so the policy
  only gates the `aginx-secret` CLI face; the old carrier read its key
  via env_file throughout). Secret store was empty (`{}`, 2 B) — the
  carrier-era `CHARTER_SK`/`api.charter` mapping was dead weight, dropped.
- A5 net-watch 诊断箱 (#112): **no repro**. sed version in service,
  unit `spawns 1` with zero restarts across the whole prior boot, zero
  segfault lines in dmesg, hand-run battery clean (10× `ping -c1 -W3 gw`,
  `wc -c`, `tail|mv`, `date`). Same-day log even shows a healthy in-place
  rejoin (0905-01:16:53 "rejoin ok"). Closed as stable; N4 bakes the same
  sed logic (paths renamed).

**Stage B/C (pour + apply).** 2 GiB rootfs pushed to `/tmp/agupd/` over
USB (5.8 s, device sha == manifest), poured device-local with one
`dd bs=4096 seek=2097153` (7.2 s, 286 MB/s — same bytes as the streamed
two-segment recipe, and agupd pread-hashes the poured body before any
commit). `agupd apply --no-reboot` all green (observed):
```
agupd: running aginxos 761731e 2026-09-04 → applying aginxos 54bf8c3 2026-09-05 to slot _a
agupd: boot: 100663296 bytes → /dev/block/by-name/boot_a (sha256 ok)
agupd: vendor_boot: 35774464 bytes → /dev/block/by-name/vendor_boot_a (sha256 ok)
agupd: state tar staged at 68719476736 (54905856 bytes)
agupd: pre-staged rootfs body verified (2147483648 bytes)
agupd: rootfs swap committed at 8589934592 (len 2147483648, old fs 2040373248)
agboot-ok: slot _a set active on 4 disks — reboots into it; 7 unmarked boots before ABL auto-rolls-back
```
`/bin/reboot2 reboot` (the old line's face; last command of the old era).

**First boot.** Trampoline performed the swap; state tar restored
(wifi.conf → Legrand AP rejoined, dhcp 192.168.0.166, ntpd clock gate,
`/etc/aginx` env+spk-cal+policy). boot.state six lines ok; provision
re-synced the 7 core manifest items (`pkg ok` ~5 min after boot).
Exactly five units ready: aginx-server + aginx-voice + aginxbrowser +
aginx-secretd + net-watch; engines live in `/usr/libexec/aginx/`.

**Receipt.** `scripts/accept/n4.sh`: **53 passed, 0 failed** (D first
boot / E 切净 / F harvest / G second boot). Highlights: bare `aginx agent
send` true brain reply; tool loop answers battery via sys-status; voice
closed-vocab answers 我在 offline and free-text puts the brain reply on
the face; `/usr/bin` has zero `ag`/`ag-*`; no relay/carrier units;
`/home/photos` intact. G: `aginx-reboot` → five units self-start, clock
gate self-healed via the bounded ntpd retries (the #112-successor
net-bringup fix, first fresh-boot proof), send+face pass again.

**End state.** `agupd status`: slot _a, version `aginxos 54bf8c3
2026-09-05`. N4 in service; the first-gen repo is archived as an asset
library (its HARDWARE.md carries the closing entry). Rollback paths
unused: trampoline 32 GiB pre-swap copy intact, slot retry counter
untouched (boots marked ok), `.factory/` untouched.

## N5 — absorption + remote channel + backup line (2026-09-05, observed)

**Artifacts.** `aginxos 6c1ee86 2026-09-05`, rootfs 2147483648 B, sha256
`567773febd1362ffd566c9ca0348e0389839a2da6f1acb12b24fcccbc84a096b`;
boot/vendor_boot reused the in-service pair unchanged (`e2ce2f17…` /
`d80b8098…`). Bundle `out/update-n5/`, manifest signed with
`.local/keys/aginx.key`.

**Pre-gate (updater first).** The fixed `aginx-update` was pushed onto the
running N4 form before any pour; its `status` printed the full
boot-control table — the `aginx-boot-ok` spawn path where the frozen
first-gen binary died. This is now a standing flash-day rule.

**Pour + apply.** Insurance: /etc /home /var/lib tars, dual-side sha,
`.local/backup-n5/`. rootfs pushed over USB 6.8 s (303 MB/s), device sha
== manifest, poured device-local `dd bs=4096 seek=2097153` (7.1 s,
289 MB/s). `aginx-update apply --no-reboot` all green, ending
`aginx-boot-ok: slot _b set active on 4 disks` — the _a→_b flip the
milestone was named for. `aginx-reboot reboot`.

**First boot.** Trampoline swapped; state tar restored (wifi → Legrand,
dhcp 192.168.0.166, ntpd clock gate, /etc/aginx). varlib-migrate done:
`/var/lib/ag/secret/store` and `voiced/vol` re-homed under
`/var/lib/aginx`, old roots gone, seven members present, stamps/done
markers alive (provision skipped re-downloads — `pkg ok` within ~10 min).
Five units self-started; aginx-gateway failed-by-design (no id yet),
restart-looped 5× until infusion.

**Trap 1 — state tar is an overlay.** state-restore extracts the whole
tar over the new rootfs, so the N4 tar's `/etc/aginx/secret.policy` and
`groups.desc` clobbered the N5-baked ones (svc.d/gateway.toml survived
only because the old tar didn't contain those names). Symptom: secretd
denied the gateway's `get relay.primary` every 5 s ("waiting for relay
secret") though the value was in the store. Fixed on device by pushing
the N5 files; class fix = `STATE_TAR_EXCLUDES` in the updater (four
image-owned members excluded, busybox `--exclude` verified on device) —
commit 128420d, effective at the next apply.

**Trap 2 — bake gap.** `n5-qr.jpg` sat in the recipe but build-rootfs.sh
never installed it (first suite run failed the QR decode with ENOENT).
Fixture pushed to `/usr/share/aginx/`; install line added (128420d).

**Identity + remote channel.** `AGINX_GATEWAY_ID` appended to
/etc/aginx/env via stdin (never echoed); `relay.primary` poured into the
sidecar via stdin; `aginx-svc restart aginx-gateway` →
`registered id=cf49973e url=agent://cf49973e.relay.aginx.net`, 8443
ESTABLISHED (/proc/net/tcp `:20FB 01`). Host side had a stale first-gen
agc token for this device id (`owner·mac-path-probe`) — `--logout`
cleared it; then the true roundtrip: reply 「AginxOS 是一台有自我的机器
操作系统——我 me 就是它的前台…」, negative `-32601: unknown avatar
'不存在的化身'`. **First remote receipt of the N line.**

**Suite.** First run 38/45: 2 device gaps (traps above) + 3 suite
expectation bugs (migrate-log wording; backup filename is `-` not `T`;
secret-get for a probe scope is policy-denied BY DESIGN — now asserted
as a positive) + the stale token + 1 cascade. After fixes: 44/45 (last
one an ERE `\{8\}` vs `{8}` bug — expect_out runs grep -E). **Final:
45 passed, 0 failed** across H migration / I absorption / J backup /
K gateway / L remote / M second boot.

**End state.** `slot _b`, `aginxos 6c1ee86 2026-09-05`, six units ready
(server/voice/browser/secretd/net-watch/gateway), gateway registered
(7 registration lines across the session's reboots — reconnect loop
proven), voice floor alive. Device carries the two trap-file pushes
(policy/groups.desc/n5-qr.jpg — byte-identical to the 128420d recipe);
the on-device updater is the 6c1ee86 build, so the state-tar exclusion
ships with the next flash-day updater push. Rollback paths unused.
