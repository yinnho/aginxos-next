# redfin 电量链硬件真源（无 Linux 内核启动 · Stage 0）

来源=逆向真机二进制（`.local/device/redfin/vendor-modules/`，REA+Ghidra，
2026-10-13）× redbull 源树对账（`/tmp/redfin-kernel` 同宗）。用途：无内核
substrate 自养机电量的寄存器级蓝图；二进制与源码冲突时以本文二进制证据为准。

## 1. SMB5 充电器（qpnp-smb5-charger.ko，SPMI regmap 直址）

| 寄存器 | 名义 | 语义（二进制证据） |
|--------|------|--------------------|
| 0x1042 bit0 | CHARGING_ENABLE_CMD | **充电使能**；1=充/0=停。写入仲裁=`CHG_DISABLE` votable（`smblib_chg_disable_vote_callback`） |
| 0x1006 bits[2:0] | BATTERY_CHARGER_STATUS_1 | **充电类型**（1–2 段=快充/taper…） |
| 0x1310 bit4 | USBIN_RT_STS | **USB 在位**（ext-vbus 供电场景先查 0x1340 bit0=USBIN_CMD_IL 挂起位） |
| 0x1410 bit4 | DCIN_RT_STS | 直流在位 |
| 0x1370 | USBIN_CURRENT_LIMIT_CFG | **输入限流 ICL 写旋钮**（qpnp-smb5.c `.usb_icl` 表：0~3A、50mA 步；10-15 LineageOS 源码对账定谳——旧记 0x1341 系误记，0x1341 实为 CMD_APSD） |
| 0x1094 / 0x1096 | JEITA 阈值槽 | `CHGR_JEITA_THRESHOLD_BASE(i)=0x1094+4i` 2B 字段，热/冷硬门（见 JEITA 层） |
| 0x1140 bit0 | DCDC_CMD_OTG_REG | OTG_EN——VBUS 反向输出（OTG 面） |
| 0x1153 bit1 | DCDC_OTG_CFG_REG | OTG_EN_SRC_CFG 清 0=VBUS 归软件控 |
| 0x1550 | TYPE_C_EXIT_STATE_CFG_REG | EXIT_SNK_BASED_ON_CC（PD 硬复位退源行为） |
| **0x1643 bit0** | BARK_BITE_WDOG_PET | **喂狗=写 1**（见下节） |
| 0x1651 | WD_CFG | 看门狗总闸（活值 0x42：bark IRQ+plugin 启动） |
| 0x1653 | SNARL_BARK_BITE_WD_CFG | snarl/bark/bite 时长+咬后禁充（活值 0xFB） |

充电状态合成（`smblib_get_prop_batt_status`）：health 门 → usb_present（0x1310
bit4）｜dc_present（0x1410 bit4）→ status。

### 看门狗全配方（源↔二进制↔活体 DT 三对齐，10-13 补刀）

驱动 `smb5_init_hw`（二进制内联进 `smb5_probe`）开狗三写：

- **0x1653 ← 0xFB**（mask 0xff）：snarl=8s(field7=0x70)｜bark=64s(field2=0x08)｜
  bite=8s+**咬后禁充**（0x83=BITE_WDOG_DISABLE_CHARGING|BITE_TIMEOUT_8S）。
  bark 时长 DT 键 `qcom,wd-bark-time-secs`，**redfin DT 无此键**（活体
  /proc/device-tree 验证 10-13）→ 64s 默认生效；snarl 走
  SW_THERM_REGULATION_WA（google,wdog_snarl_disable 同样无键=在役）。
- **0x1651 ← 0x42**（mask 0xc2）：BARK_WDOG_INT_EN(bit6)+WDOG_TIMER_EN_ON_
  PLUGIN(bit1)，AFP 触发(bit7)关。
- **喂狗=`regmap_write(0x1643, 1)`**（二进制 `wdog_bark_irq_handler` 直出；
  probe 里再喂一次）。**中断驱动非定时器驱动**：64s bark IRQ → handler 喂。

**substrate 推论**：狗只在**插充电器**时走（EN_ON_PLUGIN）；裸机不插电无狗。
插电态下无内核=bark 无人喂 → 64s 后 bark、再 8s bite → **充电被 PMIC 硬掐**。
两条路任选：① 周期 <64s 写 1→0x1643（养机守人主循环顺手喂）；② 一次性
拆狗：0x1651 mask 0xc2 写 0。警告：②与「挂死硬件仍狂充」互换风险，主循环
活着时①更安全。

### 充电旋钮全表 + 编码律（10-13 二补刀，源↔二进制对齐）

