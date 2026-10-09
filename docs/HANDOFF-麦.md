# 交接：OnePlus 6 麦克风（2026-09-22）

给下一棒。混音器、声道数、Slim 通道号、q6afe 的 slave 口参数，都已经测完，采集仍是精确的全零。不要从这些参数重开。

目击只追加在 `docs/HARDWARE.md`。这份是停手清单和最后状态，不是第二本实验日志。

## 结论

麦、WCD934x、slimbus 线和 ADSP 固件不是坏的：用户确认槽 b 的 Lineage 日常能录音。本交接没有为了复核去切槽。

L0 的 `pcmC0D1c` 是全零，不是增益小。2 秒、48 kHz、1 声道、S16 是 **192000 字节，非零样本 0**。AFE `START` 返回 0，随后 `AFE close failed -110`。喇叭能响，不能当成麦通了：喇叭是 `QUAT_MI2S` → MAX98927，不走这条 Slim。

脸上的「没听懂」是静音门，不是识别失败。

## 机器

| 项 | 值 |
|----|----|
| 机 | OnePlus 6 / enchilada / ONEPLUS A6003 |
| serial | 见 `.local/device/serials.env`（ENCHILADA_SERIAL）。fastboot / adb 必须带 `-s <该串>` |
| 槽 a | L0，内核 `6.11.0-sdm845-g2fa43795f607` |
| 槽 b | Lineage。麦在这边是好的 |
| 登录 | `ssh root@10.9.8.1`。Mac 这边常见是 `en14` 上的 `10.9.8.2`。没有这个地址，10.9.8.1 不走路由 |
| 采集 | `/dev/snd/pcmC0D1c`，48 kHz，1 声道，S16。前端 MultiMedia2，后端 `SLIMBUS_0_TX`，AFE 口 `0x4001`，dai id 3 |
| 放音 | `/dev/snd/pcmC0D0p`，`QUAT_MI2S_RX Audio Mixer MultiMedia1=1` |
| 工具 | `/bin/snd-mixer`、`/bin/snd-cap`、`/bin/snd-play`。机上没有 python3。`awk` 会段错误，不要用 |

`succ_a` 在最后一次重启前一直是 0，那次重启后没有再读。冷启动失败时 ABL 会把槽 a 标成 unbootable。恢复只做 `fastboot -s <ENCHILADA_SERIAL> set_active a`（serials.env），不 wipe、不刷机。软件 `reboot` 可能留下「能 ping、SSH 拒绝」的假活，以 `/proc/uptime` 判断是不是新内核，不要只看 ping。

写这份交接时，最后一次 SSH 超时。下面的终态是 pre-slave 那次启动收尾时目击的，不是「现在这一秒」的探针。上去先对 serial、槽、uptime，再动模块。

## 最后一次启动留下的状态

2026-09-22，用户要求换成 pre-slave 再重启。未 wipe，未刷，未热卸 q6afe。

- SSH 时 uptime **35.74s**，serial 同上（serials.env），槽 `_a`，内核同上。
- `/lib/modules/q6afe.ko` = `q6afe.ko.pre-slave`，**116976** 字节，md5 `8ed51f71ad73115f3e400a47d6ab45b5`。这只模块的字符串里没有 `slave PORT`。
- 带 slave 口配置的那只 **122016**、md5 `7ce088b1fdfac037d96339ecde199f08` 还在 `/lib/modules/q6afe.ko.init`。不要把它拷回去当修复。
- `/etc/init.d/audio-bringup` 和仓库 `devices/enchilada/bringup/audio-bringup` 已改成 Lineage 的单麦 `amic4`（见下）。开机假采集仍注释掉。
- 开机第一段之前，dmesg 里没有 slim / 路由行。第一段就是我们的 `snd-cap`。
- `/tmp/boot1.raw` **192000 字节，nz=0，rms=0**。
- 喇叭 `snd-play pcmC0D0p` 0.4 秒 **play_rc=0**。
- 语音 unit 已放回，当时 `aginx-svc status aginx-voice` 是 ready（pid 796）。

那一段的内核顺序：

```
decim port=0 mux=2 dec=0
port_mask=0x1  ch_num=128
q6slim prepare map=128
dec unmute dec=0 path=24
delayed START dai=3 rc=0          ← 在 DEF_ACT 之前
q6route open fe=1 sid=2 port=3 path=2 rate=48000 ch=1
matrix ret=998
DEF_ACT w=cf 24 20 83 94 80       ← 末字节 0x80 = 通道 128，应答 20 00 00 00
RECONFIG 应答 2f
wcd slim enable ret=0
AFE close failed -110
```

没有 `slave cfg` / `slave PORT` 行。pre-slave 不会打这些。

## 两条别人能录、我们没录到的路

