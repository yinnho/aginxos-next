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

## 7. UFS 存储契约（阶梯 3 研读，2026-10-14）[DRV][DT]

源：`/tmp/redfin-kernel/drivers/scsi/ufs/`（ufshcd.c/ufshci.h/ufs.h，
JEDEC UFSHCI 标准寄存器）+ 活体 DT。盘=128GB SK Hynix，bootdevice。

### 硬件事实

- **UFSHCI @ 0x1d84000**（窗 0x3000）；ICE @ 0x1d90000（加密面，裸件跳
  过）；PHY @ 0x1d87000（qmp-v4-lito）；IRQ 265；双 lane；ref clk
  19.2MHz；core reset 走 GCC（resets 0x4f 0x0c）。
- XBL/ABL 刚用 UFS 读过 boot.img——控制器大概率仍在活态（U1/U2 定谳）。

### 寄存器图（JEDEC 标准，ufs.h/ufshci.h 偏移枚举）

| 偏移 | 名 | 用 |
|---|---|---|
| 0x00 | CAP | NUTRS/NUTMRS 槽数（byte0=NUTRS-1? 读出即真相） |
| 0x08 | VER | 版本（0x201/0x210/0x300/0x310 合法域） |
| 0x20/0x24 | IS/IE | 中断状态/使能（裸件纯轮询） |
| 0x30 | CS | UCRDY 等 |
| 0x34 | **HCE** | bit0=host enable（装弹=写1轮询1） |
| 0x50/0x54 | UTRLBA/H | UTR 列表基址（1KB 对齐） |
| 0x58 | **DBR** | 门铃：写 1<<tag 发单；完成=位自清 |
| 0x5C | CLEAR | 完成位清除 |
| 0x60 | **RSR** | bit0=UTR 列表跑/停 |
| 0x90–0x9C | UICCMD/ARG1-3 | UIC（LINKSTARTUP 等，全 init 才用） |

### 最小读路径（若 ABL 留活——U2 判）

DRAM 布阵（MMU/cache 关=DMA 天然一致）：UTRDL @0xB0000000（1KB 对齐、
32B）、UCD @0xB0001000（512B 对齐：命令 UPIU[512]+响应 UPIU[512]+PRDT）、
数据 @0xB0002000。

- **UTRD**：DW0=0x14000000（见下代际律；旧 0x05000000 已废）；DW1=0
  （DUNL，见下）；DW2 预填 0x0F（invalid OCS，完成后硬件
  写真值，mask 0x0F）；DW3=0；DW4/5=UCD 地址 LE64；DW6=响应 UPIU
  off=0x200/len；DW7=PRDT off=0x400/len=1。
- **命令 UPIU（BE32 打包，宏 `UPIU_HEADER_DWORD`）**：[0]=0x01
  COMMAND；[1]=0x40 READ；[2]=LUN；[3]=tag；[4..7]=0（SCSI 命令集）；
  [12..15]=exp_len **BE32**；[16..31]=CDB。响应 UPIU trans=0x21。
- **READ(10) CDB**：[0]=0x28；[2..5]=LBA BE32；[7..8]=块数 BE16；其余 0。
- **PRDT 条目**：数据地址 LE64 + reserved + 字节数 LE32（≤256KB）。
- 发单：UTRLBA←列表址 → RSR←1（若停）→ DBR←1 → 轮询 DBR bit0→0 →
  OCS==0（OCS 表：0 成功/3 尺寸不匹配/5 通信失败…）→ 数据判读。
- **判据刀**：读 LBA1 首 8 字节 = `EFI PART`（GPT 头）——块尺寸
  512/4096 两世界同形（4096×1 块 vs 512×8 扇区，buffer 首都指 GPT 头）。

### 刀序（单变量，blink/freeze 二值）

U1 MMIO 探针（读 CAP+VER，VER 合法→blink；冻死=时钟没开）→ U2 活态
（HCE==1&&RSR==1→blink；否则全 init 路径：HCE 装弹+UIC LINKSTARTUP）
→ U3 UTR 往返（READ CAPACITY(10) 8B，OCS==0→blink）→ U4 首块读
（READ(10) LBA1，`EFI PART`→blink=**无内核读盘首证**）→ U5+ 写路径
（boot_a 尾部 scratch 写 pattern→强关回 L0 dd 对账=全带宽读出通道）。

### 风险

- ABL 可能关时钟离开（U1 冻死即知；GCC 时钟使能=备用大刀）；
- 全 init 路径（HCE+LINKSTARTUP+PHY 校准）深=能免则免；
- ICE/inline 加密不碰（裸件走非加密路径，CRYPTO_ENABLE 位=0）。

### 编码律（2026-10-14 三常数案后立，源码=ufshcd.c:5048/ufshci.h:42,514-563）

- **UTRD DW6/7 = offset<<16 | length（ufshci.h:556-562：`__le16 length`
  在低址 +24/+28、`__le16 offset` 在 +26/+30），offset 单位 dword（字节
  >>2）**。DW6=0x00800080（RUO 0x80 | RUL 0x80，对称）；**DW7=0x01000001**
  （PRO 0x100=PRDT 在字节 +0x400 | PRL **1 条目**）。字节粒度是
  UFSHCD_QUIRK_PRDT_BYTE_GRAN 的旁路，本机不适用。**10-14 夜翻案**：
  下文「DW7 两半装反」定谳本身是反的——u3 原值 0x01000001 才对，
  u4d/u4e/u3d 首版的 0x00010100 = 256 条目 @UCD+4，才是 DMA 不发生的形。
