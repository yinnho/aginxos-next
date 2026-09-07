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

## N5b — re-bake, 128420d lineage in service (2026-09-05, observed)

Purpose: N5⑨ left three fixes as hand-pushes on a 6c1ee86 image. This
flash bakes them in, so the image is the single source again and the
STATE_TAR_EXCLUDES class fix takes its first live ride.

**Artifacts.** `aginxos a0257a8 2026-09-05` — the local tree = pushed
128420d (updater excludes + suite fixes + QR install line) + the N5⑨
receipt commit; docs/ never enters the image, so image content is the
128420d lineage exactly, only the version string names the local sha.
rootfs 2147483648 B, sha256 `6e33504a3c0a0ecc2df85e219627ed8f0b48db74b786118effd105d5bf18a3dd`;
boot/vendor_boot reused the in-service pair (`e2ce2f17…` / `d80b8098…`).
Bundle `out/update-n5b/`, signed.

**Sequence.** Updater-first gate: the 128420d-lineage `aginx-update`
pushed onto the running N5 form — `status` printed the full boot table
(state tar capture for THIS flash ran with excludes). Insurance tars
`.local/backup-n5b/` (etc 149K / home 387K / varlib 17.8M gz). Push
(rootfs 24 s over USB this time — transport variance, hash proves the
bytes), device-side triple sha == manifest, pour `dd bs=4096
seek=2097153`, apply `--no-reboot` → **slot _b → _a flip**. Mid-run the
host adb daemon died and swallowed apply's stdout — the boot table
proved the commit anyway; check evidence, not echoes.

**Exclusion verified on the wire, pre-reboot.** The staged state tar at
64 GiB (54,934,528 B) was listed before reboot: secret.policy 0,
groups.desc 0, svc.d 0, gateway.toml 0 — while wifi.conf 1 and env
rides. The overlay trap is dead end-to-end: first boot came up with the
BAKED policy (relay.primary line present), groups.desc with backup, and
the gateway self-registered `id=cf49973e` with **zero hand infusion**
(the id rode /etc/aginx/env through the state tar). n5-qr.jpg decoded
from its baked path.

**Suite.** `n5.sh` **45/45 on the first run** — first flash in the
project's history with zero post-flash fixes: L-section remote roundtrip
(agc, reply mentions AginxOS) + negative avatar, M-section second boot
all green. End state: slot _a `a0257a8`, boot_a succ=1 (rcS marked
success — no try burn), six units ready, pkg ok (stamps survived),
gateway 11 registration lines across the session, 8443 ESTABLISHED.
No rollback paths used; insurance tars untouched.

## M47① — camera line moves house, byte-identical proof (2026-09-05, observed)

The camera trio (`cam-shot.c` + `jpegenc.h` + `raw2jpg.c`) copied from
the first-gen repo into this repo's `rootfs/src/`; `build-rootfs.sh` now
zig-compiles from the local copy. Zero-semantic-change receipt, on
device:

- `cmp` on the moved source: byte-identical. The zig cc binaries differ
  only by the embedded source path (strings diff = 3 path-fragment
  lines, 32 B size delta) — proven path metadata, not code.
- A/B on device (same desk scene, `--stream --rear --frames 3 --jpeg`):
  old build 2 shots rc=0 (34,960 / 34,986 B), new build 2 shots rc=0
  (34,480 / 35,693 B) — same size band, pulled frames show the same
  composition pixel-for-pixel. Frame-to-frame sensor noise dominates the
  spread.

These two frames double as the **M47 quality baseline**: default-mode
rear JPEG is dark (YAVG ~19/255 lineage), gray-green, washed out — the
three defects (横放/不满屏/暗) this milestone exists to fix.

## M47② — black level + gamma land in the pixel chain (2026-09-05, observed)

`campix.h` (host-testable pure pixel library, `campix_test.c` now a
`check.sh` gate) replaces the old chain inside `dump_jpeg`: RAW10 → crop
extract through the LINEAR LUT (bl subtract + renormalize; WB/AEC stats
live there) → gray-world WB → debayer/rotate/scale single pass with the
gamma display LUT at the tail. JPEGs publish by `<path>.tmp` +
rename(2) — mtime-polling readers (term eye) never see half a frame.
New args: `--bl N` `--gamma E` `--rot 0|90|270`. Device receipts:

- **bl pinned at 16.** Near-black scene (default mode exposure, yavg
  print): linear yavg = **1.7** with bl=16 → raw black ≈ 17.6 → residual
  sits inside the 0–2 acceptance band. `--bl 16` stays the default.
- **Domains cross-check.** Same scene boosted (`--gain 16 --dgain 2`):
  old-chain emulation (`--bl 0 --gamma 1`) reports raw yavg **47.9**;
  new chain reports linear yavg **33.5**; conversion
  (47.9−16)×255/239 = **34.0** — the two domains agree to measurement
  noise.
- **对拍 (the defect this step exists for).** Same dark scene, gain
  16/dgain 2: old-look frame (`--bl 0 --gamma 1`) shows the black level
  as a gray-green veil over every dark area; new-look frame (defaults)
  has true-black darks, a correctly colored door-light strip, and no
  green cast — WB gains (r=1.87/b=2.64 at this scene) finally act on a
  blacked base. Frames: `/tmp/m47b-old16.jpg` vs `/tmp/m47b-def16.jpg`
  (host copies).
- Gray path (QR/scan) rides the same LUTs — monotonic map, Bradley's
  adaptive threshold is invariant to it; full camera-QR round-trip
  receipt lands with M47⑤.
- host: `check.sh` all green incl. new campix gate
  (luts/extract/wb/debayer-rot-scale/crop).

## M47③ — AEC live: op7 per-frame exposure rides the ring (2026-09-05, observed)

`sensor_update()` (op_code 7 + 7 I2C writes, the sensor_nop family)
carries CIT/gain/dgain per queued request; the ladder state machine
drives linear-domain yavg at ~50 with one rung per 2 frames, damping
included. `aec.state` persists across launches (stale >10 min or wrong
slot -> discard); bracket mode (`--frames 3 --aec`) pre-sets gain
1x/4x/16x and picks the frame nearest target. Device receipts:
heartbeat line `(rung N trim X yavg Y)` tracks scene changes rung by
rung; covered-lens floor holds yavg 3.1 at the top rung without
oscillation.

## M47⑤g/h — Google's own CCMs + two desats land the color (2026-09-05, observed)

CCMs extracted from the device's own vendor image (redfin U1B2 factory
`/lib64/camera/com.google.ghawb.tuning.imx363.so`, float32 scan, rows
sum 1.0 — TL84 green row 0.983 deliberate): D65/TL84/INC picked from
the gray-world WB via warmth = wb_b/wb_r, piecewise. Around them:

- **WB table clamp 1020 under a CCM** (legacy 255 bit-exact): a
  per-channel clamp before the matrix destroys the ratio the matrix
  needs.
- **highlight desat** symmetric 255/max: a bare per-channel clamp on
  wb b=1.5 leaves (71,109,255) — visibly greener than the correct
  (59,91,255). Hue survives, highlights wash toward white.
- **shadow desat** (knee 20, CCM mode only): the Google matrices'
  -0.49 G cross-terms drive the R row negative on G-dominant shadow
  noise; measured darks G-R +13 -> +23 the moment a CCM turned on
  before this fix. Blends toward the pixel's own luma — blacks stay
  black.
- **emissive soft-weight 0.5** in cp_wb_measure: a green-terminal
  monitor as the room's only source dragged global means until white
  cables rendered sage (cable sensor (85,108,53), midtones 14% green);
  HARD exclusion of quads >=200 overshot to magenta (screen band
  G-R -49). Half-weight lands between emitter and reflector points,
  no region more than ~10 off neutral — what a phone renders in a
  monitor-lit room.

