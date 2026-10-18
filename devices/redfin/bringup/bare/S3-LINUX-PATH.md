# Linux 在 redfin 上把 USB 网卡拉起来的路径

真源是本地 redbull 树 `.local/device/redfin/kernel-src/redbull/`。
下面的行号都指这棵树。寄存器偏移的清单仍以 `S3-USB.md` 为准。
这份文档只写调用顺序和每一段实际做的事。

目标形态是 CDC-ECM，高速。dwc3 节点在
`arch/arm64/boot/dts/vendor/qcom/lito-usb.dtsi` 的 `dwc3@a600000`。
同一份 dtsi 里这台机器还写了两件和数据面直接相关的属性：
`tx-fifo-resize`，以及父节点的
`qcom,dwc-usb3-msm-tx-fifo-size = <27696>`。

## 总顺序

主机插上之后，软件按这个顺序动。前一段不在 dwc3 里。

1. PMIC 自己完成 Type-C 附着和 BC1.2。充电器驱动读结果。
2. 充电器决定 `ok_to_pd`（策略引擎把它叫做 `PE_START`）。
3. 策略引擎要么直接叫 `start_usb_peripheral`，要么先跑 PD 状态机，
   在 Source Capabilities 里看到 USB 通信位之后再叫。
4. `start_usb_peripheral` 只把 extcon 的 `USB` 置 1，并带上 CC 方向、
   超速和电流档。它不写 dwc3 寄存器。
5. `dwc3-msm` 的 vbus 通知处理这个事件：CDP 时先打 D+ 脉冲，
   然后 `resume`，最后 `usb_gadget_vbus_connect`。
6. gadget 核心跑枚举。`SET_CONFIG` 时清非 ep0 的 TX FIFO。
7. `f_ecm` 的 `set_alt` 在数据接口 alt 1 上打开 bulk，提交收包请求，
   再发 CDC 通知。
8. dwc3 对 bulk 和中断端点维持一条已经 `STARTTRANSFER` 过的 TRB 环。
   每个新包用 `UPDATETRANSFER` 推进。完成事件是 `XferInProgress`。

## 1. 充电器先说话

`drivers/power/supply/qcom/smb5-lib.c`

VBUS 上升时 `smblib_usb_plugin_locked` 调用
`smblib_request_dpdm(chg, true)`（约 1385 行）。
这个调节器的 enable 在 `phy-msm-snps-hs.c` 的
`msm_hsphy_dpdm_regulator_enable`：复位 HS PHY，把 D+/D- 打进
非驱动高阻，让 PMIC 做 BC1.2。
同一函数的 disable 把覆盖位清掉并重新 init PHY。

`smblib_request_dpdm(chg, false)` 在这文件里只出现在 VBUS 落下
（约 6181 行）和 Type-C 拔出（约 7971 行）。
线还插着的时候，没有人把这个调节器 disable。

高阻被拆掉的地方不在调节器上。CDP 时 `dwc3_msm_vbus_notifier`
同步调用 `usb_phy_drive_dp_pulse`（见第 3 节）。脉冲结束时把
`TERMSEL`、`XCVRSEL` 和 `CFG0` 的两路覆盖位清掉，但
**不**把 `phy->dpdm_enable` 写成 false。调节器看起来仍是 enable，
引脚已经离开高阻。`msm_hsphy_dpdm_regulator_disable` 在
`cable_connected` 为真时只清标志，连 PHY 复位都不做
（`phy-msm-snps-hs.c` 约 662 行）。

APSD 完成后 `smblib_handle_apsd_done`（约 6484 行）按结果分类。
SDP、CDP、FLOAT 会走到 `smblib_notify_device_mode`。
若 `pd_active` 已经是 1，`usb_source_change_irq_handler` 直接返回，
不再跑 BC1.2（约 6362 行）。

对端是源、本机是 sink 时，`typec_src_insertion`（约 6688 行）
设置 `chg->ok_to_pd`。条件是：不是 legacy 线，也没有
`pd_disabled`。这个布尔就是策略引擎读到的 `PE_START`。

## 2. 策略引擎什么时候拉 USB

`drivers/usb/pd/policy_engine.c` 的 `psy_changed`（约 3791 行）
订阅充电器的 `usb` 电源，不直接读 CC。

`PE_START` 为 0，且 `REAL_TYPE` 已经是 USB、USB_CDP 或 USB_FLOAT：
不进 PD 状态机。只排队 `start_periph_work`。
那个 work 只有 `start_usb_peripheral`（约 560 行）。

`PE_START` 为 1：进入状态机。sink 的入口是
`enter_state_snk_startup`（约 2607 行），顺序是：

1. 数据角色还空着或已是 UFP 时，若 `REAL_TYPE` 已是 USB 或 CDP，
   **先** `start_usb_peripheral`。