- **UTRD DW1 = DUNL（crypto 数据单元号低 32 位）**，crypto 关时保留=0
  （ufshcd.c:3631）——不是数据长度；DW3=DUNU 同理 0。旧探针 DW1=字节数
  属误读（CE=0 下大概率无害，但不合契约）。
- **UCD 布局**：命令 UPIU[0x200] + 响应 UPIU[0x200] + PRDT@+0x400
  （ALIGNED_UPIU_SIZE=512）。响应 UPIU trans=0x21、byte4=UTP 响应码、
  byte5=SCSI 状态（0=GOOD）、byte16+ sense。
- **BE 字段双字节序律**：UPIU 是字节序列——头四字节 [0]=transcode
  [1]=flags(0x40 读) [2]=LUN [3]=tag → LE dword 0x01004001；exp_len/
  LBA/块数同理「BE 目标字节 → 反排 LE 装载」。与 PS_HOLD 地址并列的
  三大常数坑：LBA `0x0100<<16`（非 0x0001<<16）、头 `movk #0x4001`、
  DW7 `movz #0x0100,lsl#16 + movk #1`（=0x01000001，10-14 夜翻案后）。
- **BE32 落盘通式=rev（第四案后立律，2026-10-14 晚）**：`movz #0xNNNN,
  lsl #16` 只对 0x000000NN 形状对；512 的 BE 字节是 00 00 02 00 →
  落盘 LE 应为 0x00020000，`0x0200<<16`=0x02000000 落成 BE32=**2**。
  u4d/u4e 的 exp_len 就这样发了 16/2 字节（非 4096/512）——两冻判词
  作废，试验不干净。**凡 BE32 一律 `mov w10,#原生值` + `rev w10,w10`
  再 str**，不再手算移位。
- **DW0 代际律（源码 ufshcd.c:3778 / ufshci.h:449-464）**：DW0 =
  type<<28 | DD<<26 | INT<<24；UFSHCI≥2.0（本机 sm7250）SCSI 读 type=
  UTP_CMD_TYPE_UFS_STORAGE(1) → **0x14000000**。旧探针 0x05000000 =
  type0（1.x 代际 SCSI 型）+INT 位，代际错（OCS 仍 0=控制器宽容）。
  INT 位仅 intr_cmd（设备管理命令）置位，SCSI I/O 不置。
- **tag=门铃槽位律**：内核 task_tag==slot，DBR bit0 → UPIU 头 tag
  字节=0（旧探针=1 不符）。
- **缓存世界**：ka.S/pi2.S 零 sctlr 写 → ABL 交接即 AArch64 启动协议
  态（MMU off、D-cache off）非缓存世界，DMA 写即所见，无需 cache
  维护；dc cvac/ivac 全免。

### 刀账（2026-10-14 实测）

- **仪器**：blink=fail-pass 唯一通道不变；判冻=12min adb 看门狗静默
  +tries:a 计数（冻=2/1，blink×3 走完=0 并自落 b 槽 L0 adb 回归）；
  mDNS 对本 L0 全盲。adb 回归时刻（80–100s）分辨不了小延迟，EL 判定弃。
- **刀0 翻案（PS_HOLD 半字节案）**：u1aa 首版 `movz #0xc264,lsl#16`
  =0xC2640000 写歪→外部中止→logo 冻；正解 `movz #0x0c26,lsl#16`+
  `movk #0x4000`。19 件 u 系同雷全翻案（u1a/u2/u3 修后皆逃）。
  mrs CurrentEL 无罪（u1ab 单变量证）。
- **绿**：u1a CAP 读返回；u2 HCE==1&&UTRLRSR==1（免全 init）；u3
  UTR 往返（门铃清+OCS==0）——**但数据空心**（见下）。
- **冻族与三常数案**：u4/u4b（LBA 256 雷字节序→修 0x0100<<16 仍冻）；
  u4 头修 0x01004001（LUN0/flags40）仍冻；u3b（头+PRDT「搬 +0x100」
  +非零门）冻——搬错了（那是 dword 单位不是字节）。**根因=DW7 两半
  装反 0x01000001（len256/off1），控制器在字节+4=命令 UPIU 肚子里读
  PRDT，DMA 从未发生**：u3 空心绿、u4 族 magic 对零、一案全解。
- **u3c**（响应门 +0x80 版）出炉即废（门错位，未刷）；**u4d**（源码
  全契约版：DW7=0x00010100+PRDT 归位+0x400）判读挂账。
- **u4d/u4e 双冻、判词作废（10-14 晚定谳）**：4096/512 两世界都冻，
  但当晚源码复核抓出 exp_len 落盘常数形错（上条 rev 律）——发的是
  16/2 字节，不是干净的契约试验。「双世界排除」不成立。
- **u3d（READ CAPACITY 分诊刀）出炉待刷**：CDB[0]=0x25、8 字节、
  DW0=0x14000000（代际修）、tag=0（槽位修）、exp_len 走 rev、数据
  缓冲先清零再非零门（防陈旧假阳）。反汇编验尸全绿。判读：blink=
  命令被收且数据到门（数据路全通→转 READ(10) 首证）；冻=转 u3e
  响应门（+0x200 transcode 0x21+status GOOD）分层。

- **10-14 夜复核两修（u3d 刷前）**：①DW7 翻案——ufshci.h:560-562
  `prd_table_length` 在 +28（低半）、`prd_table_offset` 在 +30（高半），
  正值 0x01000001；u3d 首版 0x00010100 未刷即重烤。u3 原值本就对，
  其「空心绿」另有因（DW0 代际/exp_len 形/tag 三者之一或合力）；
  u4d/u4e 双冻再添一重不干净（DW7 也错）。②DW1 归零（DUNL 保留）。
  重烤 u3d 反汇编：+0x48 DW1=wzr、+0x74/78 DW7=0x01000001，余常数不变。
