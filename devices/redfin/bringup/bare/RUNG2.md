# 阶梯 2 整机刷设计 —— redfin 裸 substrate 在役态（RUNG2，2026-10-14）

阶梯 1（#488 心跳）与 substrate 完工（#489，B13–B17）之后，本刀把
redfin 的**在役运行态**换成裸 substrate：无 Linux 内核，机器稳定活
（充电使能、看门狗喂、不自发复位），逃生门两路真考。事实基线全部引用
`TRUTH.md`（同目录查表真源）。

## 0. 目标 / 非目标

**目标**：
- redfin 整机以裸 substrate 为运行态（「整机刷」= slot a 换芯 +
  set_active，运行面即裸件）；
- 稳态可观测：静止 logo=活 / 黑屏=已回滚（二值判据定稿见 §2，fb 色带
  路随 B18 判死弃用）；
- 逃生门两路（自动回滚 / fastboot）真机验收。

**非目标**（阶梯 3+ 挂账）：网络/存储/ssh 面；功耗优化（WFI+通用定时器
刀）；双槽皆裸（阶梯 2b，救生艇语义验讫后再裁）；充电软件政策（HW
JEITA 门在 PMIC 内先扛）。

## 1. 槽位布局（D1 裁决：a=裸，b=救生艇）

| 槽 | 内容 | 语义 |
|---|---|---|
| a（active） | ka.S 裸 substrate | 在役运行态；**永不标 successful**（misc 无人写） |
| b | L0 Linux 原样 | 救生艇：3 次硬弹跳自动回滚落此 |
| 其余分区 | 不动 | userdata/system 全保 |

- **逃生 A（自动）**：任何原因重启/掉电 ×3 → tries 烧尽 → ABL 自动回
  b 槽 Linux → adb 归来。不需要人手。
- **逃生 B（结构）**：12s 强关 → VolDown+电源 → fastboot（活在 XBL，
  低于槽位层，与 payload 无关——今晚 N 次实证）。
- **面切换环（agent 可操作）**：裸 →（3 弹跳）→ Linux →（ssh
  `aginx-reboot bootloader` → fastboot `set_active a` → reboot）→ 裸。
  黑盒无人的来回通道全通。

## 2. 判活信号（D2 定稿：黑屏/logo 二值 + 电流表补强）

**B18/B18b 判死 fb 路**：面板=s6e3hc2 **命令模式**（cmdline `dsi_display0=
..._1080p_cmd` 定谳）——显存在面板 GRAM，host DRAM（0xA0000000，运行时
DT 确认未被 ABL 改址）不被扫描，ABL 推帧后写 host 内存屏不动。DSI 推帧
= 阶梯 3 挂账刀（屏幕当控制台的野心留给它）。

**在役判据（二值）**：
- substrate 早期崩 → tries 烧尽 blink×3 → **黑屏**（L0 无头，#27 验收
  即黑屏）——失败形态自带可视信号；
- substrate 稳态 → **Google logo 静止不动**。
- 盲区=「跑几分钟后死」（狗咬只掐充电不关机，屏不变）：USB 电流表补强
  ——**狗是活体证人**，环死→72s 掐充电→电流掉；电流持续=狗一直被喂=
  环活。

## 3. ka.S substrate（新刀，pi2.S 改两处）

1. **删复位段**：PON 装弹+PS_HOLD 全裁——稳态不复位（复位=tries 烧一
   命，那是阶梯 1 的动词）。
2. **主循环**：充电使能 RMW（一次，0x1042 bit0）→ `forever { 喂狗
   0x1643←1；delay ≈3e9 迭代（实测数秒级，≪64s 咬窗） }`。
3. **APID 选择承 pi2.S**：owner-gated（CHGR 0x210 扫全副本选
   owner==0；PON 不再需要）。

寄存器法纪律不变：位置无关、零字面量、零 DRAM 写。

## 4. 验收（bench 判据，全过才算）——已跑（2026-10-14，四闭一挂账）

1. **刷装** ✅：`fastboot flash boot_a ka.img` → `set_active a` → reboot
   （闸门串号过）。
2. **稳态** ✅：Google logo **静止** ≥10 分钟（wifi 全程缺席=无回滚）。
3. **充电**（电池未满时）：USB 电流表读数入充电档——**挂账**（bench 无
   表；充电 RMW 与 B16 blink 实证同源写法，代码路径有旁证）。
4. **逃生 A** ✅：12s 强关→开机 ×3–4 轮 → tries 烧尽 → 自动落 b 槽
   L0（wifi ~580s 回归；getvar 真值：a=retry0/unbootable，b=successful）。
5. **逃生 B** ✅：ssh `aginx-reboot bootloader` → fastboot 到位（闸门过）
   → 读计数器 → `set_active a` → reboot → 裸（静止 logo 眼验）。
   **面切换环闭合**。机制真值入 TRUTH.md §6。

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| 看门狗咬（72s 掐充电，不关机） | 喂狗 ≤30s 同环色带；被咬可观测（充电停）不致命 |
| 热（无软件充电政策） | HW JEITA 硬门在 PMIC（0x1094 系）；旋钮刀后续 |
| 电池尽（裸态无 FG daemon） | 棕复 ×3 → 自动落 b 槽 Linux 充电栈自愈（失败级联优雅） |
| 忙环功耗（一核满转） | 阶梯 2 接受；刀 2'=通用定时器+WFI（CNTFRQ 可读，EL1 可编程） |
| 假阳（稳态与冻死同形） | §2 二值判据：早期崩=黑屏（自动回滚），稳态=静止 logo；中期死=电流表 |
| fb 地址/格式错（写歪 DRAM） | B18 单刀先证，色带两格式同屏判读；fb 区在 reserved-memory 内，越界风险按带内偏移控制 |

## 6. 刀序

~~B18（fb 探针）~~ 已跑已判死（命令模式面板，§2）。→ ka.S 出件 → 验收
1–2（刷装+稳态静止）✅ → 验收 3–5 ✅（#3 电流表挂账）→ **阶梯 2 完结：
裸 substrate 在役**。挂账刀：DSI 推帧（屏幕控制台）、WFI 功耗、双槽裸
（2b）、电流表补验。

## 7. v2（2026-10-14 深夜，阶梯 3 回灌）

ka.S v2 = 开机先跑阶梯 3 存储自检（u4j 契约：SSU ACTIVE→W-LUN 0xD0 唤醒、
读 GPT 头+条目数组双 CRC32），过→本档 v1 常驻原样；败→时延编码复位落 L0
自报。验收：静止 logo ≥10min 无一次黑屏 + 21min adb 静默 + fastboot
`slot-retry-count:a=2`（只开机一次、从未复位）。附修 v1 x7 未初始化。
详见 TRUTH.md §7 末条、收据 #494。