物理麦都是 ADC4（AMIC4，偏置 MIC BIAS1，设备树 1.8 V）。数字口不同。

**Lineage 日常录音**（`android_device_oneplus_sdm845-common` lineage-22.2 的 `audio/mixer_paths_tavil.xml`，`handset-mic` / `voice-rec-mic` / `camcorder-mic` / `speaker-mic` 都是 `amic4`）：

`SLIM TX0` ← `CDC_IF TX0 = DEC0` ← `ADC MUX0 = AMIC` ← `AMIC MUX0 = ADC4`，`AMIC4_5 SEL = AMIC4`，一路。通道号 128（128+口 0）。

通话双麦名叫 `handset-dmic-endfire`，其实仍是模拟麦：TX7←DEC7←ADC4，再加 TX8←DEC8←ADC3。

**postmarketOS / sdm845-mainline UCM**（enchilada `HiFi.conf`）的底麦是另一口，而且在主线上能出样本：

`MultiMedia2` ← `SLIMBUS_0_TX` ← AIF1 `TX7` ← `DEC7` ← `ADC MUX7` ← `ADC4`。不抢 TX7，不发 slave PORT。扬声器在那份 UCM 里是 MultiMedia3，我们这棵树的扬声器是 MultiMedia1，而且已经能响，不要按那份 UCM 去改喇叭。

L0 两条都录过，都是全零：TX7/通道 135，以及上面的 TX0/通道 128。

## 不要再试

下面每一条都有 `HARDWARE.md` 里的目击。`ret=0`、应答字节、矩阵返回值都不是样本。

| 做过的事 | 结果 |
|----------|------|
| 混音器指到 TX7/DEC7/ADC4，采集中读寄存器 | 偏置、AMIC4、MCLK、TX 时钟开着，IFC 口 7 已使能，无上溢/下溢，PCM 仍全零。AMIC4 `0x0611=0xb4`，MICB1 `0x0622=0x50`，BIAS `0x0601=0x80`，TX7 通路 `0x0aa1=0x24` |
| 给 AMIC 补上 MCLK | 采集时 MCLK 开了，仍然全零 |
| AIF2/AIF3 抢走 TX7 后，改成只挂当前 AIF | 通道列表不再空，仍然全零 |
| 后端从强制 2 声道改成 Slim 采集 1 声道 | 矩阵都接上，全零 |
| ADM 路由 | `path=2` 是 `ADM_PATH_LIVE_REC`，port=3 是 `SLIMBUS_0_TX`。`matrix ret` 见过 998、999、1000，那是 `wait_event_timeout` 剩下的 jiffies，不是 DSP 错误。真正的失败是 `-EINVAL` 或超时 |
| Slim 管理器是否拒绝通道 | `DEF_ACT`（MC `0x21`）应答 **0x20**。高通头文件里 `MSM_SAT_SUCCSS` 就是 `0x20`。`RECONFIG` 应答 **0x2f**（0x20\|0x0F），成功位是立的，没有被证明是 NACK。不要为了 0x2f 改协议 |
| TX PGA 静音位做 40 ms 边沿 | `dec unmute ... path=24`，仍然全零 |
| q6afe 加 CAF slave 配置（SVC `0x10235`、`CDC_REG_CFG_INIT` `0x10237`、PORT `0x10233`，`map0=7`） | DSP `ret=0`，仍然全零。PORT 把口写死成 7 |
| 换回不带这些配置的 pre-slave，安卓单麦，开机第一段 | 仍然全零。所以「写死的口 7」不是唯一原因 |
| `SLIMBUS_CONFIG` 改成 16 槽、`psize=32` | `DEVICE_START` DSP **0x9**，接着读 I/O 错误。保持 `psize=24` |
| `slimbus_dev_id=1` | START 超时 `-110`。不要重试 |
| `SLIMBUS_1_TX` / `SLIMBUS_2_TX` | DSP 0x9 / EINVAL。不要重试 |
| 9 月 18 日那套模块换回去 | 当天有过一次能量（48 kHz 1 声道，rms 44 然后 132，识别出「你好」）。后来按那套文件恢复，仍然全零。那次不是配方 |
| 「只有开机第一段有能量」 | pre-slave 那次第一段就是全零 |

`ASM_DATA_CMD_EOS`（`0x10bdb`）的 `not expecting rsp` 跟在 close `-110` 后面，是结果不是原因。

## 现在这棵树相对上游多出来的改动

q6afe 已经换成没有 slave 口的那只。下面这些在 pre-slave 那次重启里没有换，仍是改过的模块。尺寸是装上去时记的，那次重启没有覆盖它们。