Host gates pin all of it: campix_test hand-values for every matrix
path, and the differential harness (git-HEAD vs current, 5000 trials)
holds bit-exact in the legacy ccm==NULL path.

## M47⑤i — placement + cpufreq: the fps regression root-caused and closed (2026-09-05, observed)

Burn probe `/data/local/tmp/thprobe2` (1e9-iter pinned register burn,
idle machine):

- **scaling_max_freq is silently clamped and boot-baked**: writes
  return rc=0 but read back capped (A55 policy 1.363 GHz vs cpuinfo
  1.805; cpu6 1.478 vs 2.208; cpu7 1.766 vs 2.4). No LMh IRQ, no
  cmdline cap, no userspace writer, no recovery after 9 min idle.
  Uncapping is impossible; the attempt was deleted.
- **scaling_min_freq writes stick** (readback-verified, restored on
  teardown; SIGKILL leaks until reboot).
- **Core classes under the caps**: cpu0 (A55) 1.47 s, cpu6 0.68 s,
  cpu7 (prime) 0.57 s. A 4-thread A55 fan-out runs SLOWER than one
  A55 alone (2.21 s — little-cluster derate). Floating single thread
  1.13 s. Dual-big bimodal: 0.68 s at full capped freq (sampled
  1478400/1766400 mid-burn) or ~1.35 s (transient droop).
- **THE decisive probe, run DURING streaming**: cpu6 held 0.68 s
  (full speed, unchanged) while cpu7 ran 1.13 s = exactly 2x — and
  /proc/stat showed cpu7 100% non-idle from the chain itself. The
  prime degrades under sustained streaming load (current limit at
  1.766 GHz); cpu6 never does.
- Camera IRQs (cpas-cdm, csid-lite, ife-lite, msm_drm) all land on
  cpu0.

Streaming config matrix (fps / extract / debayer ms, same scene):

| config | fps | extract | debayer |
|---|---|---|---|
| floating threads (regression) | 24.6 | 8.7 | 21.7 |
| pin mask{6,7} + floors + th2 | 35.4 | 3.7 | 17.4 |
| + cache-blocked rotated walk | 35.7-36.4 | 3.7 | 16.7 |
| explicit pins th2 (main7/worker6) | 22.9 | 6.4 | 30.2 |
| explicit pins th1 (main7) | 25.0 | 6.0 | 27.0 |
| **mask{6,7}, unpinned threads, th2 (SHIPPED)** | **36.4-36.8** | **3.7** | **16.7** |

Conclusion, now in code comments: the process pins to the {cpu6,cpu7}
MASK with threads left unpinned — the scheduler routes around the
intermittently-drooping prime in real time; every FIXED assignment
measured 27-30 ms debayer. Don't re-add per-thread pins. The
cache-blocked rotated walk (CP_ROT_GROUP=8 staging) is bit-exact
(campix_test + differential harness) and perf-neutral — kept, it is
structurally right for L1. Debayer is compute-bound (~42 cyc/px), not
memory-bound.

## M47⑤k — preview look (NR/tone/sat/sharpen) shipped + perf closed (2026-09-05/06, observed)

Three ports, all credited in campix.h, all host-pinned by campix_test:
- **hqdn3d** (ffmpeg vf_hqdn3d, GPL-2.0+): temporal-default ls0/lt12,
  banded == whole-plane bit-exact; spatial off (line_ant chains rows —
  cannot band, costs too much single-threaded).
- **contrast stretch** (RPi rpi/contrast.cpp, BSD-2): quantile knots
  q01/median-pinned/q95, mapped through gamma — drops in as the display
  LUT, zero per-pixel cost.
- **saturation + sharpen** (RPi semantics): sat Q8 294; sharpen
  3x3-unsharp threshold/strength/limit with row-band parallelism.

Perf campaign (same dark scene, yavg 6-9, AEC rung 0, th2 mask{6,7}):

| config | fps | post-pass ms |
|---|---|---|
| look full (NR+tone+sat+sharpen live) | 12.9 | 29 |
| **look live (NR+tone+sat; sharpen stills-only)** | **26.2-27.4** | **5.4-5.9** |
| look off | 35.0-35.8 | ~0 |
| -O3 -mcpu=cortex_a76 | 26.2 (no gain, reverted) | — |

Chain at 26.2 fps: extract 3.7 / wb 4.0 / debayer 21.6 / post 5.5 /
raw 1.3 / enc 7.8 (amortized x2/30 frames).

Laws discovered on device:
- **Sharpen is stills-only.** The live 565 frame gets a 1.5x
  nearest-neighbor upscale in term — sharpening BEFORE that upscale is
  half-eaten by the stretch, and its ~8.3 ms/frame scalar cost was 40%
  of the chain budget. JPEG stills keep the full look (sharpen folds
  into their band walk). Live rides NR+tone+sat.
- **The post pass is compute-bound scalar uop count** (stride-3
  interleaved walks + LUT gathers); -O3/A76 tuning bought zero — LLVM
  won't vectorize these loops. Static grow-only plane caches killed the
  musl mallocng per-frame mmap churn (look-off 24.3 -> 35.4 fps).
- **th4/A55 killed by arithmetic**: even row-split makes A55 spans the
  critical path (regression); weighted split gains nothing (both A76
  saturated). SM7250 is 1+1+6 (cpu0-5 all A55 max 1804800 — probed),
  not 2+2+4 — only 2 big cores exist.
- **The parser segfault that ate a night**: unknown flags silently
  became `only_slot = atoi(flag-arg)` — a `--vf-frames 3` typo fed
  slots[3] and crashed the stream banner's first deref. Not a code bug
  in the optimization; the parser now errors on typo'd flags (strict
  bare-digit tail).

Visual A/B (same scene JPEGs, look-on 381832 B vs look-off 255619 B,
vision-model judged): look-on has NO green/magenta cast (⑤j chain
holding), look-off shadows show green/magenta speckle; look-on residual
noise gets amplified by sharpen's fixed threshold 4 (a bright-light
value — RPi's noiseFactor threshold scaling is the known follow-up if
the user flags dark-scene grain; deferred for the user's daylight
judgment first).

## M42c step 1 — command-first protocol + one-glance pair bootstrapping (2026-09-06, observed)

The 2026-09-05 product law killed the scan→ordinal→spell-password→confirm
flow. First step landed on device today (dev push; bake #19 pending —
`/usr/bin/aginx-voice` and `/usr/bin/aginx-term` are ahead of the image,
aginx-voice restarted onto the new protocol, six units ready after).

What is on the device now:
- **Protocol**: single idle state (no dwell states). 「连网」 = check wlan0
  IP first → try remembered wifi.conf → only then open the eye for a code.
  Pair codes (AGINXPAIR1, superset of WIFI:) beat WIFI: codes; both
  auto-act (pull-style, no readback). 取消 closes the viewfinder
  (Act::EyeClose); the eye itself (Act::Eye) is protocol-born now —
  NetState::ConfFail/NoConf opens it without asking.
- **PairApply chain** (daemon): join_wifi → identity 3-key merge into
  /etc/aginx/env (0600 tmp+rename, HOME preserved, svc re-reads on spawn)
  → quick clock (ntpd 2×10s alternating, `date +%Y≥2026` gate, non-fatal)
  → `aginx-svc restart aginx-gateway` + `aginx-server` with ready re-check
  (failed/breaker units restartable, M42e receipt). aginx-voice is never
  restarted by itself (self-kill).
- **Minting tool** (host-only, crates/pair): `aginx-pair` emits JPEG (the
  device `aginx-qr` only decodes JPEG — PNG would have been dead on
  arrival), tests round-trip through the device decode chain.

