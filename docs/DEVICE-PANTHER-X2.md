# DEVICE-PANTHER-X2 — 黑豹X2 立项（黑匣子 agent 节点·廉价档）

> **状态**：已立项，机器采购中（用户 2026-10-01 拍板「先买一台」）。
> 姊妹档：`docs/DEVICE-ORIN-NX.md`（本地大脑档，未立项）——不同仓位
> 不冲突。前提：#422 重新定位（黑匣子 ARM 服务器、服务器板卡一等
> 机器）；本仓加机 checklist = `devices/README.md`（D14）。

## 0. 一句话判定

**能刷，且是 redfin/enchilada 之外最顺的移植线。** 矿渣出身的 RK3566
盒子，DTB（`rk3566-panther-x2.dtb`）躺在 Armbian 设备树里现成，TF 卡
刷零砖险，Rust musl static userspace 整体白捡。座位 = 黑匣子 agent
节点的**廉价档**（二手 ¥88–150），「一台 ¥100 的 agent 服务器」。

## 1. 为什么它从「电视盒子」最痛类里被摘出来了

电视盒子通病是 vendor 内核+DTB 考古；黑豹X2 例外在**矿机出身**：
量产矿渣存世量大，社区养了多年——Armbian 25.0 时代仍在出 panther-x2
专用镜像（HAOS 固件站有专目录，连 NPU/VPU 硬解镜像都有人维护），
Armbian 论坛可证 DTB 就在 `rockchip/rk3566-panther-x2.dtb`。无考古。

刷入两路都是被千百遍验过的（CSDN 实录）：

1. **TF 卡刷**：dd/rufus 写卡 → 插卡上电即启动（原 eMMC 系统不动，
   零砖险）；要进 eMMC 就在系统里 `dd if=/dev/mmcblk0 of=/dev/mmcblk1`。
2. **线刷**：RKDevTool + loader + 双公头 USB + 按复位键进 maskrom
   ——BootROM 兜底，近不可砖。

## 2. 事实卡（2026-10-01 网查；⚠=到手实测项，D14 纪律 not probed 就是 not probed）

| 项 | 值 |
|---|---|
| SoC | RK3566 4×Cortex-A55（22nm，满载 ~4W，天然 7×24 挂机体质） |
| 内存 | 4GB（**买 4+32 版**；2G 版也存在 ⚠到手验） |
| eMMC | 29.1G 实录（lsblk；boot0/boot1 各 4M 引导分区） |
| TF | 独立卡槽，可启动（W1 载体） |
| 网口 | RJ45 ⚠速率（百兆/千兆）待探 |
| wifi | **BCM43430**（SDIO vendor 0x02d0/device 0xa9a6 实读，2026-10-07 #452；树莓派 3B 同款 Cypress wifi+BT combo）。驱动腿 2026-10-07 #453 落地：brcmfmac 五件+固件从卖家 p2 **debugfs 收割**（p2 挂载双锁 EINVAL/EBUSY 绕道）进树，wlan0 出生+连 AP+relay 全收据；双腿默认路由落 wifi，拔网线即无事件 |
| 电源 | ⚠接口/规格待探 |
| 串口 | 机内 TTL ⚠（Rockchip 惯例 1500000 波特） |
| 价格 | 闲鱼二手 ¥88–150 |
| 无屏/无modem/无摄像头 | 黑匣子定位下零负担 |

**32G 论证**：L0 镜像 46MB、整机占用 66MB、opt-in 全家几百 MB
（aginxbrowser 82MB 是最大件）——32GB eMMC 是 500 倍余量，另有 TF
槽可扩。喊挤的是 Docker/Ubuntu 全家桶用户，不是我们。

## 3. 移植形状（D14 checklist 映射）

**白捡层**：aarch64 musl static userspace 全部（aginx/gateway/svc/
pkg/net-watch…），rootfs 配方 arch 通用。

**重做层**（`devices/panther-x2/`）：

- `device.toml`：**首台无头机最小形状**——`[device]`/`[paths]` 起步，
  `[panel]`/`[input.*]`/`[audio]`/`[camera]` 全免。hwd schema 对缺段
  的容忍度是 W1 第一问（fail-fast 只应针对整文件缺失）。
- `modules.txt` + `bringup/`：按实测 insmod 序抄；无 modem 无触控，
  bringup 面比手机窄得多（eth/disk/clock 三件）。
- `boot/`：**烤线新形状** = 整 SD/eMMC 镜像组装（idbloader + u-boot +
  extlinux + kernel + DTB + rootfs 分区）。W1 白嫖 Armbian 件，W2
  收编自烤。
- `cam/`：无（摄像头线已冻结，#422）。

判据 = platform 一行不改。若发现 rootfs 配方 Qualcomm 残留（应无），
按 D14 豁免登记法处理（对齐 `[update.layout]` 先例）。

## 4. 阶段门草案

- **W1 SD 卡白嫖启动**：Armbian u-boot + kernel + panther-x2 DTB
  （extlinux 指向我们的 rootfs.img）→ 判据：L0 起、svc 带 net-watch、
  网口 DHCP 拿址、ssh 双通道真连。
- **W2 eMMC 落地 + 内核收编**：dd 进 eMMC；kernel/U-Boot 自烤进
  `boot/` 线 → 判据：拔卡从 eMMC 独立启动。
- **W3 节点在册**：opt-in aginx + gateway，relay 注册
  （`agent://panther-x2.relay.aginx.net`），agc 远端往返真答 →
  判据：**通电插网即 agent 节点**——黑匣子整机验收。
- **W4 更新面（挂账后置）**：A/B 与 agupd 对 Rockchip 分区适配
  （`[update.layout]` 槽位）。¥100 节点先拿「拔卡重写」当恢复路。

## 5. 采购/到货清单

- 黑豹X2 **4+32 版**（闲鱼 ¥88–150；可让卖家确认能进 maskrom/复位键在）
- USB-TTL 线一根（¥10 档，1.5M 波特）——卡刷不顺时唯一的眼睛
- TF 卡 8G+ 一张（W1 载体）
- 电源按机器接口配（到货看 ⚠）

## 6. 待拍板

1. codename 定名 `panther-x2`（目录=机名 D14 律）——默认此名，到货前
   可改。
2. W3 过后要不要买第二台——验证「机器是数据」的复制性（D14 的真正
   考题：加第三台机 platform 仍零改动）。

---
资料来源（2026-10-01 网查）：CSDN《PantherX2 刷机流程》
（blog.csdn.net/xueyong0403/article/details/144947582，lsblk/dd 实录）；
Armbian 论坛（DTB 证据）；瀚思彼岸 HAOS panther-x2 固件目录；
smzdm（4+32 配置）；二手行情文（¥88/矿渣背景）。
