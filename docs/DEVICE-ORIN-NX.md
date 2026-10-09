# DEVICE-ORIN-NX — Jetson Orin NX 选型（agent 卡「本地大脑档」）

> **状态**：选型分析，未立项、未采购。2026-09-09 立；2026-10-09 按 #422 黑匣子翻案刷新框架——服务器板卡已一等（首购=X2，见 `DEVICE-PANTHER-X2.md`），本档座位=「更大脑档」在册观望（AGENTS.md Positioning）。
> 前提文档：一代仓 `aginxos/docs/DEVICE.md`（v1 立型：RK3576 · ¥500 BOM · 三阶段投入，只读引用；其「五官」叙事随 #422 作废）；本仓 README / docs/HARDWARE.md（redfin 现状与烘焙链）。

## 0. 一句话判定

**不能「刷」，能「移植」——重做的只有设备绑定层，Rust musl userspace 整体白捡。** 它的座位不是替代 v1 卡（RK3576 路线不动），是 agent 服务器要不要开「本地大脑档」的决策：把「断云只降级」升格为「断云 agent 仍完整在岗」。

## 1. 为什么不能直接刷

现有镜像与烘焙链整机绑定 Qualcomm 手机启动世界，Orin NX 是另一个星球：

| 绑定点 | redfin 现状 | Orin NX |
|---|---|---|
| 启动链 | XBL（熔签）→ 自研 trampoline → 4.19 downstream kernel | NVIDIA UEFI(EDK2, QSPI) → L4T BSP 内核 |
| 存储/分区 | UFS + Qualcomm GPT（`/dev/block/by-name/boot_a` 等） | **无 eMMC**：QSPI 放 bootloader，rootfs 住 NVMe |
| OTA 语义 | agupd 写 slot + ABL 自动回滚 | nvbootctrl slot + ROOTFS_AB（L4T 原生支持） |
| 身体件 | imx363→软 ISP、cs35l41 功放 DSP、DMIC、DSI panel 直驱、触控 | BSP 全带，载板外设齐全 |
| 内核 | 自己拼 downstream + vendor modules | NVIDIA 现成（JetPack 6.x / L4T r36，内核 5.15 档） |

`fastboot` 刷一个 aginxOS image 上 Jetson——不存在这种东西。反过来 JetPack 的 Ubuntu userspace 也不是我们要的：**只取它的 BSP（内核+驱动+UEFI），userspace 换成自己的 musl static**——AginxOS 本来就是这个形状。

## 2. 为什么移植比听起来近

AginxOS 的架构恰好把贵的东西和便宜的东西切开了：

**白捡层（~100%）**：Rust musl static userspace——aginx 路由/server、svcd、pkg、download/update、aginxbrowser 引擎、研究引擎 aginxresearch。musl static 不挑发行版，任何 aarch64 Linux 直接跑。「系统」这半个 OS 整体迁移零成本。

**重做层（`devices/<orin>/`）**：boot 脚本、kernel+DTB、bringup 件、cam。但方向反转：**redfin 上吃过最大的苦（拼 downstream、手搓软 ISP、功放 DSP 固件交接）在 Jetson 上是 NVIDIA 的工资**。BSP 把驱动全做完了，UEFI 是标准件，A/B 不用自研。

架构已预留多设备：`devices/` 下 redfin / enchilada / panther-x2 三线在册（X2 已把「服务器板卡一等」落成事实，#448–#453）——Orin 只是再加一行 `device.toml`，不是架构事件。

## 3. Orin NX 事实卡

| | Orin NX 8GB | Orin NX 16GB |
|---|---|---|
| CPU | 8× Cortex-A78AE | 同 |
| GPU | Ampere 1024 CUDA 核 + 32 Tensor 核 + 2× NVDLA v2 | 同 |
| AI 算力 | ~70 TOPS 档 | ~100 TOPS（Super 档 ~157，40W） |
| 内存 | 8GB LPDDR5 128-bit | 16GB LPDDR5 128-bit |
| 带宽 | 102 GB/s（理论峰值） | 同 |
| 功耗 | 10-25W 可配 | 10-25W（Super 40W） |
| 存储 | 无 eMMC，NVMe | 同 |

（算力/价格为档位概念，采购时以 NVIDIA data sheet 与当日报价为准。）