Suite `scripts/accept/m42c.sh`: **15/15 PASS** on the in-service device —
fixture pair minted on host (stdout echoes none of the five fields), pushed,
device aginx-qr decoded the exact five-segment payload back; `--inject 你好/
状态/连网` all answer from the new protocol on a connected device (连网
returns 网已连 without touching wifi.conf); face doc is the new schema (no
list/psk keys, hint 「按住音量下说话 · 音量+对码」, state stays "idle").

Deliberately NOT in this step: the real pair receipt (fresh boot, no adb,
human holds the code → gateway registers to relay) — that is the #198
human product receipt, after bake #19. ASR n-best tap-to-correct is phase
two (ag-asr source is not in this repo).

## 2026-09-06 — M47⑤p 取景黑帧根因：CCI 瞬态幻步 + AEC 毒态落盘（两护栏）

症状链复盘（用户判「一片一片/颗粒」的那帧）：pull 下来的
/run/aginx-voice/eye.raw 近乎全黑；aec.state = `0 0 1.000 0.3`。rung 0 =
梯子顶（CIT cap 1642 + 16x 模拟 + 2x 数字 = 32x），真暗房在这个配置下也
meter >5 —— yavg 0.3 是**曝光写没落地**的特���，不是光照事实。

根因：此前实验 kill 掉 cam-shot 实例引发 CCI I2C 瞬态（已知自愈几分钟）；
瞬态窗内的曝光寄存器写静默 NACK（ioctl 成功、I2C 失败），AEC 状态机照常
记账 5→1→0 的「幻步」，停在 rung 0，把 0.3 当作场景亮度写进 aec.state。
毒态落盘 → 后续会话冷启继承。

**探针收据（同房间、健康链路）**：手动跑 daemon 同参 cam-shot，冷启
rung 5（yavg 4.3）→ rung 1 落地 38.7（精确 9x）→ rung 0 落地 78.4 →
fine trim 0.83 落地 64.6，带内收敛，13.8 fps；op-1 更新链、增益寄存器、
CIT trim 全部在役。probe.jpg（84389 B）目视：曝光正常、无色块、无颗粒。
**黑帧是毒会话，画质链本身没病。**

两护栏（cam-shot.c ⑤p，部署 /usr/bin/aginx-cam-shot 3502256 B）：
1. teardown 不落黑态：`rung==0 && last_y<5.0` 时跳过 aec_state_write——
   下次会话走 rung-5 冷下降，每一步都被 yavg 重新验证。
2. 梯子尽头周期重发：park 在 0 或 RUNGS-1 且 ratio>8 时每 2*window 帧
   重发同 rung 更新——会话中途瞬态自愈后曝光能落地，无需重开眼。

复验：新二进制 14 s 探针 191 帧（13.6 fps）收敛如常（rung 0 trim 0.83
yavg 62），teardown 正常落盘 `0 0 0.828 62.0`——下次开眼首帧即正确亮度。

**⑤p 归因更正（2026-09-06，用户证词）**：当晚手机是**扣在床上**的——镜头
被床品遮死，yavg 0.3 是真实光照，不是毒态。「CCI 瞬态 → 幻步 → 黑帧」这条
因果链没有成立证据，特此更正。两护栏本身保留（teardown 不落黑态、梯子尽头
重发仍是正确的防御性设计，无副作用）。

## 2026-09-06 — M47⑤q 取景左右分离根修 — 回收移到链后（读完才还槽）

**根因（源码证明）**：dump_jpeg 的 extract 阶段是 slot 缓冲的**唯一读者**
（memcpy 把 2016×930 crop 暂存进缓存；debayer/box/post/encode 全跑在私有
平面 g/plane5f/px5/rgb 上）。传感器自由跑 ~60fps，主循环只有链速
~13.4fps——8 深的 request 队列被永久饿穿，刚入队的 request 在**下一个
SOF**（≤16ms）就执行：extract 还在读的 slot，驱动已经开始覆盖——左右两半
各来自不同帧，即用户看到的取景左右分离。

**修法（M41c 持帧纪律同法理）**：ring 回收从「publish 前」移到「publish
之后」——slot 只在 extract 读完最后一次后才还给管线。帧率、AEC、publish
语义零改动。时序收据（⑤q 构建）：extract 3.6 / wb 3.8 / debayer+box 44.5 /
post 17.8 / raw write 2.5 / jpeg enc 6.1 ms，~13.4fps 循环。

**用户判定（部署后真用）**：「除了有点卡，卡的时候会出现红色大圈，不卡的
时候都挺好的，感觉方向对了」——左右分离已愈；卡与红圈转 ⑤r/⑤s。

## 2026-09-06 — M47⑤r 取景卡根修 — cam-shot PDEATHSIG（voice 死亡不再孤儿占机）

**症状与日志复盘**：取景中每隔一阵卡死数秒。用户整晚
/var/log/aginx-svc/aginx-voice.log：卡顿窗内 voice「up」重启 7 次（svc
拉起），每次都伴随 `eye spawn ... exit 2` + `eye stuck frame, respawn`
三连。

**根因（kill 复现，逐字命中日志签名）**：voice 死亡时其 `--forever`
cam-shot 被 re-parent 给 init（PPid 1），继续向 eye.raw 流帧、**无限期占住
相机节点**（voice 无信号处理器；Rust `Child` 的 Drop 不杀子进程）。复现：
开眼中 `kill -9 <voice>` → 孤儿 cam-shot 继续 mtime 跳动；svc 秒级拉起新
voice，VolUp 再开眼 → 新 cam-shot `exit 2`（setup ioctl 失败，节点被占）
→ 10s 首帧 stuck 预算 → respawn ×3 = 用户看到的卡。

**修法（cam-shot 侧，对任何 spawner 都成立）**：run_stream 开头
`prctl(PR_SET_PDEATHSIG, SIGTERM)`——父死即收到我们已优雅处理的 SIGTERM
（forever 模式 on_stop 正常 STREAMOFF + aec.state 落盘）；外加
`getppid()==1` 竞态护栏（prctl 前父已死则拒绝碰硬件）。

**部署收据（新二进制，多轮）**：开眼 cam=13087 流帧正常 → `kill -9` voice
→ 3s 后 `cam=[]`（孤儿随父死）→ voice 回来重开眼即干净（cam=13356 流帧
正常），日志 delta 仅一条「up」——零 exit 2、零 stuck。

## 2026-09-06 — M47⑤s 红色大圈根修 — fringe 门两域中性（自发光晕不再被 CCM 染红）

**先定位**：term 的眼渲染（blit_eye_raw + upscale565 直写 DRM 后缓冲）是
纯重采样，画不出圆——红圈必然在 eye.raw 内容里，即 cam-shot 像素链。

**根因（真实 campix 链在 host 完整复现，/tmp/circle_sim）**：⑤j fringe 门
只测 **WB 后**中性比——循环论证。自发光晕（屏幕光晕）本身是中性像素，
我们的 CT 增益恰在 CT 偏离时把它染成非中性（wb 1.43/1/2.58 下
(200,200,200) → WB 后 286/200/514，比值 2.57）→ 门判「有色」放行 →
CCM（色度放大 ~1.4×）+ sat 1.15 → 一整圈饱和品红-红环（模拟晕带修前
(250,120,255) 级别）。暗房怼亮屏（20cm QR 场景）是最坏 case；每次
cam-shot 重启首帧用未平滑 CT（cp_ct_smooth_init 每进程重置）+ AEC rung-0
过曝把晕拉宽——与「卡的时候出红圈」完全同相。