- **10-14 夜 UFSDxe 逆向（Qualcomm 本机 UEFI UFS 驱动=最近参照实现）**：
  factory bootloader FBPK→xbl 段 0x9fc00000 FV→gzip 包内 FV→UFSDxe.efi
  （508K，Ghidra 全反编译）。定谳：①UTRD 组装 FUN_d2c0：DW0=type<<28|
  dir<<25|int<<24、DW1=0、DW2=0xF、DW6/7=(off_bytes&0x3fffc)<<14|len——
  DW7 翻案独立证实；②**SCSI 读 type=0+INT=1 → DW0=0x05000000**（u3 旧值，
  非 Linux 的 0x14000000——「代际律」只是 Linux 习惯，不是本控制器
  刚见过的形）；③**tag=递增计数器（ctx+0x3c++），与槽位无关**——
  「tag=槽位律」撤销；④门铃前写 IS(0x4020)=5 清中断、查 RSR(0x4060)，
  完成判=IS bit0 且 DBR 位清；⑤**全部 HCI 访问 32 位**。
  UFS MMIO 基 0x1d80000、HCI=+0x4000；UEFI 跑 cacheable+显式 cache 维护
  （FUN_133dc）。uefiplat.cfg：0xB0000000 ∈ "Kernel Expanded"
  0xAFE00000–0xB7E00000 SYS_MEM（普通 DDR，非 carveout）。
- **64 位写嫌疑（全冻共因）**：u3 是唯一 `str w10,[x3,#0x50]`（UTRLBA
  32 位写）且唯一过门；u3b/u4/u4b/u4c/u4d/u4e/u3d 七件全 `str x10`
  （64 位写 32 位外设寄存器）全冻。另：u3 头 LE 0x01400001 落盘 01 00 40 01
  =flags0/LUN0x40——「空心绿」真因是坏 LUN（OCS 只管传输层）。
  **律：MMIO 一律 32 位 `str wN`**。u3e=u3d 单变量换回 32 位写，出炉
  机器码对 u3d 仅 +0xb8 一条差（f900286a→b900506a）。
- **u3e 冻（10-14 夜）**：64 位写单独不是真凶。证据形=唯一过门的 u3 是坏
  命令（LUN0x40/flags0，无数据），凡有效读+要数据者七件全冻——一比特仪器
  混五种结局。**响应 UPIU 偏移律**：byte6=response、byte7=SCSI status
  （Linux be32(dword_1)&0xFFFF；UFSDxe FUN_d1c4），旧档 +0x205 错位。
  嫌疑新增 UNIT ATTENTION（上电后首条有效命令常回 CHECK CONDITION，真驱动
  都重试）。**u3g=时延编码仪器**：命令体=u3e、任何结局都复位、阶段编码进
  复位前等待 2+30×stage 秒（CNTVCT）：0 过/1 门铃 1s 超时/2 OCS≠0/3 trans
  或 response 坏/4 status≠GOOD/5 数据零；仍冻=CPU 中止（或 CNTVCT 被陷）。
- **u3g 首判（10-14 21:48 刷）**：adb 354s 回归、slot_b、串号 redfin——
  三轮全复位（非冻）。按基线 40–100s + 3×30×stage 判 **stage3**
  （310–370s 窗；stage2 窗 220–280、stage4 窗 400–460）：门铃清+OCS=0，
  但响应 UPIU byte0≠0x21 或 byte6(response)≠0。首次把失败钉到阶段：
  CPU 中止/总线挂死/门铃超时全排除。旁证：L0 内核 DEVMEM 关且
  0xA0000000–0xB7E00FFF 为 System RAM——复位后读 DDR 残留路不通。
- **u3h 判（10-14 21:57 刷）**：adb 285s 回归 slot_b；以 u3g 校准基线≈78s →
  d≈69s → 62s 类：响应 0x21 但 Response(byte6)≠0。**UFSDxe 响应律**：成功=
  Response==0；CHECK CONDITION=Response≠0&&Status==2&&sense 长≠0（本机非
  GOOD 必伴 Target Failure）；FUN_91a0 解 sense（+0x20 长 BE16、+0x22=0x70、
  +0x24 key），**key 6 UNIT ATTENTION 视为无害**。BlockIo->Reset=FUN_14dc=
  向 W-LUN 0xD0 发 START STOP UNIT POWERDOWN（"PON"）；pre-EBS 事件组
  b1eb3d10-… 的 UFSDxe 处理只置标志、不下电。L0 侧 LUN0 在
  （SKhynix H9HQ16AFAMMDAR，sda 4096B 块）。u3i=读 sense key+UA 重试一次。
- **u3i 判（10-14 22:10 刷）**：adb 285s 回归 slot_b（同 u3h 数）→ 62s 类=
  **sense key 2 NOT READY**（非 UA，重试支未走）。对上 UFSDxe：Reset=PON
  （SSU POWERDOWN→W-LUN 0xD0），开 W-LUN 先 SSU ACTIVE。u3j 照 Qualcomm 序：
  SSU ACTIVE→W-LUN 0xD0（UA 重试 1）再 READ CAPACITY→LUN0（UA 重试 ≤2）。
