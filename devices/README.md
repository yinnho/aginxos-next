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

## W2 上机 checklist（裸 L0 → agent 服务器，#449/#450 收据固化）：

刷机/骑乘之上把机器升格为 relay 可寻址的 agent 服务器。顺序即依赖序：

> **同型号复制线（#451）**：再来一台 panther-x2 不走本清单——
> `devices/panther-x2/fleet.sh`（snapshot 满配快照 → kit 收身份三件 →
> init 铺树+注身份+切 init=，全程可回滚）。本清单是它的手工底稿与
> 异型机参照。

1. **包装配**：`aginx-pkg opt-in codex`（裸 bar 验收：`codex exec` brain
   真答）→ `opt-in aginx`（树包；url 漂移有 build-pkg 闸拦，sha 不合
   改走 `install <名> <本地tar> <sha256>` 本地通道）→
   `opt-in aginx-gateway`（依赖闭包自动带 aginx-secretd）。
2. **codex 家**：真源只许 `/home/.codex`（config.toml+auth.json，0600，
   从 Mac `~/.codex` 拷）。镜像自带 `/root/.codex → /home/.codex`
   符号链接（旧镜像手工 `ln -sfn` 补上，旧目录备份不删）。
   **铁律：手动腿真答 ≠ 网关腿真答**——网关守护 HOME=/home，没钉
   CODEX_HOME 时代码读的是另一个家；空家=默认连官方云=国内死循环。
   自 #450 起安装链写的条目自带 `[command.env] CODEX_HOME`。
3. **config 真源 `/etc/aginx/config.toml`**（0600，secret 全程管道不
   回显）：`[server] access="private"`；`[relay]` id/domain=
   relay.aginx.net/port=8443/use_tls=true/url/relay_secret；
   `[auth] jwt_secret`（`od -An -tx1` 64hex）。
   **relay id 律：只收字母数字**——`panther-x2` 形被拒，落
   `pantherx2`（pair 链自 #450 起自动归一，手填自己守）。
4. **名册**：system 条目=树包 boot 自动落（codex 形）；codex 条目手铺
   `/var/lib/aginx/gateway/agents/codex/aginx.toml`（#410 配方）。
   Mac `agc --bind <配对码>` 配对。
5. **验收三证**（全过才算 W2 关）：① `agc agent://<id>.relay.aginx.net/
   system` 真答+sessionId；② 同址 `/codex` 真答；③ `--session` 续话
   原句复述（codex resume thread 通）。超时先查 spool 存根与
   rollout 所在 sessions 树+cwd——**判网关腿别看手动腿**。


## D14 三律

1. crates 里出现机型字符串（redfin/1080/2340/event1/sm7250…）即违宪；
   `crates/hwd` 读 device.toml 是唯一合法来源，check.sh 设 grep 门。
2. 平台可为机型留**槽位**（quirk 开关、affinity 表、参数串），但不得留
   **默认机型**——device.toml 缺失 fail-fast。兜底=隐性机型假设回流。
3. 机型目录之间不互相 import；共享资产上提 `scripts/` 或 `rootfs/`，
   不建 `devices/common/`（两台机不值得，三台再说）。

细则（2026-09-08 同批立，自 docs/ARCH.md 整档迁入）：

- 分辨率分层：运行时显示几何以 **DRM 枚举为真值**（term 主路径已是）；
  device.toml `[panel]` 是**非 DRM 消费者**的唯一来源（voice HTML 三钉、
  cam `--aspect`、host 测试、touch 原生范围）。不符时记警告不致命。
- 老仓 boot/ 管线以 plain copy 迁入 `devices/redfin/boot/`（基线注记
  `from aginxos@0534ea8`），老仓封存为只读档案。
- 豁免注记：update 的 SWAP/BAK/STATE 偏移与冻结的 first-gen trampoline
  对偶，暂留 crates（登记于 device.toml `[update.layout]`）；豁免解除时
  连 trampoline 一起参数化。boot_ok 的 per-LUN GPT slot 语义同理——只抽
  「值的来源」，不抽「方法」，第二种 slot 方法出现才抽象。

当前机型：redfin（首目标，在役）· enchilada（bring-up 线，P5 另立计划）·
panther-x2（服务器盒，W1 目录骑乘卖家 Armbian 链，boot_style=armbian-ride）。