**修法（两域中性律）**：fringe 门改为「**任一域**中性即塌缩」：
`(输入域 mxi*2<=mni*3 || WB 域 mxw*2<=mnw*3)`，亮度触发仍看 WB 后（伪影
所在）。反射灰（被光源染色、WB 后中性——⑤j 设备晕 raw 158/211/86 →
226/211/222）走 WB 域分支得救；自发光晕（输入即中性、被增益染色）走输入
域分支得救；真彩色内容（两域比值都 >1.5，如红 LED 255/120/90）保色。

**收据**：campix_test 全绿（含新增 ⑤s 回归向量；旧向量 200/220/255 与
200/220/240 本就是输入近中性、改判塌缩是正确行为，换成真彩色向量
150/210/255 与 120/220/240 并手算期望）；circle_sim 修后晕带
(243,243,243)→(255,255,255) 无环；部署后（3505208 B）流帧回归正常，
拉帧目检无全局色偏、无色环（暗房天花板景，灰面无色）。最坏 case（近距
亮屏）留待用户真用收据。

### 2026-09-06 场收据杂项

- **adb push 剥执行位**：推二进制到 /usr/bin 后必须 `chmod +x`，否则
  spawn 直接 EACCES（os error 13）。
- **拼接 pidof 输出会误配**：两条 pidof 连打的输出肉眼常读错行——用
  /proc/PID/status 的 Name/PPid 验证（本场又中招一次）。
- **vol_up.bin 单发不开眼**：裸 release（ev 1,115,0）不算按键，必须
  press（vol_down.bin）+ ~0.15s + release 完整一对。
- **`stat -c %y` 有纳秒精度**——同秒流帧活性判定用它。
- **voice 整晚 7 次 up 重启**：svc 秒级拉起；个体死因未定位（无 OOM/
  panic 记录）。⑤r 之后后果已被包住，死因另立案、不阻塞。
- **kmsg CRM "Watchdog timer exited already" 刷屏**：流帧稳态 ~284 条/s，
  疑似无害、代价未量化，记录在案。

## 2026-09-06 — M47⑤t 取景启动+节奏根修：冷启状态恢复 + th3 + 系统分核

用户证词（⑤q 部署后真用）：「一开始还是会有，然后很快就好了，但是还是
不丝滑，感觉很卡」——两个独立病灶：开眼瞬间的亮度爬坡 + 旧会话残帧闪现；
以及稳态节奏抖。

**病灶一（冷启爬坡，捕获复现）**：rm aec.state 后冷启，3 个亮度平台
~1.2s（f02 r44/g52/b45 暗 → f03-06 r101 → f07+ r150 稳态）——旧的
「>10 分钟陈旧态丢弃」守卫把上一会话的收敛成果全扔了，每次都从梯子顶
(rung 5) 走一遍。f00/f01 里的 r143 旧亮度 = **上一次会话的残帧**还留在
eye.raw 文件里（poll_eye 在眼关时把 raw_mtime 复位 None，开眼首询把旧
mtime 误判为新鲜帧直接 blit——上一场景闪现 ~0.5-1s）。

修法（cam-shot.c + term main.rs）：
1. **删陈旧态丢弃**：aec.state 是起点不是真理——aec_cfg_init 把 rung 折
   进配置时曝光，梯子随后用实测 yavg 逐帧复验，错了按常速走梯，永不更
   坏。/run tmpfs 天然限界到本次开机。⑤p 黑态不落盘（写侧护栏）保留。
2. **`g_vf_threads` 2→3**：debayer 是访存延迟受限（⑤i），第 3 线程填
   cpu6 停顿泡而 cpu7 降频时依然有产出——≥100ms 帧占比 48%→4%，帧时
   摆动 ±25ms→stdev ~4ms（fps 10.98→11.14，赢在分布不是吞吐）。th4
   重新摊平分布丢模式；A55 扇出降速（⑤k 律）。
3. **term 停入 {0..5}**：眼流期间 sched_setaffinity 钉小核（term 的
   565→888+双线性放大 ≈ 37% 个大核，⑤i 探针），眼停全掩码归还。**钩子
   跟着眼标志走而不是视图模式**——第一版挂在 poll_eye 里（仅 Voice 态
   轮询）泄漏：音量+ 由 voice 守护处理与视图无关，Voice 态开眼后按
   BACK 退启动器，流还在但 poll_eye 不再跑，term 永久钉死小核。现挂主
   循环按 face doc 的 eye 标志翻（face 轮询因此改为全模式，渲染/防灭
   屏仍限 Voice 态）。
4. **aginx-qr 自钉 {0..5}**：2Hz 解码爆发（100-300ms）此前无掩码落在大
   核对上。A55 上慢 ~2.5×，但链零代价。

**在役收据（2026-09-06，term 在 Launcher 态全程未触屏）**：
- 掩码三段：term 0-7 → 眼开 **0-5** → 眼关 **0-7**；cam 6-7（th3 活）；
  qr 0-5，流中 3 次后台解码全命中（WIFI fixture 载荷）且节奏循环中无
  可归因 hitch。
- 节奏（700 次 stat 纳秒采样，~73 帧）：中位 93ms / stdev 3.9-4.1ms /
  ≥100ms 帧 3/73（4%）——对照 ⑤t 前基线 48%/±25ms。诚实注记：term 两
  轮都在 Launcher 态没做逐帧 blit，此分布证明 th3+qr 自钉；term blit 上
  A55 的真人手感留用户判。
- 冷启走线：t5（12 分钟陈旧 rung-1 态恢复）与 t6（新鲜 rung-0 态），
  同方法采样，**首帧即稳态亮度**（99/99/81 与 82/86/70 全程平）——
  对照 ⑤t 前三平台爬坡，同样的采样法当时清清楚楚拍到 r44→101→150。
- 关眼干净：cam 进程退净无孤儿，aec.state 正常落盘（t6: `0 0 1.000
  24.6`）。
- 残帧闪现的渲染门（eye_open 时间戳拒旧帧）是代码级修复，可见症状待
  用户下次真用确认。

## 2026-09-06 — M47⑤u 取景 NR 关停（voice 层）+ 子进程日志可见性

用户判词复盘（⑤t 部署后真用）：「开眼还是卡…只是可怜感严重」+「看看修
颗粒感之前的代码」。方向报告 + 旗标 A/B 定案：**可怜感 = 颗粒修复的价**
——⑤o 全分辨率 demosaic+面积均值把 debayer 从 21.6ms 抬到 ~60ms，⑤m 整
面空间 NR 再加 17.2ms，fps 26.2→10.9（p3 探针：强迫曝光亮/暗同 fps =
链速上限，非 AEC）。同旗标 A/B：`--nr 0:0:0:0` → 13.6fps（post 17.8→
1.4ms）；`--nr 0:0:0:0 --tone 0 --sat 1.0` → 15.1fps。

**修法（voice 层两处，cam-shot 零改动）**：`eye_spawn` 参数加
`--nr 0:0:0:0`——⑤o 面积均值已结构性砍颗粒（√1.29×），NR 在其上是纯
开销，取景关、出片仍全 look；子进程 stdout/stderr 从 `Stdio::null` 改落
`/run/aginx-voice/cam.log`（每次开眼截断一份，stderr 挂 stdout 的 dup 共
享偏移 = 2>&1 语义，开文件失败退回 null）。**真实会话第一次可见**
aec 走线与 vf: 链路心跳——此前每次诊断都要手工重跑同参 cam-shot。

**在役收据（dev push /usr/bin/aginx-voice 4a7101d6，aginx-svc restart，
六单元 ready）**：
- 旗标生效：cam.log 首行 `look: nr off 0:0:0:0 tone 1.08 sat 1.15 …`；
  链路行 post **0.8-1.1ms**（⑤t 在役 17.8ms）。