| 模块 | 在跑的改动 | 备份 |
|------|------------|------|
| `q6afe-dai.ko` 168104 | `prepare` 里 50 ms 后 `START`，不在 trigger 里启动。注释写「DPCM 不会调用 slim trigger」，但 dmesg 里有 `q6slim trigger START`。最后一段里定时器比 `DEF_ACT` 先到 | `.160520`：在 `prepare` 里同步 `START` |
| `snd-soc-wcd934x.ko` 581096 | AIF 通道列表空时抢 TX7；DEC 打开后把 PATH_CTL bit `0x10` 置上 40 ms 再清掉 | `.pre-unmute` 580760 |
| `snd-soc-sdm845.ko` 90272 | AMIC1–5 拉上 MCLK；Slim 采集强制 1 声道；codec 通道图里若有 135 就把 AFE 图定成 135，否则用 `tx_ch[0]` | `.pre-1ch` 90088 |
| `slim-qcom-ngd-ctrl.ko` 136168 | 只加了 DEF_ACT / RECONFIG / GENERIC_ACK 的 printk | `.pre-log` 135688 |
| `q6routing.ko` 662168 | 只加了 `q6route open` / `matrix` 的 printk | `.pre-route` 661664 |

仓库里没改过的 `q6afe-dai`（`86quan` 上 `git show HEAD:sound/soc/qcom/qdsp6/q6afe-dai.c`）是在 `prepare` 末尾同步调用 `q6afe_port_start`，没有这只 50 ms 定时器。postmarketOS 能录的就是这种主线顺序，不是我们这份定时器。

曾经有一段日志是 `wcd slim enable` 先于 `delayed START`，当时 PCM 也是全零，而且当时 q6afe 还带着别的改动。所以不能把「把 START 挪到 enable 之后」写成已经证明的修复，也不能写成已经干净地否掉。

源码树：`ssh 86quan`（`106.75.32.216`），`/home/ubuntu/op6/linux`，分支 `enchilada-cam`。vermagic 必须是 `6.11.0-sdm845-g2fa43795f607`。`ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-`。不要装 vermagic `7.1.0-rc1` 的 `snd-soc-wcd-common.ko`。

## 硬约束

- 不要热卸、不要热替换 `q6afe.ko`。APR 服务 4 会注销，放音和采集一起 `-110`，直到按电源键冷启动。换文件后重启，再录。
- 不要 unbind / rmmod `slim-qcom-ngd-ctrl` 或 `171c0000.slim-ngd`。卡会没，进程进 D。`power/control` 保持 `on`。
- `q6afe-dai`、`snd-soc-sdm845`、`snd-soc-wcd934x`、`q6routing` 可以在 q6afe 还加载着的时候热换。卸的时候先卸依赖它们的卡驱动。
- 不要扫 `/sys/kernel/debug/regmap/217:250:0:0/registers`（IFC）。读这个文件会卡在 Slim 总线上。codec 那份 `217:250:1:0` 能读，地址是四位十六进制（`0611:` 这种）。debugfs 不一定挂着，要的话 `mount -t debugfs none /sys/kernel/debug`。
- 采集前把语音停掉，否则它占着 PCM。停：把 `/var/lib/aginx/units/aginx-voice.toml` 挪成 `aginx-voice.toml.hold`，再 `aginx-svc stop aginx-voice`。恢复：挪回来，`aginx-svc reload` 然后 `aginx-svc start aginx-voice`。只 `start` 会报没有这个 unit。`stop` 还没退出时 `start` 会报 stopping。
- 开机脚本里的假 `snd-cap` 保持注释。它会吃掉第一段。BusyBox 不允许 if 里只有注释，用 `true`。
- 数样本在 Mac 上做，把 raw 拉回来。全零就是 `nz=0`。
- 不提交厂商固件，不 wipe userdata，不写 SIM，不 `rproc` stop，不发明没探针到的节点。

## 若还要做，只做这一次

q6afe 已经是 pre-slave。把 `q6afe-dai` 和 `snd-soc-wcd934x` 换回仓库里没改过的版本（dai 用同步 `prepare` START，wcd 不要抢 TX7、不要那段静音边沿）。混音器维持现在的 `amic4`，或者改回主线 UCM 的 TX7/DEC7/ADC4，二选一，不要两套叠在同一次采集上。语音停着，只录一段。

还是全零就停。不要再加 AFE 参数，不要再重启着试通道号。剩下的差别在 Slim 数据通道和这套 ADSP 固件，相对于能录音的 sdm845-mainline，不在混音器。

采集：

```sh
snd-cap /dev/snd/pcmC0D1c 2 /tmp/cap.raw 48000 1
```

2 秒成功时应是 192000 字节。dmesg 里要看的是 `port_mask`、`ch_num`、`delayed START` 或同步 START 的返回值、有没有 `AFE close failed -110`。样本数在 Mac 上数。