2. 然后才 `usbpd_startup_common` → `pd_phy_open`。
3. VBUS 在的话，等 Source Capabilities，超时是 `SINK_WAIT_CAP_MIN_MS`
   量级，这棵树里用 500ms 左右的等待。
4. `pd_eval_src_caps`（约 932 行）：第一条固定 PDO 带
   `PD_SRC_PDO_FIXED_USB_COMM`，本机仍是 UFP，而且还没
   `pd_connected`，才再叫一次 `start_usb_peripheral`。
   这一次发生在发 Request 之前。

`start_usb_peripheral`（约 560 行）写的是 extcon，不是端点：

- `EXTCON_PROP_USB_TYPEC_POLARITY`
- `EXTCON_PROP_USB_SS` = 1
- `EXTCON_PROP_USB_TYPEC_MED_HIGH_CURRENT` 按对端电流档
- 然后 `extcon_set_state_sync(..., EXTCON_USB, 1)`

`pd_phy_open`（`qpnp-pdphy.c` 约 368 行）在这之后才打开 PD PHY：
角色、`USBPD_REV_20`、`EN_CONTROL` 先 0 再 1、帧过滤、
`RX_ACKNOWLEDGE = 0`。GoodCRC 由 PHY 按 `MSG_CONFIG` 产生。
软件要读走 `RX_BUFFER` 再把 `RX_ACKNOWLEDGE` 写 0，否则
`pd_phy_write` 看到缓冲仍被占就返回 `-EBUSY`。

## 3. dwc3-msm 接到 extcon 之后

`drivers/usb/dwc3/dwc3-msm.c`

`dwc3_msm_vbus_notifier`（约 3360 行）：

- 充电器类型是 `POWER_SUPPLY_TYPE_USB_CDP` 时，先
  `usb_phy_drive_dp_pulse(mdwc->hs_phy, 200)`。
- 然后 `queue_work(resume_work)`。

`msm_hsphy_drive_dp_pulse`（`phy-msm-snps-hs.c` 约 552 行）拉高
200ms 的 FS D+：`CFG0` 两路覆盖、`UTMI_CTRL0` 的 `TERMSEL`、
`UTMI_CTRL1` 的 `XCVRSEL`。到时这三处都清掉，再等 20ms。
函数末尾会 `enable_power(false)`。gadget 真正启动时电源会再开。

`dwc3_otg_start_peripheral`（约 4588 行）在 resume 把控制器拉起来之后：

1. `dwc3_override_vbus_status(true)`，QSCRATCH `HS_PHY_CTRL` 的
   `UTMI_OTG_VBUS_VALID`。
2. `usb_phy_notify_connect`。
3. `dwc3_msm_block_reset(true)`。
4. `dwc3_set_prtcap(DWC3_GCTL_PRTCAP_DEVICE)`。
5. `dwc3_dis_sleep_mode`。
6. `usb_gadget_vbus_connect`。这一步才进 gadget 核心，清
   `DCTL.SFTDISCONNECT`，由控制器拉 D+。

probe 里 `dwc3_msm_extcon_register` 会同步读当前的 `EXTCON_USB`。
已经是 1 就当场调用 `dwc3_msm_vbus_notifier`，把 `vbus_active`
写成 true（约 3433 行）。这一下发生在后面的 `dpdm` 判断之前。

随后（约 4146 行）只有三件同时成立才等待调节器关掉：

- `vbus_active` 仍是 false
- 拿得到 `dpdm` 调节器
- `regulator_is_enabled` 为真（也就是 `phy->dpdm_enable`）

三件里缺一件，就直接 `queue_delayed_work(sm_work)`。
`dwc_dpdm_cb` 只在 `REGULATOR_EVENT_DISABLE` 且
`drd_state == DRD_STATE_UNDEFINED` 时才补这一下。
开机时充电器已经把 `EXTCON_USB` 置上的话，dwc3 不会等这次
disable。线还插着时也没有人去 disable，见第 1 节。

`f_ecm.c` 640 行附近写明：configured 不等于主机在轮询通知端点，
active 不等于主机已经在用数据端点。

## 4. 事件字

`core.h` 里 `dwc3_event_devt` / `dwc3_event_depevt`。bit 0 为 1
是设备事件，类型在 bits 11:8。bit 0 为 0 是端点事件：

| 字段 | 位置 |
|---|---|
| 端点号 | bits 5:1，物理端点号 |
| 事件类型 | bits 9:6 |
| 状态 | bits 15:12 |

类型值：`XferComplete` = 1，`XferInProgress` = 2，
`XferNotReady` = 3。

`gadget.c` 的 `dwc3_process_event_entry`（4138 行）：
`is_devspec` 为 0 进 `dwc3_endpoint_interrupt`，否则当设备事件。

`dwc3_endpoint_interrupt`（3382 行）：