- fps：新鲜会话 **14.2-14.3**（在役 ⑤t 10.98；A/B 预测 13.6，真路径更好）。
  debayer+box 56ms 原样（⑤o 的价，下一步杠杆）。
- 节奏（34 个帧间隔）：中位 **73.3ms** / stdev 4.5ms / max 87ms /
  **≥100ms 帧 0/34**——对照 ⑤t 在役 93ms 中位、4% ≥100ms。
- 可见性即收据：重开会话首行 `aec: start rung 0 … from aec.state`，
  **frame 1 yavg 74.2 带内**——⑤t 的状态恢复第一次在真实���径拍到；
  teardown `vf: stop requested after 62 frames` + aec.state 落盘正常。
- ⑤r 回归随新 spawn 结构保持：眼开中 `kill -9 voice` → 3s 内 cam=[]
  （PDEATHSIG），svc 拉起 voice，重开眼即干净流（无 exit 2）。
- 注记一：紧接的第二会话 fps 掉 11.4、debayer+box 70-80ms——⑤i 已知
  大核降频（连续多会话热/电流），链路行首次把这事拍在真实路径上。
- 注记二：cam.log 混入已知 CRM watchdog kmsg 刷屏（~83% 行数，
  ~1.25MB/30s 会话，tmpfs 每开眼截断）——读日志 `grep -av kmsg`；
  未动 cam-shot（并行会话在役）。

颗粒真眼判定归用户（NR 关停在暗房是否带回颗粒 = ⑤o 结构性削减是否
足够）；若可怜感仍在，下一杠杆 = 方向2 把面积均值折进 demosaic
（56ms → ~30ms 段）。

## 2026-09-06 — boot 卡死 modem 步：cam_sensor_vsync_dev 内核 BUG → cpu5 楔死（收据）

**现象**：开机 bootcard 停在 modem 行不动（touch/camera/battery 已 ok，
modem/wlan/audio/net 行不出），无 wlan0，用户看到的是 handoff 5 分钟超时
兜底拉起的 aginx-term（boot.state 无 done 行时 aginx-term-handoff 150×2s
超时后照样杀 bootcard 起 term——所以屏幕活着但没网）。

**根因链（/var/cpu5-wedge-forensics.txt 646 行全档，kmsg-follow.log 每
boot 重建）**：
1. camera-bringup 照常加载 cam_sensor_vsync_pb + cam_sensor_vsync_dev（
   boot 后 ~30s）。载入瞬间撞 race：CAM_WARN "Watchdog timer exited
   already"（cam_req_mgr CRM）在前，`cam_vsync_qmi_work` 随即
   `list_add corruption. next/prev is NULL` → `kernel BUG at
   lib/list_debug.c:26`——每 63s 一发（CRM watchdog 周期），首发 kmsg
   +35.2s，全程 24 发。同栈 2026-08-31 以来几十次 boot 首见楔死 →
   race 性，非确定。
2. kworker/u16:5（pid 290）卡在 cam_vsync_qmi_work 里自旋，cpu5 永不
   让出（sched_debug：curr->pid 恒 290，cpu_load[0..4] 全 5121，
   arch_timer 风暴 ~300-500/s）。cpu5 的 cpuhp/5、migration/5、
   kworker/5:* 全饿死（R 态停 __switch_to）。
3. radio-bringup 走到 `insmod wlan.ko`（+46.8s）→ qdf_cpuhp_init →
   cpuhp_issue_call 等 cpu5 → D 态永塞（pid 2380）；audio-bringup 的
   msm_pm 同塞在 lpm_probe → cma_alloc → drain_all_pages（pid 2518）。
4. radio-bringup 不出 `modem ok`/`wlan ok` 行 → bootcard 永停 modem 步。

**救机**：现场快照进持久 /var 后 `/usr/bin/aginx-reboot reboot`（shutdown
慢但走完了，>25s 才掉 adb）。重启后一切正常：modem ok / wlan ok wlan0 /
audio ok，`list_add corruption` 与 cam_vsync 在新 boot kmsg **零出现**，
相机照常取景（用户开眼 409 帧正常）。唯一遗留：wifi join rc=2——
scan 里没有 'Legrand AP'（AP 掐了，2026-09-02 同款），net-watch 每 ~45s
自动 rejoin，AP 回空中即自愈。

**busybox 新坑（awk/netstat/diff 之后第四弹）**：grep 的 BRE `\|` 交替
不生效（`grep "a\|b"` 空输出）——一律 `grep -aE "a|b"`。

**防御候选（未动手，等点头）**：camera-bringup 里去掉
cam_sensor_vsync_pb/cam_sensor_vsync_dev（Google sensor-vsync→QMI 提示
路径，我们无 SLPI/QMI 服务在跑，从未消费其符号）——动手前需验证无其他
已载模块 import 其符号。

## 2026-09-06 — #227 M47-AF 裁决：LC898129 伺服闭环证实 + 夜景光学 A/B 全部无效（收据）

**问**：后摄 imx363 从不自动聚焦。假设链：stock HAL 走的 OIS-subdev INIT 我们
没做 + 0xF01A 是不是 AF target 未证。

**OIS INIT（新 --ois 面，cam-shot additive）**：in-stream rc==0 首次——kmsg
`CAM-OIS: cam_ois_power_up: 142 Using default power settings`（DT 无电源表，
默认 {SENSOR_VAF, CAM_VAF, 1, 2ms}）+ `qcom,ois ac4a000... Linked as a
consumer to regulator.9`，users 1→2，slave-info latched，init_settings
ACK。standalone -110 复盘：LC898129 CORE 骑在 sensor 轨上（slots[0] DT 无
SENSOR_VAF），只开 cam_vaf 时核心死，写必超时。**INIT 不是 AF 不动的根因**
（对照组无 --ois 照样 ACK）。

**裁决实验（三次迭代，前两次全部作废）**：
1. 梯度 A/B（0x000 vs 0x3ff，pin 曝光增益）：今晚四组 A/B/C/D/E/F/E2/F2
   里目标效应 0.1–1.3，同目标批间漂移 2.5–3.9，**漂移吞掉效应** → 无可
   复现光学差。作废原因后来才看清：夜景帧 mean≈40-52/255、std≈53，
   梯度能量≈噪声地板；场景无细节时聚焦变化不产生梯度差。
2. cit 1600→6400 想拉出噪声坑：mean 39→52（+33%≠+300%），**帧长钳制**。
3. 真正的裁决换了量具——0x0538 连读（DWORD=小端 float32）：

| target 0xF01A | LOP float (0x0538 连读) | (code−512)/256 |
|---|---|---|
| 0x000 | -2.03/-1.97/-2.06/-2.00（G）、-2.29..-1.77（J） | -2.000 |
| 0x200 | 全部 ≈0（denormal 级） | 0.000 |
| 0x3ff | +1.90/+1.83/+2.02/+1.60 | +1.996 |

   **线性、三点定标、±0.2-0.3 采样抖动**——纯命令回声会精确回读不会抖，
   这是 hall/滤波后的实测位置：**LC898129 AF 伺服环在咬 0xF01A，透镜
   载带有真实位移，电气+固件链全通**。邻域：0x0534≈0x0538≈0x053c（三
   个滤波阶段），0x0530=0。0x0538 单次读与目标无关的旧结论（A/B/C/D
   各不同）是 float 没解码 + 单采样噪声。
   附带翻案：今晚早些"lens came alive between 17:25 and now"是批间漂移
   幻觉，17:25 的 afx A/B「完全相同」同样只是噪声地板上无细节。