- **u3j 过（10-14 22:21 刷）**：adb **103s** 回归 slot_b、串号 redfin——落
  2s 过窗（84–105s；次档 32s 类≥174s）。**无 Linux 内核下 UFS 数据路首通**：
  SSU ACTIVE→W-LUN 0xD0 后 READ CAPACITY→LUN0 回 GOOD 且 DMA 落盘 8 字节
  非零。**根因定谳**：交接前 bootloader 栈把 UFS 设备留在 PowerDown（UFSDxe
  BlockIo->Reset=PON），普通 LU 一律 NOT READY；须先 SSU ACTIVE 唤醒 W-LUN。
  控制器/SMMU/UTRD/UCD/PRDT/UPIU/数据 DMA 全链在役。下一刀 u4f=READ(10)
  LBA1→「EFI PART」magic 门（读盘首证）。
- **u4f 过——RUNG3 读盘首证（10-14 22:26 刷）**：adb **106s** 回归 slot_b、
  串号 redfin（过窗 84–109s）。SSU ACTIVE→W-LUN 0xD0 唤醒后 READ(10) LUN0
  LBA1 1 块 4096B，缓冲首 8 字节 == "EFI PART"。**无内核读盘契约（定稿）**：
  UTRLBA/RSR 32 位写 → SSU（DW0 0x10000000、DW7 0x01000000、头 01 00 D0 tag、
  CDB 1B…[4]=0x10、无数据）→ READ（DW0 0x14000000、DW7 0x01000001、头 01 40 00
  tag、exp_len rev(字节数)、CDB 28 [2..5]=LBA BE [7..8]=块数 BE、PRDT size=字节-1）；
  每发 tag++、门铃 1s 超时、OCS→响应 0x21→Response→sense 逐层判。收据 #492。
- **u4g 判（10-14 22:35 刷）**：adb 649s → 182s 类=门铃 1s 超时（CPU 未死、
  非 OCS）。SSU/读 #1 字节级同 u4f（皆过），新者=读 #2（LBA2→0xB0010000）。
  嫌疑=DMA 目的地址出 UEFI 的 UFS SMMU 域可达范围（成功 DMA 皆落 0xB0000000
  首 12KB）。u4h=读 #2 缓冲改 0xB0003000 单变量 + 分命令超时码（SSU 182/
  读#1 302/读#2 332）。
- **u4h 判（10-14 22:51 刷）**：adb 1097s → 332s 类=读 #2 门铃超时（缓冲已改
  0xB0003000，地址无罪）。同证：读 #1 过、头健全、**头 CRC32 中**（GPT 头逐字节
  对+设备上 CRC 例程对）。读 #2=同槽第三条命令；过关者皆只两条。UFSDxe 每发：
  敲铃前 IS=5+查 RSR==1、等待中盯 IS 0x31804、出错 UTRLCLR、完成写 IS=1——
  我方全无。u4i=SSU→读 LBA2→读 LBA1 + 敲铃前旁读 RSR/IS。
- **u4i 过（10-14 23:12 刷）**：adb 100s——SSU→读 LBA2(第2条)→读 LBA1(第3条)
  皆 GOOD，敲铃前 RSR/IS 皆净。**u4g/u4h「读 #2 超时」翻案=我方 bug**：crc
  例程以 w3 装多项式 0xEDB88320 踩了全局 HCI 基址 x3，读 #2 门铃写进 DDR
  0xEDB88378、轮询读回自写的 1→假超时。DMA 地址/SMMU/第三条命令三嫌疑全撤。
  **全局寄存器律**：x3 HCI 基/x8 UTRD/x9 UCD/x20 CNTFRQ/x23 tag/x25 缓冲/x29
  码/x24 返址——子程序禁写；反汇编验尸加「全局寄存器写入审计」一项。
  u4j=u4h+crc 多项式改 w13。
- **u4j 过——GPT 分区表无内核自校验读通（10-14 23:16 刷）**：adb **103s**
  （过窗 84–109s）。SSU 唤醒→读 LBA1 GPT 头（magic+HeaderSize 92/SizeOfEntry
  128/N 19/ELBA 2 健全）→**头 CRC32 中**→按头读 LBA2 条目数组 2432B→
  **数组 CRC32 中**：LUN0 19 条分区（ssd…super/userdata）逐字节读对。CRC 在
  设备上算（反射 0xEDB88320），自校验无需宿主比对。收据 #493。
- **ka.S v2 在役验收（10-14 23:21 刷 slot a）**：开机先跑 u4j 存储自检（SSU
  唤醒+GPT 双 CRC），过→v1 常驻 substrate（充电使能+永续喂狗），败→时延复位
  自报。判据三证：①logo 静止 ≥10min、用户眼验全程无一次黑屏（任一失败档单轮
  ≤332s 即复位黑屏）；②21min adb 静默（失败档全部超窗）；③**fastboot
  `slot-retry-count:a=2`**——刷写重臂 3、整段只开机一次耗一命，从未复位
  （失败路径每轮烧一命、3 轮落 b）。=存储自检过、常驻不复位。逃生 B 收尾：
  VolDown→fastboot（闸门过）→`set_active b`→reboot→L0 adb 37s。v2 修 v1
  潜伏 bug：无 CHGR APPS 副本时 x7 未初始化即进喂狗环（ka_main 先 x7=-1）。
  盲区承 v1：「跑几分钟后死」与常驻同形——电流表挂账。
- **u4k 过——按 GPT 名找分区读通（10-15 00:30 刷）**：adb **97s**（过窗）。
  u4j 双 CRC 校验后的条目数组里按 UTF-16LE 名找 "boot_b"（+56 u64 0x0074006F006F0062
  ＋ +64 u64 0x62005F，含 \0 终止符防前缀误中；8 对齐整字比）→ 取 +32
  StartingLBA → READ(10) 1 块 → 首 8 字节 "ANDROID!"。期望由 L0 只读 dd 定：
  条目 6、LBA 33288、boot header v3。**名→LBA→数据** 全链在裸件上闭合。收据 #495。