- 物理端点 0 和 1 全部交给 `dwc3_ep0_interrupt`。
- 其它端点的 `XferInProgress` 才调用
  `dwc3_gadget_endpoint_transfer_in_progress`。这是 bulk 和中断
  把包交回 gadget 驱动的路径。
- 这些端点的 `XferComplete`（3431 行）只把计数加一，然后 `break`。
  不交回请求。
- `XferNotReady` 只用于等时端点的 `start_isoc`。bulk 和中断没有
  这条补救。

所以非控制端点必须事先有挂上的请求。主机先发、控制器 NAK，
这棵驱动不会因此再排队。

## 5. 端点配置和 TX FIFO

`gadget.c` `dwc3_gadget_set_ep_config`（640 行）：

- 控制端点（物理号 ≤ 1）打开 `XFER_COMPLETE_EN`（bit 8）和
  `XFER_NOT_READY_EN`（bit 10）。
- 其它端点只打开 `XFER_IN_PROGRESS_EN`（bit 9）。
- `EP_NUMBER` 在 param1 bits 29:25，值是物理端点号。
  USB 地址 `0x81` 的物理号是 3，`0x01` 是 2，`0x82` 是 5。
  算法是 `(地址号 << 1) | 方向`。
- IN 端点的 `FIFO_NUMBER` 在 param0 bits 21:17，值是
  `物理号 >> 1`。

`lito-usb.dtsi` 有 `tx-fifo-resize`，所以
`dwc->needs_fifo_resize` 为真（`core.c` 约 1321 行）。
msm 父节点把 `tx_fifo_size` 设成 27696（`dwc3-msm.c` 约 3966、2113 行）。
两个都在时，`dwc3_gadget_resize_tx_fifos`（201 行）才干活。

`ep0.c` 的 `dwc3_ep0_set_config`（668 行）在 `SET_CONFIG` 里：

1. 读 `GTXFIFOSIZ(0)` 的低 16 位当作 ep0 IN 的深度，写入
   `last_fifo_depth`。
2. 把其余 IN 端点的 `GTXFIFOSIZ(物理号 >> 1)` 写成 0。
   物理号从 3 开始，步长 2。通知端点是 `GTXFIFOSIZ(1)`，
   bulk IN 是 `GTXFIFOSIZ(2)`。

之后每个 IN 端点第一次 enable 时分配。深度按
`GHWPARAMS0` 的 `MDWIDTH`（bits 15:8，单位是 bit，要除以 8）
和 1024 字节的 max packet 算。`mult` 对普通 bulk 和中断是 1。
新 FIFO 的起始地址接在已分配深度之后。寄存器高 16 位是起始，
低 16 位是深度。RAM 用尽就返回 `-ENOMEM`，该端点深度保持 0。

ep0 的 FIFO 0 由复位后的默认值撑着，控制传输不依赖这次分配。
通知端点和 bulk IN 依赖它。深度为 0 时，`STARTTRANSFER` 仍可以
返回成功，IN 方向没有缓冲可发。

## 6. 非控制端点的 TRB 环

`__dwc3_gadget_ep_enable`（747 行）对 bulk 和中断：

1. `resize_tx_fifos`（仅 IN，且上面两个条件都成立）。
2. `dwc3_gadget_start_config`：第一次是 `DEPSTARTCFG`。
   传输资源一直留到核心复位或休眠，所以不是每次 USB 总线复位
   都重做（`gadget.c` 约 618 行）。
3. `SETEPCONFIG`，`DALEPENA` 对应位置 1。
4. 分配 `DWC3_TRB_NUM`（256，`core.h` 707 行）个 TRB。Link TRB
   （`TRBCTL` = 8）只在最后一项，`HWO` 常置，指回第一条
   （`gadget.c` 799 行）。中间 254 项保持 `HWO` = 0。
   `STARTTRANSFER` 时 TRB0 的 `HWO` 还是 0，控制器若预取到下一条，
   看到的必须是普通空 TRB，不能是已经置了 `HWO` 的 Link。
5. 立刻对 TRB0 发一次 `STARTTRANSFER`。此时 TRB0 还是 0，
   没有 `HWO`。这一次只为了把传输拉起来。
6. 命令成功后读 `DEPCMD`，`bits 22:16` 是资源号
   （`DWC3_DEPCMD_GET_RSC_IDX`，`gadget.h` 148 行）。

之后每个请求走 `__dwc3_gadget_kick_transfer`（1557 行）：

- 传输还没开始：`STARTTRANSFER`，参数是 TRB DMA 的高、低 32 位。
- 已经开始：`UPDATETRANSFER | PARAM(资源号)`。参数寄存器是 0。
- `dwc3_send_gadget_ep_cmd`（442 行）：非等时的 `UPDATETRANSFER`
  把 `CMDACT` 和 `CMDIOC` 都清掉，不等命令完成。databook 把这叫
  No Response Update Transfer。