**根因改判**：AF 硬件活的，「不聚焦」= **我们栈里从来没人做对焦扫描**
（stock 才做 AF sweep；cam-shot 的 --af-sweep 是老的 cci0 偶地址探针，
同名不同物）。夜景下手写 0x000/0x3ff 光学验证无效，白天/照明下待做
最终光学复验。

**工具收据**：--ois / --af-write :32 / --af-read×4（上限 4，重复同址合法）
全上机；--af-read 单次 DWORD 小端拼 float。

### #227 补收据（同夜）：--af-scan 面上机，扫描机器全链通

`aginx-cam-shot --stream 0 --rear --frames 105 --cit 1600 --gain 16
--dgain 2 --af-scan --out /tmp/ois.raw` → rc 0。粗扫 16 步×68 code
（0..0x3fc）+ 细扫 9 步（峰±32、步 8）全部**流中 op2 MANUAL_MOVE_LENS
写 ACK**（SKIP=2 弃、MEAS=2 均值），末步回写最佳并打印
`af: FOCUS code=0x3b8 sharp=83.37 (step-0 81.53, +2%)`——25 步 DAC 阶梯
+ 收尾回写一气呵成，机器证明。锐度=中心 50% 裁剪 |梯度| x+y 池化
（RAW8，与 host python 量具同几何）。

阶梯读数 80.85–83.37 全程平（±1.5%）＝噪声地板上的预期形状——
夜景无细节场景光学峰不可分辨，**量具已备好，等白天/照明+细节场景
做最终光学复验**。

两个疑点均闭案，非本 run 所为：
- kmsg `[OISFW]:RamWrite32A/RamRead32A sid:0x3b` -110 刷屏＝**旧环回放**
  ——本 run 首次 kmsg dump 回放了整环（31 条 START_DEV，今晚全部
  run），OISFW 段落在早前 --ois run 的 FW 下载位置；本 run 自己的窗口
  （actuator INIT 之后）零 OISFW/CCI 错。本 run 无 --ois。
- 本 run 新增 kmsg 只有老的 RDI EPOCH `-14` + 一条
  `cam_actuator_update_req_mgr: Can't add Request ID: 0 to CRM`
  （首个 op2 与流 epoch 竞速，阶梯照走完，良性观察项）。
- pid 452 一度被误读为孤儿 cam-shot：实为短命 aginx-voice（⑤r 线的
  病，不占相机），复查时已退出，与本案无关。教训：adb 拼接输出
  `pidof A; pidof B` 要带标签读，别按行猜归属。

## 2026-09-07 — 开机体验⑤ 语音侧上机：接线员分流 + 英文话术 + term 直进 Voice（收据）

aginx-voice / aginx-term musl 推 /usr/bin（chmod 755，agsvc 监督），重启验收：

- **英文 TTS 首条收据**：`/var/bin/aginx-tts "Operator. Go ahead."
  /tmp/en-test.wav` → exit 0，113888 字节（≈3.5s）。melo
  vits-melo-tts-zh_en 出英文整句——此前零收据的风险点闭案。
- **接线员分支（已连网）**：`aginx-svc restart aginx-voice` 两次，每次
  face 文档 `lines=[["Operator. Go ahead."]]`、`eye=false`、state idle；
  日志增量（offset 对齐）只有一行 `up (local=true, brain=true, …)`——
  铃（play_ring）与 speak 无任何错误行；`/run/boot.state` 含
  `wifi ok`（判 Up 分支的输入）。铃+英文问候扬声器出声两次（用户已人耳确认：铃与问候均听到——2026-09-07）。
- **term 直进 Voice**：kill 旧 term，handoff 常驻环 2s 内以新二进制
  重启（pid 1577），无崩溃环；开机默认 Mode::Voice 生效路径 =
  face 文档渲染（voice 侧收据）+ 常驻环存活。
- **未收**：断网分支（英文警告+自动睁眼）——现网 wifi.conf 不可毁，
  主机 34/34 测试覆盖，fresh-boot 留 bake #19；bootcard Matrix 雨/
  END——initramfs 每次 boot 重建，同炉 bake #19。

## 2026-09-07 — P0 视口帧流真机首收（aginxbrowser main 9cffa34，#235 后续）

交接文档 ~/Documents/aginx/aginxbrowser-p0-viewport-stream-done.md（M 系列 Mac
实测）。本日 fresh clone main（9cffa34，v0.2.8 之后未发版）→ macOS cargo-zigbuild
aarch64-musl --features screenshot（85MB 静态）→ svcd 单元换血重拉。

**构建坑（host）**：rusty_v8 build script 走 github CDN 下载 librusty_v8.a.gz
黑洞（ESTABLISHED 但零字节 10min+）。解法＝~/.cache/rusty_v8 已有 8月29 件
（v150.4.0 同版），直接预填 target/{,aarch64-unknown-linux-musl/}release/gn_out/
obj/librusty_v8.a + 同 URL 的 .sum → 跳过下载。

**设备坑**：aginxbrowser 是 agsvc 单元——裸 kill 会被监督者用旧二进制抢先重拉
（实测：新引擎 Address in use 死掉，bench 打了一轮 v0.2.8 假数据）。正路：
换好二进制后 `/usr/bin/aginx-svc restart aginxbrowser`。

**测量（1080×2340 视口，36 节×298px=10728px 高测试页，jpeg q80）**：
- 新路视口抓帧 `captureScreenshot{captureBeyondViewport:false}`：
  **中位 148ms/帧，y=0→8388 全程 144–166ms 纹丝不动**（png 239ms 同平）。
  帧字节随位置变化 244–269KB＝真帧。**「成本与页高无关」在 Pixel 5 成立。**
- 对照 v0.2.8 老路同级页 ~1400ms/帧 → **~9.5×**。
- screencast 流（jpeg q80 + ack 背压 + scrollBy 20Hz 驱动）：**6.21fps**，
  中位帧距 164ms，metadata scrollOffsetY 真值（8388 封顶）；瓶颈＝SoC 带产出
  ~150ms（泵 33ms 节拍喂得饱），对照 Mac 19.3fps。
- **静止 10s 零新帧**（vfdisp 计数不涨）＝damage 门空闲成本零；引擎 RSS 54MB。
- **live 闭环**：evdev 合成拖动（event2 y 1800→700）→ scrollTo → 帧流 →
  /tmp/vf.jpg → vfdisp → DRM 面板；拖前 y=0（第 1–3 节）拖后第 4–6 节，
  CJK 渲染清晰（样张 /tmp/vf_a_y0.jpg /tmp/vf_live_after.jpg）。

**新 bug（回报 aginxbrowser）**：`captureBeyondViewport:true`（老路全页）在新
引擎上 1080×10728 页必炸 `frame buffer size mismatch`（-32601）——老路对照
拿不到，同时是他们发版前该修的。

**客户端坑（/tmp/vstream.py，已修）**：damage 门静止时连接零流量——10s 读
超时把 reader 判死；解法＝60s 长轮询 + 0.3s 心跳 Runtime.evaluate（顺带喂泵，
泵只在消息间隙打点）。

**设备现态（known state）**：aginxbrowser 单元＝main 9cffa34（新）；live.py +
vfdisp.new + http.server(8123) 常驻，面板=可拖测试页；term loop 未恢复；
/var/bin/aginxbrowser.v028/v025 备份在位。

## 2026-09-07 — P0 修复复验全绿（aginxbrowser main 3fd39d8，真机）

上条反馈的回修。3fd39d8 两修：legacy 全页路径 jpeg 臂「PNG 字节当 raw RGBA」
根因修复（quality 顺带真生效）+ screencast 泵每 tick settle 5ms 驱动页面 JS
事件循环（静默连接不再冻结页面时间）。重构建（89,087,976B）换血复验。

