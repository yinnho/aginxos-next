# panther-x2 boot assets — `.local/device/panther-x2/` 布局与再生法

本机无自有启动链（W1 目录骑乘，#66）：卖家 u-boot + boot.scr + uInitrd
整链白嫖，rootfs 树住 p2:`/aginxos/`，`/boot/armbianEnv.txt` 的
extraargs 追加 `init=/aginxos/init` 切入；回滚=删该半截。故无 pack
脚本、无 boot.img、无 trampoline——`boot/` 只放切入脚本与本案。

## 布局

- `ride-init` — 切入脚本，烤线（boot_style=armbian-ride）装到树根
  `/init`，即卖家盘上的 `/aginxos/init`。在 git，随机型目录走。
- `dropbear/bin/{dropbear,dbclient,dropbearkey}` — ssh 三件，烤线硬门。
  静态 musl aarch64、与机型无关：2026-10-07 自 `.local/device/redfin/
  dropbear/` 逐字拷贝（再生法见 devices/redfin/boot/assets.md，同源同
  字节）。不进 git。
- `../fleet.sh` + `../fleet-config.tpl` — fleet 复制线（#451）：snapshot
  从在役机抽满配无密快照、kit 收身份三件、init 铺新板。产物落
  `.local/device/panther-x2/fleet/`（快照+manifest 可明文；kit/ 三件
  0600 是密件，永不出 .local）。

## 明确不需要的资产（与手机机对账）

vendor ramdisk / trampoline 对 / radio blob / qcom+ath10k firmware /
rmtfs EFS 种子 / modules/ —— 无 vendor 世界、无 modem、rk_gmac 内建
（modules.txt 空清单即收据）。

## 恢复线（未破，挂账）

maskrom + rkdeveloptool：2026-10-07 实测 USB 入 maskrom 后 Mac 侧不枚举
（原因未查）。串口=console ttyS02 1500000（cmdline 实读），USB-TTL
线购入后接。在这两条线破之前，armbianEnv.txt 的任何编辑前必须：
boot.scr/armbianEnv.txt 备份到 Mac + ride-init 链逐环节机上预检。