`dwc3_prepare_one_trb`（约 1235 行）对 bulk 和中断：

- `TRBCTL` = Normal（1）。
- OUT 且不是 stream：置 `CSP`（bit 3），短包不结束整个传输。
- 需要中断时置 `IOC`（bit 11）。
- 不在这里置 `LST`。环靠 Link TRB 转回去。
- 先写前三个字，`wmb()`，最后才把 `HWO` 或进去
  （databook 4.2.3.2，约 1270 行）。

控制端点不走这条环。ep0 仍是一次性 `STARTTRANSFER`，TRB 类型是
Setup / Data / Status，事件是 `XferComplete`。

总线复位时 `dwc3_gadget_reset_interrupt`（约 3649 行）停掉活动传输、
把设备地址清 0、通知 gadget 驱动。ep0 再 `ep0_out_start`。
非控制端点的资源号因为不是核心复位，驱动选择保留；
真正停传输用的是 `ENDTRANSFER`。

## 7. f_ecm 在 alt 1 做什么

`f_ecm.c` `ecm_set_alt`（533 行）：

- 控制接口只允许 alt 0。`usb_ep_enable(notify)`。
  描述符里这个端点是中断 IN，`bInterval` 对应约 32ms
  （`ECM_STATUS_INTERVAL_MS`），`wMaxPacketSize` 至少能装下
  16 字节的速率通知。
- 数据接口 alt 0：关掉数据端点，不收发包。
- 数据接口 alt 1：`gether_connect`。

`gether_connect`（`u_ether.c` 1077 行）：

1. `usb_ep_enable` bulk IN，再 enable bulk OUT。
   这两下进第 6 节的 TRB 环和第一次 `STARTTRANSFER`。
2. `alloc_requests` 只是把请求放进空闲链表，此时还不交给控制器。
3. 若网卡已经 `netif_running`：调用 `ecm_open`，`is_open = true`。
   否则调用 `ecm_close`，`is_open = false`。
4. `netif_carrier_on`。
5. 只有网卡已经打开才 `eth_start` → `rx_fill` → `rx_submit`。
   这时才有 OUT 请求挂在控制器上。高速时队列长度是
   `qmult * DEFAULT_QLEN`，不是 1。

然后 `ecm_set_alt` 无条件 `ecm_notify`。

通知状态机（`ecm_do_notify`，约 370 行）：

- 状态从 `ECM_NOTIFY_CONNECT` 开始。
- 8 字节：`bmRequestType = 0xA1`，`bNotificationType = 0x00`
  （`NETWORK_CONNECTION`），`wValue = is_open ? 1 : 0`，
  `wIndex` 是控制接口号，`wLength = 0`。
- 这次完成之后状态变成 `ECM_NOTIFY_SPEED`，完成回调里再发下一条。
- 速率通知类型 `0x2A`，`wLength = 8`，后面两个 `le32` 都是
  `ecm_bitrate()`。高速时是 `13 * 512 * 8 * 1000 * 8`，
  不是 480000000。

`f_ecm.c` 434 行写明：很多主机会等到自己的网卡 open 之后才开始
读通知端点。第一条通知会在 FIFO 里等很久。
`is_open` 为假时，`wValue` 是 0，主机看到的是「没连上」。

`SET_ETHERNET_PACKET_FILTER`（`ecm_setup`，482 行）只把
`wValue` 存进 `cdc_filter`。默认值在 alt 1 时被写成
`DEFAULT_FILTER`（`u_ether.h` 88 行）：广播、全部组播、混杂、单播。
这个过滤器决定设备往主机送哪些帧，不决定主机发不发 ARP。

收包缓冲按 `u_ether.c` `rx_submit`（约 166 行）对齐到 max packet
的整数倍。ECM 没有额外帧头，bulk 里就是以太网帧，短包结束一帧。

## 8. 一包什么时候算结束

`dwc3_gadget_ep_reclaim_completed_trb`（`gadget.c` 3088 行）在
`XferInProgress` 里决定要不要把请求交回去。下面任一成立就交回：

- 事件状态带 `DEPEVT_STATUS_SHORT`（`core.h` 1438 行，bit 1），
  并且这条 TRB 没有 `CHN`
- TRB 控制字带 `IOC`（bit 11）
- TRB 控制字带 `LST`（bit 1）

`HWO` 仍为 1 时不回收，控制器还在用这条 TRB（3197 行）。

OUT 的准备（`dwc3_prepare_one_trb`，1252 行）对非 stream 的 OUT
置 `CSP`（bit 3）。短包不把整个传输停掉，控制器转到下一条 TRB。
`IOC` 仍会让 `XferInProgress` 产生。短包的状态位就是上面的
`DEPEVT_STATUS_SHORT`。