**换血坑二则（补上条的 svcd 教训）**：
- /tmp 是 tmpfs，跨设备 mv 到 /var/bin 退化为写模式 → 运行中二进制
  `Text file busy`。正路：先同盘 mv 到 /var/bin/*.new 再 rename 到位。
- adb shell 里 setsid+& 的链会挂住 shell 本身（已知无害），回查用后续查询。

**四条清单（全部 PASS）**：
1. 原崩溃形状 fullpage jpeg q80 ×3：**1651–1686ms、1,209,106B、JPEG magic
   正确**（Mac 217–249ms，~7× 符合 SoC 差）。不再炸。
2. 无参 capture 同页：1176ms、1,253,082B PNG——与 v0.2.8 老引擎逐字节同尺寸，
   legacy 兼容未破。
3. 静默监听+自驱动页（setInterval 100ms 改 DOM，客户端只 ack）：
   **39 帧/5s，帧距稳 ~131ms**——比他们 Mac 的 7 帧/5s 更顺，settle 修法
   真机彻底成立，心跳可退役。
4. quality A/B：q50=186,196B vs q80=266,672B——**参数真生效实锤**（视口
   路径 q80 上轮 242–270KB 本轮同域，视口路径本来就尊重 q）。

**回归护栏（无退）**：视口抓帧中位 149.5ms（上轮 147.9）全平 y=0→8388；
screencast 6.08fps（上轮 6.21，噪声内）；idle 静默 12s 零帧——上一条记的
「5s 出 1 帧」实为 startScreencast 首帧快照（启动即推当前状态，排队在
盲吸收窗里），非滴漏。引擎 RSS 51,896KB（fullpage 三连后无滞留）。

**结论**：3fd39d8 真机复验通过，aginxbrowser 可发 v0.2.9。真机数字组
（148ms 全平 / ~9.5× / 6.2fps / RSS ~51–55MB）随版本号贴上游 follow-up。
设备现态：aginxbrowser 单元＝main 3fd39d8（pid 387）；live demo 重挂在
新引擎上待真人拖动；term loop 仍未恢复。

**真人收据（补记同日）**：用户上手拖动 live 页（新引擎 3fd39d8），原话
「很快啊」——首条真人触摸闭环收据。6fps 档被感知为顺：滚动位置即时跟手
（scrollTo 直投）、帧流背压只留一帧在途不积延迟，是「跟手+追帧」而非
「等帧」。感知验收通过，产品化前无需再提 fps。

**收尾（同日晚）**：demo 全拆——vstream live / vfdisp / http.server(8123)
全灭（/proc 扫描两遍零残留），/tmp spike 工件（脚本/页面/帧文件/日志）清空，
term loop 恢复（pid 13331/13336，01:24:50 起无重启）。六单元全 ready，
aginxbrowser 单元保留在 main 3fd39d8（pid 387）。设备回到产品态。

## 2026-09-07 全环路首通：指令→母体前台→分身工具执行→HTML 上屏

用户命题测试：一条「今天天气怎么样？」走完产品主环路三段——(1) 指令到
母体前台；(2) 后端分身真执行（工具链）；(3) 结果以 HTML 上屏。

**腿1 母体前台**：`aginx agent send "今天天气怎么样？"` UDS
/run/aginx.sock 阻塞往返 OK（母体 me v0 设计=无账本无工具面，单次 brain
直答——本腿只验前台通，不验工具）。

**腿2 分身执行**：化身 小喜（aginx agent create）走完整工具环——前台
路由 → spawn aginx-runtime（ledger 先记）→ `aginx commands --json` 发现
工具（含 web 面）→ brain 发 tool_call → server 侧执行
`aginx web fetch`（wttr.in）→ tool_result 回喂 → brain 成文。实测取回
**真实郑州天气**（IP 定位 34.77°N 113.72°E）。母体自己答不了天气而分身
能答——架构分工（母体=前台、分身=干活的）首次以产品语义验明。

**腿3 HTML 上屏**（视口管线复活）：`vstream.py ask` 新模式——发问前先
publish 占位页（磷光风「正在问 小喜▊」闪标），ask 线程走同一 UDS 前台问
分身，回复经 markdown-lite 转磷光 HTML（黑底绿字、问句块、粗白数据），
tmp+rename 落 /tmp/ask.html，`location.reload()` 重载；screencast 流
（0.3s 心跳）持续把视口帧写 /tmp/vf.jpg，vfdisp mtime 监视 → DRM 上屏。
**实测：回复 19.3s 到，重载后内容 1412px 单屏容纳，面板帧 211,586B，
截图核对磷光版天气明细全部渲染**（问句块/实况列表/粗白数值/页脚戳）。
整链无 adb 参与，面板侧可触摸拖动（touch_thread 沿用）。

**管线小注**：CDP.call id 分配加 ilock（ask 模式双线程并发调 CDP 的竞态
修复）；vfdisp 启动容忍帧文件缺失（mtime 轮询 continue），冷启动即黑屏
打底不炸。

**设备现态**：term loop 再次让位（restore wrapper 13335 + aginx-term
13336 已杀）；rig 在跑——http.server(8123) 15058 / vfdisp 15059 /
vstream ask 15060，日志 /tmp/{ask,vfdisp,httpsrv}.log。用户过目后拆 rig
恢复 term（脚本 /tmp/rig.sh 同目录拆除法照旧）。截图已按用户要求放
桌面：aginxos-ask-weather-panel-0744.jpg。

**白底修补（同日，用户目检立案）**：用户指「下面一片白色」——短文档
（内容 1412px < 视口 2340px）时，`html{background:#000}` 只把根元素盒子
涂黑，文档盒以下露出引擎合成面默认白底。**该引擎 screencast 路径不做
根背景向画布的传播**（标准 Blink 应覆盖整视口）——上游 aginxbrowser
可修项，先页面侧兜底：`html,body{background:#000}` +
`body{min-height:100vh}`。复跑第二轮 ask（20.9s，河南郑州，偏南风
12km/h/UV 7），整幅 1080×2340 全黑收据（截图 0750-black）。此坑只在
「文档矮于视口」时显形——长页 demo（10728px）从未踩到。
〔订正见下条：本轮根因分析有误，0750-black 系假收据〕

**白底真因订正（同日，第三/四轮 + csstest 隔离实验）**：上条「白底修补」
的修法从未生效——**引擎 CSS 静默丢弃 `vh` 单位**。csstest（同页三变体，
Page.getLayoutMetrics 为口）：`min-height:100vh` → 内容高 400px（声明
被丢，等同没写）；`height:2340px` 与 `min-height:2340px` → 2340px；
viewport JPEG 对比 vh 版绿条以下全白、px 版全黑。第二轮 0750-black 因此
是假收据：那轮回复内容恰好 ≥2340px 撑满视口（黑白一直由回复长度随机
决定），第三轮同模板、内容 1412px → 白底复现（截图 0801，日志
`reply on panel, content 1412px`）。附带：同页 navigate 与
`location.reload()` 渲染逐字节同（md5 一致），reload 路径无辜。
**真修**：模板 `body{min-height:2340px}`。第四轮 ask（15.8s，郑州：
阴 31°C/偏南风 12km/h/UV 7）内容高 2340px，整幅全黑，拉帧核对后用户
实机确认「确实全黑」（截图 0817-black，桌面）。**上游 aginxbrowser
可修项两条**（待 v0.2.9 后反馈引擎线）：① `vh` 单位支持；② 根背景向
画布传播（文档矮于视口时露合成面白底）。

**设备现态（第四轮后）**：term loop 仍让位；rig 在跑——http.server(8123)
15058 / vfdisp（obs 仪表版）14964 / vstream ask 第四轮 16901，日志
/tmp/{httpsrv,vfdisp,ask4}.log。用户发话后拆 rig 恢复 term（脚本
/tmp/rig.sh 拆除法照旧；csstest 留了 ~4 个引擎 target 未关，拆时一并
restart aginxbrowser 清）。

**Archify 第三方 HTML 上屏（09-07 晚，成果画布首战）**：archify（tt-a1i，
MIT，51.5k★）= Agent Skill：typed JSON IR → Node.js 确定性编译成自包含
HTML。Mac 侧 clone 后手写 AginxOS 架构 IR（12 组件/2 边界/13 连线/
3 卡片，/tmp/archify/aginxos.architecture.json）。**验证门三轮拦截**实证
其修复回执机制：标签-组件重叠（诊断给建议坐标，labelDy 修）→ 标签-标签
0px 间距 → showcase 档 boundary-title 字号收敛门不认 1530px 宽画布
（诊断无细节，读源码定因）→ 降标准档过。deliver 721,252B 单文件
（zh-CN/dark/classic，sha256 2dc26426…）。推设备 /tmp/arch.html，
showarch.py（复用 vstream 机器）开页+screencast → vfdisp 上屏。
**引擎首战成绩：整页渲染成功**——content 1560px 单屏容纳，组件盒/
连线/边界框/三卡片/中文全渲染，拉帧核对+桌面留档
aginxos-archify-on-panel-0831.jpg；HTML 同存桌面 aginxos-arch.html。
引擎 RSS **124MB**（基线 ~52MB，重页翻倍——待反馈引擎线）。
未验：JS 交互面（主题切换/搜索/export 菜单）；另 Target.getTargets 从
第二 CDP 连接返回空（target 注册表疑似按连接隔离，CDP 客户端互通性
存疑）。**产品假设「面板吃第三方 agent 成果 HTML」首血**。

## 2026-09-07 — 自然入口→分身画图全环路：brain 产 IR、编译门修复回执、面板出图（收据）

上条 archify 是手写 IR。本条把「画一下系统架构」这种自然入口接通：占位页
（「正在让 小喜 画架构图▊」）→ UDS 前台问化身 小喜，prompt 内嵌 archify
schema + 硬约束（枚举/画布/间距/少用连线标签）+ AginxOS 内容要点；分身回
JSON IR 落 /tmp/ir.N.json → **Mac 扮演 twin-server 编译器**（archloop.py：
pull ir.N → `archify deliver --json` → 过则 push arch2.html + arch.done
标记最后推；不过则 push 机器诊断 diag.N.json 回喂分身修复；3 轮尽推手写
兜底页）→ 设备侧 archask.py 见 done 即导航图页，screencast→vfdisp 上屏。

**三轮实录（修复回执机制首次在真 brain 上走通）**：
| 轮 | 分身耗时 | IR | archify 判决 |
|---|---|---|---|
| 1 | 217.9s | 4494B | 拒：connections[3] aginx-core→gateway 自带 via 段违反端点方向（clean-flow/endpoint-side-direction，诊断含建议修法） |
| 2 | 178.3s | 4620B | 拒：同一条线穿过无关组件 aginxbrowser（2px 间隙，clean-flow/edge-through-node） |
| 3 | **37.0s** | 4655B | **过，deliver 714,615B** |

第 3 轮 37s 说明修复 prompt 里诊断已够具体——brain 只需小改。全程 ≈7.5 min
（分身 433s + 编译/传输 ~20s），瓶颈全在 brain 长输出，编译器毫秒级。
分身自己的分解与我的手写版不同（9 组件：user-input/voice-input/aginx-core/
avatar-runtime/aginx-gateway/brain-api/data-memory/cloud-relay/output-panel，
多了 data-memory，没有独立「面板/浏览器」对）——brain 按自己的理解组织，
不是照抄。**面板收据**：导航后 content 1080×1274 单屏容纳，拉帧目检组件
盒/连线/边界框/卡片/中文全渲染（桌面 aginxos-brain-arch-panel-1641.jpg；
IR+HTML 同存桌面 aginxos-brain-arch.*）。引擎 RSS 142MB——注：同引擎还挂
着 csstest 残留 target，非纯净对照。触摸拖动照旧（touch_thread 沿用）。

**意义**：这是「分身干活→机器验证→诊断回喂→自修复」第一次完整落地——
修复回执不只对人类工程师有效，对 brain 同样有效。Mac 编译器即 M37
twin-server 的预演（真形态=86quan 上 archify 服务化，agpkg 装包）。
文件握手（标记最后推）粗糙但可靠，正式化时换 UDS/网关通道即可。

### 字号读不清修法：svg 切条 + 两个引擎新坑（09-07，用户判「根本看不清楚」）

整图 fit 上屏后组件字号 ~8.7px（archify svg 内字号是 viewBox 单位的 9–12，
fit 1080/1370=0.79×）。放大尝试三连败，全部是引擎坑：

1. **JS 变异 style 不重绘**：Runtime.evaluate 设 svg width=2100/2400/3000px
   （inline CSS），布局探测器（clientWidth/scrollWidth）如实报告 2100/2400/
   3000，但 screencast 帧逐字节不变（md5 全同）——paint 路径无视。
2. **初始 CSS 同样无效**：把 width:2400px !important 烤进 HTML <head>，
   加载期就位，帧仍逐字节同。
3. **svg width/height 属性也无效**：`<svg width="2400" height="1241">`，
   帧仍同。结论：**该引擎 svg paint 把 viewBox fit 到容器盒，对元素自身的
   CSS/属性尺寸一概不理**——与 vh 丢弃同族的静默忽略，布局与 paint 脱钩。

（过程坑：3000px「裁决帧」我目检误判为已裁剪放大，md5 对账才发现四帧
全同——**看帧先 md5，别让预期替眼睛看**。）

**修法（绕过一切尺寸输入）**：svg 切竖条叠放。先从编译产物提取组件盒
x 坐标（四列：100–250/450–600/800–950/1150–1300），把缝切在巷道
345/690/1035；复制 svg 四份，各改 viewBox 为 `[x,0,345,708]` 条，inline
style width:100% 叠放（条间虚线分隔）。每条变成竖构图，fit 放大
~2.8×（966px 容器/345 单位条），字号 ~8.7→**~31px**，整页 3245px 竖滑
阅读，触摸竖拖平移照旧。切缝只切边界框横线，组件盒零损伤。
拉帧目检：标题/组件标签/副标签全部清晰可读（桌面
aginxos-brain-arch-strips-1705.jpg）。

**产品含义**：横构图成果画布上竖屏面板，切条是通用的「转排」方案；
正式化应在编译侧做（twin-server 输出竖排变体），而不是设备端后处理。

**第四/五连败与交接（09-07 深夜）**：svg 内容 `font-size` 属性 ×1.5
（31 处，页高 3245→3264）与 viewBox 条宽 345→250（arch7，四条更窄本应
再放 ~1.4×、页高应到 ~11k）帧仍 md5 全同。arch7 已对账排除管线失误：
设备/主机 md5 两端一致、showarch2 URL 指向 arch7、layout 探测在跑——
**paint 对 svg 的内容字号与 viewBox 宽度均无感，唯一生效的只有 viewBox
裁剪窗口的位置**。客户端缩放到此为止（用户裁决「丢给 aginxbrowser
做」）：五连败表 + vh/根背景重申 + P2 观察（RSS 翻倍、Target.getTargets
按连接隔离）+ 复现包路径，打包在
`~/Documents/aginx/aginxos-svg-zoom-engine-feedback.md` 交接引擎线。
面板停在 31px 切条版（arch7 在载，渲染同 arch5），等引擎侧缩放支持
（`Emulation.setPageScaleFactor` 或 svg 尺寸在 paint 生效）后退役切条。