**载板路线（上车的便宜门）**：Orin Nano 与 Orin NX 同属 P3767 模块家族，共用了 P3768 载板——Orin Nano Super 开发套件（~$249 / ¥1800 档）= P3768 载板 + Orin Nano 8GB 模块。**先买便宜套件把设备线全跑通，日后拔下 Nano 插上 Orin NX 16GB 模块（~$699 / ¥5000 档），载板不换**。这正好复刻 v1 三阶段「每步可停」的哲学。

## 4. 档位判决：16GB

GPU 两档同核心数，差在内存容量。系统 + aginxbrowser + SIP 语音件（通话腿的嘴耳）+ 本地模型的组合挤 8GB 会紧；本地 7-8B LLM 在 16GB 才有实操空间。**选 16GB；8GB 只在「先验证平台」语境下有意义——而那个语境里更该买的是 Orin Nano Super 套件（§3 载板路线）。**

## 5. 它买到了什么（对照 v1 卡）

| | v1 卡（RK3576, 6 TOPS） | Orin NX 档 |
|---|---|---|
| 断云 | 只降级：远端 brain 不可达 | **断云可用**：本地 7-8B 推理撑 brain 档 + SIP 语音件照常出勤 |
| 视觉 | ISP 够用档 | GPU ISP 可用，但摄像头线已冻结（#422）——不按卖点计 |
| AginxBrain | 网关为主 | 全本机（key 路由 + 本地模型都在卡上） |
| BOM | ~¥500（万台档） | 模块+载板 ¥5000 档（单件） |
| 功耗/形态 | ~5W，卡形态，无风扇 | 10-40W，盒子形态，要散热 |

产品论点的变化：旧世界观是「云 = 模型农场，卡 = 身体」；Orin 档把云降为**可选加速器**——隐私优先、无网环境、按 token 成本敏感的用户，买的是「这台 agent 服务器不依赖任何云也在岗」。这是另一条产品线，不是 X2 档的升级版。

## 6. 坑与缓解

1. **无 eMMC，rootfs 住 NVMe**：掉电可靠性要设计。双层 A/B——QSPI bootloader 自身 slot（nvbootctrl 管）+ NVMe 上 rootfs A/B（ROOTFS_AB）。agupd 的整分区 swap 管线要对着 NVMe 重写，不是改设备名那么简单。
2. **价格仓位**：模块级 ¥5000 是 v1 BOM 的 10 倍，但分属两个仓位（旗舰实验线 vs 量产线），不构成对比。
3. **JetPack 的重量**：与最小依赖哲学有张力。可接受的做法 = 只当 BSP 用（内核+驱动+UEFI），userspace 全 musl static 自己烘——rootfs 从 L4T 的 Ubuntu 底子剥到只剩 BSP 依赖，或直接用 NVIDIA 的 rootfs 抽象自烘。剥离深度是 bring-up 期第一个要验证的问题。

## 7. 与 v1 三阶段的关系

**v1 卡线随 #422 重读**：阶段 -1（旧手机验证软件资产）已被超越——redfin/enchilada/X2 三机在役，镜像即产品；RK3576 量产卡与「USB 五官」叙事作废，若重开按黑匣子世界观另立。Orin 是并行的产品线决策：「本地大脑档」要不要开。若开，建议序：Orin Nano Super 套件起步跑通 devices/<orin> 设备线 → 换 NX 16GB 模块升算力 → 再谈产品化（服务器板卡起步位已被 X2 占掉，起步件届时再拍）。

## 8. 若立项：阶段门草案

- **W1 BSP 落地**：L4T 引导 + 自烘 rootfs（musl userspace 进驻），串口/网联 SSH 通——判据 = L0 底座起（svc.d 两单元 + ssh + pkg 面）
- **W2 A/B 与更新**：ROOTFS_AB + agupd 对 NVMe 适配，断电中断更新可回滚——判据 = 拔电重试 10 次不砖
- **W3 语音腿（SIP 件）**：载板 mic/speaker 接入 aginx-call 通话腿（agent 对外的嘴耳）——判据 = sip.aginx.net 对讲闭环；摄像头/显示是冻结线（#422），不在门内
- **W4 本地大脑**：Qwen-7B 级本地推理常驻 + brain 路由全本机——判据 = 拔外网，agent 仍可在岗办事

## 9. 待拍板

1. 「本地大脑档」产品线开不开、何时开（现在 / v1 量产后 / 观望）
2. 起步件：Orin Nano Super 套件（¥1800）先行，还是直接 NX 16GB（¥5000 档）
3. 深水区预算：rootfs 剥离深度（贴 L4T 底 vs 剥到近乎裸 BSP）——影响 W1 工期一个量级