**编码律**（`smblib_set_charge_param` 二进制定谳）：`raw字节 =
clamp(µ值, min, max) 取 (µ值−min)/step`，单字节 regmap_write——全部
旋钮同一形状，substrate 一把尺子全用。

| 旋钮 | 寄存器 | 量程（µA/µV） | 步进 |
|------|--------|---------------|------|
| **FCC 快充电流** | **0x1061** | 0–8,000,000 | 50mA |
| **FV 浮充电压** | **0x1070** | 3,600,000–4,790,000 | 10mV |
| **USB ICL 入限流** | **0x1370**（qpnp-smb5.c 表：0–3,000,000；旧记 0x1341 误） | 0–3,000,000 | 50mA |
| OTG 电流限 | 0x1152 | 500,000–1,000,000 | 250mA |
| ICL 现值/上限读 | 0x1107 / 0x1106 | 50mA 步同上 | — |
| AICL ICL/状态读 | 0x1108 / 0x110A | — | — |

配套腿：**挂起=0x1340 bit0 写 1**（USBIN_SUSPEND，恢复写 0）；
**ICL 硬覆盖=0x1342 bit0**（绕过 APSD/AICL 协商，substrate 手动供电面）；
**0x1341 真身=CMD_APSD**（适配器检测触发，非 ICL）。七外设一家：chgr/dcdc/
batif/usbin/dcin/typec/misc @0x1000..0x1600 = PPID 0x210..0x216（SID 2），
ICL 读面在 DCDC（0x1106/0x1107）、写面在 USBIN（0x1370）——跨外设非跨页。

**JEITA 三层**（热防护全貌）：
① 硬比较门：0x1094+4i 阈值槽（2B），超热/超冷 PMIC 自主掐；
② 硬降流：JEITA_CCCOMP_CFG_HOT=0x1092 / COLD=0x1093（25mA 步，0–1575mA
FCC 削减），使能位在 JEITA_EN_CFG=0x1090——**驱动在 SW JEITA 生效时
显式关掉硬斜率**（清 EN_*_SL_* 四位），redfin 走的是这条路；
③ 软温区（warm/cool）：google 政策层 FCC/FV/ICL 投票降额——substrate
自研温控时照抄层②③分工：硬门留 PMIC，软区自判。

**votable 架构**：全部关键动作（CHG/ICL/FCC/ suspend）经 pmic-voter 仲裁，
`vote(votable, VOTER名, en, val)`；无内核世界可整体换成单策略函数。

## 2. QG 电量计（qpnp-qgauge.ko）

- **/dev/qg udata = 144B/QG_MAX=18**（二进制定谳：`qg_device_write` 门
  `<0x90`、copy 0x90；写后 `vote(UDATA_READY_VOTER)` + queue `process_udata_work`）。
  内核出 kdata、用户态回 udata 的分裂设计 → 无内核 substrate **自己就是
  FG daemon**。
- **VBAT 平均**：`qg_base+0x80`，2B，µV = raw × 194.637（0x2f84d/1000）。
- **cc_soc**：FIFO 处理后缓存值（`chip+0x72c`），门控窗口 6000+0x9c5 tick。
- **OCV→SOC**：电池档案双 LUT（充/放 `+0xa8/+0xd0`）线性插值
  （`qg_interpolate_soc`），表来自 DT 档案+at24 EEPROM。
- **SDAM 布局**（`qg_sdam_write_all` 二进制 ↔ qg-reg.h 全对上）：
  `0x46` VALID(1B)、`0x47` SOC(1B)、`0x48` TEMP(2B)、`0x4A` RBAT(2B)、
  `0x4C` OCV(4B)、`0x50` IBAT(4B)、`0x54` TIME(4B)、`0x58` 循环计数(16B)、
  `0x68` 学习容量(2B)、`0x6A/0x6E` ESR 充/放 delta(4B)、`0x72/0x74` ESR
  SF(2B)、`0x76` 老化级(1B)、`0x80` MAGIC(4B)、`0x84` FLASH_OCV(1B)、
  **`0xBC` PON_OCV(2B，写给 PBS 用)**。
- **SDAM 发现**：扫 DT 子节点，SPMI 外设 `reg+4` 类型 = **0x2e** 即认，
  基址来自 pm7250b.dtsi（机况=DT，D14）。
- **MSOC 硬件刻度**：force 路径 `qg_write(base2+0xBF, 1B)`，值=round(SOC×
  2.55)——**1/255 单调 SOC 寄存器**（binary 直出魔数 0x51eb851f>>37=/100）。