因此 bulk OUT 上收到 ARP 时，驱动看到的是类型 2 的事件，状态里
带短包位。类型 1 的 `XferComplete` 在非 ep0 上不会走到这里。

IN 方向没有 `CSP`。长度短于 max packet 的那一条 TRB 发完，
主机 ACK 之后，同样靠 `IOC` 产生 `XferInProgress`。
`NETWORK_CONNECTION` 是 8 字节，短于通知端点的 16 字节
max packet，走的就是这条。`SPEED_CHANGE` 连头带数据是 16 字节，
和 max packet 一样长，是一整包，不是短包。

控制器没把事件写进事件缓冲时，驱动无事可做。
`XferNotReady` 只启动等时端点（3366 行）。bulk 和中断 NAK 了，
不会因此再排队。

## 9. 和当前裸机的关系

裸机已经能让 Mac 认出 `Aginx ECM` 并且把 `en10` 标成 active。
环上对过的事实：

- `SET_INTERFACE` 数据接口 alt 1 到达过（`s20 alt=1`）。
- 通知端点的 `STARTTRANSFER` 返回过成功（`s20 ln=1`）。
- 速率通知没有完成记录（没有 `ln=2`）。
- 没有 `s20 arp`，没有 `s20 icmp`。
- 按第 5 节算出的 FIFO 写进去了，值非 0
  （`f1=0x420082`，`f2=0xc40082`）。命令没有再报 `ce`。

源码里和这些事实对得上的差别：

- 非控制端点的完成路径是 `XferInProgress`。这棵驱动把非 ep0 的
  `XferComplete` 空过。
- bulk / 中断使用第 6 节的环和 `UPDATETRANSFER`。
- IN FIFO 在 `SET_CONFIG` 时按 ep0 的深度重新分配。
- 通知的 `wValue` 跟着 `is_open`。速率字段用 `ecm_bitrate()`。
- OUT 请求只在 `netif_running` 时挂上，高速时一次挂
  `QMULT_DEFAULT * DEFAULT_QLEN` 个（5 × 2 = 10）。
  挂上之前主机发出的包会被 NAK。这棵驱动不因为 NAK 再补请求。

VBUS 仍在时，没有人 disable `dpdm` 调节器。dwc3 在 extcon
已经是 USB 的情况下也不等它。把 D+/D- 从高阻放出来的是 CDP
脉冲结束时对覆盖位的清除。

两张环在写到的范围内，`s20 ev` 都是 0。第一张（Link 紧挨 TRB0）
bulk OUT 资源号是 2（`s20 rs=0x20002`）。第二张（256 项，Link
在最后一格）同样是资源号 2，通知端点资源号 3
（`s20 rs=0x30003`），`s20 ln=1`。同名的 `s20 rs=0x80f00000`
是更早的 `DCTL` 读数。

把 Link 挪到第 256 项，在日志还盖得住的那段里仍然没有
`XferInProgress`。这段有多长，第 10 节按 ring50 的时间戳算过：
盖不住 `ln=1` 之后的 8ms 以后。非控制端点没开
`XFER_NOT_READY_EN`。主机的令牌若被 NAK，软件看不到
（`gadget.c` 3366 行）。

## 10. 端点地址、通知何时进 FIFO、ring50 的限额

`f_ecm.c` `ecm_bind`（748 行起）分配端点的顺序是：

1. `usb_ep_autoconfig` bulk IN
2. bulk OUT
3. 通知（中断 IN）

`dwc3_gadget_init_endpoints` 把非控制端点按物理号尾接到
`ep_list`：`ep1out`、`ep1in`、`ep2out`、`ep2in`……
ep0 不进这张表。`epautoconf.c` 沿这张表找第一个方向和类型都匹配、
还没被占的端点，再把 `ep->name` 里的号写进 `bEndpointAddress`
（96 行起）。所以这棵树上 Linux 发布出去的地址是：

| 用途 | 名字 | USB 地址 | 物理号 | `GTXFIFOSIZ` |
|---|---|---|---|---|
| bulk IN | ep1in | `0x81` | 3 | 1 |
| bulk OUT | ep1out | `0x01` | 2 | 无 |
| 通知 | ep2in | `0x82` | 5 | 2 |

裸机的 `CFG_DESC` 把两个 IN 对调了：通知是 `0x81`（物理 3），
bulk IN 是 `0x82`（物理 5），bulk OUT 仍是 `0x01`（物理 2）。
高速描述符的其余字节和源码一致。通知是中断、MPS 16、
`bInterval = 9`。`USB_MS_TO_HS_INTERVAL(32)` 是
`ilog2(32 * 1000 / 125) + 1 = 9`，服务间隔 32ms。
DEPCFG 的 `bInterval_m1` 是 `bInterval - 1 = 8`（`gadget.c` 723 行）。
bulk MPS 512。ECM 功能描述符的 `wMaxSegmentSize` 是
`ETH_FRAME_LEN` 1514（`0x05EA`）。

