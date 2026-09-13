# enchilada 资产登记（D14/§7：blob 全在 gitignored `.local/device/enchilada/`）

raw-boot 形态（boot.img + 直读 userdata ext4）不需要 redfin 那套
vendor ramdisk / trampoline / radio / preload 资产。build-rootfs.sh
按 device.toml `boot_style = "raw-boot"` 门掉那些段。

## boot-out/（pack-boot.sh 产物，本地）

- `enchilada-boot.img` — 内核 6.11.0-sdm845 + initramfs（initramfs-init
  在本目录）。刷 `fastboot flash boot_a`；再生：`./pack-boot.sh`（内核
   Image/dtb 来源见 lab/，E3 收据在 docs/HARDWARE.md）。

## dropbear/（运维通道 sshd，与 redfin 同一份构建产物）

静态 musl 三件，zig cc 构建（zig cc 的 AR 必须 LLVM——macOS BSD ar
静默弃 ELF；再生流程见 devices/redfin/boot/assets.md）。从
`.local/device/redfin/dropbear/` 复制（2026-09-13），sha256：

- `bin/dropbear`    330ba587310e8189d200d93b7e90b9a0132b41ced98261d8a890dfe44694e5ea
- `bin/dropbearkey` ee7e421adfb709457f91ec101f159f14df7c9b7c6cdd982596fb251f18348d65
- `bin/dbclient`    a820b930977d75a17553be22a854a1c61eef5ba30aefbff7fcfffc8f2baac439

## lab/（E3/E4a 实验材料，本地）

LOS 恢复件（los-boot_b/dtbo/dtbo-zero）、seed v1–v4（busybox 最小根，
E4b 起被真 L0 取代）。slot b 的 LOS 是回退线，别动。