- **alarm 爬链熄火机制定谳**（悬案闭）：`is_scaling_required` 四道熄火门
  （二进制内联于 `qg_scale_soc` 实核）：①profile 未载 ②|catch_up−msoc|<
  delta 且非 0/100 边界 ③catch_up==msoc ④catch_up>msoc 而无输入或
  **USB 在位但 charge_status≠CHARGING/FULL**（msoc≠0 时）。任一命中→
  `alarm_cancel` 静默停爬。爬链自身无病——**病在无人再进**：`qg_scale_soc`
  的常态重入者=process_udata_work（FG daemon 写 /dev/qg udata 触发），L0
  无 FG daemon → 单步后熄火。substrate 解法=自己的 FG daemon 周期写 udata
  （或 force 直写 MSOC 寄存器绕过爬链）。
- **关机路径**：`qpnp_qg_shutdown` 只做 cycle_count 落账——**SHUTDOWN_SOC
  焊笔不在关机路径**（在 process_udata_work）；晨间 07:35 重放之谜维持悬案。

## 3. 平台层（msm_icnss.ko，附产）

VPH 主轨 IIO 三带：**<3.449V=LOW / ≥3.603V=NORMAL**（中间 MID），跨带 →
QMI 递 wifi 固件（TX 回退/省电）→ 重挂 adc_tm5 迟滞阈值。深放时 wifi 固件
全程知情。

## 4. 政策层（substrate 可自研替代，不必复刻）

google-battery=AACR 寿命算法（容量衰退/循环宽限）；google-bms=gbms 存储
（charge-tracker 序列）；pmic-voter=投票仲裁；of_batterydata=DT 档案
（电阻 ID/LUT/老化）；qti_qmi_sensor=充电子系统 QMI 传感。

## 5. 上电依赖序（battery-bringup 脚本实证）

google-bms → at24（**m24c08@50 @ i2c 98c000**，qg 探测要读 batt_eeprom，
缺它 probe 永远 -517 defer）→ qpnp-qgauge → sm7250_bms → google-battery
→ google_charger（+qti_qmi_sensor）。

## 6. SPMI 运输层（substrate 怎么写这些寄存器，lito.dtsi+spmi-pmic-arb.c）

以上全部 SPMI 地址的真传法——裸 substrate 无 regmap，直踩 PMIC arbiter：

**内存图**（lito.dtsi `qcom,spmi@c440000`，EE=0/channel=0）：

| 区 | 基址 | 大小 | 用途 |
|----|------|------|------|
| core | 0x0c440000 | 0x1100 | 版本@+0、APID 表@+0x900+4N |
| chnls(写) | 0x0c600000 | 0x2000000 | 每通道 0x10000 槽 |
| obsrvr(读) | 0x0e600000 | 0x100000 | 每 APID 0x80 槽（v5, EE=0） |
| intr | 0x0e700000 | 0xa0000 | 中断面（初期可不用） |
| cnfg | 0x0c40a000 | 0x26000 | ownership 表@+0x0700+4N |

**协议**（v5，core+0 ≥0x50000000 即 v5）：
1. **寻址**：PPID = `sid<<8 | (periph>>8)`；充电链全家在 PM7250B（**SID 2**）：

   | 外设 | 基址 | PPID | 内容 |
   |------|------|------|------|
   | CHGR(smb5) | 0x1000 | **0x210** | §1 全部旋钮/看门狗前的寄存器 |
   | QG 表计 | 0x4800 | **0x248** | vbat@+0x80、MSOC@+0xBF |
   | CHG SDAM | 0xB000 | 0x2B0 | 充电器粘滞旗 |
   | QG SDAM | 0xB100 | **0x2B1** | §2 学习账本（发现=类型 0x2e） |
   | VADC | 0x3100 | 0x231 | VPH/热读数 |
   | ADC_TM | 0x3500 | 0x235 | 阈值迟滞监控（VPH 三带挂这） |
   | PON | 0x800 | 0x208 | 开机原因 |
   | revid | 0x100 | 0x201 | PMIC 身份 |

   附产（DT 深放阈值，10-10 案硬件根源）：vbatt-empty=3000mV（cold 同）、
   cutoff=3200、low=3300、low-cold=3700。
   APID 发现=扫 core+0x900+4N
   （到 core_size 止）：`ppid=(val>>8)&0xfff`，bit24=IRQ 主；
   写权=cnfg+0x0700+4N `val&7 == 0`（APPS EE）。
2. **窗口**：写=`chnls+0x10000×APID`（CMD@0x00、WDATA0@0x10、WDATA1@0x14）；
   读=`obsrvr+0x80×APID`（CMD@0x00、RDATA0@0x18、RDATA1@0x1C）；
   STATUS@0x08 轮询：**DONE=bit0、FAILURE=bit1、DENIED=bit2、DROPPED=bit3**。