- **u5a 过——写路径首证，双边验收（10-15 05:24 刷）**：裸件 adb **100s**（过窗：
  WRITE(10) FUA + 回读 512 字逐字中）。L0 侧 dd LBA 25095–25097 宿主核：25096
  w0="AGXWRT01"、戳 0xEC7492F（CNTVCT≈12.9s，三轮末轮所写）、w2..511 依戳复算
  **0 差**；邻块 25095/25097 仍全零（只写一块）；boot_b 首 16 块 md5 49aa9390…
  与刷前基线同（救生艇无损）。**写契约**：DW0 0x12000000（CT1、DD host→device）、
  UPIU 头 01 20 00 tag（flags W）、CDB 2A 08(FUA) LBA BE … 块数 BE、exp_len=字节、
  PRDT 指源缓冲。目标地址=校验后 GPT 名查 boot_a start+16384 且 ≤ end（不硬编码）。
  boot_a 未见 PON/LBA 写保护（无 DATA PROTECT）。收据 #496。
- **喂狗落错寄存器——ka v1/v2 实为每轮暂停充电（10-15 读码定谳，未上机验）**：
  SPMI 命令只带 `addr&0xff`，外设由 APID 定（PPID=sid<<8|addr>>8）。v1/v2（及
  B15/B16）用 CHGR 的 APID（PPID 0x210）发「0x1643←1」，实落 **CHGR+0x43=0x1043
  CHARGING_PAUSE_CMD_REG**；真看门狗 BARK_BITE_WDOG_PET_REG=**MISC_BASE(0x1600)
  +0x43**（PPID 0x216）从未被喂（smb5-reg.h：CHGR_BASE 0x1000、MISC_BASE 0x1600、
  CHARGING_ENABLE_CMD 0x1042、CHARGING_PAUSE_CMD 0x1043、BARK_BITE_WDOG_PET
  0x1643、BATTERY_CHARGER_STATUS_1 0x1006 bits[2:0] 0 INHIBIT…3 FULLON 4 TAPER
  6 PAUSE 7 DISABLE）。推论：ka 使能充电一次后每轮写暂停；插电 72s 看门狗咬→
  禁充——**在役 substrate 极可能从未充电**。B15/B16 的「喂狗 DONE」只证事务完成，
  不证写到对的寄存器。**SPMI 律：每个寄存器先按 addr>>8 定外设 PPID，再取该 PPID
  的 owner-gated APID**。修法（待做 v3）：apid_misc=own(0x216) 喂 0x1643；chgr
  写 0x1043←0 解暂停；运行态读 0x1006/0x1042/0x1043/0x1651 落盘状态块
  （LBA 25096）验证。收据 #497。
- **ka.S v2.1 短验过（10-15 05:49 刷 slot a）**：v2 + GPT 名查找（双 CRC 后按名找
  boot_b、首块验 ANDROID!，过才进 ka_main）。判据：用户眼验 05:56 前无一次黑屏
  （最慢失败档 392s 单轮必复位）；816s adb 全程静默至用户进 fastboot；
  `slot-retry-count:a=2`（只开机一次、从未复位）。短验后 `set_active b`→L0 adb
  37s 回归（v2.1 SPMI 段仍带 #497 缺陷，不久留）。收据 #498。

## 8. 异常与 EL 契约（S0-3 实测，2026-10-15）[DT]

- **裸件运行于 EL1**（CurrentEL 定谳；boot 记录与 EXC 现场三轮全
  `el=1`）。「EL 判定」挂账闭案。
- **VBAR_EL1 装订活**：16 槽标准表（2KB 对齐、`.vectors` 段
  `ALIGN(0x800)`），`msr vbar_el1`+`isb` 后异常即入桩；u1f 网结论
  （2026-10-13）在 Rust 线复证。
- **MMU-off 全 Device 属性 → 非对齐访问必 Alignment fault**，无需
  先动 SCTLR.A（SCTLR.A 只辖 Normal 内存；Device 内存对齐检查硬开）。
  `ldr w6,[x7]`、x7=RAM+1 实测三轮全炸。
- **ESR 形状**（Data Abort current EL）：EC=0x25、DFSC=0x21（对齐
  错）如预期；**IL=1**——与 32 位访问不符，alignment fault 的 ISS
  零修饰格式硬件恒报 IL=1（判 ESR 别拿 IL 推访问宽度）。
- **向量槽定位**：current EL using SP_ELx 的 sync 槽 = 表偏移 0x200
  （16 槽序：象限 [SP0, SPx, lower-A64, lower-A32] × [sync, irq,
  fiq, serror]，idx = 象限×4+kind）——实测 s=4。
- **ELR = 触发指令自身地址**（非下一条）；FAR = 非对齐地址原样。
- **trap 后 UFS 事务可重铺**：`Ufs::new`+`init()` 即可继续读写，无需
  重发 SSU（设备仍 ACTIVE）——异常遗言落盘复用主流程契约。

## 9. MMU 与 cache 契约（S0-4 实测，2026-10-15）[DT]

- **单 L1 表形状在 Rust 线复证**（u1x-probe.S 2026-10-13 首证）：
  T0SZ=25 使 TTBR0_EL1 直指一张 L1（1GB 块、512 项×8B），无需 L0。
  TCR=`0x2_0000_3019`（T0SZ=25|SH0=inner|TG0=4KB|IPS=40bit）。
