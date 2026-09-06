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
## 2026-09-07 — 开机体验⑤ 语音侧上机：接线员分流 + 英文话术 + term 直进 Voice（收据）

aginx-voice / aginx-term musl 推 /usr/bin（chmod 755，agsvc 监督），重启验收：

- **英文 TTS 首条收据**：`/var/bin/aginx-tts "Operator. Go ahead."
  /tmp/en-test.wav` → exit 0，113888 字节（≈3.5s）。melo
  vits-melo-tts-zh_en 出英文整句——此前零收据的风险点闭案。
- **接线员分支（已连网）**：`aginx-svc restart aginx-voice` 两次，每次
  face 文档 `lines=[["Operator. Go ahead."]]`、`eye=false`、state idle；
  日志增量（offset 对齐）只有一行 `up (local=true, brain=true, …)`——
  铃（play_ring）与 speak 无任何错误行；`/run/boot.state` 含
  `wifi ok`（判 Up 分支的输入）。铃+英文问候扬声器出声两次（人耳
  确认留用户）。
- **term 直进 Voice**：kill 旧 term，handoff 常驻环 2s 内以新二进制
  重启（pid 1577），无崩溃环；开机默认 Mode::Voice 生效路径 =
  face 文档渲染（voice 侧收据）+ 常驻环存活。
- **未收**：断网分支（英文警告+自动睁眼）——现网 wifi.conf 不可毁，
  主机 34/34 测试覆盖，fresh-boot 留 bake #19；bootcard Matrix 雨/
  END——initramfs 每次 boot 重建，同炉 bake #19。