主机按设备发出的描述符选端点。裸机的 DEPCFG 用
`EP_NUMBER = 物理号`、`FIFO_NUMBER = 物理号 >> 1`，
和它自己的描述符是同一套号。`s20 f1` 写的是 `GTXFIFOSIZ(1)`，
`s20 f2` 写的是 `GTXFIFOSIZ(2)`，分别对上通知和 bulk IN。
把地址改成 Linux 那张表，主机只会跟着新描述符走，
不会单独把包打进已经武装的端点。

时序上裸机少走了 `set_config` 里的两步。`composite.c`
`set_config`（984 行起）在 SET_CONFIG 的状态段之前，对接口 0 和
接口 1 各调一次 `set_alt(..., 0)`：

- 控制接口 alt 0：`usb_ep_enable(notify)`。
  `__dwc3_gadget_ep_enable` 在这里对全 0 的 TRB0 发
  `STARTTRANSFER`，Link TRB 已经在第 256 项。
- 数据接口 alt 0：不开 bulk。`ecm_set_alt` 末尾仍调用
  `ecm_notify`（599 行）。此时 `gether_connect` 还没跑，
  `is_open` 仍是 false，第一条 `NETWORK_CONNECTION` 的
  `wValue` 是 0，在状态段之前就进了通知 FIFO。

数据 alt 1 才走 `gether_connect`。网卡已经 `netif_running` 时，
它调用 `ecm_open`（`is_open = true`，再发一条 `wValue = 1`），
然后 `eth_start` → `rx_fill`。网卡没打开就调用 `ecm_close`
（`wValue = 0`），并且不 `rx_fill`。bulk 端点在两种情况下都已经
`usb_ep_enable`，也就是都有一条空 TRB 上的 `STARTTRANSFER`。
没有 OUT 请求时，主机发出的帧被 NAK，事件缓冲里没有记录。

裸机在 SET_CONFIG 里做了通知端点的 DEPCFG、DALEPENA 和
`GTXFIFOSIZ(1)`，`STARTTRANSFER` 留到 SET_INTERFACE alt 1 的
状态段完成之后。ring50 这一次启动的计数器是 19.2 MHz：

- `su=0x10900` 是 SET_CONFIGURATION 1，在启动后 10.511s。
- `su=0x100010b01` 是数据接口 SET_INTERFACE alt 1，10.549s。
- `s20 rs=0x20002`、`s20 rs=0x30003`、`s20 ln=1` 都在 10.552s。
- `su=0x4321` 在 10.560s。

SET_CONFIG 到 `ln=1` 是 41ms，长过一个 32ms 服务间隔。
这 41ms 里通知端点已经在 `DALEPENA` 里，传输还没
`STARTTRANSFER`。主机这段时间里的 IN 令牌会被 NAK。
`f_ecm.c` 434 行和 652 行写的是同一件事的后半段：很多主机要等
自己的网卡打开才读通知端点；alt 1 也不表示数据端点上已经有流量。
第一条通知可以在 FIFO 里待着，完成事件要等主机来读。

`su=0x4321` 拆开是 `bmRequestType = 0x21`、`bRequest = 0x43`、
`wValue = 0`、`wIndex = 0`、`wLength = 0`。
`0x43` 是 `USB_CDC_SET_ETHERNET_PACKET_FILTER`。
`wValue = 0` 表示四个过滤位都清掉（`cdc.h`：混杂、全部组播、
单播、广播，默认值 `DEFAULT_FILTER` 是这四位的或，等于 `0x000F`）。
`ecm_setup` 只把这个值存进 `cdc_filter`。它管的是设备往主机送
哪些帧。主机自己发不发 ARP，不看这个字段。

ring50 不能用来证明「`ln=1` 之后控制器再也没写过 ep≥2 的事件」。
`s20_log` 写满 80 条就直接返回。这次启动的第 80 条就是
`su=0x4321`，离 `ln=1` 只有 8ms，而通知端点的服务间隔是 32ms。
限额之后 `s20_event` 仍会处理事件，环上不再有 `s20 ev`。
所以 `s20 ev = 0` 只覆盖到这条过滤器请求为止。
ping 仍然全丢，邻居没有装上。限额把后面的记录吃掉了，
数据面也还没通。

同一条环里，第一次总线复位之后的 `su=0xa7e9e080` 是 EP0 TRB
自己的低 32 位地址（同一条启动里 `s20 tr=0xa7e9e08000000c23`），
紧跟着 `ce=0x60001`。那一次 SETUP 没有落到缓冲里。
后面的复位之后才是 `SET_ADDRESS`（`su=0x10500`）和描述符请求。

