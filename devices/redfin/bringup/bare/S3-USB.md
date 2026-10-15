# S3 USB 网络：机况与寄存器地图（案头，2026-10-15 深夜）

真源声明：本文件一切偏移/bit 取自本地 redbull 内核树
`.local/device/redfin/kernel-src/redbull/`（下引 `文件:行`），设备树
`arch/arm64/boot/dts/vendor/qcom/lito-usb.dtsi` + `lito-gdsc.dtsi`。
ABL 逆向路线已探（`backups-20261013/abl_a.img` = ELF32 包装的 UEFI FV，
`_FVH`@0x3020、压缩体 0x3000–0x46000 高熵）——但内核树全源在册，先走
「Linux 只当说明书」正路；ABL 留给无源码死角（如 PMIC LDO 直写偏移）。

## 路线裁决：HS-only 设备模式，跳过 QMP SS PHY

- 目标形态 = CDC-NCM 控制通道，USB 2.0 High-Speed（480M）绰绰有余。
- 内核正牌支持此形：`dwc3-msm.c:2084-2112`（POST_RESET 事件）明写
  「无 ssphy、只跑 HS/FS」的 QSCRATCH 三步序列；`lito-usb.dtsi` 的
  `qcom,core-clk-rate-hs = <66666667>` 是它的专属时钟档。
- QMP-DP combo PHY（`ssphy@88e8000`，~140 寄存器 init 表）整族不碰；
  其 LDO（pm8150_l9）也不上。S3 刀面减半。

## 地址地图

| 件 | 地址 | 来源 |
|---|---|---|
| dwc3 core | 0x0a600000（0xcd00） | lito-usb.dtsi:60-62 |
| QSCRATCH（wrapper 寄存器） | 0x0a600000 + 0xf8800 | dwc3-msm.c:73 |
| QUSB2 HS PHY | 0x088e3000（0x110） | lito-usb.dtsi |
| QMP SS PHY（不用） | 0x088e8000 | lito-usb.dtsi |
| GCC | 0x00100000 | — |
| USB30 GDSC | 0x10f004 | lito-gdsc.dtsi:10 |
| ahb2phy_top_cfg | 本机无 `ahb2phy_base` 资源（probe NULL 分支跳过） | dwc3-msm.c:3928-3941 |

核心 IRQ：GIC SPI 240（`lito-usb.dtsi` dwc3 子节点）。裸刀先轮询
GEVTCOUNT，不接中断。

## GCC 面（gcc-lito.c）

BCR（reset，写 1 脉冲后清 0）：
- USB30_PRIM_BCR @ **0xf000**（gcc-lito.c:2606）
- QUSB2PHY_PRIM_BCR @ **0x12000**（gcc-lito.c:2601）

RCG（clock source，`cmd_rcgr`）+ CBCR（branch，bit0 使能）：
| 名 | RCG | CBCR | 频率 |
|---|---|---|---|
| usb30_prim_master（core_clk） | 0xf020 | 0xf010 | HS=66.7M（GPLL0/4.5）；SS=133.3M（gcc-lito.c:866-873） |
| usb30_prim_mock_utmi（utmi_clk） | 0xf038 | 0xf01c | 19.2M（XO） |
| usb30_prim_sleep（sleep_clk） | — | 0xf018 | 32k |
| cfg_noc_usb3_prim_axi（iface_clk） | — | 0xf07c | —（gcc-lito.c:1061-1065） |
| aggre_usb3_prim_axi（bus_aggr_clk） | — | 0xf080 | —（gcc-lito.c:972-979） |
| usb3_prim_clkref（xo/refgen 腿） | — | 0x8c010 | QMP 用，HS-only 可不动 |

RCG 编程正典（clk-rcg2）：CFG 寄存器选 parent/src-div → UPDATE bit
（CMD_RCGR+0x4 bit0）写 1 → 轮询清零。master 的 GPLL0 在 S0 已点亮。

GDSC（gdsc.c:26-38）：**0x10f004 bit0 SW_COLLAPSE←0 = 上电**，poll 同址
bit31 PWR_ON（或 +0x4 状态口 bit16 POWER_UP_COMPLETE）。

## HS PHY 上电序（phy-msm-snps-hs.c msm_hsphy_init，:359-482）

前置：电源三轨 + GCC 时钟 + QUSB2PHY BCR 脉冲（assert→100–150µs→deassert，
:329-343）。然后逐条 write-readback：

