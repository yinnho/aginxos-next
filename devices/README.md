# devices/ — 机型是数据（D14）

平台（`crates/` + `rootfs/` 通用配方 + `scripts/` 通用入口）对具体机型零引用。
一切机型差异住 `devices/<codename>/`，烤机时 `DEVICE=<codename>
./scripts/build-rootfs.sh` 选择注入。**加一台机 = 新目录 + 一条 bring-up
线，platform 一行不改**——这是本目录存在的判据。

## 目录形状

| 件 | 是什么 | 烤进去哪 |
|---|---|---|
| `device.toml` | 机型档案：`[device]/[panel]/[input.*]/[audio]/[quirks]/[affinity]/[camera]/[paths]/[adb]`。`[v1]` 段只登记不接线 | `/etc/aginx/device.toml`——`hwd::load_or_exit()` 的读点；缺失=开机 fail-fast，绝不兜底 |
| `modules.txt` | ramdisk 模块有序清单（顺序即数据——依赖序是实测出来的，注释里记实测收据） | `.ko` 拷 `/lib/modules`，bringup 按序 insmod |
| `bringup/` | 硬件 bring-up init 脚本（audio/battery/camera/cell/radio/touch…名字即 rcS 调用点） | `/etc/init.d/` 原名装放 |
| `boot/` | 本机 boot 风格的打包线（pack 脚本、trampoline 源、assets.md 资产重生成法） | 不进 rootfs 镜像——刷机前手工跑 |
| `cam/` | 传感器源三件：`cam-shot.c`/`campix.h`/`campix_test.c`（时序/寄存器/标定=机型；JPEG 编码是平台，留 `rootfs/src/`） | `scripts/build-cam.sh <dir>` 编出 `aginx-cam-shot` |

本地资产（vendor ramdisk、voice/ocr 栈、radio blob……）不进 git，住
`.local/device/<codename>/`，布局与重生成法见该机 `boot/assets.md`。

## 加机 checklist

1. `mkdir devices/<codename>/`——目录名即 `[device]name`，两者必须一致
   （烤机门与 update 拒刷门都查）。
2. `device.toml`：照 redfin 的 schema 抄，逐字段**从探针/kmsg 实测填**，
   不许猜（「not probed」就是 not probed，AGENTS 纪律）。优先填平台已有
   槽位（`[quirks]` 开关 / `[affinity]` 表 / `eye_stream_args`/`qr_scan_args`
   参数串）；槽位不够时先谈加槽位，不在 crates 里写 `if <机型>`。
3. `modules.txt` + `bringup/`：从新机上真正 insmod 成功的序列抄，
   顺序原样。
4. `boot/`：本机 boot 风格的打包脚本（vendor-boot / dtbo / …）。
5. `cam/`：相机线到那台机时才需要。
6. `.local/device/<codename>/`：本地资产落位 + `boot/assets.md` 记
   重生成法。
7. `DEVICE=<codename> ./scripts/build-rootfs.sh` 出图，刷机走该机
   `boot/` 线。

## D14 三律（全文见本地 docs/ARCH.md）

1. crates 里出现机型字符串（redfin/1080/2340/event1/sm7250…）即违宪；
   `crates/hwd` 读 device.toml 是唯一合法来源，check.sh 设 grep 门。
2. 平台可为机型留**槽位**（quirk 开关、affinity 表、参数串），但不得留
   **默认机型**——device.toml 缺失 fail-fast。兜底=隐性机型假设回流。
3. 机型目录之间不互相 import；共享资产上提 `scripts/` 或 `rootfs/`，
   不建 `devices/common/`（两台机不值得，三台再说）。

当前机型：redfin（首目标，在役）· enchilada（bring-up 线，P5 另立计划）。
