# 裸机线真源 TRUTH —— ABL 交接契约 + SPMI v5 arbiter 契约（2026-10-14）

本文件是无 Linux 内核启动线（bare/）的**查表真源**：逆向与实验落定的事实
全部集中于此，往后每一刀探针看到啥不对直接翻这里对表，不再现场猜。

来源标注：`[BIN]`=abl_a.img Ghidra 反编译；`[DRV]`=spmi-pmic-arb.c
（mainline + redfin 实跑 vendor 副本 `/tmp/redfin-kernel/drivers/spmi/`
两版同义核对）；`[DT]`=真机 vendor_boot DTB 反编译
（`boot-out/vendor_boot_unpack/dtb` → dts）；`[BEN]`=真机探针阶梯实测。

## 1. ABL 交接契约 [BIN]

| 项 | 值 |
|---|---|
| 内核装载/解压落点 | **0xA7E80000**（.data 常量 @RVA 0x61950；校验 `addr & 0x1FFFFF == 0x80000`） |
| 入口调用 | `(*(void(*)(dtb,0,0,0))(0xa7e80000))(dtb,0,0,0)`，x0=DTB |
| CPU 状态 | MMU OFF、I/D-cache OFF、TLB 已 invalidate、DAIF 全掩、EL=ABL 的 EL |
| 接受的载荷 | LZ4 frame（`04 22 4D 18`）/ gzip（`1F 8B 08`）/ raw arm64 Image（magic `0x644D5241` @+0x38，text_offset 必须 0x80000） |
| 备用压缩落点 | 0x82200000（gzip）；解压目标同样过 2MB+0x80000 校验 |

B1–B4 六代冻死根因：按 0x80008040 绝对链接，字面量偏 ~0x278 万，BSS 清零
首写 0x80008xxx = 低 DRAM TZ fence 区 → 总线永不应答（stall 非 abort，
VBAR 接不住）。**修法 = 全位置无关、零 DRAM 写、零字面量。**

## 2. SPMI PMIC arbiter v5 契约 [DRV][DT]

真机 DT 节点 `qcom,spmi@c440000`（compatible `qcom,spmi-pmic-arb`）：

| reg 名 | 基址 | 大小 | 用途 |
|---|---|---|---|
| core | 0x0C440000 | 0x1100 | VERSION@0x0、FEATURES@0x4、APID 表@0x900+4N（512 项正好铺满） |
| chnls | 0x0C600000 | 0x2000000 | **写**通道窗，每 APID 0x10000 |
| obsrvr | 0x0E600000 | 0x100000 | **读**观察窗，每 APID 0x80，另加 `0x10000*ee` |
| intr | 0x0E700000 | 0xA0000 | 中断 |
| **cnfg** | **0x0C40A000** | 0x26000 | **owner 表@0x700+4N**（`val & 7` = 拥有者 EE） |

- `qcom,ee = 0`、`qcom,channel = 0` [DT] → APPS 观察窗偏移项 `0x10000*0=0`，读地址 = `obsrvr + 0x80*apid` 无附加项。
- 版本判定：`VERSION@core+0`，v5 ∈ [0x50000000, 0x70000000)。
- APID 数真源：`FEATURES@core+0x4 & 0x7FF`（不拍 512）。
- APID 表项：`ppid = (v>>8) & 0xFFF`；v==0 = 空位跳过；bit24 = IRQ owner 标。

### 命令字（fmt_cmd_v2，驱动与 battery-truth §6 逐位一致）

```
cmd = (opcode << 27) | ((addr & 0xFF) << 4) | ((len-1) & 7)
```

地址域**只有 8 位**——寄存器高位由 APID 的 PPID 承载；高地址位漏掩码 =
命令字 bit26:12 塞垃圾（违例命令）。arbiter 内部 opcode（非 SPMI 总线码）：
READ=15、WRITE=14、EXT_READ=13、EXT_WRITE=2、EXT_READL=1、EXT_WRITEL=0。

### 通道内寄存器 [DRV]

CMD@0x00、CONFIG@0x04、STATUS@0x08、WDATA0@0x10、WDATA1@0x14、
RDATA0@0x18、RDATA1@0x1C。STATUS 位：DONE=bit0、FAILURE=bit1、
DENIED=bit2、DROPPED=bit3。轮询 DONE→查 1/2/3 位判果。

### v5 所有权铁律（本文件最重要的一条）[DRV]

- **同一 PPID 可挂多个 APID 副本**——正是为了让多个 EE 各自可写同一外设；
  **owner 表（cnfg+0x700+4N）的 `val&7` 指定允许写该 APID 的 EE**。
- 写通道软件门：`apid_data[apid].write_ee != ee → -EPERM`（驱动选 APID 时
  就地拒绝）；硬件层对非 owner 的 chnls 窗访问行为未证——**这是 B7/B12
  冻死的最强候选**。
- 读观察窗无 write_ee 门（任一 APID 副本皆可读）。
- **正解姿势**：扫描全表收集该 PPID 的**所有** APID，读 owner 表选
  `owner&7==0`（APPS）的那个副本再发写事务；取首个匹配是错的。

### 驱动 probe 的全部初始化 [DRV]