| # | 寄存器（PHY 基址+） | 位操作 |
|---|---|---|
| 1 | 0x94 CFG0 | UTMI_PHY_CMN_CTRL_OVERRIDE_EN(bit1)←1 |
| 2 | 0x50 UTMI_CTRL5 | POR(bit1)←1 |
| 3 | 0x54 COMMON0 | FSEL(bits6:4)←0 |
| 4 | 0x58 COMMON1 | PLLBTUNE(bit5)←1 |
| 5 | 0xa0 REFCLK_CTRL | REFCLK_SEL(bits1:0)←0x2 |
| 6 | 0x58 COMMON1 | VBUSVLDEXTSEL0(bit4)←1 |
| 7 | 0x60 CTRL1 | VBUSVLDEXT0(bit0)←1 |
| 8 | 参数覆盖（dtsi 三对，`hsusb_phy_write_seq` value→offset） | 0x63→0x6c(X0)、0x85→0x70(X1)、0x17→0x74(X2) |
| 9 | 0x5c COMMON2 | VREGBYPASS(bit0)←1 |
| 10 | 0x64 CTRL2 | USB2_SUSPEND_N(bit2)\|USB2_SUSPEND_N_SEL(bit3)←1 |
| 11 | 0x3c UTMI_CTRL0 | SLEEPM(bit0)←1 |
| 12 | 0x50 UTMI_CTRL5 | POR←0（PHY 出复位） |
| 13 | 0x64 CTRL2 | USB2_SUSPEND_N_SEL←0 |
| 14 | 0x94 CFG0 | OVERRIDE_EN←0 |

电源三轨（lito-usb.dtsi:106-108）：vdd=pm8150_l5（0.88V）、
vdda18=pm8150_l12、vdda33=pm8150_l2。RPMH 管的轨——裸刀直写 SPMI 的
LDO 偏移**待 pin**（开放项①，候选源：ABL 逆向 / pm8150 手册）。

## dwc3 core 面（core.c / core.h）

寄存器偏移（core.h:87-146）：GCTL 0xc110、GSTS 0xc118、GUCTL 0xc12c、
GUCTL1 0xc11c、GSNPSID 0xc120（先读验身 0x5533xxxx）、GUSB2PHYCFG(n)
0xc200+4n、GEVNTADRLO/HI/SIZ/COUNT(n) 0xc400+n*0x10、DCFG 0xc700、
DCTL 0xc704、DSTS 0xc70c。

顺序（core.c:936 dwc3_core_init 收敛出的裸刀正典）：

1. 读 GSNPSID 验身（0xc120 高 16 位 0x5533）。
2. GUSB2PHYCFG(0)：USBTRDTIM←9（UTMI 8-bit，core.c:680-682）+ 清
   SUSPHY（连接完成前）。
3. DCTL←CORESOFTRESET（bit11，core.h:253）软复位，poll DCTL 回落。
   HS PHY init（上节 14 步）在此前/此后各一次均可——内核序 =
   soft_reset 内先 `usb_phy_init`（core.c:206-209）再 DCTL 复位。
4. GCTL：清 PRTCAPDIR(bits13:12) 再写 0b10（DEVICE，core.c:132-133
   `dwc3_set_prtcap`，core.h:248）。
5. **HS-only QSCRATCH 三步**（dwc3-msm.c:2084-2112，POST_RESET 正牌）：
   QSCRATCH_GENERAL_CFG(0x0a6f8808)：
   a. PIPE_UTMI_CLK_DIS(bit8)←1；usleep 2–5µs
   b. |= PIPE_UTMI_CLK_SEL(bit0)|PIPE3_PHYSTATUS_SW(bit3)；usleep 2–5µs
   c. &= ~PIPE_UTMI_CLK_DIS
6. 事件缓冲（core.c:417-426）：GEVNTADRLO/HI(0)←缓冲物理址；
   GEVNTSIZ(0)←SIZE(bit0-15)；GEVNTCOUNT(0) 清零（写回读值清 count）。
   裸刀缓冲 = 静态数组 + 轮询，EHB 位不开。
7. DCFG(0xc700)：DEVSPD(bits2:0)←0（HS）；DCFG devaddr=0 等枚举期
   SET_ADDRESS 再写。
8. DCTL(0xc704)：清 SFTDISCONNECT(bit1)→**上线**，Mac 端即见高速设备
   拉电流。

## EP0 → CDC-NCM（后续刀面，概要）

- EP0：DALEPENA(0xb008) 使能 EP0；SETUP 事件三段处理（标准请求
  GET_DESCRIPTOR/SET_ADDRESS/SET_CONFIGURATION + 类请求）。
- 描述符表：IAD + CDC-NCM 两接口（ECM 静态数组可直接上，NTB 后进）；
  bulk IN/OUT EP1 + interrupt EP2。TRB 环 16 对齐。
- macOS 原生 NCM 驱动自动绑定（若嫌重可先 CDC-ECM：老 Mac NCM 栈坑多，
  ECM 兼容面广但 1500 MTU）。**裁决留给刀上实测**。
- smoltcp + 自环 device 帧收发（S5 syscall 层的 socket 面在彼不在此）。

## 开放项

1. pm8150 L5/L12/L2 直写偏移（ABL 逆向候选）。
2. RCG CFG 寄存器 parent 编码核对（gcc_parent_map_0：XO/GPLL0 档位表，
   写刀时从 gcc-lito.c parent_map + freq 表复核）。
3. 事件缓冲/event 语义细读（dwc3 event 大小端与 EventSize 对齐）。
4. ABL FV 解包脚本备用（0x3020 _FVH 起，压缩体 LZMA 概大）。
