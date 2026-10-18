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
3. DCTL←CSFTRST（**bit30**，core.h:420）软复位，poll 该位自清。
   core.h:253 的 bit11 是 **GCTL**.CORESOFTRESET，不是 DCTL。DCTL bit11
   是 ACCEPTU2ENA，写 1 不会自清，poll 必然超时。
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

## EP0 → CDC-ECM/NCM（s20 刀面全配方，2026-10-16 案头收讫）

### 设备/EP 寄存器总表（core.h 真值）

| 寄存器 | 偏移 | 关键位 |
|---|---|---|
| DCFG | 0xc700 | DEVSPD bits2:0（HS=0/FS=1）；**DEVADDR=addr<<3（bits9:3，mask 0x7f<<3）** |
| DCTL | 0xc704 | SFTDISCONNECT bit1（清=上线）；**CSFTRST bit30**（bit11=ACCEPTU2ENA） |
| DSTS | 0xc70c | CONNECTSPD bits2:0（HS=0）；DEVCTRLHLT bit22 |
| DEVTEN | 0xc708 | DISCONNEVTEN bit0｜USBRSTEN bit1｜CONNECTDONEEN bit2（轮询刀三开即可；gadget.c:2588 全表多 EVNTOVERFLOW/CMD 等） |
| DALEPENA | **0xc720** | bit n=物理 EP n（ep0 OUT=bit0、ep1 IN=bit1）——⚠ 概要旧文误写 0xb008，此为正 |
| GUSB2PHYCFG(0) | 0xc200 | USBTRDTIM=9<<10（UTMI 8-bit）；**SUSPHY=bit6、ENBLSLPM=bit8——发任何 DEPCMD 前必须清、完命令后复原**（gadget.c:398-420，HS/FS 铁律） |
| DEP_BASE(n) | 0xc800+n·0x10 | PAR2=+0x00、PAR1=+0x04、PAR0=+0x08、DEPCMD=+0x0c |

DEPCMD 编码（core.h:534-553）：opcode 低 8 位（SETEPCONFIG=1、SETTRANSFRESOURCE=2、
SETSTALL=4、CLEARSTALL=5、STARTTRANSFER=6、ENDTRANSFER=8、DEPSTARTCFG=9），
PARAM bits31:16，CMDIOC bit8，**CMDACT bit10 写 1 轮询自清**，完成态
DEPCMD bits15:12（0=OK）。STARTTRANSFER 参数（ep0.c:82-83）：
**PAR0=TRB 地址高 32 位、PAR1=低 32 位**（写反是经典坑）。

### 事件缓冲协议（gadget.c:4250-4321）

- 轮询：`count = rd(GEVNTCOUNT(0)) & 0xfffc`（count 恒 4 倍数，每事件 4 字节）。
- 处理 count/4 条后**回写消耗字节数到 GEVNTCOUNT(0)=ack**（:4313），缓冲即可复用；
  自维护 lpos=(lpos+count)%SIZE 环走。
- 事件 u32 解码：**bit0=1 设备事件**（type=bits11:8，event_info=bits24:16；
  RESET=1、CONNECT_DONE=2）；**bit0=0 EP 事件**（epnum=bits5:1、
  event=bits11:8：XFERCOMPLETE=1/XFERNOTREADY=3/EPCMDCMPLT=7、
  status=bits17:14、param=bits31:16）。XFERNOTREADY 的 status：
  CONTROL_DATA=1、CONTROL_STATUS=2。
- ConnectDone：读 DSTS CONNECTSPD 定速；**HS 下 EP0 maxpacket=64**
  （gadget.c conndone，SS 才 512）。

### EP0 状态机（ep0.c 全读收讫）

TRB 16 字节：bpl(缓冲 lo)、bph(缓冲 hi)、size（len bits0-23；完成态 TRBSTS
bits28-31，SETUP_PENDING=2）、ctrl（HWO=bit0、LST=bit1、CHN=bit2、
**TRBCTL=bits9:4**：SETUP=2/DATA=5/STATUS2=3/STATUS3=4/normal=1、
ISP_IMI=bit10、IOC=bit11）。