代码已经按这一节改过，并刷进了 boot_a。ring51 是这次的环，
见第 11 节。`s20_set_cfg` 在 DEPCFG 和
DALEPENA 之后、返回状态段之前调用 `s20_notify`：这一下包含
空环上的 `STARTTRANSFER` 和 `wValue = 1` 的
`NETWORK_CONNECTION`，然后把 `link` 置 2。通知完成时再发
`SPEED_CHANGE`。bulk IN 那时还没 enable 的话，无偿 ARP 记到
`garp`，等 SET_INTERFACE alt 1 再发。SET_INTERFACE 不再改
`link`，也不再补发连接通知。

`s20 ev`、`s20 ln`、`s20 arp`、`s20 icmp` 不占那 80 条，另外
最多 16 条。TRB 环没动。

## 11. ring51：通知提前之后，第一包完成了

这次启动是 seq 6558 起，`freq=19200000`。Mac 再次把 `en10`
标成 active，并分到 `169.254.195.198`。`ping 169.254.1.1`
仍是全丢。环在 10.638s 之后没有新记录；16 条数据面配额只用了
6 条，所以不是限额把后面的事件藏起来。soak 的下一包 `f=` 要
很久才写，这段空白只说明没有新的 USB 事件。

SET_CONFIG（`su=0x10900`，启动后 10.507s）同一毫秒里已经有
`f1=0x420082`、通知资源号 `s20 rs=0x30003`、`s20 ln=1`。
连接通知是在 SET_INTERFACE 之前交出去的。alt 1 在 10.544s，
bulk OUT 资源号仍是 `s20 rs=0x20002`。`su=0x4321` 仍是
过滤器 `wValue = 0`。

随后三条端点事件，类型都是 `XferInProgress`（`(ev>>6)&0xf == 2`）：

| 时刻 | 原始字 | 物理端点 | 状态半字节 |
|---|---|---|---|
| 10.624s | `0x6084` | 2，bulk OUT | `0x6` = SHORT \| IOC |
| 10.636s | `0x4086` | 3，通知 | `0x4` = IOC |
| 10.638s | `0x408a` | 5，bulk IN | `0x4` = IOC |

状态位在 `core.h` 1437 行：bit 1 是 `DEPEVT_STATUS_SHORT`，
bit 2 是 `DEPEVT_STATUS_IOC`。通知端点的完成紧跟着
`s20 ln=2`（速率通知交出去）和 `s20 arp=1`（无偿 ARP）。
bulk IN 的事件说明主机 ACK 了这帧无偿 ARP。
bulk OUT 那一包没有引出第二次 `s20 arp`，所以它不是指向
`169.254.1.1` 的 ARP 请求。`s20_on_out` 对认不出的帧不落日志。

没有第二包。`gadget.c` 收回一条 TRB 时 `dwc3_ep_inc_deq`
（3094 行）把读指针拨到下一项；下一条请求写在
`trb_enqueue`，`dwc3_prepare_one_trb` 末尾再
`dwc3_ep_inc_enq`（1285 行）。索引碰到 Link（第 255 项）就
绕回 0（`dwc3_ep_inc_trb`，154 行）。ring51 那一版每次都改
TRB0 再 `UPDATETRANSFER`。第一包之后控制器已经停在 TRB1，
TRB1 的 HWO 仍是 0。后面的令牌不再落到 TRB0 上。速率通知是在
连接通知完成之后写回 TRB0 的，环上没有第二条通知端点事件。

下一版按这个指针走：完成时 `TRB_DEQ` 拨到下一项，新请求写在
`TRB_ENQ`，再 `UPDATETRANSFER`。`s20 q` 记下物理端点和写下去
的下标。Link 仍在第 255 项。写 TRB 前先 `dc ivac` 那一条缓存行，
因为一行里有四条 TRB，设备不窥探，直接 `civac` 会把已经完成的
HWO 盖回 1。

## 12. ring52：指针在走，ICMP 目的地址写错了

这次启动 seq 6707。bulk OUT 从 `s20 q=0x200`（下标 0）接到
`0x209`（下标 9）。每来一个短包（`ev=0x6084`，物理端点 2，
状态 SHORT|IOC）就拨到下一项。通知从下标 0 到 1
（`q=0x301`，`ln=2`）。无偿 ARP 写在 bulk IN 下标 0
（`q=0x500`），主机 ACK（`ev=0x408a`）。这和 Mac 邻居表里
`169.254.1.1` → `02:00:00:00:00:02` 对得上。

这 9 个 OUT 短包都挤在启动后 10.64s 到 11.22s，没有一条
`s20 icmp`，也没有第二条 `s20 arp`。数据面日志配额 24 条在
11.22s 用完，其中 13 条是 `s20 q`。ping 发生在这之后，
环上看不到。

