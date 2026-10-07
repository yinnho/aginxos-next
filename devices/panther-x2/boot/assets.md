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
rmtfs EFS 种子 —— 无 vendor 世界、无 modem、rk_gmac 内建。
（modules.txt 曾空清单，2026-10-07 #453 起载 wifi 五件，见下。）

## wifi 腿资产（#453，2026-10-07 收割）

卖家 p2 挂载双锁（ro=EINVAL 脏日志、ro,noload=EBUSY 与 rw 活挂冲突）
——收割走 **debugfs 对裸设备只读 dump**，绕过挂载层：

- `debugfs` 本体：Mac 侧 zig cc 按 `scripts/build-resize2fs.sh` 同配方
  出 aarch64-musl 静态件（lib/ss 必须进 lib 序：et→ss→e2p→blkid→
  support→ext2fs；`make subs` 先行否则 config.h 补丁被重写），
  scp 到板 /root/debugfs。
- 抽法：`/root/debugfs -R "dump <p2 内路径> <板上输出路径>" /dev/mmcblk0p2`
  （modules.dep 定链；固件在 /lib/firmware/brcm/）。
- `.local/device/panther-x2/modules/` — brcmfmac 五件+ bca/cyw 备件
  （.ko，烤线按 modules.txt 取五件；wcc 后挂+bind 重探坑在
  modules.txt 头注）。
- `.local/device/panther-x2/firmware/brcm/` — brcmfmac43430-sdio
  {bin,txt,clm_blob} + panther,x2 板级 NVRAM + b0 双件（烤线整目录
  cp，内核 fallback 链自动选）。
- 再生法=回卖家系统或本配方重抽（内核升级后 vermagic 变须重收）。

## 恢复线（未破，挂账）

maskrom + rkdeveloptool：2026-10-07 实测 USB 入 maskrom 后 Mac 侧不枚举
（原因未查）。串口=console ttyS02 1500000（cmdline 实读），USB-TTL
线购入后接。在这两条线破之前，armbianEnv.txt 的任何编辑前必须：
boot.scr/armbianEnv.txt 备份到 Mac + ride-init 链逐环节机上预检。