3. **命令字**：`cmd=(op<<27)|((addr&0xff)<<4)|(len-1)&7`；
   op：**WRITE=14、READ=15**（EXT_READ=13、MSTR_RW=8/9）；**单事务 ≤8B**，
   多字节顺序装 WDATA0/1。
4. 中断面（可选）：acc_enable=wr+0x100+0x10000×N、irq_status=+0x104、
   irq_clear=+0x108；主接收态 intr+0x10000×m+4n。

附产：PON@pm7250b+0x800（开机原因，substrate 开机日志的根）。
**活体验证（10-13 relay 腿）**：dmesg `PMIC arbiter version v5 (0x50010000)`
✓；/proc/iomem 五区+pshold-base 0c264000 逐字节全对 ✓。APID 号不预钉——
substrate 运行时自发现（Linux 侧 STRICT_DEVMEM 拦死 /dev/mem，表读不到
不阻碍裸机腿）。



## 7. 重启/关机动词（msm-poweroff.c+qpnp-power-on.c+lito.dtsi，10-13）

裸 substrate 的 reset 面，两段式=**先在 PON 装弹、再在 SoC 侧拉闸**：

1. **PON 装弹**（SPMI，PON@0x800/PPID 0x208）：
   `PS_HOLD_RST_CTL=base+0x5A`（类型域 mask 0xF）、
   `PS_HOLD_RST_CTL2=base+0x5B`（RESET_EN=bit7）。
   序列：清 0x5B bit7 → **等 500µs**（10 个睡眠钟+50% 容差）→ 0x5A 写
   类型（**warm=0x01 / shutdown=0x04 / hard=0x07**）→ 0x5B bit7 置回。
2. **SoC 拉闸**：写 0 到 **PS_HOLD=0x0c264000**（lito.dtsi `restart@c264000`）
   → PMIC 执行已装弹动作。
3. **铁律：拉闸前先静默 SPMI arbiter**——PS_HOLD 在 SPMI 事务中途拉低会
   把某些 PMIC 锁死；Linux 走 TrustZone SCM（SCM_IO_DISABLE_PMIC_ARBITER），
   裸 substrate 须同样 SMC（或确认事务空闲窗口后立即拉闸）。
   重启前 PON 重启原因清 0（reason=UNKNOWN=0）。

重启=装 warm(0x01)+拉闸；关机=装 shutdown(0x04)+拉闸（关机路径多一步
dload 清零+SDI 关，裸机无此包袱）。

## 未闭项

晨间 SHUTDOWN_SOC=0 重放之谜（电量战役在案）；SPMI APID 表活体实读
（设备 wifi 打盹未应，devmem 命令在上节尾）。
（10-13 已闭六批：①看门狗全配方+0x1140/0x1153/0x1550 命名 ②充电旋钮全表+编码律+挂起/覆盖腿+JEITA 三层 ③qg alarm 熄火机制+MSOC 0xBF 1/255 刻度 ④SPMI 运输层全协议 ⑤电量链外设 PPID 全表+深放阈值 ⑥重启/关机动词=PON 装弹 0x5A/0x5B+PS_HOLD 0xc264000 拉闸+静默 arbiter 铁律。）

## ka v3 验收基线（2026-10-15，L0 只读取证）

- **L0 无 debugfs**（/proc/filesystems 无 debugfs、/sys/kernel/debug 不存在）+
  DEVMEM 关 → L0 侧**读不到 PMIC 原始寄存器**；v3 状态块须由裸件自读自落盘。
- **MISC（PPID 0x216）APPS 有 owner 副本**（源码推断）：qpnp-smb5.c smb5_init_hw
  写 SNARL_BARK_BITE_WD_CFG_REG(0x1653)+WD_CFG_REG(0x1651)，失败即
  `pr_err("Couldn't configue WD config")`；L0 dmesg 无此行、亦无 SPMI 权限类报错
  →APPS 能写 MISC→v3 `own(0x216)` 应有解。smb5-lib.c:7766 喂狗=
  `smblib_write(BARK_BITE_WDOG_PET_REG=0x1643, BIT0)`（bark IRQ 内）。
- **L0 充电态基线**（电池近满）：battery/status=Charging、charge_type=**Taper**、
  current_now≈3mA、VBAT 4.455V、USB_PD 5.17V；google_charger dmesg 每 2s 记
  usbv/usbc（mV/mA）≈5.2V/12–280mA=L0 软电流表。→ v3 状态块 0x1006 bits[2:0]
  健康预期=4 TAPER/5 TERMINATE（近满时）；**=6 PAUSE 即证 #497**。
- 旁记：L0 dmesg 37.148s 一条 WARNING（kthread.c:906 __kthread_queue_delayed_work，
  显示/触控激活路径），与充电/SPMI 无关，未追。