`s20_tx_icmp` 把应答的 IPv4 目的地址写成了 `OUR_IP`。
以太网目的是请求方的 MAC，IP 目的却是设备自己。主机收到帧，
IP 层不认。源地址改为 `OUR_IP`，目的地址改为请求里的源地址
（`OUT_BUF[26..30]`）。`s20 q` 只记每个端点的前两条。

这一版刷上之后，`en10` 是 `169.254.103.115`。`ping 169.254.1.1`
三次全回，ttl 255，往返 1.3–3.1 ms。隔了一阵再打两次，仍全回。
邻居是 `02:00:00:00:00:02`。这次启动的环还没拉。

下一版把 ARP、ICMP 和 TCP 交给 smoltcp。地址仍是
`169.254.1.1`。TCP `2323` 在连接建立后送出一行 `aginx`，
环上记 `s20 tcp=1`。一次只挂一条 IN（`max_burst_size = 1`）。

这一版已经刷上。`en10` 是 `169.254.212.211`。`ping 169.254.1.1`
两次全回，ttl 64（smoltcp 的默认跳数），往返 1.2–2.9 ms。
对 `2323` 收满 6 字节，内容是 `aginx` 加换行。这次启动的环还没拉。

再下一版仍先回 `aginx`。之后读到换行：`id` 回 `redfin`，`ping` 回 `pong`，
空行不回，别的回 `?`。环上 `s20 cmd` 的值是 1 / 2 / 0。

这一版已经刷上。`en10` 是 `169.254.219.152`。ICMP 两次全回，ttl 64。
同一条 TCP：`aginx`，然后 `id` → `redfin`，`ping` → `pong`，`nope` → `?`。

`up` 这一版已经刷上。`en10` 是 `169.254.23.47`。ICMP 两次全回。
同一条 TCP：`up` → `up 46 seq 7528`，两秒后再问 → `up 48 seq 7530`，
`id` 仍是 `redfin`。

## 13. `fb`：从控制口进 fastboot

L0 上 `aginx-reboot bootloader` 走的是 `reboot(RESTART2, "bootloader")`。
`msm-poweroff.c` 的 `msm_restart_prepare` 做两件事，然后
`qpnp_pon_system_pwr_off(WARM_RESET)`，最后把 PS_HOLD 写成 0：

- pm8150（SID 0，`qcom,system-reset`，`qcom,store-hard-reset-reason`）
  PON 外设 `0x800`。gen2 时 `SOFT_RB_SPARE`（偏移 `0x8F`）的 bit7:1
  写成 `PON_RESTART_REASON_BOOTLOADER << 1`（原因值 0x02）。
  gen1 则 bit7:2 写成原因左移 2。subtype 在偏移 `0x05`：
  `0x04/0x05` 是 gen2，`0x01/0x02/0x03` 是 gen1。
- IMEM `0x146AB000 + 0x65C` 写 `0x77665500`。
- 热复位：先清 `PS_HOLD_RST_CTL2`（`0x5B`，gen1 且 revision 0 时用
  `0x5A`）的 bit7，等 500µs，把 `0x5A` 的低 4 位写成
  `PON_POWER_OFF_WARM_RESET`（0x01），再把 bit7 置上。
  pm8150 没有 `qcom,warm-reset-poweroff-type`，类型保持 0x01。
- `lito.dtsi` 的 `qcom,pshold` 是 `0x0C264000`。写成 0。

misc 里的 `bootonce-bootloader` 在 2026-09-09 已证明 ABL 不认。
这条命令不走那条。

控制口收到 `fb` 先做只读检查。副本不在、owner 不是 APPS、
subtype 不认识，就回 `fb <错误码>`，不复位。检查过了回 `fb`，
等这条 IN 完成再等 20ms，然后写原因并拉 PS_HOLD。

这一版已经刷上。`en10` 是 `169.254.101.9`。先 `up 45 seq 7693`，
再 `fb`，应答就是 `fb`。大约 10 秒后 fastboot 出现，product
是 redfin，槽位仍是 a。随后从 fastboot 重启回裸机。

`log` 读环上最近四条（游标前 4 块，旧的在前），读失败不复位。

这一版用上一块镜像的 `fb` 自己进了 fastboot 再刷上。`en10` 是
`169.254.57.89`。`log` 回了四行，末行是 `#8017 s20 tcp=0x1`。

`bat` 用 soak 同一组寄存器现读 f 字，读失败不复位。这一版同样
经 `fb` 刷上。`en10` 是 `169.254.39.160`。应答
`bat f=593effe3ff450117`：电压原始 22846（约 4447 mV），电流
原始 -29（约 -9 mA），MSOC 255（TRUTH：255=100%），充电状态
0x45（bits[2:0]=5 TERMINATE）。