- **L1+L2 混合布阵活**：e0/e1/e3=Device 1GB 块（MMIO 全居此），e2=
  表描述符→L2（512×2MB WB）；L2 局部覆 NC（attr idx2）即 DMA 窗。
  `.bss` 里 `#[repr(align(4096))]` static mut 双表即可满足 TTBR 对齐
  律——无需改 rust.ld。
- **MAIR 三属性**=`0x00_44_FF_04`（idx0=0x04 nGnRE、idx1=0xFF WB、
  idx2=0x44 NC）；描述符低位 Device=0x401 / WB=0x405 / NC=0x409
  （attrindx=bits[4:2]）。
- **DMA 区 NC 布阵下 ufs.rs 零维护点**：UTRD/UCD/DATA/ARRAY/STAGING/
  栈全落 [0xB0000000,0xB0200000) 16MB 窗（L2 项 384..391），read10/
  write10 无需任何 cache 清洗——cache-on 后 GPT 双 CRC 与日志环读写
  三轮全对。
- **SCTLR M|C|I（0x1005）可开**，序列=tlbi vmalle1→dsb→isb→配
  TTBR/TCR/MAIR→isb→tlbi→dsb→isb→SCTLR orr→isb。开闸后取指不断流
  （代码区 WB），零 EXC。
- **cache 收益实测**：GPT 双 CRC 链（2×4KB 读+2×CRC32+解析）cache-off
  ≈24,200 cycles → cache-on ≈10,870 cycles @19.2MHz（1.26ms→0.57ms，
  **2.2×**，含翻译开销；主要来自 I-cache 对 CRC 循环）。256 块扫环
  ≈462k cycles（≈24ms，≈96µs/块，DMA NC 窗读）。
- **Rust 优先级雷在案**：`0x8000_0000 + i << 21` 解析成
  `(0x8000_0000+i)<<21`——描述符 PA 错+踩 bit52（Contiguous 位），
  开闸即 fault。**填表算式必须显式括号**；反汇编审计（看 movk 立即数
  形状）是唯一逮法——类型/测试全过也拦不住。
- **开闸后 sysreg 读不炸**：CurrentEL 在 MMU-on 世界照读（boot 记录
  el=1 维持）。

## 10. GICv3 与通用定时器中断契约（S0-5 实测，2026-10-15）[DT]

- **机器地址以活内核 boot DT 为准，dtsi 只当线索**：本机实跑
  `/sys/firmware/devicetree/base/soc/interrupt-controller@17a00000/reg`
  = GICD **0x17A00000**(+0x10000)、GICR **0x17A60000**(+0x100000,
  8 帧 × stride 0x20000)。vendor 树 bengal.dtsi 的 0x0F200000/
  0x0F300000 是同名变体的布局：0x0F2 区读回恒 0（无停摆、假静默），
  0x0F3 区总线停摆（挂死无 EXC）——S0-5 前两刷的根因。
- **GICD_TYPER 活读 = 0x201143b**（ESPI+LPI 在位，ITLinesNumber 0x3b
  → 1920 线）。内核 gic_init_bases 的第一次 GIC MMIO 就是读它——
  先读 TYPER 再动 dist，且每步落环步进记录（停摆不产 EXC 也不产
  仪器码，只有步进记录能定位）。
- **初始化序照内核**（gic_init_bases→gic_smp_init→gic_dist_init→
  gic_cpu_init 蒸馏）：TYPER 读 → dist 0→RWP→0x13→RWP → find_rd →
  RD 唤醒 → CPU 接口（SRE=7 回读 bit0→PMR 0xf0→IGRPEN1）→ SGI/PPI
  配置。dist CTLR 写 0x13 读回 **0x12**（ARE|G1A；非安全态 G1 位
  读 0）——照常递送，IAR1 确认即证。
- **RD 匹配式 = `(typer>>32) == Aff3<<24|Aff2<<16|Aff1<<8|Aff0`**
  （内核 __gic_populate_rdist 原式；MPIDR 分字段提取，不能裸
  `&0xFFFFFFFF`——MT 位 24/U 位 31 会混入）。本核（redfin CPU0）
  匹配 **帧 0**。RD 唤醒 = WAKER 清 ProcessorSleep(bit1) 轮询
  ChildrenAsleep(bit2) 清零，终读 0x0。
- **SGI/PPI 配置面在 RD+0x10000 第二帧**：IGROUPR0（+0x080）置
  Group1-NS、ISENABLER0（+0x100）写 1 set、IPRIORITYR（+0x400 起
  一字节一 INTID）给 0xa0（须 < PMR 0xf0）。
- **DAIF 立即数 = F|I|A|D 4 位域的掩码**（bit0=F bit1=I bit2=A
  bit3=D，max 0xf）：解 IRQ 掩 = `msr daifclr, #2`（内核
  arch_local_irq_enable 原文）；`#1` 只清 FIQ 位、`#0x80` 直接汇编
  错。掩 = `daifset, #2`（IRQ 入口首步，EOI 降优先级后防杂响重入）。
- **vtimer INTID=19**（PPI3；活内核 /proc/interrupts arch_timer =
  GICv3 19 Level 定谳——PPI 排布非上游惯例 13/14/11/10）。arm =
  disarm→CNTV_CVAL=cntvct+δ→CNTV_CTL=1；δ=19200000（1s）实测
  **dt=19200039/40/21 进槽**（误差 ≤40 拍=2µs，三轮一致）；入口
  读 CNTV_CTL=5（ENABLE|ISTATUS）。IAR1 回 19、EOI 后 disarm。
