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
| 0x1341 | USBIN_CURRENT_LIMIT_CFG | **输入限流 ICL**（`smb5_usb_set_prop` 写） |
| 0x1094 / 0x1096 | JEITA 阈值对 | bulk 2B 读（`jeita_update_work`），热限充门 |
| 0x1140 / 0x1550 | （未命名） | `smb5_usb_set_prop` 写入面，用途待考 |

充电状态合成（`smblib_get_prop_batt_status`）：health 门 → usb_present（0x1310
bit4）｜dc_present（0x1410 bit4）→ status。看门狗：snarl IRQ 不写寄存器，走
`SW_THERM_REGULATION_VOTER` 投票+延迟工作（喂狗寄存器待挖）。

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

## 未闭项

0x1140/0x1550 命名、看门狗喂狗寄存器、ICL/FCC/FV 完整写链、JEITA HW
配置细节、qg alarm 重排死亡路径（战役在案）、晨间 SHUTDOWN_SOC=0 重放之谜。