VERSION 读 → FEATURES 读 → APID 表扫描 → owner 表逐 APID 读缓存。
**零解锁写、零时钟操作**——纯 mmio 事务在 ABL 退出后的 EL1 上理应直接可
用（Linux 就是这么跑的）。若选对 APID/EE 仍不行，问题在硬件状态不在软件
姿势。

## 3. PMIC 侧事实 [BEN][DT]

- PM7250B：CHGR=PPID 0x210（SID2），PON=PPID 0x208（SID2）。
- 充电使能寄存器 0x1042 bit0；喂狗 0x1643←1；PON 装弹 0x85A←1（warm）/
  0x85B bit7（保险），两段序列见 battery-truth.md §7。
- PS_HOLD 拉闸 = 0x0C264000 ← 0，前置铁律：arbiter 总线闲。

## 4. 探针阶梯账（B 系列，单变量递进）

| 刀 | 内容 | 结果 | 裁决 |
|---|---|---|---|
| B1–B4 | 绝对链接各代 | 冻死 | 根因=链接基址（§1） |
| B5 | 36B 纯心跳 | **blink×3+回滚** | L1 首证（commit 8a5a241） |
| B6 | 校准延时 | blink | 延时尺度定标 |
| B7 | 全量 substrate（未掩码命令） | 冻死 | — |
| B8 | core 表首项读 | blink | core 窗可读 |
| B9 | 512 项全扫 | blink | 读空间清白 |
| B10 | 未掩码读事务 | blink | 现判：违例命令被命令解析器即拒，未上总线 |
| B11 | 未掩码写事务 | blink | 同上 |
| B12 | 掩码修复全量 substrate | 冻死 | 根因=取首个 APID 副本（见 §5 裁决） |
| B13 | 单发合法读（掩码） | **blink×3**（80s 回 adb） | 合法读事务清白 |
| B14 | cnfg owner 窗纯读 | **blink×3**（25s 回 adb） | owner 表可查（cnfg 窗可读） |
| B15 | owner-gated 单写（选 owner==0 副本发 0x1643←1） | **blink×3**（25s 回 adb） | owner-gated 写清白（且证 CHGR 有 APPS 副本） |
| B16 | 全量 substrate v2（充电 RMW+5 喂狗+PON 装弹+PS_HOLD，全 owner-gated） | **blink×3**（25s 回 adb） | **substrate 完工** |
| B17 | PON 所有权判别（找到→闪灭/没有→故意冻死） | **blink×3**（25s 回 adb） | PON 有 APPS 副本，复位动词两级（PON warm+PS_HOLD）皆可用 |
| B18 | splash fb 双格式色带（0xA0000000+row500/700） | 无带，黑屏 | fb 判活路判死：面板=命令模式（cmdline `_1080p_cmd`），显存在面板 GRAM，host DRAM 不被扫描 |
| B18b | 白带单格式判别（0xFFFFFFFF 全格式不透明） | 无带，黑屏 | 同上定谳；DSI 推帧=阶梯 3 挂账刀 |
| ka | RUNG2 在役 substrate（充电 RMW+永续喂狗，不复位） | **静止 logo ≥10min+双逃生真考** | 阶梯 2 整机刷在役（§6） |

## 5. 裁决与下一阶梯

1. **B7/B12 冻案闭案**：取首个 ppid 匹配的 APID 副本 → 撞上非 APPS 拥有的
   副本 → 硬件拒绝行为=挂死。正解=owner-gated 选择（§2 铁律），B16 全量
   重跑闪灭实证。
2. **substrate 完工（B16）**：ABL 退出后的裸 EL1 上，SPMI 充电使能、喂狗、
   PON 装弹、PS_HOLD 拉闸全部可做——无 Linux 内核与 PMIC 对话的能力
   已齐，阶梯 2（整机刷）解锁。
3. B17 判 PON 归属：**blink=PON 有 APPS 副本**——复位动词两级皆可用，
   substrate 全保留（PON warm 为首选，PS_HOLD 为兜底）。

## 6. 在役态（RUNG2，2026-10-14）

redfin 运行面=裸 substrate（ka.S，slot a active），b 槽=L0 救生艇
（`slot-successful:yes`，retry=7）。五项验收四闭一挂账（电流表无表）。

- **稳态判据（二值）**：静止 Google logo=活；早期崩=blink×3→黑屏
  （L0 自动回滚）；盲区（环死狗咬）=USB 电流表（72s 掐充电→电流掉）。
  fb 判活路已判死（B18/B18b，命令模式面板）。
- **逃生 A 机制真值**（fastboot getvar 实读）：每次开机经 ABL 烧一条
  tries；烧尽后 ABL 把 a 标 `slot-unbootable:yes`（不只是换 active），
  落 b。实测强关→开机 ×3–4 轮触发（刷装/set_active 重臂 tries=3 后，
  第 4 次开机落 b）。回裸=fastboot `set_active a`（清 unbootable+
  重臂 tries）。
- **面切换环（全通，2026-10-14 闭）**：裸 →（12s 强关→开机 ×3–4）→
  b 槽 L0（wifi ~580s 回归实测）→（ssh `/usr/bin/aginx-reboot
  bootloader`）→ fastboot（闸门串号核对）→ `set_active a` → reboot →
  裸（静止 logo 眼验）。