- **WFI 被 vtimer IRQ 唤醒入槽 5**（IRQ current-EL SPx 象限，紧挨
  S0-3 的 sync 槽 4）；入口掩中断→IAR1→记环→PS_HOLD 复位。三命
  烧完 ABL 自动回滚 b 槽 L0。零 EXC。

## 11. PSCI CPU_ON 契约（S0-6/S0-6b 实测，2026-10-15）[DT]

- **PSCI 面以活 DT psci 节为准**：`/sys/firmware/devicetree/base/psci`
  compatible="arm,psci-1.0"、method="smc"、无 legacy fn-id 属性——
  用标准编号、arm64 走 SMC64 变体。EL1 发 `smc #0` 同步入 EL3，TZ
  固件受理（ABL 交接后世界与 L0 内核拉副核同路径）。
- **PSCI_VERSION(0x84000000) 回 0x10001 = PSCI 1.1**（唯一不回错误码
  的调用）。**CPU_ON = 0xC4000003** = `PSCI_FN_NATIVE(0_2, CPU_ON)`
  （内核 psci.c:605 同值）。fn 编号非直觉：0=VERSION 1=SUSPEND
  2=OFF **3=CPU_ON** 4=MIGRATE；SMC64 位即 0xC4 前缀。返回码 0=
  SUCCESS / -1=ALREADY_ON / -6=NOT_PRESENT（int32 负值）。
- **SMCCC 参数位铁律（S0-6 首刷实证）**：x1-x3 传入参数位，TZ 返回
  时**允许置未定义**——asm 若声明 `in("x1")`，编译器可把调用侧活值
  缓存该寄存器跨 smc 复用，读回 0（`on aff=` 打 0 即此）。断根 =
  参数位一律 `inout(...) => _`；x4-x7 调用者保存须 lateout 声明。
- **入口态实测**：目标核从 entry_pa 以 EL1、MMU off、x0=context_id
  启动（ctx 如数送达——7 槽互异就是证据）；DAIF 掩。**VBAR_EL1
  per-core**——副核必须自装向量表再开中断；栈各核自立（本件
  4KB×7）。secondary_entry 全 asm：掩 DAIF→装 VBAR→立栈→跳 Rust。
- **多核拓扑 = 恒等映射（S0-6b RD_TYPER 全表定谳）**：主核裸
  MPIDR=**0x81000000**（U 位 1、MT 位 0、Aff2=0、Aff1=0）；副核
  自报 = **0x81000100..0x81000700**（U=1、MT=0、Aff2=0、Aff1=1..7）。
  GICR 8 帧 RD_TYPER 全表：帧 N 的 affinity 高 32 = 0x0N00、
  **ProcessorNumber[23:8]=N**、帧 7 独有 LAST(bit4)——帧号=PN=Aff1。
  三角（帧↔真 MPIDR↔DT target）= **恒等**：target 0x100*i 唤醒的核
  自报 Aff1=i、对应帧 i、PN=i，主核 Aff1=0=帧 0。无差一、无宽松
  映射。**初读「U|MT|Aff2=4|差一」是宿主侧十进制误换**（2164261120
  误读 0x81040000，实为 0x81000100）——教训：心跳收割的 u64 落环
  应以十六进制记录（hex_text），十进制人读必换算验证。inout 补修
  同刷实证：`on aff=` 打 0x100..0x700 真值。
- **mailbox/栈放 NC DMA 窗内免维护**：sec_mailbox 0xB0010000
  （7×32B：magic/mpidr/seq/cnt）+ sec_stack 0xB0011000（7×4KB）
  都在 [0xB0000000,0xB0200000)——主核 NC 映射访问 ↔ 副核 MMU-off
  物理访问（Device 型）双侧非缓存，零一致性维护。心跳节流 1M 拍
  @19.2MHz ≈ 52ms/拍。

## 12. SPMI v5 Rust 线与电源面真相（S1-1–S1-3c 实测，2026-10-15）[DRV][DT]

- **运输层（crates/bare spmi，内核逐函数对账）三轮逐字一致**：
  `spmi v=0x50010000`（v5 窗判定过，与 L0 dmesg 同值）、
  `feat=0x80050200`→apid_count=**0x200=512**（正好铺满表窗）。
  obsrvr+0x80×apid 读通道/RDATA0@+0x18/STATUS 轮询序（DONE→DENIED
  →FAILURE→DROPPED）反汇编审计与内核 wait_for_done 同序；写路径
  WDATA0@+0x10 先落再写 CMD@+0x0，与内核 pmic_arb_write_data 同序；
  命令字编码 fmt_cmd 与内核 fmt_cmd_v2 逐位一致。
- **flat 地址高字节=外设号（S1-2b 立法）**：CHGR@0x1000=**PPID 0x210**、
  DCDC@0x1100=0x211、BATIF@0x1200=0x212、USBIN@0x1300=**0x213**、
  DCIN@0x1400=0x214、TYPEC@0x1500=**0x215**、MISC@0x1600=**0x216**
  （PPID=sid2<<8|外设号；证据=pm7250b.dtsi 中断说明符 `<0x2 0x10 …>`
  逐外设递增）。SPMI 访问=每 PPID 独立查 apid。**S1-1/S1-2 曾把
  0x1310 当 PPID 0x210+0x10（实读 CHGR INT_RT_STS），「USBIN 缺口」
  定谳因此作废**。
