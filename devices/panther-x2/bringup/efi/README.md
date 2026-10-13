# Stage 1：X2 UEFI 直启（无 Linux 证明题）

用户 2026-10-13 批准。题目：**aginx 面在无 Linux 内核的机器上活**。
载体=panther-x2（RK3566）的卖家 u-boot——它带 EFI_LOADER（EFI 固件面），
EFI app 即「操作系统」。分级：

- **M0（本地，已绿 2026-10-13）**：`hello/` 出 PE/COFF aarch64，
  qemu+edk2 串口出字——构建链预验完成。
- **M1（真机亮灯）**：X2 上 u-boot `bootefi` 跑 hello，串口/HDMI 出字
  =无 Linux 启动链成立。
- **M2（网络面）**：EFI_SIMPLE_NETWORK 起 DHCP+UDP，Mac 侧探到
  「aginx-efi」应答——通电插网即节点（无内核版）。
- **M3（直启正身）**：M2 复盘后定形状（no_std 移植 vs EFI substrate
  承机器面、aginx 随 Stage 3 换底）。

## M0 实况（全部已验证）

依赖：nightly + `rustup target add aarch64-unknown-uefi` +
`rustup component add rust-src`；brew qemu（自带
`share/qemu/edk2-aarch64-code.fd`）+ mtools。

```sh
cd devices/panther-x2/bringup/efi/hello
cargo +nightly build          # -Zbuild-std，产物 target/.../aginx-efi-hello.efi
cd .. && ./mk-fat.sh          # hdiutil 造裸 FAT + mcopy 装 EFI/BOOT/BOOTAA64.EFI
./run-qemu.sh                 # edk2 从 FAT 默认路径启动 app
```

预期串口（-nographic 即 stdio）：

```
aginx-efi-hello: no kernel, just us
firmware vendor: EDK II
firmware rev: 0x10000
```

坑位记录���
- 镜像源 rsproxy 的 `uefi-services` 停在 0.26——但该 crate 已并回主件；
  **只用 `uefi = "0.41"`**（`uefi::helpers::init()` 无参、`#[entry]`
  无参、firmware 读数走 `uefi::system::` 免参函数）。
- macOS 挂 FAT 要 msdosfs kext（免 sudo 挂不上）——mtools 纯用户态绕开。
- hdiutil `-layout NONE` 出的即裸 FAT 字节，qemu 直吃，无需转换。
- hello 是独立小灶（自带空 `[workspace]`），nightly unstable 不进
  主 workspace/check.sh 稳定链。

## 真机上机（M1，等 X2 回手边）

两车道（都不碰卖家链主体）：

- **A · TF 卡**：卡上 idbloader+u-boot（Armbian panther-x2 件或从在役
  eMMC dd 出 boot0/boot1）+ boot.scr 指 `bootefi` 本 app。插卡即起，
  eMMC 零风险。
- **B · eMMC 骑乘（可逆）**：hello.efi 拷 p1，boot.scr 尾部加
  `load mmc 0:1 ${loadaddr} hello.efi && bootefi ${loadaddr}` ——回滚=
  删该半截（与 ride-init 同纪律）。

无串口眼（USB-TTL 挂账未购）时先走 B+HDMI 面，或补购 TTL（1.5M 波特，
ttyS02）。