1. **武装 SETUP**（dwc3_ep0_out_start，ep0.c:296）：TRB=(缓冲=TRB 自身地址，
   len=8, TRBCTL=CONTROL_SETUP, HWO|ISP_IMI|IOC|LST)，物理 ep0 发
   STARTTRANSFER。**SETUP 包 8 字节直接落进 TRB 缓冲**（inspect 以
   `ctrl=(void*)dwc->ep0_trb` 读，ep0.c:872）。
2. XFERCOMPLETE(ep0) → 解析 SETUP：
   - `wLength==0` 两段式：处理后等 NRDY(STATUS)。
   - `wLength!=0` 三段式：方向=bRequestType&0x80。**DATA 段立即起**
     （CONTROL_DATA TRB + STARTTRANSFER，不等 NRDY(DATA)——ep0.c:149-180
     长注释：等了会在背靠背 SETUP 时死等）。
   - IN 描述符答 wLength 与描述符长取 min，短包即完。
3. NRDY(STATUS) → 在事件指定的物理 EP 上武装 STATUS3（三段式）/STATUS2
   （两段式）TRB（len=0）→ XFERCOMPLETE → 回 SETUP_PHASE、重新武装 ①。
4. 失败/stall：物理 ep0 发 SETSTALL(4) 后重新武装 SETUP（stall_and_restart）。
5. **SET_ADDRESS**（DCFG DEVADDR=addr<<3）与 **GET_DESCRIPTOR** 的分工
   （ep0.c:835-867）：dwc3 只吃 GET_STATUS/SET_FEATURE/CLEAR_FEATURE/
   SET_ADDRESS/SET_CONFIGURATION/SET_SEL/SET_ISOCH_DELAY；**GET_DESCRIPTOR
   不在册，归 gadget 层**——裸刀自答 DEVICE(1)/CONFIG(2)/STRING(3)/
   DEVICE_QUALIFIER(6)。HS 主机必问 Qualifier（答合法副本），BOS(0x0F)
   可 stall（2.0 设备）。

### ECM 描述符（f_ecm.c 模板 → 裸刀静态字节）

裁决沿用：先 CDC-ECM 上机（兼容面最广、零类请求负担），NCM（NTB 大帧）
后进。device 描述符 bcdUSB=0200、**IAD 形 0xEF/02/01**；config 88B：
IAD(8)+控制接口(9, class 02/06/00, 1 端点)+CDC header(5, bcdCDC 0110)+
union(5, 0→1)+ECM(13, iMAC 字符串指向 12 hex 字符)+notify EP(0x81 IN int
16B)+data alt0(9, 0 端点)+alt1(9, 2 bulk)+bulk IN(0x82, 512)+bulk OUT
(0x01, 512)。EP 物理映射：ep0/ep1=控制，ep2(bulk IN 0x82→物理 5?)——
**物理 EP 号=USB EP 地址·2|方向**（wIndex_to_dep 律）：bulk IN 0x82→物理 5、
bulk OUT 0x01→物理 2、notify 0x81→物理 3。SET_CONFIGURATION 后对每个
非 ep0 EP：DEPSTARTCFG(9)→SETEPCONFIG(1)（param0=EP_TYPE bulk=2<
<16?，见 gadget.c:665-736；SETTRANSFRESOURCE(2) param0=1）→DALEPENA 或位。

### s20 验收

- Mac 端 `system_profiler SPUSBDataType` 见新 HS 设备（idVendor/idProduct
  自选值）+ `ioreg` 见 CDC ECM 接口绑定。
- 环记录：枚举序列回放（SET_ADDRESS wValue、SET_CONFIG wValue、
  各 GET_DESCRIPTOR 类型/长）、事件总数、DSTS/DCFG 读数。
- 数据面（s21）：EP1/EP2 bulk 对发 + ARP/ping（smoltcp 后进）。

## 开放项

1. pm8150 L5/L12/L2 直写偏移（ABL 逆向候选）。
2. RCG CFG 寄存器 parent 编码核对（gcc_parent_map_0：XO/GPLL0 档位表，
   写刀时从 gcc-lito.c parent_map + freq 表复核）。
3. 事件缓冲/event 语义细读（dwc3 event 大小端与 EventSize 对齐）。
4. ABL FV 解包脚本备用（0x3020 _FVH 起，压缩体 LZMA 概大）。