- **【寻址律三档——S1-3b 环证立法，S1-3c 补证】**同一 PMIC 寄存器
  依地址档位走三条完全不同的总线命令，档位选错=事务 DONE 但落点
  错/不落：
  1. **短命令**（arb READ=15/WRITE=14）：总线只带 **5 位**寄存器地址
    （内核 spmi.h 明文 "5-bit address"）——偏移 ≥0x20 一律静默截断
    成 addr&0x1F。**<0x20 的读全真**（S1-2b 电源面叙事幸存），
    ≥0x20 的短命令读=读到偏移低 5 位的别名，写=写到别处。
  2. **8 位 EXT**（arb EXT_READ=13/EXT_WRITE=2，总线 op 0x20-0x2F
    读/≤0x0F 写）：地址位够 8 位，s13b 环证事务 DONE 但写不驻
    （三寄存器两外设全不驻、13min 观察窗 misc_rt 平 0）——当时判
    「EXT 死区」，S1-3c 证明 LONG 同死，死的不止这一档（见第 3 档
    与世界门悬案）。
  3. **16 位 EXT-LONG**（arb **EXT_WRITEL=op 0/EXT_READL=op 1**，
    总线 op 0x30-0x37 写/0x38-0x3F 读）：内核充电页的唯一走法——
    PMIC regmap `.reg_bits=16`（qcom-spmi-pmic.c:112）且 regmap-spmi
    以 `addr <= 0xFF` 分界（regmap-spmi.c:130/145/170/185），充电
    寄存器 flat 全 >0xFF → 内核一律走 LONG。外设字节走 APID 寻由，
    CMD 字只带 8 位偏移。**S1-3c 环定谳（ring md5 017a5018…cad8）：
    LONG 写也不驻**——c42/m53/aw2 三试验逐字复刻 s13b 形状，且与
    内核侧逐位逐序对账全同（fmt_cmd_v2 无 sid 位、WDATA 先落、
    RW=chnls+0x10000×apid、wait_for_done 同窗同序），即命令形状
    无懈可击。**结论升格：三档写全 ACK-DONE-不驻，读三档全真**
    （跨档交叉一致+物理合理）。
- **【回溯失效，S1-3b 改判】**①S1-2「写路径 Rust 首证」**撤**——
  那次写走的短命令，0x40 截成 0x00+恒等写，空洞；②S1-2b 的
  `0x1544=0x02（EN_SNK_ONLY）`条目**死**（短命令实读 0x1504）；
  ③B15/B16 asm 写配方降级为「事务级成立」（DONE 无 DENIED 真，
  落点驻留从未验过）。电源面叙事本身（PLUGIN=1/TERMINATE=满电/
  CC_ATTACHED=1）全用 <0x20 读，**不受损**。
- **owner 表上机实测（B 系推断升格为显值）**：CHGR 0x210 单副本
  **apid=0xe2 owner=0**；USBIN 0x213 单副本 **apid=0xe5 owner=0**；
  TYPEC 0x215 单副本 **apid=0xe7 owner=0**；MISC 0x216 单副本
  **apid=0xe8 owner=0**——电量链四外设全 APPS 可写。
- **【裸世界电源面真相，S1-2b 三轮实测——S1-1 缺口叙事撤案】**
  `u0=0x2a94510` 恒定：**USBIN 0x1310=0x10 → PLUGIN=1（输入在！）**、
  0x1006=0x45（bits[2:0]=5 TERMINATE——电池 100% 满的正当态）、
  **Type-C 0x150B=0xa9 → CC_ATTACHED=1**、debounce 完、SNK 模式、
  VBUS_ERROR=0；0x1506=0x02 **Rp 3.0A 检出**（供电器拉值）。裁决：
  **Type-C 状态机是 PMIC 内自主硬件，裸世界活着且已 attach；充电器
  看得见 USBIN；不存在 TCPM 前置**。与 L0 稳态（present=1/Charging/
  100）完全一致。
- **【狗面真相，s13b/s13c 环读——ka 130s 无咬闭案】**裸世界 POR 默认
  `WD_CFG(0x1651)=0x00`（谁也没武装过狗）、
  `SBB(0x1653)=0x0c`（SNARL=62.5ms 档/BARK=**128s 档**/BITE=0）。
  内核 probe 写的是另一形状（SBB←SNARL 8s|BARK=64s 档|BITE 8s|
  bit7 咬后禁充、WD_CFG←0x42）。ka 线 130s 无咬根因=**狗从未被
  装配**（WD_CFG=0），非时序差一秒。BITE=0 档=裸世界即使装配也
  只有 bark 中断无 bite，无禁充/复位风险。
- **【世界门悬案，S1-3c 开案】**L0 活体对证（2026-10-15 18:36）：
  `wdog-bark` 中断计数在涨（3→5，~60s 节奏）=狗在内核世真武装真叫；
  而裸世界 13min 零 bark+aw 全不驻。内核 arb 写路径与我 Rust 逐位
  逐序全同（fmt_cmd_v2/write_data/wait_for_done 三对账），充电驱动
  无 SEC/secure 门（源码级无据）→ 差别只能来自世界：内核世带着
  TZ/SCM/AOP 全套启动（msm_scm 驱动在 dmesg 名册），裸世界没有。
  两种待判解释：①TZ 在内核启动时解锁充电页（或 PMIC 安全寄存器
  ACK-drop 仅对未解锁 master）；②内核自己的写其实也没落，狗是
  TZ/AOP 装的（bark 节奏 64s vs 128s 可判：S1-3d 回网后先测）。
  **S1-3d 判别刀已备**：sp54=CHGR 0x1054 官方 CHARGER_SPARE（6 位
  备用，内核仅调试痕）翻位读回——落=裸世写本通（死者皆安全寄存器
  个案）；平=充电页写整死（世界门实锤）→S1 充电控制改走 RPMh/AOP
  路线立项。
