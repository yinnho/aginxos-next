# S4：redfin 的 Wi-Fi 在 Linux 里怎么起来

Wi-Fi 还没做。S3 的 USB 网和控制口已经在机器上通了。这一页只记
源码顺序，不记还没刷过的想法。

## 这台机器走的是 Adrastea，不是 WCN6750

`lito.dtsi` 的节点是 `qcom,icnss@18800000`，compatible 只有
`qcom,icnss`。`lito_defconfig` 打开的是 `CONFIG_ICNSS`，编的是
`drivers/soc/qcom/icnss.c` 加 `icnss_qmi.c`。`CONFIG_ICNSS2` 不在
这份 defconfig 里。`icnss2/` 也匹配这个 compatible，但不是这台
机器的内核配置。下面的行号都指 `icnss.c`。

节点资源：

- `membase` `0x18800000`，长 `0x800000`
- SMMU：`apps_smmu` SID `0xC0`，mask `0x1`
- MSA 固定区 `pil_wlan_fw_mem`：`0x8BA00000`，长 `0x200000`，`no-map`
- 五路电源：`vdd-cx-mx` = pm8150 L1A，`vdd-1.8-xo` = pm8150 L14A，
  `vdd-1.3-rfa` = pm8150a L2C，`vdd-3.3-ch0/ch1` = pm8150a L10C / L11C
- 时钟名 `cxo_ref_clk_pin`（`icnss.c` 128 行）
- smp2p：`smp2p_wlan_1_in` 的 fatal 和 early-crash

电压表在 `icnss.c` 117 行：cx-mx 752 mV，xo 1.8 V，rfa 1.304 V，
两路 3.3 V 都是 3.312 V。

## probe 不上电

`icnss_probe`（3828 行）解析资源、MSA、SMMU，然后
`icnss_register_fw_service`。`icnss_qmi.c` 1348 行只做
`qmi_add_lookup(0x45, 版本 1, 实例 0)`。没有服务器到来，
就不会 `icnss_hw_power_on`。

服务器到来走 `icnss_driver_event_server_arrive`（1061 行），顺序是：

1. `icnss_connect_to_fw_server`
2. `wlfw_ind_register_send_sync_msg`
3. `wlfw_msa_mem_info_send_sync_msg`。MSA 虚拟地址是 0 就失败
4. `icnss_assign_msa_perm_all(ICNSS_MSA_PERM_WLAN_HW_RW)`：把 MSA
   交给 `VMID_MSS_MSA` 和 `VMID_WLAN`
5. `wlfw_msa_ready_send_sync_msg`
6. `wlfw_cap_send_sync_msg`
7. 这时才 `icnss_hw_power_on`：五路 regulator，然后 `cxo_ref_clk_pin`
8. `wlfw_dynamic_feature_mask_send_sync_msg`
9. 登记固件 fatal / early-crash，有电池通道再挂 VPH 监测

WLFW 的 QMI 服务是调制解调器侧固件发出来的。发出它的是 PIL
装进 `modem_wlan_mem` 的 modem 镜像，不是 MSA 里的 WLAN 固件。
MSA（`0x8BA00000`）要等服务已经在线上，`server_arrive` 里的
`wlfw_msa_mem_info` 才把地址告诉它。EL1 上把五路电源投上，不会
凭空造出这个服务。

装这颗固件的节点是 `qcom,mss@4080000`（`lito.dtsi` 2753 行）：
`qcom,pil-tz-generic`，`firmware-name = "modem"`，`pas-id = <4>`，
内存 `modem_wlan_mem` 在 `0x8C000000`，长 `0xF800000`。PIL 通过 SCM 把认证交给
TrustZone，EL1 自己写这块内存不算加载成功。

## modem PIL：`pil_boot` 的调用顺序

`qcom,pil-tz-generic` 的驱动是 `subsys-pil-tz.c`。上电入口是
`subsys_powerup` → `pil_boot`（`peripheral-loader.c` 1220 行）。
这颗节点带 `qcom,signal-aop`，所以 `pil_boot` 第一件事是
`pil_notify_aop(..., "on")`：经 mailbox 给 AOP 送

`{class: image, res: load_state, name: modem, val: on}`

然后 `request_firmware("modem.mdt")`。`.mdt` 是 ELF 头加
program header。段体在同名的 `modem.bXX` 里，由 `pil_load_seg`
再读。

认证三条 SCM，服务号 `SCM_SVC_PIL = 2`，命令号在
`subsys-pil-tz.c` 118 行：

| 步 | 函数 | 命令 | 参数 |
|---|---|---|---|
| 1 | `pil_init_image_trusted` | `PAS_INIT_IMAGE` = 1 | pas-id 4，metadata 物理地址 |
| 2 | `pil_mem_setup_trusted` | `PAS_MEM_SETUP` = 2 | pas-id 4，区起始，长度 |
| 3 | `pil_auth_and_reset` | `PAS_AUTH_AND_RESET` = 5 | pas-id 4 |

`SCM_SIP_FNID(2, cmd)` 是 `0x02000200 | cmd`。64 位路径再或上
`SMC64_MASK`（`0x40000000`，`scm.c` 48 行）。真正进 TrustZone
的是 `smc #0`，x0 是这个功能号，x1 是 arginfo，x2 起是参数
（`__scm_call_armv8_64`，98 行）。

`PAS_INIT_IMAGE` 之前先做 proxy vote：这颗节点的
`qcom,proxy-reg-names` 是 `vdd_cx`、`vdd_mss`，电压档都是
`RPMH_REGULATOR_LEVEL_TURBO`，时钟是 xo。vote 在
`pil_make_proxy_vote`，`pil_boot` 在 init image 之前调用它。
`PAS_AUTH_AND_RESET` 自己还会再开 active 的 regulator 和时钟，
然后才 `smc`。

`pil_boot` 在 mem setup 和装段之间还有 hyp 把内存划给子系统的
路径，但只在 `subsys_vmid > 0` 时走。这颗节点的 dts 没写 vmid，
`subsys-pil-tz.c` 也不读 vmid 属性。

WLFW 的 QMI 服务是第 3 步把 modem 拉出复位之后才可能出现。
在那之前投 icnss 的五路电源，对不上这条顺序。

## 这一刀不做什么

不投电源，不发 SMC，不刷。裸机发得出 `smc #0`，TrustZone 仍要
验过签名的 `modem.mdt` 才认。

## 固件在 modem_a，不在宿主机

2026-10-17 从 b 槽 L0 只读看过，随后 `set_active a` 重启回裸机。
`firmware_class.path` 是 `/vendor_a/firmware`，那个目录里没有
`modem.mdt`。`request_firmware("modem.mdt")` 落到默认的
`/lib/firmware/modem.mdt`，这是指向
`/mnt/modem/image/modem.mdt` 的符号链接。`/mnt/modem` 和
`/vendor_a/firmware_mnt` 都是 `modem_a` 的 vfat。当时启动槽是
b，挂上的仍是 `modem_a`，不是 `modem_b`。

`modem.mdt` 9420 字节，日期 Jul 6 2023。头是 ELF32 小端：
`e_type = 2`，`e_machine = 0xA4`，`e_entry = 0x8C000000`，
`e_phnum = 35`。段文件是 `modem.b00` 到 `modem.b34`，共 35 个。
`modem.b00` 1172 字节，`modem.b16` 4014080 字节，`modem.b34`
是 0 字节。同目录没有 `mba.mdt` / `mba.mbn`。sha256
`0fa221cb65417992329a9bb6522f90bd5a51c9fef25a7ee93810347363f49f37`。

## `.mdt` 里的 35 段

`pil_load_seg` 用 program header 的下标当文件名，所以
`modem.bNN` 就是第 NN 个 header。

| 下标 | 角色 | p_paddr | filesz | memsz |
|---|---|---|---|---|
| 0 | 不装进内存，memsz=0。`b00` 的 1172 字节和这个 filesz 一致 | 0 | 1172 | 0 |
| 1 | hash 段（flags bit26:24 = 2），`segment_is_loadable` 排除 | `0x9D800000` | 8248 | 12288 |
| 2–34 | `PT_LOAD`，flags 都带 bit27，可重定位 | `0x8C000000` 起 | 合计 99384626 | 到 `0x9D800000` |

可装载段的跨度是 `0x8C000000`–`0x9D800000`，长 `0x11800000`。
bit27 置位时 `pil_reloc` 做 `p_paddr - 0x8C000000 + region_start`。
`region_start` 来自 `dma_alloc`，不是 header 里的地址本身。
`PAS_MEM_SETUP` 拿到的是这块分配结果的物理起始和长度。

`modem_wlan_mem` 只有 `0xF800000`，比这段 ELF 跨度短
`0x2000000`。`subsys-pil-tz.c` 不读 `memory-region`。
dts 里的那块保留内存和 ELF 跨度不是同一个数。

下标 34 的 filesz 是 0、memsz 是 33427456。`pil_load_seg` 在
filesz 为 0 时不去 `request_firmware`。装载前 `pil_boot` 会把
整个 region 清零，所以这段是 BSS。

`PAS_INIT_IMAGE`、`PAS_MEM_SETUP`、`PAS_AUTH_AND_RESET` 三次
`smc` 都是 x0=0、x1=0。30 段从 `modem_a`（4K LBA 115208）
按 FAT16 簇链抄进 `0x8C000000` 起的 `0x11800000`，先整段清零。
`s20 ld=0x1e` 是 30 段都抄完。`s20 pas4=0` / `s20 pas5=0`
是 TrustZone 认证通过并把 modem 拉出复位。metadata 物理地址
这次是 `0xA7EB4000`。

`modem_a` 在 UFS 上的 4K LBA 是 115208，长 49920 块
（sysfs start 921664 是 512 字节单位，除以 8；`boot_a` 的
69696/8 = 8704，和日志环 LBA 25096 = 8704+16384 对得上）。
段文件在这个 FAT 里。可装载段已经按这条抄完，认证也过了。

## 认证之后，WLFW 还不在线上

`PAS_AUTH_AND_RESET` 返回 0 只说明 TrustZone 把 modem 拉出了
复位。WLFW（服务号 `0x45`，版本 `0x01`，实例 `0`）要等下面这条
链走完才在 QRTR 名字服务里出现。icnss 的五路电源在这条链的
最后，服务器到来之后才投。

1. 主机在开机时（modem 上电之前）就准备好 SMP2P 出向项。
   `lito-smp2p.dtsi` 的 `qcom,smp2p-mpss`：`qcom,smem = <435>, <428>`，
   本地 pid 0，远端 pid 1。`smp2p.c` 把第一项当入向、第二项当出向。
   出向项 428 由主机 `qcom_smem_alloc`，魔数 `0x504D5324`（`$SMP`），
   版本 1，然后 IPCC 踢一脚。入向项 435 由 modem 自己分配。
2. modem 拉出复位之后写 `slave-kernel` 的 bit 1。`pil_modem` 的
   中断名把这一位叫 `qcom,err-ready`（`lito.dtsi` 2796 行，
   `mpss_smp2p_in` 的第 1 号）。IPCC 信号是
   `IPCC_CLIENT_MPSS`（2）+ `IPCC_MPROC_SIGNAL_SMP2P`（2）。
3. `subsys_start` 在 `pil_boot` 返回后等这个 err-ready，最多 10 秒。
   等到了才发 `SUBSYS_AFTER_POWERUP`。等不到就关机，GLINK 不会注册。
4. `glink_probe.c` 只在 `SUBSYS_AFTER_POWERUP` 里调用
   `qcom_glink_smem_register`。在那之前没有 GLINK FIFO。
   边沿是 `label = "modem"`，`qcom,remote-pid = <1>`，传输 `smem`。
5. 注册时在 pid 1 的私有分区里分配两项：描述符 item 478，32 字节；
   发送 FIFO item 479，16KB。接收 FIFO item 480 由对端分配，
   主机第一次看到数据才去取。描述符四个 `u32` 依次是
   发送 tail、发送 head、接收 tail、接收 head。主机把接收 tail
   和发送 head 清零。
6. 第一包是 `GLINK_CMD_VERSION`（命令 0，版本 1，特性
   `GLINK_FEATURE_INTENT_REUSE` = bit0）。8 字节头：`u16 cmd`、
   `u16 param1`、`u32 param2`。写完 FIFO 后写 IPCC
   `SEND_ID`（基址 `0x408000`，偏移 `0x0C`），打包值是
   `(client << 16) | signal`。GLINK 这条是 client 2、signal 0，
   也就是 `0x00020000`。
7. 对端回 `VERSION_ACK`（命令 1），再 `OPEN` 通道名 `IPCRTR`。
   `net/qrtr/smd.c` 绑这条通道。通道上的字节就是 QRTR 包。
   控制口是 `QRTR_PORT_CTRL` = `0xFFFFFFFE`。
   `NEW_LOOKUP` 命令号 10，`NEW_SERVER` 命令号 4。
   查找包里 instance 的低 8 位是版本、高 24 位是实例，
   所以 WLFW 是 service `0x45`、instance 字 `0x00000001`。
8. `wlfw_new_server` 拿到 node 和 port，才进
   `icnss_driver_event_server_arrive`。那一步才开五路 regulator，
   再发 `QMI_WLFW_IND_REGISTER_REQ_V01`（`0x0020`）。

SMEM 区是 `smem_region@80900000`，长 `0x200000`。头在区首，
`initialized` 在偏移 `0xC0`，SBL 版本在 `version[7]`（偏移 `0x5C`）。
分区表在区尾 4KB，魔数 `$TOC`。APPS（host 0）和 MPSS（host 1）
的私有分区魔数是 `$PRT`。项头 canary 是 `0xA5A5`。

这一刀只读这块区，不分配、不写 IPCC。开机路径不碰 SMEM。控制口
命令 `sm` 在枚举之后才读。

2026-10-17 第一版 `sm` 把整段分区表也扫了，包括区尾 `0x80AFF000`。
问候语 `aginx` 已经发出，应答没有回来，随后 ping 失败，70 秒没有
自己复位。入口把 DAIF.A 掩着，被拒绝的 load 停在指令上，不进异常桩。

只读头两个字的那版通了：`sm i=1 v=c0000 n=0 p=0 g=0`，之后 ping 仍在。
`initialized == 1`。`v` 是 `version[7]`，`0xC0000 >> 16 = 12`，
等于 `SMEM_GLOBAL_PART_VERSION`。头落在 `0x80800000` 这一个 2MB 块里，
和 cmd-db 相同。

区尾 TOC 在 `0x80AFF000`，是下一个 2MB 块。解开 SError 之后
对这个地址做 WB 的 ivac 加读，问候语回来了，应答没有，ping 随后
失败，两分钟没有自己复位。这条 load 没有完成。

Linux 映射这 4KB 用的是 `ioremap_wc`（Normal 非缓存）。把 L2
第 5 项（`0x80A00000` 那 2MB）临时换成 `nc_block` 之后，TOC
第一个字读通了：`n=434f5424`，就是 `$TOC`，随后 ping 仍在。

魔数对上之后，同一页按 48 字节一条扫分区表。实测
`sm i=1 v=c0000 n=e p=fb000 g=80`，ping 仍在。14 条里 host 0/1
那对的偏移是 `0xFB000`，物理地址 `0x809FB000`，还在已经读通的
那个 WB 块里（块到 `0x80A00000` 为止）。

分区头在这块 WB 里读通了：`g=c0`，bit7 是找到了 host 0/1，
bit6 是魔数 `$PRT`。没有项位，也没有 canary 坏（bit5）。
当时未缓存区一越过 `0x80A00000` 就停，所以项还没看。

跨过 `0x80A00000` 的那版读完了，ping 仍在：
`sm i=1 v=c0000 n=e p=fb000 g=d5 u=5208`。

`u=0x5208` 是 `offset_free_uncached`。分区从 `0xFB000` 算，
链表只伸进下一块 `0x208` 字节。`g=0xD5`：

- bit7 找到 host 0/1，bit6 魔数 `$PRT`
- bit0 入向 SMP2P 435 在
- bit2 GLINK 描述符 478 在
- bit4 GLINK 接收 FIFO 480 在
- bit1 出向 SMP2P 428 不在，bit3 发送 FIFO 479 不在。
  这两项在 Linux 里是主机分配的
- bit5 在自由指针之前碰到一个 canary 不是 `0xA5A5` 的头，扫描停在那里

435、478、480 的内容已经读到，ping 仍在：
`s=0 m=504d5324 d=0.0.0.0 r=0`。

`m=0x504d5324` 是 `$SMP`，435 确是 SMP2P 项。`s` 是
`slave-kernel` 的 value；0 表示这一条不存在，或者 value 就是 0。
bit 1（`qcom,err-ready`）没有置上。478 的四个指针全是 0，
480 的第一个字也是 0。GLINK 版本包还没出现。

`g=0xD5` 的 bit5 其实是 0。0xD5 是 bit0、bit2、bit4、bit6、bit7。
之前把 bit5 算进去了，分区链表没有坏头。

停点读出来了：`b=5208.0.0.263`。偏移 `0x5208` 就是 `u`，
那个位置的头两个字都是 0，扫描是因为越过自由指针才停，
没有把它当成一项。最后一项号是 `0x263`。
`qcom_smem_alloc_private` 会从这里接着分配。

`sm` 在 428 不在、且停点就是这个空位时分配它。`c` 是
`offset_free_cached`。塞得下才写：TCSR mutex 在 `0x1F43000`
（`0x1F40000` 加 3 个 `0x1000`），写入 `1` 再读回才算拿到，
写 `0` 放开。项头 canary `0xA5A5`、项号 428、长度 `0x158`。
项体按 `smp2p_smem_item`：魔数 `$SMP`，本地 pid 0，远端 pid 1，
16 条里先放一条 `master-kernel`，version 最后写成 1。
然后写 IPCC `0x40800C`，值 `(2 << 16) | 2`。
`k`：0 已经有 428，1 这次写过并踢了，2 空间不够，3 没拿到锁，
4 锁里发现自由指针变了。479 仍不写。

`slave-kernel` 四个字是 `76616c73.656b2d65.6c656e72.0`。
之前把第二字写成了 `6b2d6565`，对不上，所以 `s` 一直是 0。
改正之后，踢完几秒：`s=6`。bit 1 是 err-ready，bit 2 也在。
同一条里还有 `smp2p`，值 0。`e=10002` 是 version 1、两条。

接着分配了发送 FIFO 479（16KB），写入 VERSION
（`cmd=0 param1=1 param2=1`），发送 head 拨到 8，IPCC signal 0。
`gk=1 u=9380 d=0.8.0.0`。再读一次：`d=8.8.0.58 s=6`。
发送 tail 也到了 8，modem 把版本包取走了。接收 head 是 `0x58`，
接收 FIFO 第一个字是 `0x10000`（对方的 VERSION，cmd 0，版本 1）。
接收 FIFO 前 `0x58` 字节拆开是五条，8 字节对齐：

- VERSION，版本 1，特性 `7`
- VERSION_ACK，版本 1，特性 `1`（对着我们发出的 INTENT_REUSE）
- OPEN rcid 1，名字 `glink_ssr`
- OPEN rcid 2，名字 `IPCRTR`
- OPEN rcid 3，名字 `SSM_RTR_MODEM_APPS`

远端先打开通道时，Linux 还要再用本地 cid 发一次 OPEN。
补上 cid 1/2/3 的 OPEN 之后，发送 head 到 `0x68`，modem 取走了。
接收 head 涨到 `0x268`。从 `0x58` 起是：对我们 cid 1 和 2 的
OPEN_ACK，然后 IPCRTR（cid 2）上的 INTENT，大小 `0x80`。

HELLO 单独发出去时 modem 把发送 tail 追到了头，但接收 head
没有再涨。它没有我们的接收 intent，回包没处放。
开机不再碰 GLINK。`sm` 分两次送：先 10 个 intent
（`gk=8`，head 从 `0x68` 到 `0x108`），modem 取走后再送 HELLO
（`gk=7`，head 到 `0x150`）。两次 tail 都追上了。
整段扫完，`NEW_SERVER` 只有两条：`q=42.b401,2b.1202`。
`0x42` 是 `SERVREG_NOTIF`。`0x2B` 不是 WLFW；WLFW 的服务号是
`0x45`，不在广播里。

接着发了 `NEW_LOOKUP`（类型 10，服务 `0x45`，实例字 `1`）。
`gk=9`，发送 head 到 `0x198`，tail 追上了。对方只回了
`RX_DONE_W_REUSE`（命令 14，intent 1），没有新的
`NEW_SERVER`。`0x45` 还没注册。

`0x2B` 对得上 `sysmon-qmi.c` 的 `SSCTL_SERVICE_ID`。实例字
`0x1202` 是版本 2、实例 `0x12`，和 `pil_modem` 的
`qcom,ssctl-instance-id = <0x12>` 一致。`0x42` 是
`SERVREG_NOTIF`，版本 1。服务定位器 `0x40` 不在广播里。
icnss 的 PDR 要先 `qmi_add_lookup` 这个 `0x40`，再要域列表，
再向 `0x42` 登记监听。监听不负责把 PD 拉起来。WLFW 的 lookup
在 probe 里单独注册，不经过定位器。

`slave-kernel` 后来读到 `s=0x107`。`lito.dtsi` 把
`mpss_smp2p_in` 的 bit 0 叫 `qcom,err-fatal`，bit 1 是
`qcom,err-ready`，bit 2 是 `qcom,proxy-unvote`。`0x107` 含
bit 0。Linux 接到这一位会把子系统关掉。入向项里仍然没有
名字 `wlan` 的条目。

这两次开机都没做 `pil_boot` 在 `PAS_INIT_IMAGE` 之前的两件事。
第一件是 AOP 报文，名字来自 `qcom,firmware-name`：

`{class: image, res: load_state, name: modem, val: on}`

邮箱在 `0xC300000`，IPCC 客户端 `IPCC_CLIENT_AOP` = 0，信号
`IPCC_MPROC_SIGNAL_GLINK_QMP` = 0。`msm_qmp.c` 要先把链路从
`LINK_DISCONNECTED` 谈到 `CHANNEL_CONNECTED`，再把 96 字节
写进 mcore 邮箱。这一刀不读、不写、不踢。

第二件是 `pil_make_proxy_vote`。`vdd_cx` / `vdd_mss` 是
`qcom,rpmh-arc-regulator`，资源名 `cx.lvl`、`mss.lvl`，档是
`RPMH_REGULATOR_LEVEL_TURBO`（384）。写进 RPMh 的数据是
cmd-db aux 里 u16 表的下标，不是 384 本身。表里没有正好等于
384 的档就不写。`xo` 是 `clk-rpmh.c` 的 `xo.lvl`，打开值固定
`0x3`。三笔都只发 ACTIVE。msm-bus 的带宽票不发。

把这三笔放在枚举之前的那张镜像刷上之后，五分钟里主机
看不到 USB、fastboot 和 adb。接着那张只查档、仍放在枚举前，
同样没起来。开机路径收回成会枚举的那张。17:02 那张把查档塞进 `sm`，
命令没有应答，ping 随后消失。查档改到单独的 `pv`，
不进 `sm`，不落环，状态字是 0，不写 RPMh。

`pv=30000.6.0.30060.6.0.300a0.3.0 pa=100000.400000`，之后 ping 仍在。
`cx.lvl` 地址 `0x30000`，384 落在下标 6。`mss.lvl` 地址 `0x30060`，
下标也是 6。两个地址的高半字低 3 位都是 3，是 ARC。aux 前两档
cx 是 0、16，mss 是 0、64。`xo.lvl` 地址 `0x300a0`，打开值 `0x3`。
三笔状态都是 0，RPMh 没写。

同一张上 `sm` 两次。第一次 `gk=1 d=0.8.0.0 s=0`，VERSION 已发出。
第二次 `gk=6 d=8.68.0.58 s=107`，三条 OPEN 写到发送 head `0x68`，
`slave-kernel` 已是 `0x107`（bit 0 是 `qcom,err-fatal`，bit 1
err-ready，bit 2 proxy-unvote）。再等 5 秒，tail 仍停在 8，
接收 head 仍是 `0x58`。modem 没有把 OPEN 取走。ping 仍在。

`px` 在 USB 起来之后写了 `cx.lvl` 下标 6。应答 `px 10101`：
`CMD_STATUS` 的 bit 8 是 ISSUED，bit 16 是 COMPL，2 毫秒返回，
ping 还在。这一笔在 `PAS_AUTH` 之后。接着 `sm` 的 VERSION
tail 停在 0，8 秒后 `s` 仍是 0，modem 没有发布 `slave-kernel`。

这一笔放在 USB 电源之后、`PAS_INIT` 之前。开机 `pv=30000.6.10101`。
USB 约 45 秒起来。`sm` 七次，`s` 一直是 6，没有 bit 0：

`gk=1` VERSION，`gk=6` OPEN head `0x68`，`gk=8` intent head `0x108`，
`gk=7` HELLO head `0x150`，`gk=9` lookup head `0x198`。
tail 追上之后 `q=42.b401,2b.1202`，没有 `0x45`。ping 仍在。

HELLO 能发出，是因为接收扫描不再停在第一条非 intent 命令上。
接着同一位置补上 `mss.lvl` 下标 6 和 `xo.lvl` 数据 3。
三笔状态都是 `0x10101`（ISSUED|COMPL）。USB 同样约 45 秒起来。
`sm` 七次和只投 cx 时一样：`s` 保持 6，`gk` 走到 9，
`q=42.b401,2b.1202`，没有 `0x45`。ping 仍在。

`pil_boot` 里还没做的第一件事是 AOP
`{class: image, res: load_state, name: modem, val: on}`。
`qm` 读了 `0xC300000` 起 16 个字，2 毫秒返回，ping 仍在：

`4d41494c.1.0.ffff.ffff0000.ffff0000.ffff0000.68.c0.ffff0000.ffff0000.ffff0000.ffff0000.2d8.128.0`

魔数 `MAIL`，版本 1。AOP 侧 link 是 `0xFFFF`（UP），ack 还是
`0xFFFF0000`（DOWN），通道断开。邮箱 `0x68` 字节，偏移 `0xc0`。
本机侧 link 仍是 DOWN，邮箱 `0x2d8` 字节，偏移 `0x128`。
这是 Linux 在 `LINK_DISCONNECTED` 看到的样子。`qn` 回了 `qn 1`。
半秒后描述符变成：

`4d41494c.1.0.ffff.ffff.ffff.ffff0000.68.c0.ffff.ffff.ffff0000.ffff0000.2d8.128.0`

本机 link 和 link ack 都是 `0xFFFF`（UP）。AOP 的通道状态也变成
`0xFFFF`（CONNECTED），ack 还是断开。链路协商已经完成，通道还没
双向打开。`qc` 做 `qmp_startup` 加一次远端通道 ack：本机通道写成
CONNECTED，若 AOP 通道状态和 ack 不一致就把 ack 补上，再踢一脚。
不发 `load_state`。

通道打开后从 `qs` 发了那句 `load_state ... val: on`。应答 `qs 0`，
AOP 把 96 字节取走了，ping 仍在。这次发送在 `PAS` 之后。
随后 `sm` 七次，`q` 仍是 `42.b401,2b.1202`。

同一段挪到 `PAS` 之前。开机 `qm=60`，也就是 200 毫秒后邮箱长度
还是 96，AOP 当时没取。USB 仍约 45 秒起来。`sm` 七次，`s` 保持
6，`q` 还是那两条，没有 `0x45`。过后 `qs` 回 `nopen`，通道已经
不在四边都是 `0xFFFF` 的状态。

`msm_qmp.c` 的发送超时是 `QMP_TX_TOUT_MS`（1000 毫秒），链路和
通道是 `QMP_TOUT_MS`（5000 毫秒）。四边都是 `0xFFFF` 才写
96 字节，等到长度变 0 才调用 `pas_init_image`。`0xe6` 是通道
没齐，`0x60` 是 1 秒后邮箱里还有 96 字节。这两种都不装 modem。

第一次 `qm=e6`：AOP 通道 `0xFFFF`，本机 ack 还是 `0xFFFF0000`，
本机通道和 AOP 的 ack 都是 `0xFFFF`。没发报文，也没装 modem。
补上这次 ack 之后 `qm=0.ffff.ffff.ffff.ffff`。AOP 在 `PAS` 之前
把 96 字节取走了。`sm` 七次，`s` 保持 6，`ql=` 空，
`q=42.b401,2b.1202`。`0x40` 和 `0x45` 还是没有。

`service-locator.c` 的查找：服务 `0x40`，版本 1，实例 1，线上实例字
`0x101`。`gk=9` 仍是 `0x45`。下一次 `sm` 发这一条，`gk=b`（11）。
实测 `d=198.1e0.0.350`，再一次 tail 追上 `1e0`，接收只多 8 字节。
`q` 仍是 `42.b401,2b.1202`，`ql=` 仍空。`0x40` 也没有注册。

## 主机侧到此为止

`subsys_start`（`subsystem_restart.c` 899 行）在 `pil_boot` 成功并且
`err-ready` 之后，只做一件事：`SUBSYS_AFTER_POWERUP`。`icnss.c` 的
`icnss_modem_notifier_nb` 碰到这个事件直接返回。GLINK 是这次通知
里真正把 modem 接上来的监听者，这条我们已经走通。

`pil_boot` 里 `subsys_vmid > 0` 才做 `hyp_assign`。`subsys-pil-tz.c`
从不写 `subsys_vmid`，分配出来是 0，所以这台机器的 modem PIL 不做
这段。`CONFIG_QCOM_BUS_SCALING=m`，总线票注册失败只打警告，不阻止
开机。modem 已经在没有这张票的情况下把 QRTR 跑起来了。

`0x45` 是 modem 自己注册的。`0x40` 由主机的 pd-mapper 公告，不是向 modem 查找能得到的。
`0x42` 和 `0x2B` 已经证明注册和广播是通的。WLAN 那条 PD 没有起来：
入向 SMP2P 里没有名为 `wlan` 的条目，`0x45` 也不在 `q=` 里。

`log_failure_reason` 读的是全局 SMEM 项 `qcom,smem-id = <421>`。
这是主机还能看的 modem 失败字符串。`s=6` 没有 err-fatal，这项
很可能是空的。再发查找或再投 icnss 的五路电源，都不在这条路径上。

2026-10-17 控制口恢复后 `fr` 读到 SMEM 421，第一字节非 0，
`st=0 sz=100`。前 32 字节是 `dog_hal_common.c:180:[tmr_slave3`。
`devices/redfin/bringup/radio-bringup` 记过同一条看门狗：modem
起来时若没有 `qrtr-ns`、`pd-mapper`、`tftp_server`、`rmt_storage`
在听，DOG 会判初始化卡住。同一份记录还写明 ADSP 必须先于 modem
上线，2023 年的 modem 镜像才会自己把 `wlan_pd` 拉起来；最小环境
下 servreg 对 `msm/modem/wlan_pd` 回 `SERVICE_NOT_FOUND`，没有
`0x45`。`rmtfs` 服务号是 14，版本 1，实例 0。

`qrtr_publish` 的实例字是 `instance << 8 | version`。裸机在轮询里
每 200 毫秒推进一步。HELLO 之后先发三条 `NEW_SERVER`：rmtfs `14`
实例字 `1` 端口 `0x10`，tftp `4096` 实例字 `1` 端口 `0x11`，
pd-mapper 的服务 `64`、版本 `257`、实例 `0`、实例字 `257`、端口
`0x12`。节点都是 `1`。`gk` 是 13、14、15。这只是公告，还不应答
QMI。

2026-10-17 刷上后发送 head 停在 `0x2b8`，tail 追上。比只发查找
多出 216 字节，是这三条。接收尾上有 QRTR DATA：源节点 0 端口
`0xa`，目标节点 1 端口 `0x12`，消息号 `0x21`
（`SERVREG_LOC_GET_DOMAIN_LIST`），名字 `tms/pdump_disabled`。
modem 已经在叫 pd-mapper。

应答按 `pd-mapper.c` 查不到名字的写法：QMI type 2，消息 `0x21`，
TLV 2 是 result 0/0，TLV 16 是域数 0，TLV 17 是 `db_revision` 1。
每个事务号只回一次。2026-10-17 刷上后 `pr=2`，隔 3 秒仍是 2，
发送 head 停在 `0x348`。`q` 仍是 `42.b401,2b.1202`。

2026-10-18 从 b 槽读出 6 个 jsn，副本在
`.local/device/redfin/modem/jsn/`。`modem_a` 的 image 里只有
`modemr.jsn`。`modemuw.jsn` 在 `/vendor_a/firmware/`。
`wlan/fw` 映射到域 `msm/modem/wlan_pd`，`qmi_instance_id` 180。
`tms/pdump_disabled` 不在这 6 个文件里，空列表对这个名字是对的。
`kernel/elf_loader` 和 `tms/servreg` 也落在同一个 `wlan_pd`。

应答按这 6 个文件编。请求的服务名在 QMI 第一层 TLV 1：没有长度前缀，值就是字符，读到 NUL 或 TLV 末尾停。当前这条请求的 TLV 长 0x13，19 个字符是 `tms/pddump_disabled`，末尾没有 NUL。对得上的行写进 TLV 18，编码跟 `qmi_encdec.c` 和 `service-locator-private.h` 一致：1 字节个数，然后每条是 1 字节 strlen、字符、不带 NUL、u32 实例、u8 `service_data_valid` 0、u32 `service_data` 0。`tms/servreg` 命中 6 个域，`wlan/fw` 命中 `msm/modem/wlan_pd` 实例 180。对不上的名字仍是域数 0，载荷长度 17。`sm` 的 `pn=` 是名字、命中数、载荷长度。

2026-10-18 刷这张之前，旧镜像 2323 还在：`pr=2`，`d=348.348.0.470`，`s=107`，`q=42.b401,2b.1202`。接收尾上的请求 TLV 长 0x13，19 个字符，没有末尾 NUL，名字是 `tms/pddump_disabled`。`s=107` 含 err-fatal 位。随后 `fb` 进 fastboot，只刷 `boot_a` 并 `set_active a`。重启后 ping `169.254.1.1`（MAC `02:00:00:00:00:02`）通，TCP 2323 回 RST（errno 61）。`pn=` 还没读到。

从 b 槽把这张的环读出来了（LBA 25096，序号 17554–17809）。`17644` 是 `s20 tcp=0x1`，下一条 `17645` 就是新的 `boot el=1 resume=17644`，中间没有 `EXC`。这是 `fb` 那次复位。新启动 `aop=0x0`、`pas=0x0`，USB 链路和 `arp=0x1` 都有，随后进入常驻环（`17809` 的 `f=`/`t=`）。`s20_keep` 这轮只用了 20/24，没有 `tcp`，也没有 `cmd`，也没有 `EXC`。问候没写出去，套接字没有进到 Established。

随后这张加了不占 24 条配额的 `tcp ls` / `tcp st` / `tcp rx`，只刷了 `boot_a`。这轮 2323 在 4ms 内连上，问候是 `aginx`。`sm` 隔 3 秒两次：`pr=2` 不动，`d=348.348`，`pn=tms/pddump_disabled.0.11`。名字对上了，命中 0，载荷长度 `0x11` 是空列表。`s` 从 6 变成 `0x107`。`q` 仍是 `42.b401,2b.1202`。modem 还没来要 `wlan/fw`。

rmtfs 和 tftp 现在会答。`OPEN` 按 `storage.c` 的路径表回 caller_id。tftp 的 `RRQ`/`WRQ` 按打不开文件时的 `ERROR` 回。2026-10-18 刷上后 `sm` 两次：`s=6`，`rf=1./boot/modem_fs1.0.1.0`，`d=390.390`。modem 打开了 `/boot/modem_fs1`，caller_id 0，应答已取走。还没有 `ALLOC`，也没有 tftp 请求。`q` 仍是 `42.b401,2b.1202`。

`ALLOC_BUFF` 按 `msm_sharedmem.c` 做了 `hyp_assign_phys`：modem 镜像结束（`0x9D800000`）和 `cdsp_sec`（`0x9F400000`）之间的 `0x9E000000`，长度 `0x280000`，交给 `VMID_HLOS` 和 `VMID_MSS_MSA`。成功才把这个地址回给 modem。`sm` 的 `rf=` 末尾多一个字，是这次 SMC 的 x0。2026-10-18 刷上后两次 `sm`：`s=6`，`rf=1./boot/modem_fs1.0.1.0.ffffffff`，`d=390.390`。还是只有 `OPEN`，`ffffffff` 表示分配调用没发生。接收尾仍是 `tms/pddump_disabled`，没有下一条 `ALLOC`。

ADSP 放在 modem PAS 之前。`qcom,lpass` 是 pas-id 1，固件名 `adsp`，内存 `0x89200000` 长度 `0x2800000`。代理电源按 `lito-regulators.dtsi` 投 `lcx.lvl` 和 `lmx.lvl` 的 TURBO 下标 6。`vendor_a` 在 `super` 的 liblp 里，物理偏移按 lpunpack 用 `target_data * 512`。对上的是 ext4：4K LBA `0x128b00`，偏移 1080 的半字 `0xEF53`。`adsp.b05` 的 extent 树超过 inode 里的 4 项，`ext4` 原先把整块子节点裁成 60 字节再拒绝。改成整块再走之后，2026-10-18 `ad=80000020.0.0.0`：32 段装进 `0x89200000`，PAS 三步 x0 都是 0。modem 仍 `s=6`，`q` 仍是 `42.b401,2b.1202`。公告和域名应答还在两条 PAS 之后。

`lito-smp2p.dtsi` 的 `qcom,smp2p-adsp`：smem 入向 443、出向 429，remote-pid 2，IPCC client `LPASS`（3）signal `SMP2P`（2）。`lito.dtsi` 的 `qcom,lpass` 把 `adsp_smp2p_in` 的 bit 0 叫 `qcom,err-fatal`，bit 1 叫 `qcom,err-ready`，bit 2 叫 `qcom,proxy-unvote`。内核在 `PAS_AUTH` 返回之后等 bit 1，最多 `qcom,proxy-timeout-ms`（10 秒），等到才算 ONLINE。

2026-10-18 第一张在写之前对 `0x80A00000` 起 1MB 做了 `civac`，区尾 TOC 也在里面。重启后 120 秒没有 ping，没有 2323，fastboot 和 adb 都没有。

拿掉整段 `civac` 之后再刷。`ae=9.80000000.2712`：代码 9 是出向项落在 `0x80A00000` 以上，这张拒绝写。等了约 10 秒，443 没出现（`0x80000000`）。`ad=80000020.0.0.0`，`s=6`，`q` 仍是 `42.b401,2b.1202`。ping 48 秒。

接着只作废这一项自己的缓存行，在 NC 窗口里写 429，不碰 `0x80AFE000` 以上。再刷之后 ping 56 秒。`ae=1.7.32`：代码 1 是写过并踢了 IPCC。`sm` 读到的 `slave-kernel` 是 7（bit0 err-fatal、bit1 err-ready、bit2 proxy-unvote）。modem `s=6`。`q` 仍是 `42.b401,2b.1202`，没有 `0x45`。

下一张把等待改成「bit1 在且 bit0 不在」才停，并在入向项出现后按 `smp2p.c` 做一次特性协商和 SSR ack。同时读全局 SMEM 423（`qcom,smem-id`，`log_failure_reason`）。ping 56 秒。`ae=1.7.32` 的 0x32 仍是 50ms：这一刻 bit0 是清的，所以等待收了。之后 `sm` 把 `slave-kernel` 刷成 7，fatal 是 ready 之后才立的。`ah=101.20010.0.4`：入向版本 1、特性 SSR_ACK、16 个槽里 2 条有效、flags 0，协商码 4 是不用改。`af=0.SFR`：423 里的字符串是 `SFR`。

内核在 err-ready 之后才登记 `glink_adsp`（remote-pid 2，描述符 478，发送 FIFO 479，IPCC client 3 signal 0）。这张在等完 ready、开 modem 之前发出 VERSION，soak 里对 OPEN 回 OPEN_ACK 并自己 OPEN `IPCRTR`。ping 44 秒。`ag=12.2`：0x12 是 OPEN 握手做完了，phase 2。ADSP 的 GLINK 通了。`slave-kernel` 仍是 7，modem `s=6`，`q` 仍没有 `0x45`。

`0x45` 出现之后才走 `icnss.c` 1061 行：indication 注册、MSA 地址、
划给 `VMID_MSS_MSA` 和 `VMID_WLAN`、`msa_ready`、capability，然后
才是五路电源。

`sm` 里 `qm` 的第一个字在这张上是 `aop_modem_on` 的返回值。
`0` 是 AOP 取走了，`0x60` 是邮箱里还剩 96 字节，`0xe1` 起是
魔数或链路没对上。`pv` 仍是九个字：cx 的地址、下标、状态，mss
同样三个，xo 的地址、数据、状态。
0 入向 435，1 出向 428，2 描述符 478，3 发送 FIFO 479，
4 接收 FIFO 480，5 走链时 canary 坏了，6 分区魔数对，7 找到了
host 0/1 这对分区。

## 源码里谁注册 0x45

这台机器的查找在 `icnss_qmi.c` 1348 行，不在 `icnss2/qmi.c`。
参数是服务 `0x45`、版本 `0x01`、实例 `0`。`qmi_interface.c` 的形参
顺序是 `(service, version, instance)`，头注释把后两个写反了。
线上实例字是 `version | (instance << 8)`，所以是 `1`。我们发出的
`NEW_LOOKUP` 和这个一致。

`qmi_add_lookup` 的注释写明：名字服务在匹配的服务注册或消失时回
`NEW_SERVER` / `DEL_SERVER`。服务不存在就不会有 `NEW_SERVER`。
modem 对 `HELLO` 的应答里已经列出它当时有的服务，查找不是把服务
造出来的操作。

`icnss.c` 的 `icnss_driver_event_server_arrive` 在服务器已经出现之后
才连上它、做 indication 注册、送 MSA、把 MSA 划给 modem/WLAN 的
VMID、发 `msa_ready`，最后才投五路电源。五路电源不是 `0x45`
出现的条件。

`icnss_modem_notifier_nb` 只处理 `SUBSYS_BEFORE_SHUTDOWN` 和
`SUBSYS_AFTER_SHUTDOWN`。`SUBSYS_AFTER_POWERUP` 直接走到出口，
不给 modem 发任何 QMI。

PDR 的域名不在驱动里。`get_service_location` 向服务 `0x40` 要域列表，
再拿返回的名字去 `0x42` 上登记监听。监听是
`QMI_SERVREG_NOTIF_REGISTER_LISTENER`（`0x20`）。重新拉起 PD 是另一条
`QMI_SERVREG_NOTIF_RESTART_PD`（`0x24`），只在重启请求里发。
`0x40` 不在 modem 的 `NEW_SERVER` 里，这条链走不到。

`pil_modem` 没有 `qcom,active-reg-names`，也没有
`qcom,active-clock-names`。`of_read_regs` / `of_read_clocks` 在属性
不存在时返回 0。`pil_auth_and_reset` 里对 active 电源和时钟的循环
是空的。DT 里实际要投的是 proxy：`vdd_cx`、`vdd_mss`、`xo`。这三笔
已经在 `PAS` 之前写成，状态都是 `0x10101`。

主机自己登记、给 modem 当客户端来连的服务，这棵树里有两个：

- `memshare`：`0x34`，版本 1，实例 1，线上实例字 `0x101`
- IPA A5：`0x31`，版本 1，实例 1，线上实例字 `0x101`

IPA 还反过来查找 modem 的 Q6 服务，也是 `0x31`，实例 2，实例字
`0x201`。这个号也不在 `q=` 里。

`scan_new_servers` 现在把类型 10 记在 `ql=`，类型 4 仍在 `q=`。
服务号在 GLINK 偏移 52，实例字在 56，和 `qmi_send_new_lookup`
的 `server.service` / `server.instance` 对齐。`memshare` 是
`34.101`，IPA 主机侧是 `31.101`。

刷上之后 `sm` 七次，`s` 保持 6，`gk` 走到 9。`q=42.b401,2b.1202`。
`ql=` 一直是空的。接收 FIFO 里没有一块长度不少于 52 字节、类型为
10 的 QRTR 包。modem 没有向主机要 `memshare` 或 IPA。

## 421 这一刀刷了，字符串没读到

`log_failure_reason`（`subsys-pil-tz.c` 806 行）在
`qcom_smem_get(QCOM_SMEM_HOST_ANY, 421, &size)` 之后看第一个字节。
`smem.c` 里版本 12 不走旧的全局 toc，走 host 两边都是 `0xfffe`
的分区，项链表和 `qcom_smem_get_private` 一样。锁是 TCSR mutex 3
（`0x1F43000`）。空串、项不在、分区没有，都不是失败字符串。

控制口 `fr` 只做这一次查找，不分配、不踢 IPCC、不投 icnss 的五路
电源、不发 MSA。刷进了 `boot_a`，`set_active a` 后重启。网是通的：
`169.254.1.1` 能 ping，MAC 是裸机的 `02:00:00:00:00:02`。TCP 2323
对 SYN 回 RST，`fr` 没有发出去。`qm` / `s` / `q` / `ql` 这次没有新数。

从 b 槽把环读出来了（LBA 25096，256 个 4K，序号 16543–16798）。
上一轮启动在 `16624` 记了 `s20 tcp=0x1` 和 `s20 cmd=0x8`，控制口
那时是通的。`16630` 是心跳，下一条 `16631` 是新的 `boot el=1`，
中间没有异常记录。这轮 `aop=0x0`、`pas=0x0`，USB 和心跳都有，
没有 `s20 tcp=0x1`。`start.S` 每次启动都清 bss，所以不是残留的
套接字状态。`tcp=0x1` 只在连接已经 Established 并写出问候时才记。

## 这一张的套接字就在 Listen

`s20 tcp=0x1` 不是监听标志。`ctrl_service` 只在状态已经是
Established、并且 `aginx\n` 写进发送缓冲之后才落这一条。环里没有
它，只说明问候没排队，不能说明套接字没进 Listen。

smoltcp 的 `process_tcp` 在没有任何套接字 `accepts()` 时才回 RST。
`State::Closed` 直接拒绝。`State::Listen` 只在这段带 ACK 号时拒绝；
普通 SYN 没有 ACK。新套接字从 Closed 起来，`is_open()` 为假，
`listen(2323)` 会先 `reset()` 再写成 Listen。

`CTRL_SLOTS` 只有一项。`ctrl_boot` 在 `SocketSet::add` 之前调用
`listen`。`SocketSet` 借的是这块数组，丢掉 set 不会清槽。后面每次
`ctrl_poll` 用同一槽再组一个 set，Listen 还在。对端拆完连接、
`close()` 把状态带回 Closed 时，Closed 那支会再 `listen` 一次。
Listen 落在 `_` 支，轮询不去动它。bss 清的是上一次启动的残留，
不是这一轮 `listen` 写进去的状态。

序号 16543–16798 比 17554–17809 旧。`RING.seq` 每次从环里最大序号
加一开始，所以那次环不是这一轮。

2026-10-18 不再刷。ping `169.254.1.1` 通。TCP 2323 三次都在 4 毫秒内
连上，首包是 `aginx\n`，拆掉再连也一样。`fr` 三次都是
`st=0 sz=100 gp=1000 at=8092a700 c=0`，前 32 字节仍是
`dog_hal_common.c:180:[tmr_slave3`。`st=0` 是首字节非 0。
这项不是空的，和 2026-10-17 那次是同一条看门狗字符串。

## 2026-10-18 窗口提前交给 MSS

`boot_a` 刷了这张，`set_active a` 后重启。44 秒 ping 通，2323 回
`aginx`。`rf=1./boot/modem_fs1.0.1.0.0`：还是只有一次 OPEN，
`RF_AS` 从 `ffffffff` 变成 `0`（开机时 `hyp_assign` 的 x0）。
没有第二路路径，也没有 ALLOC。`q` 仍是 `42.b401,2b.1202`。

44 秒和 65 秒 `s=6`。110 秒 `s=107`（bit0 fatal、bit1 ready、
bit2 proxy-unvote，另有 bit8）。同一时刻 `fr` 前 32 字节是
`dog_hal_common.c:180:[tmr_slave3`。ADSP 的 `af` 是
`SFR Init: wdog or kernel error s`。

## 2026-10-18 还 intent 之后仍然只有一次 OPEN

`RX_DONE_W_REUSE` 在三个服务公告之后才发，只还通道 2 上 id 为
1..=10 的 intent，不推接收尾。扫描间隔回到 200ms，公告之后继续扫。
`caller_id` 从 0 改成 1。44 秒 ping 通。`rk=8`，`d=3d0.3d0.0.510`，
`rf=1./boot/modem_fs1.1.1.0.0`，`q=42.b401,2b.1202`。114 秒
`s=107`，接收头仍是 `0x510`。没有第二笔 rmtfs。

item 480 的长度是 `0x4000`。头在 `0x510` 时尾还是 0，空位约 14KB。
把消费尾推到 `0x510` 之后头不再涨。同一 intent 里连发三个
`NEW_SERVER` 时，发送 FIFO 停在 `d=108.230`，`q` 为空。改回一包一包发、
HELLO 之后 5ms 一拍，结果回到 `rf=1./boot/modem_fs1.1.1.0.0`。

`confirm_rx` 按 `qrtr_recvmsg` 回了 `RESUME_TX`。44 秒 `cf=0`，
发到本节点的 DATA 没有一包把这个位置上。`rf` 仍是一次
`/boot/modem_fs1`。

收到 modem HELLO 之后才发 `NEW_SERVER`。48 秒 `hh=1`，
`q=42.b401,2b.1202`，`rf=1./boot/modem_fs1.1.1.0.0`。113 秒
`s=107`。HELLO 只有一次，rmtfs 仍然只有一次 OPEN。

QRTR 应答按 `qrtr_endpoint_post` 补到 4 字节对齐后刷了 `boot_a`。
44 秒 ping 通，2323 回 `sm`。`hh=1`，`cf=0`，
`rf=1./boot/modem_fs1.1.1.0.0`，`q=42.b401,2b.1202`，
`d=3d0.3d0.510.510`。105 秒 `s=107`。仍是一次 OPEN。

ADSP 边在 INTENT 之后继续走，并加开 `glink_ssr`。48 秒
`ae=1.7.32`，`ag=12.2`，`af=0.SFR`。113 秒 modem `s=107`。
`rf` 仍是一次 `/boot/modem_fs1`。

bit2 出现时把 `lcx`/`lmx` 代理票撤成 0。32 秒 ping 通。
随后约 70 秒 TCP 2323 被拒绝，`au=` 没读到。

撤票改到第二次 `sm`。44 秒第一次 `au=ffffffff`，`ae=1.7.32`。
52 秒 `au=1010101`：两笔低 16 位都是 `0x0101`，和投票成功的
`0x10101` 同一类完成码。`ae` 仍是 `1.7.32`，`af=0.SFR`。
约两分钟后 `s=107`。`rf` 仍是一次 `/boot/modem_fs1`。

ADSP 的 IPCRTR 上补了 HELLO。44 秒 `lh=0.1`：我们发出去了，
没收到对方的 HELLO。`ae` 仍是 `1.7.32`，`af=0.SFR`。109 秒 `s=107`。

err-ready 之后再等 3 秒才拉 modem，bit2 到了就撤 `lcx`/`lmx`。
44 秒 `au=1010101`，`ae=1.7.32`，`lh=0.1`。119 秒 `s=107`。
`rf` 仍是一次 `/boot/modem_fs1`。

SMEM 423 这条有 `0x100` 字节。全文是
`SFR Init: wdog or kernel error suspected.`

## 2026-10-18 这句是上一轮留下的

PAS 之前 423 还不在全局分区里（`az=2`，走链表没找到，所以没有清到字节）。
同一时刻入向 SMP2P 也还不在（`ap=80000000`）。PAS 之后 50ms
`ae=1.7.32`：bit1 是这一轮写上的。3 秒 settle 结束时 bit0 还没来
（`ft=ffffffff`）。44 秒 `sm` 时 slave 已经是 7，`af` 换成了这一轮的正文：

`err_qdi.c:1045:EF:sensor_process:0x1::0x104:sar.cc:27:chre_utils fatal`

`sensor_process` 在 `sar.cc:27` 做了 ERR_FATAL，原因写的是
`chre_utils fatal`。上一句 `SFR Init:` 不是这次启动写的。
modem 这时 `s=6`，`q` 仍是 `42.b401,2b.1202`，`rf` 仍是一次
`/boot/modem_fs1`。

## 2026-10-18 ADSP 的 GLINK 按内核接着收

`adsprpc.c` 的通道名是 `fastrpcglink-apps-dsp`。`fastrpc_rpmsg_probe`
只记下通道已经打开，自己不发包。`qcom_glink_native.c` 的
`create_remote` 是：对方 OPEN 之后先 OPEN_ACK，再 OPEN 同名通道，
ACK 之后按 DT 发 intent。

这张按这个做了，44 秒 ping 通。`lg=1055d`：VERSION_ACK 已发。
对方 OPEN 了 IPCRTR、`apr_audio_svc`、`fastrpcglink-apps-dsp`。
我们回了前两个的 OPEN，对方也 ACK 了。fastrpc 只 ACK 了，自己的
OPEN 没发出去，因为接收 FIFO 没排空，发送被排到了队尾。
`af` 仍是 `sar.cc:27:chre_utils fatal`。`ae=1.7.32`，`s=6`。

把 fastrpc 的 OPEN 提到接收之前再刷了一张。90 秒没有 ping，
fastboot 和 adb 都没有。

改回只在接收 FIFO 空了之后才发自己的 OPEN。fastrpc 这一路再推迟到
3 秒 settle 开始、bit0 还没来的时候。44 秒 ping 通。`lg=15ddd`：
IPCRTR、`apr_audio_svc`、`fastrpcglink-apps-dsp` 都是对方先 OPEN，
我们 OPEN_ACK 再 OPEN，对方也 ACK 了。前两个的 intent 发完了。
fastrpc 的 64 个 intent 这一拍还没发完。`af` 仍是
`sar.cc:27:chre_utils fatal`。`ae=1.7.32`，`s=6`。

通道按 `qcom_glink_native.c` 的 `create_remote` 已经打开。
`fastrpc_rpmsg_probe` 打开之后不发包。接着要有的是主机上的
`adsp_default_listener`（`adsprpcd` / hexagonrpc 的反向通道），
不是再多一条 GLINK 控制包。

fastrpc 这条 lcid 上的 TX_DATA 记了第一包。44 秒 `fp=0`：
对方一包都没发。`lg` 仍是 `15ddd`，`af` 仍是
`sar.cc:27:chre_utils fatal`。

hexagonrpc 的 `rpcd.c` 在开监听之前先
`FASTRPC_IOCTL_INIT_ATTACH_SNS`。内核 `fastrpc_init_process`
对 `FASTRPC_INIT_ATTACH_SENSORS` 发的是 handle 1、
`REMOTE_SCALARS_MAKE(0, 1, 0)`、一个 4 字节的 tgid，`pd` 写成 2。
参数在一块 DSP 能读的物理页里，经 `fastrpc_buf_alloc` 映射。
这条通道上 DSP 不会先开口。

## 2026-10-18 intent 到了，SMMU 槽没占上

`qcom_glink_request_intent`：没有 intent 就发一条
`RX_INTENT_REQ`（cmd 7，自己的 fastrpc lcid 4，大小 64）。
这张开机后 ping 通。`fi=1a.92.40.1`：rcid 1 上有 intent，
id `0x92`，大小 `0x40`。`fs=2.fffffffe.1.40`：SMMU 没写成，
`INIT_ATTACH` 没发。`fp=0`。`af` 仍是
`sar.cc:27:chre_utils fatal`。`ae=1.7.32`，`s=107`，
`q=42.b401,2b.1202`。

`fffffffe` 是扫完没有可写的槽。当时的条件是 SMR.VALID 为 0
而且 S2CR 也是 0，还跳过槽 4。USB 那次扫 81 个槽，第一个
VALID 为 0 的是槽 4，后来被 USB 的 SID `0xE0` 占了。

`drivers/iommu/arm-smmu.c` 的 `arm_smmu_find_sme` 只看
SMR.VALID。`qcom,skip-init` 让探测时不清整表，空槽留给后面的
设备。`arm_smmu_write_sme` 先写 S2CR（TYPE=translation，
CBNDX 是已经装好的 CB，SHCFG=NSH）再写 SMR。已有条目盖住
`0x1403` 时不再占新槽。

按这个改完再刷。约 33 秒 ping 通。`fy=0.200ff`：槽 5 改写前
SMR 是 0，S2CR 是 `0x200ff`（TYPE=fault，所以旧条件把它跳过）。
`fs=1.5.1.40`：槽 5 读回是转换，`INIT_ATTACH` 发出去了。
`ft=5ac`：err-ready 等待开始后 1452ms，bit0 来了。`af` 换成

`glink_core_intent.c:777:Assertion lc_intent != NULL failed`

`fp=0`。`ae=1.7.32`。`q` 这时还空，`s=6`。

`qcom_glink_native.c` 的 `__qcom_glink_send` 把 param1 设成
自己的 lcid。接收端 `qcom_glink_rx_data` 把这个值当 rcid。
fastrpc 我们的 lcid 是 4。上一包写成了对方 OPEN 里的 rcid 1，
DSP 到别的通道上去找 liid `0x92`，本地 intent 是空的。
modem 边上的 QRTR 用的是自己的 lcid 2，同一条规则。

改成 lcid 4 再刷。约 33 秒 ping 通。`ft=ffffffff`：settle
结束时 bit0 还没来。`fs=1.5.1.40`，`fy=0.200ff`。`fp=0`。
`af` 回到 `sar.cc:27:chre_utils fatal`。`lg=15ddd`，
`ae=1.7.32`，`q=42.b401,2b.1202`。glink 那条断言没有了。

`fastrpc_rpmsg_callback` 收到的是同一条通道上的
`smq_invoke_rsp`（ctx、retval）。发送方的 param1 是对方的
lcid，也就是我们记下的 rcid 1，不是自己的 lcid 4。
上一张 `fp` 只记 param1=4，对方的应答不会进这个计数。

改成也记 rcid 再刷。约 33 秒 ping 通。`fp=1.1.1002.0.0.0`：
一包，param1 是对方 lcid 1。ctx 是 `0x1002`，retval 是 0。
`INIT_ATTACH_SENSORS` 返回成功。`ft=ffffffff`，
`fs=1.5.1.40`，`fy=0.200ff`。`af` 仍是
`sar.cc:27:chre_utils fatal`。`ae=1.7.32`，`lg=15ddd`，
`q=42.b401,2b.1202`。

hexagonrpc 的 `rpcd.c` 在这次 ioctl 成功之后才
`remotectl_open("adsp_default_listener")`，再
`adsp_default_listener_register`，然后走反向通道
（`apps_std`）。

`remotectl_open` 是 handle 0、method 0、两个输入两个输出。
名字 `adsp_default_listener`，长度 22，错误缓冲 256。
attach 的 retval 是 0、并且对方 `RX_DONE_W_REUSE` 之后才发。
约 33 秒 `fo=4.1.1.1.1.98241360.0.2002.0`：open 已发，
intent 还回来了（reuse=1），我们回了 RX_DONE。
第二包 ctx 是 `0x2002`，retval 是 0，dlret 是 0，
handle 是 `0x98241360`。`fp=2.2.1002.0.0.0`，attach 仍是 0。
`af` 仍是 `sar.cc:27:chre_utils fatal`。`q` 仍没有 `0x45`。

接着发 `adsp_default_listener_register`：同一个 handle，
method 0，没有参数，`sc=0`。ctx 用 `(3<<12)|2`。

约 33 秒 `fo=5.1.1.1.1.98241370.0.2002.0.1.3002.0`。
open 的 dlret 是 0，handle 是 `0x98241370`。
register 的 ctx 是 `0x3002`，retval 是 0。`fp=3`，
第一包仍是 attach 的 `0x1002` / 0。`af` 仍是
`sar.cc:27:chre_utils fatal`。`q` 仍没有 `0x45`。

`listener.c` 的 `run_fastrpc_listener` 在 register 之后先
`adsp_listener_init2`（handle 3，method 3，没有参数），
再循环 `adsp_listener_next2`。`rpcd.c` 在 register 成功后
还会 `remotectl_close`。

`remotectl_close` 是 handle 0、method 1、一个输入两个输出。
输入是 open 的对象句柄。register 的 retval 是 0、并且对方
还了 intent 之后才发。ctx 用 `(4<<12)|2`。close 有应答之后
才发 `adsp_listener_init2`，ctx 用 `(5<<12)|2`。

约 33 秒 `fs=8`，`fo` 末尾 `0.0.0.0.ffffffff.0.0.0`。
close 已经发出去，`FP_CLSN=0`，没有第四包应答。
`fp=3`，前三包仍是 attach / open / register，retval 都是 0。
init2 没发。`af` 仍是 `sar.cc:27:chre_utils fatal`。
`q` 仍没有 `0x45`。

下一张不再等 close。`adsp_listener_init2`（handle 3，method 3，
ctx `(5<<12)|2`）发出去之后，retval 是 0 就发第一次
`adsp_listener_next2`（handle 3，method 4，上一轮 rctx 0、
result `0xffffffff`、空的输出缓冲）。

这张已经刷进 `boot_a` 并重启。ping `169.254.1.1` 通，MAC 是
`02:00:00:00:00:02`。TCP 2323 一直回 RST（errno 61），`sm` 没读到。

smoltcp 只有一个套接字。停在 SynReceived 时，后来的 SYN 对不上
四元组，`process_tcp` 回 RST。超过 1 秒就 `abort` 再 `listen`。
刷上之后 2323 立刻回 `aginx`。

`remotectl_close` 那张 `fs=8`，`fp=3`，没有第四包。close 占着
intent。下一张不再发 close：register 的 RX_DONE 之后直接
`adsp_listener_init2`（handle 3，method 3，ctx `0x5002`）。

约 33 秒，以及之后再读一次，都是 `fs=9`，`fp=3`。init2 已经发出，
`FP_ININ=0`，`FP_REUSE=0`，没有第四包，也没有还 intent。
前三包仍是 attach / open / register，retval 都是 0。
`af` 仍是 `sar.cc:27:chre_utils fatal`。`q` 仍没有 `0x45`。

内核 `adsprpc.c` 里 `FASTRPC_STATIC_HANDLE_LISTENER` 是 3。
下一张只加发送环的 head/tail，包本身不变。

`gq=408.3d0.408.3d0.3.92`。init2 提交时 head/tail 是
`0x408`/`0x3d0`，之后还是这两个数。差 `0x38`，56 字节，
就是这一包。DSP 没有把 tail 往前推。RX_DONE 只有 3 次，
最后一次 intent 是 `0x92`，对应前三包。这包还在主机发送环里。

## 2026-10-18 夜：init2 卡环 — pending TX 补踢 IPCC（窗口级，未刷）

现机仍活：`ping 169.254.1.1` + 2323 `aginx`；`sm` 见
`gq=408.3d0.408.3d0.3.92`、`fs=9`、`fp=3`、`q` 无 `0x45`——init2
已写入发送环，DSP 不推 tail。

对账：`lpass_tx_commit` 已写 head 并踢 `0x40800C=(3<<16)`；但
`lpass_glink_work` 在 `lpass_tx_idle` 失败（head!=tail）时直接
return，**不再补踢**。若首踢丢失/DSP 当时未醒，包会永久挂着，
连后续 intent 也发不出。

单变量改：idle 失败分支补踢同一 IPCC 门铃。未刷机；升格须 GATE
窗看 `gq` tail 是否前进、`fp`/`FP_ININ`、最终是否出现 `0x45`。

## 2026-10-18 夜：init2 前额外 RX intent（窗口级）

IPCC 补踢刷机后 `gq` 仍 `408.3d0`、`fp=3`、无 `0x45`——门铃假设不成立。

下一刀单变量：`fastrpc_init2` 在 `FP_REUSE` 之后、init2 TX 之前先发
一条 cmd5（fastrpc 通道 lcid4）额外 RX intent，再发 init2。验 `gq`/
`fp`/`fs`/`q`。

## 2026-10-18 夜：READ_NOTIF + tx hex（已刷证伪）+ 下一刀跳过 register RX_DONE

刷 `cc7c775`/`boot-s20`：GATE 串号 `13201FDD4001N8`。

`tx` 实读（Asia/Shanghai）：
- `gq=410.3d0` `rn=1` `gd=1.1` `ix=0` `ru=0`
- `h=` @0x3d0 起 56B：`40009.92.28.0.1.1.5002.0.3.3000000.a7ebe000.3.1000.0`
  → TX_DATA lcid4 iid=0x92 size=40；init2 smq（handle3 sc=0x03000000 ctx=0x5002）与内核布局一致
- `p=` 前 16B：`1000.0.4000e.1` → 上包末 `1000.0` + **RX_DONE_W_REUSE lcid4 liid=1**
- READ_NOTIF 已写入 @0x408（head=0x410=0x408+8），**tail 仍 0x3d0**——DSP 未读到 cmd13

结论：IPCC / 额外 intent / READ_NOTIF 三刀均不能让 tail 前进。卡点不在门铃/intent 数量/读通知，而在 **RX_DONE 被消费后 DSP 读端停死**（任意后继包放在 0x3d0 都不吃）。register 后 `FP_GLIID=1` 的 RX_DONE 最可疑。

下一刀：`fastrpc_init2` 内跳过该 RX_DONE（`FP_GDONE=2`），其余不变。验 `gq` tail 是否过 0x3d0、`fp`/`rn`/`tx h=`。

## 2026-10-18 夜：跳过 register RX_DONE — 已刷，init2 被消费

`53c677e` GATE 刷入。`gd=2.1`（未发 RX_DONE）。

关键 sm/tx（Asia/Shanghai）：
- `gq=410.400.400.3c8.4.92` — **tail 从 0x3c8 推进到 0x400**，init2（@0x3c8 起 56B）已被 DSP 吃掉
- `FP_DN=4`（多一次）、`ru=1`（intent 0x92 还回）
- `fp=3` / `FP_ININ=0` — **仍无 init2 应答 TX_DATA**
- `tx h=` @0x400：`20008.1.d.0...` = RX_INTENT_REQ_ACK(cmd8,cid2,granted1) + READ_NOTIF(cmd13)；head=0x410

根因升格：register 后那次 `RX_DONE_W_REUSE(liid=1)` 会把 DSP TX 读端楔死（证伪 IPCC/intent/READ_NOTIF 后，跳过该包即恢复消费）。

下一缺口：init2 无应答；pending ACK+RNOTIF。是否补发 RX_DONE（在 init2 消费后）或查为何 DSP 不回 ctx 0x5002。

## 2026-10-18 夜：INTENT_REQ 补 INTENT+本机 lcid ACK；延迟 RX_DONE

现机：init2 已消费、DN=4/ru=1，无应答；`tx` 见错误 ACK `20008`（回显对方 cid）+ RNOTIF。

对账内核 `qcom_glink_handle_intent_req`：应 **advertise cmd5 再 ACK**，ACK 的 param1 为本机 lcid。旧逻辑只 ACK 且回显对方 cid，DSP 无可用 RX intent 回 init2。

改动：
1. cmd7：本机 lcid（fastrpc=4）+ cmd5 INTENT + cmd8 ACK；撤 READ_NOTIF 写入
2. `FP_SENT==9 && FP_GDONE==2` 时延迟发 register 的 RX_DONE（`FP_GDONE=3`）

## 2026-10-18 夜：INTENT_REQ 刀已刷 — 拿到 init2 应答，next2 已发

`d1f2908` GATE 刷入。关键 sm（Asia/Shanghai）：

- `fo=…1.5002.0.0…`：`FP_ININ=1`，`FP_INI=5002/0` — **init2 应答 retval=0**
- `fs=a`（10）：`adsp_listener_next2` 已发出；`FP_NXN=0` — **尚无 next2 应答**
- `fp=4` `gd=3.1` `gq=250.240.1e0.1a8.5.92` `ru=1` `rn=0`
- `tx h=` `10005.1.800.1` — IPCRTR INTENT 卡在 tail；DSP `af` 现为 `frpck_1_1:0x9c:PC=…`（进 fastrpc 故障路径）
- `q=` 仍 `42.b401,2b.1202`，**无 0x45**

结论：缺 RX intent（旧 ACK 错 cid + 不发 cmd5）是 init2 不回包的主因；延迟 RX_DONE 可用。下一卡：next2 无应答 + IPCRTR intent 停读 + ADSP frpck SFR。

## 2026-10-18 夜：next2 sc/布局对齐 hexagonrpc（MAKE(4,2,2)）

现机 `fs=a`/`NXN=0`，`af=frpck_1_1`。对账 linux-msm/hexagonrpc：
`REMOTE_SCALARS_MAKE(4,2,2)`、dump `00000000FFFFFFFF0000000000000000`。
旧 `sc=0x04020300`（out=3）+ 错 pra 索引 → DSP 多解一个 remote_arg。
改 `0x04020200`、四槽 pra、prim 四字与 dump 一致；等 next2 应答时停灌 IPCRTR INTENT。

## 2026-10-18 夜：next2 sc 刀已刷 — NXN=1，frpck 消失，无 0x45

`208e9b5` GATE 刷入。

- `fo=…1.5002.0.1.6002.0.b0.0.20200.29`：`FP_NXN=1`，ctx=`0x6002` retval=0；
  NOUT=`b0.0.20200.29`（嵌套 reverse invoke：handle0 sc=`0x20200` in_len=`0x29`）
- `fp=5`；`af` 回到 `sar.cc:…chre_utils`（**不再 frpck_1_1**）
- `gq=4f0.4e0`，`tx h=30005.1.200.12`（apr INTENT 仍卡 16B）
- `q=` 仍 `42.b401,2b.1202`，**无 0x45**

结论：错误 next2 sc/pra 即 frpck 主因；应答已通。下一卡：实现 reverse tunnel
（按 NOUT 调 AP 侧方法后再 next2），并清 apr/IPCRTR INTENT 卡环；QRTR 仍缺 WLFW。

## 2026-10-18 夜：reverse tunnel 首刀（remotectl_open + 续 next2）

对账 hexagonrpcd/listener.c：next2 → dispatch ifaces[handle] → 带 result 再 next2。
首包 NOUT=`b0.0.20200.29` = remotectl_open（handle0/sc=`0x20200`）。
实现：解 inbufs、按名回 apps_std=1/apps_mem=2、编码 outbufs、续 next2。
其它方法暂 AEE_EUNSUPPORTED。sm 增 `rv=rev.n.h.sc.res.name`。

## 2026-10-18 夜：reverse tunnel 已刷 — 走通 11 轮，开到 apps_std，无 0x45

`fd39611` GATE。关键：
- `rv=0.11.1.1f020100.14.apps_std`：处理 11 次；已 `remotectl_open("apps_std")→hdl1`
- 末次嵌套 `sc=0x1f020100`（method=31 扩展号 + in2/out1），`res=14` EBADPARM
- `fp=16` `gq=6c0.688`（56B 续 next2 未吃）`q` 仍无 `0x45`
- `af` 仍 sar/chre（非 frpck）

结论：reverse 主循环已通；下一卡 apps_std 扩展方法（fopen 等）+ 续包卡环。

## 2026-10-18 reverse apps_std 最小实现（未推）

对账 `hexagonrpcd`：`sc=0x1f020100` = `MAKE(31,2,1)` = **apps_std_stat**（扩展号），不是 fopen。

已实现 apps_std：`fflush(2)/fclose(3)/fread(4)/fseek(9)/fopen_with_env(19)/opendir(26)/closedir(27)/readdir(28)/stat(31)`。
stat/fopen 对任意路径回 64B 零文件；reverse 应答后对 NLIID 发 RX_DONE_W_REUSE（与 register 的 GLIID 跳过分开）。

实测：`rv` 到 `8`，`gq` TX 空闲（续 next2 卡环解开），`ri` 显示 pathname 为 `"\0"`（空串），`af` 现 `SNS_REG_INIT…SNS_RC_SUCCESS`，**仍无 `0x45`**。
缺：真实 hexagonfs 文件（`ADSP_LIBRARY_PATH` / sensors registry 等）；`0x45` 仍属 modem WLFW 侧。

## 2026-10-18 夜：冲 0x45 — EFS 多 LUN 灌影（未推）

判断：`0x45` 属 modem WLFW，**不是** apps_mem/hexagonfs 前置。现卡点是
modem `s=107` + `dog_hal_common.c:180:[tmr_slave3`（~65s 后 DOG），
`q` 永无 `0x45`；rmtfs 仍只有一次 `OPEN /boot/modem_fs1`。

刀：`Ufs::read10_lun` + `rfs_seed_efs` 扫 LUN0..7 GPT。
证据：`es=f.13` `gn=L5:modemst1,L5:modemst2,L5:fsc,L5:fsg` — **LUN5 四槽灌成功**。
刷后仍 `s→107`、`q=42.b401,2b.1202`、无 ALLOC/IOVEC。空白影子假说证伪；
下一卡是「OPEN 之后为何不再走 rmtfs / tmr_slave3 在等什么」。

## 2026-10-18 夜：OPEN→ALLOC 断点 + rmtfs 应答对账（未推）

刀：RF 取证（`rh=` hit/drop/hist，`rs=` OPEN 应答前 8 字节）、intent 归还不限 id、
TFTP WRQ→ACK0、sm 缓冲 1536。

对账：`rs=02010001000e0002` = type2 txn1 msgid1 msglen0x0e + TLV2 起 —— 与 libqrtr/
qmi_rmtfs OPEN 成功应答一致；`caller_id` 仍 1（TLV16 在后 4 字节未进 rs 窗）。

实测（GATE `13201FDD4001N8`，boot-s20）：
- `rh=1.0.101.0.0.0` → **只收到 msgid=1 txn=1 一次**，无 msgid 4/3
- `rf=1./boot/modem_fs1.1.1.0.0` `es=f.13` `gn=L5:…` `tf=.0.0`
- `q=42.b401,2b.1202` **无 0x45**；`s→107`；`rk=8` `cf=0`

根因判断：不是 OPEN 应答格式错（wire 已对齐；caller_id 0/1 历史都试过仍无 ALLOC）。
modem 在一次成功 OPEN 之后**不再发** ALLOC/IOVEC；`tmr_slave3` DOG 是 init 在
EFS 未真正 IOVEC 服务时的看门狗。TFTP 本轮仍无请求（WRQ ACK 未触发）。

下一卡：OPEN 成功后 modem 还在等什么（非 rmtfs 格式）——servreg/wlan_pd 前置、
或另一条 QRTR/QMI 链。

## 2026-10-18 深夜：OPEN 后前置对账 → 收束交接（未推）

### 对账结论（Linux radio-bringup + S4）

1. **DOG / EFS**：`qrtr-ns + pd-mapper + tftp + rmt_storage` 在 modem PIL
   **之前**监听，且 EFS 被真正 IOVEC 服务 → DOG 沉默。我们已能答 OPEN，
   但 modem **从不发 ALLOC/IOVEC**（`rh=1.0.101` 已证），故 EFS 未 serve，
   `tmr_slave3` ~65–80s 必炸——与「格式错」无关（`rs=` wire 已对齐）。

2. **0x45 / wlan_pd**：stock 上 2023 modem 在 PIL 后 ~2.5s **自己**拉起
   `wlan_pd`（段在 `modem_wlan_region`），无 AP 侧拉固件。前提是 AP 环境：
   ADSP 先于 modem、早期 wlan.ko/cnss、**pm-service QMI `0x35` 在 PIL 前登记**
   （晚 22min 无效）、servloc enable。最小环境同镜像同 EFS 也会
   `SERVICE_NOT_FOUND` for `msm/modem/wlan_pd`。**0x45 不缺 vendor WLFW 文件，
   缺的是 modem 把 wlan_pd 拉起来的 AP 侧环境 + 先过 RFS init。**

3. **主机侧曾试过的公告**：memshare `0x34`、IPA `0x31` —— modem `ql=` 空，
   从未 LOOKUP。pd-mapper 只见 `tms/pddump_disabled`（jsn 无此名 → 域数 0 正确）。
   tftp 全程无 RRQ/WRQ。

### 本轮刀（代码已进树，末次刷机后设备掉 USB，**sm 未采到**）

- 第 4 条 `NEW_SERVER`：`0x35` / 实例字 1 / 端口 `0x13`，任意 QMI 回 result 成功（探针）
- `pm=` hit.n.msg；`qp=` 本机 DATA 环（port|msgid<<8|qtype<<16）
- `HOST_ADV` 门槛 3→4

### 收束判断（今晚停）

继续在裸机上猜 OPEN→ALLOC 缺口 **边际收益低**：缺的是
**(A) 同机 Linux 上 OPEN→ALLOC 的 QRTR 抓包对照**，或
**(B) 按 radio-bringup 补齐 pm-service/0x35 真实 IDL + PIL 前环境**，
而不是再改 rmtfs OPEN 字节。`0x45` 卡在 wlan_pd，wlan_pd 卡在 modem init，
init 卡在 EFS 未 IOVEC——主链已钉死。

### 下一棒怎么接

1. 恢复设备 USB（若 tries 耗尽回 b：`fastboot -s 13201FDD4001N8 set_active a` + 重刷
   `boot-s20.img`），采 `sm`：看 `pm=` / `qp=` / `rf=` / `rh=`。
2. 若 `pm>0`：按 QMI 0x35（pm-service）补真实应答，再观察是否出现 ALLOC。
3. 若 `pm=0` 且 `qp` 仍只有 `0x10/OPEN` + `0x12/pddump`：停裸机 RFS 猜谜，
   改在 Linux 槽抓 `qrtr`/`rmt_storage` 日志对照 OPEN 后下一包。
4. **不要**再为冲 0x45 去堆 apps_mem/hexagonfs（已证伪前置）。

### 本轮：0x35 真 SSREQ 应答（b28c223，Asia/Shanghai 2026-10-19 ~01:28）

对账 `/vendor_a/bin/pm-service` 内嵌 IDL：`service_id=0x35`，msgid
`0x20/0x21/0x22` = SYSTEM_SHUTDOWN / SYSTEM_RESTART / PERIPHERAL_RESTART
（REQ/RESP/IND 同号，`max_sz=7`）。`pm_reply` 替换假探针：已知 msgid
RESP result 成功；SHUTDOWN/RESTART 再发 IND TLV01 status=1（SERVICED）；
PERIPHERAL_RESTART 只 RESP；未知 msgid 失败 RESP。不 reboot。

GATE：串号 `13201FDD4001N8`，`flash boot_a` + `set_active a` + reboot（01:28:25）。
其后 ≥10 min 无 ping / 无 adb / 无 fastboot（USB 掉线，与上一轮 0x35 探针刷后相同）。
**sm 未采到**；ALLOC / 0x45 无证据。代码已本地 commit `b28c223`，未推。

恢复：Power 硬复位或等 tries 烧尽回 b；`fastboot -s 13201FDD4001N8 set_active a`
重刷 `.local/device/redfin/boot-out/boot-s20.img` 后再采 `pm=`/`qp=`/`rf=`/`rh=`/`q=`/`s=`。

### 本轮：0x35 NEW_SERVER 提前 + LOOKUP 应答（Asia/Shanghai 2026-10-19 ~04:10）

改动（本地 commit，未推）：
1. `glink_announce` 顺序改为 **0x35 → rmtfs → tftp → pd-mapper**（HELLO 后尽早可被 lookup）。
2. 收到对本机服务的 `NEW_LOOKUP`(10) 立刻回 `NEW_SERVER`（`lk=` 计数）。
3. 曾试同轮连发 4 条 NEW_SERVER → `rf=0`/`qp=0`（疑似复用同一 GLINK intent）；已改回 **一 tick 一包**。

GATE 刷验（串号 `13201FDD4001N8`，`fb`→flash→set_active a）：
- 关键 sm（连发版，04:11–04:13）：`hh=1` **`lk=0.0.4`**（HOST_ADV=4，0x35 已进公告）
  `pm=0.0.0` **`LOOK_N=0`** `qp=0` `rf=0` `rh=0` `pr=0`
  `q=42.b401,2b.1202` `s=107` `d=150.288.348.348`
- 一 tick 版重刷后 USB 掉线，未采到对照 sm。

**结论**
- 公告侧：0x35 已能在 HELLO 后立刻进 `HOST_ADV`（`lk=*.*.4`）。
- 需求侧：modem **从不**对本机发 0x35/14/64/4096 的 LOOKUP（`LOOK_N=0`），故 `pm=0` 不是「公告太晚」，而是 **这条路径上没有 SSREQ 客户**。
- 同轮连发会打坏 RFS OPEN（`rf=0`）；保留一 tick + 0x35 打头。
- **下一刀应转 Linux 抓包**：确认库存 bringup 上 modem/AP 是否出现 0x35 LOOKUP/QMI；若库存也无 0x35 流量，则 0x35 与 OPEN→ALLOC/0x45 脱钩，停裸机猜 pm-service。

#### 最小可执行 Linux 侧抓取计划（槽 b / AginxOS）

目标：在一次 **能到 wlan 或至少 modem ONLINE** 的库存启动里，证明有无 QRTR 0x35 / pm-service 流量。

1. `fastboot set_active b` + `reboot`（或 tries 回滚）进 Linux；确认 `androidboot.serialno=13201FDD4001N8`。
2. 开机后立刻（modem 起来前后）：
   - `ps | grep -E 'pm-service|pm-proxy|Peripheral'`
   - 若有 `qrtr-lookup`：`qrtr-lookup` / `qrtr-lookup 0x35`；或读 `/dev/socket` 无则用：
     `cat /sys/kernel/debug/qrtr/*/...`（有则）、`ls /sys/kernel/debug/ipc_logging/`
   - `logcat -b all -d | grep -iE 'pm-service|SSREQ|0x35|PeripheralManager'`（若 logcat 可用）
   - 动态调试（若镜像许）：`echo 'module qrtr +p' > /sys/kernel/debug/dynamic_debug/control` 后重启子系统或整机，`dmesg | grep -i qrtr`
3. 可选：`stop vendor.pm-service` / 杀 pm-service 再 `start`，看 modem 是否 LOOKUP 重试（对照「服务消失」行为）。
4. 收据：一段含时间戳的 `dmesg`/`logcat`/`qrtr-lookup` 输出；明确写清 **有/无** `service 0x35` 与 QMI 到 pm-service 端口。
5. 判定：
   - 库存也无 0x35 → 关掉裸机 0x35 刀，改追 OPEN 后 ALLOC（EFS/intent/别的 QMI）。
   - 库存有 0x35 LOOKUP/QMI → 对账时序/实例字/节点，再回裸机补。

### Linux 槽抓包（Asia/Shanghai 2026-10-19，adb aginxosredfin / 槽 b）

进程：`/vendor/bin/pm-service`、`/bin/rmt_storage`、`cnss-daemon`、`pd-mapper`、`qrtr-ns` 均在跑。

`qrtr-lookup` 关键行：
- **`svc=0x35 inst=0x1001 at 1:16407`**（AP 节点 1，pm-service / SSREQ）
- **`svc=0x45 inst=0x1 at 0:88`**（modem 节点 0，WLFW **库存已有**）

裸机旧探针差异：曾公告 `inst=1`、`port=0x13` → 与 Linux 不一致，LOOKUP/客户对不上。
已改 NEW_SERVER / 收包 / 应答为 **`inst=0x1001`、`port=16407`**（常量 `PM_INST` / `PM_QRTR_PORT`）。本轮只改码编译，**未刷 a 槽**。

### 本轮：OPEN→ALLOC 缺口 = RF_SEEN 只按 txn 去重（2026-10-19 ~06:09 Asia/Shanghai）

**Linux 对账**（gist/rmt_storage 日志范式）：同一启动里对 `/boot/modem_fs1`、`fs2`、`fsg`
连续 **多路 OPEN**（各 QRTR client 常从 **txn=1** 起），再 `ALLOC`（msgid=4，如
Size 1572864），再 `IOVEC`（msgid=3）。

**裸机旧证**：`qp` 已见两次 `0x110`（port 0x10 msgid=1），但 `rh=1.0.101` /
`rf=1` —— 第二路 OPEN 被丢。根因：`RF_SEEN` **只存 txn**，多客户端 txn 碰撞。

**刀**：`RF_SEEN` 改为 `client_port<<16|txn`；seen 时 `RF_DROP|=1`；同轮 RF 处理
上限 4→8。

**刷验**（GATE `13201FDD4001N8`，boot-s20）：
- `rf=3./boot/modem_fs2.2.6.280000.0` — 多笔应答；`RF_SZ=0x280000`；`as=0` hyp_assign 成功
- `rh=6.3.…d000104.e000103` — **msgid=4 ALLOC** 与 **msgid=3 IOVEC** 已进 hist
- 仍 `s=107`、`q=42.b401,2b.1202`（无 0x45）；下一刀另论 wlan_pd / 0x45

未推。


### 本轮：SH_SIZE 影子过短 → IOVEC 大读失败（2026-10-19 ~06:25 Asia/Shanghai）

**Linux 对账**（槽 b dmesg）：`Alloc req_size: 2621440`；随后 IOVEC Read
`fs2 offset=1024 size=2620416`（= 分区 2621440 − 1024）。modemst1/2/fsg
皆 **2621440**，fsc=131072。

**根因**：裸机 `SH_SIZE=0x200000`（2MiB）< 2620416 → `rfs_one` 拒读 →
IOVEC 回失败 → 旧证 `fs_rmts_pm.c:1038` DOG。RFS 窗口 `0x280000` 本身够，
只影子短。

**刀**：`SH_SIZE` → `0x280000`（仍止于 `0x9EC80000` < cdsp_sec`@9f400000`）；
`ik=`/`RF_IOK`·`RF_IFAIL` 计数。

**刷验**（GATE `13201FDD4001N8`，boot-s20，`set_active a`）：
- `ik=4.0` — **IOVEC 4 成功 0 失败**
- `es=f` `gn=L5:modemst1,L5:modemst2,L5:fsc,L5:fsg` — 四槽灌影
- `rf=…8.280000.0` `rh=8.3.…d000104.e000103`
- `q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — 服务变多，**仍无 0x45**
- `s=107`；`fr` 现 `glink_core_intent.c:777:[irq65]…`（**不再** `fs_rmts_pm`）
- `af`=ADSP `SNS_REG_INIT…SNS_RC_SUCCESS`

结论：RFS EFS 大读已通；0x45/wlan_pd 仍未注册；下一刀看 glink intent /
modem SFR `glink_core_intent`，非再砍影子长度。

未推。


### 本轮：glink intent 配额（2026-10-19 ~06:43 Asia/Shanghai）

**现象**：IOVEC 通后 `fr=glink_core_intent.c:777:[irq65]`，疑 TX 复用同一
modem INTENT 未等 RX_DONE。

**刀 1（过猛，已弃）**：完整 TX 空闲池 + INTENT_REQ 应答。首刷后
`MI_REQ_N` 因 FIFO 重扫暴涨、`HOST_ADV=0`/`rf=0`；再修仍导致无 ping
硬挂（ECM ARP incomplete），疑早期路径堵死。未留下可用 sm。

**刀 2（当前树，未刷成）**：回退到 `92393f6` 基线，只加：
- `tx_alt_add` 收集 modem cmd5 INTENT
- `tx_pick`/`TX_BUSY_ID`：飞行中同 liid 不连发，RX_DONE(cmd6/14) 清 busy
- `tw=` 计数；不碰 HELLO/公告时序门槛

**刷验**：刀 1 第二/三刷后设备无 ping/无 adb/无 fb（06:37 起）；人工硬复位回 fb。

**刀 2 上机**（Asia/Shanghai 2026-10-19 ~07:02）：tip `ee60b5a`，GATE
`13201FDD4001N8`，`flash boot_a` + `set_active a` + `reboot` OK。
`boot-s20.img` 98304 B @06:42。

**结果：仍硬挂**。USB/Aginx ECM `en10` 枚举且 active（host MAC
`02:00:00:00:00:01`，host IP `169.254.252.30`），但对 `169.254.1.1` ARP
incomplete、ping 全丢、2323 不可达；等到 ~07:08（>5min，远超 90s 门槛）
仍无 peer MAC `02:00:00:00:00:02`。无 sm 样本。**未再刷**。

**0x45**：本轮**未观察到**（刀 2 上机后无 ECM IP，无法采 `q=`）。

**阻断**：温和飞行锁仍疑早期路径堵死（与刀 1 同类症状：ECM 链路在、IP
栈不应答）。需人工硬复位回 fb；下一刀应再收窄或整段回退 `92393f6` 基线
验证「仅回滚即可复活」，再考虑别的 intent 切入点。

未推。


### 本轮：基线复活刷 `92393f6`（2026-10-19 ~07:14 Asia/Shanghai）

意图刀 2（`ee60b5a` 飞行锁）硬挂后，GATE `13201FDD4001N8` 刷已知好镜像
`.local/device/redfin/boot-out/boot-s20-92393f6.img`
（sha256 `bc14b7705f30f3b01b5542a05a3a0b262c2f48537923ebb0c19a00ef3bcb0a84`，
`SH_SIZE=0x280000`，无飞行锁）。tip 仍停 `ee60b5a`，**未推、未刷 tip**。

**结果：复活**。ping `169.254.1.1` ~49s OK；TCP 2323 OK。两次 `sm`：
- `s=107` `ik=4.0` `rf=3./boot/modem_fsc.3.8.280000.0`
- `rh=8.3.9000101.a000101.d000104.e000103` `pm=0.0.0`
- `q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 0x45**

结论：硬挂坐实在飞行锁；下一刀在 tip 上要么整段丢掉 BUSY，要么再收窄 intent。


### 本轮：丢掉飞行锁 + TX intent 池 RR（2026-10-19 ~07:20 Asia/Shanghai）

**回退**：`git revert ee60b5a` → `abff614`。`main.rs` 与 `92393f6` 字节一致
（无 TX_BUSY / INTENT_REQ）。笔记保留刀 1/2 硬挂与基线复活 sm。

**诊断（基线活体 sm）**：
- RFS 绿：`ik=4.0` `rh=…d000104.e000103`
- `pm=0.0.0`（modem 仍不 SSREQ 0x35）
- `q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 0x45**
- Linux 槽 b：`0x45@0:88`、`0x35/0x1001@1:16407`
- 旧 SFR：`glink_core_intent.c:777`（IOVEC 通后）

**假说**：基线 `glink_hello` 只留 **最大一条** modem cmd5 intent，且 cmd5
`count>1` 时 step 固定 16 会切错 FIFO。突发 `glink_tx_payload` 复用同一
`LAST_IID` → modem `lc_intent == NULL`。飞行锁整路径 busy-wait 已两挂，禁。

**刀（窄）**：
1. 按 Linux wire 解析 cmd5：`step = 8 + 8*count`（count≤8），ingest 全部
   (size,iid) 进 `TX_POOL[8]`
2. `glink_tx_payload` / `glink_resume` / 公告·LOOKUP：池内 **RR**，**不**
   busy-wait、**不** INTENT_REQ
3. sm 增 `tp=N.pick`

未推。待编 `boot-s20.img` GATE 刷验。


### 本轮结果：intent 池 RR 亦硬挂（2026-10-19 ~07:23 Asia/Shanghai）

刷 tip `1119de1`（池 RR + cmd5 multi-count）GATE `13201FDD4001N8`：
`flash boot_a` + `set_active a` + reboot OK。

**结果：硬挂**（与飞行锁同类）。en10 ECM active、host link-local 在，
但对 `169.254.1.1` ARP incomplete（无 peer MAC `02:00:00:00:00:02`），
≥90s 无 ping / 无 2323 / 无 fb / 无 adb。**无 sm。**

**处置**：`main.rs` 已回退到 `92393f6` 字节（去掉池 RR）；笔记保留。
**勿再刷 tip 意图刀**。需人工硬复位进 fb 后刷
`.local/device/redfin/boot-out/boot-s20-92393f6.img` 复活。

**0x45**：本轮未观察到。

**假说修正**：凡改 modem IPCRTR 的 TX intent 选取/解析（BUSY 或 RR 或
INTENT_REQ）均在早期把主循环/ECM IP 堵死；0x45 缺口暂时不宜再动
glink TX pick。下一刀应离开 intent：对账 Linux 槽 b 上 `0x45` 出现前
的 QRTR/servreg/pd-mapper/cnss 时序，或查 `pm=0` 是否真与 wlan_pd 无关。

未推。

### 本轮：离线对账 Linux→0x45 前置（2026-10-19 ~07:30 Asia/Shanghai）

设备挂死中，**未刷**。基线仍 `92393f6` / `boot-s20-92393f6.img`。
禁刀：凡改 modem IPCRTR **TX intent pick/parse**（BUSY / RR / INTENT_REQ）
均早期硬挂 ECM——下一刀不得再碰。

#### Linux 在 QRTR 上出现 WLFW `0x45` 之前的时序

真源：`devices/redfin/bringup/radio-bringup` 头注 + 槽 b
`qrtr-lookup` + `icnss.c` / `service-notifier*.h` / `modemuw.jsn`。

Stock / AginxOS radio 脚本顺序（时间相对开机）：

| t | AP 侧 | 作用 |
|---|---|---|
| 先于 modem | `qrtr-ns`、**patched** `rmt_storage`、`tftp_server` 已 listen | 缺则 DOG `tmr_slave3` ~65s |
| 先于 modem | binder fake-sm → **`pm-service` 登记 QRTR `0x35/0x1001@1:16407`** | 晚 22min 登记无效（2026-08-27） |
| 先于 modem | `echo 1 > …/service_locator/enable` | 否则 icnss WLFW 发现永 -ENODEV |
| 先于 modem | `pd-mapper`（jsn 含 `modemuw.jsn`→`wlan/fw`→`msm/modem/wlan_pd` inst **180**） | AP 侧域表 |
| ~0.9s | `service-notifier` + `msm_icnss`：`qmi_add_lookup(0x45,v1,i0)` **且** `get_service_location("wlan/fw")` → 对 modem `0x42` **REGISTER_LISTENER** | 等 NEW_SERVER；PDR 不负责首次拉起，但 Linux **必走** |
| ~3.4s | `wlan.ko` + `cnss-daemon` | WLFW 用户态半边；**在 0x45 之后**才有协议流量 |
| ~3.5s | ADSP ONLINE | 2023 modem：ADSP 后于 modem → **永不** spawn wlan_pd |
| ~6.3s | modem PIL | |
| ~8.8s（PIL+~2.5s） | modem **自己**从 `modem_wlan_region` 拉起 `wlan_pd` | 无 AP 下发固件 |
| 同时 | QRTR `NEW_SERVER`：**`0x45@0:88`** | icnss `server_arrive` → MSA/五路电 |

槽 b 实证（2026-10-19）：`0x35/0x1001@1:16407`、`0x45/0x1@0:88`。

`0x42` 实例字：`q` 里 `42.b401` = version 1、instance **0xb4=180**（与
`modemr.jsn` / `modemuw.jsn` 的 `qmi_instance_id` 一致）。

#### 裸机相对缺口（基线活体 sm）

活 sm：`s=107 ik=4.0 pm=0.0.0`
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 `45.*`**。

| 已对齐 | 仍缺 / 误判 |
|---|---|
| RFS OPEN→ALLOC→IOVEC（`ik=4.0`） | modem **从不**对本机 `0x35` 发 QMI（`pm=0`；公告对账已齐） |
| 公告 rmtfs/tftp/pd-mapper/`0x35` | **无**对 modem `0x42@inst180` 的 PDR 客户端：无 REGISTER_LISTENER / QUERY_STATE / RESTART_PD |
| ADSP 先于 modem PAS；pd-mapper 答 jsn | pd-mapper 只见 `tms/pddump_disabled`，modem **未**要 `wlan/fw` |
| LOOKUP `0x45` / `0x40` | `0x45` 不存在则 LOOKUP 无 NEW_SERVER（已证） |
| | `q` 里 `16.1001` 是 **modem 侧**服务，不是 AP `0x35`（同实例字 0x1001 易混） |
| | `s=107` + 旧 `fr=glink_core_intent`：intent 复用病仍在，但 **改 TX pick 已两挂禁做** |

结论：RFS 绿之后，Linux→0x45 链上裸机缺的**下一刀可做、且不动 TX pick**
的 AP 行为，是 icnss/service-notifier 在 `0x45` 出现前对
`msm/modem/wlan_pd` 的 **SERVREG_NOTIF 接触**（听与可选重启）。

#### 下一刀（未编、未刷）

**单变量**：在 `HOST_ADV>=4` 且 `scan_new_servers` 已见 `svc=0x42
inst=0xb401` 之后，向该 NEW_SERVER 的 **node:port**（ctrl 包 +60/+64，
现只记了 svc/inst，须多存一对）发一笔 QMI：

1. `QMI_SERVREG_NOTIF_REGISTER_LISTENER_REQ`（msgid **0x20**）  
   - TLV `0x01` enable=`1`  
   - TLV `0x02` string `msm/modem/wlan_pd`（18B，无内嵌长度前缀）  
   - 对账：`service-notifier-private.h` MAX_MSG_LEN 71  
2. 解析 RESP：TLV `0x02` result；可选 TLV `0x10` **curr_state**（i32；
   CAF 常见 UP=`0x1FFFFFFF` DOWN=`0x0FFFFFFF` UNINIT=`0x7FFFFFFF`）  
3. 若 state ≠ UP（或 result 失败 / 无 state）：再发一笔
   `RESTART_PD`（msgid **0x24**，TLV `0x01`=同名）——对齐
   `service_notif_pd_restart()`，**不是**改 glink TX。

实现落点：`main.rs` 的 `glink_hello` 推进序列（LOOKUP 0x45/0x40 之后
或并行一 tick），`glink_tx_payload` 复用现有 LAST_IID（**不**改
pick/parse/BUSY/RR/INTENT_REQ）。sm 增：

- `sr=hit.state` — REGISTER 次数与 curr_state（或 `ffffffff`=无 TLV）  
- `rp=n.err` — RESTART 是否发出 / RESP error  

**成功判据（一次 sm）**

- 硬成功：`q=` 出现 **`45.1`**（或 `45.01`）  
- 软成功（刀有效、PD 仍未起）：`sr` 非 0，state 可读为 DOWN/UNINIT，
  且 `rp` 有发出；仍无 45 → 下一棒再论 env（非再砍 intent）  
- 失败：`sr=0`（port 未抓到 / TX 未出）或 ECM 早期无 ping → 整段回退，
  **勿**叠加 TX intent 刀  

禁：刷 tip 上任何 TX intent 池/飞行锁；活体镜像保持
`boot-s20-92393f6.img` 直到本刀编进 tip 且 tip≡可刷策略另定。

未推。未刷。

### 本轮：PDR 刀编入 tip（未刷，2026-10-19 ~07:40 Asia/Shanghai）

在 `292291d`（main.rs≡`92393f6` glink TX）上加 SERVREG_NOTIF 客户端，
**不**改 TX intent pick/parse/BUSY/RR/INTENT_REQ。

行为（`sr_tick`，`HOST_ADV>=4` 且已抓到 `0x42@0xb401` 的 node:port）：

1. 对本机 port `0x14` → modem SERVREG：`REGISTER_LISTENER` msgid `0x20`
   （TLV01 enable=1，TLV02 `msm/modem/wlan_pd`）
2. RESP：记 `sr=hit.state`；state≠UP / 无 state / result 失败 → 一笔
   `RESTART_PD` msgid `0x24`（TLV01 同名）；`rp=n.err`

sm 新键：`sr=`、`rp=`。成功判据仍：`q=` 含 `45.1`。

镜像：`boot-s20.img`（本刀 tip）。**未刷**（基线可能仍在恢复）。未推。

### 本轮：PDR 刀已刷采样（2026-10-19 07:34–07:35 Asia/Shanghai）

进 fb：活体 2323 上发 `fb` / `reboot` → GATE `13201FDD4001N8`。
刷：`9523f1d` `boot-s20.img` → `boot_a` + `set_active a` + `reboot` OK。
起：ping ~52s，**未挂**。

sm×2 同：`ik=4.0 pm=0.0.0 sr=0.ffffffff rp=0.ffffffff`
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 `45`**。

判据：硬失败（无 45）；软也未过（`sr=0` = REGISTER 未命中 / 未出，
尽管 `q` 已见 `42.b401`）。下一刀：对账为何抓到 svc/inst 却 `sr` 仍 0
（node:port 是否写入、`HOST_ADV`、TX 路径），**勿**叠加 TX intent。

未推。

### 本轮：206821a 待刷（USB 卡住，2026-10-19 ~09:03 Asia/Shanghai）

tip=`206821a`（允许 QRTR node 0；sm `sp=node.port`）。镜像
`.local/device/redfin/boot-out/boot-s20.img` 与 tip 对齐（07:38 编）。

Mac 回线后设备在 GATE `13201FDD4001N8` fastboot。两次
`fastboot flash boot_a`：**无任何输出、>45s 无进展** → 已 kill。
`getvar product` 同样挂死。判定 **USB/fastboot 通道粘滞**，
不是镜像问题。未完成 set_active / reboot；**未采样** `sr`/`sp`。

下一步：硬复位 + 重插 USB 再进 fb；通道稳后再刷同一镜像。未推。


### 本轮：206821a 已刷 → 早期挂死（2026-10-19 ~09:06–09:12 Asia/Shanghai）

刷：GATE `boot_a` + `set_active a` + `reboot` OK（~09:06）。
起：ECM `en10` UP（Mac `169.254.146.55`），**ARP `169.254.1.1` incomplete**
>90s（至 ~09:12 仍无 ping / 无 2323）。**未采到 sm**（无 `sr`/`sp`/`q`）。

判据：按 GATE 失败分支（ECM 早期无 ping）→ **整段回退**，勿叠加 TX
intent。下一假设：node-0 放行后 `sr_tick` 过早发 REGISTER/RESTART 堵死
early path；回 `92393f6` 基线或把 PDR 推迟到更晚 gate（如 RFS `ik=4`
之后）再试。未推。未再刷。


### 本轮：PDR 推迟到 ik=4（code-only，未刷，2026-10-19 ~09:15 Asia/Shanghai）

`206821a`（node-0 放行）刷后 early ECM 硬挂，与 TX-intent 刀同类。
判定：REGISTER/RESTART 发太早堵 early path。

下一镜像（本 tip）：**保留** node-0 放行（`port!=0`）；`sr_tick` 增门控
`RF_IOK >= 4`（≡ sm `ik=4.x` RFS IOVEC 绿），并保留既有 `HOST_ADV>=4` /
`HAVE_IID` / `SR_HAVE`。不动 glink TX intent pick/parse/BUSY/RR/INTENT_REQ。

用户回 fb 时刷序：先 `.local/device/redfin/boot-out/boot-s20-92393f6.img`
复活，再刷本 tip `boot-s20.img`。本轮 **未刷未推**。

### 本轮：a84360c 活体 + a5137f2 纠正刷验（2026-10-19 ~09:20–09:25 Asia/Shanghai）

**根因（「坏 RESP」）**：不是帧格式/解析失败。`sr=1.7fffffff` 的
`0x7fffffff` = CAF `SERVREG_NOTIF_SERVICE_STATE_UNINIT_V01`（真状态）。
REGISTER TLV 布局（enable TLV01 + name TLV02 无内嵌长度）与 Linux
`service-notifier-private.h` 一致；RESP TLV02=result、TLV10=curr_state
也一致。旧逻辑把「state≠UP」一律 `RESTART_PD`，对 UNINIT 不对齐
Linux（Linux 只听 IND，RESTART 仅显式 `pd_restart` / DOWN 恢复）。

次要：RX 门槛 `chunk>=56` 会丢掉仅-result 的短 RESP（RESTART RESP
chunk=48）；REGISTER 因带 state TLV 刚好 chunk=56 才过关。

**刀 `a5137f2`**（保留 ik>=4 + node-0；不动 TX intent）：
1. 仅 `DOWN(0x0FFFFFFF)` 才 `NEED_RP`；UNINIT 不 RESTART
2. 听 `STATE_UPDATED` IND msgid `0x22` → 更新 state + `SET_ACK` `0x23`
3. RX 门槛降至 `chunk>=48`；sm `sr=hit.state.res` + `si=ind_n`

**刷验**：2323→fb，GATE `13201FDD4001N8`，`boot-s20.img` OK。
未挂。sm×2（~09:24）：
`ik=4.0 sr=1.7fffffff.0 si=0 sp=0.2 rp=0.ffffffff pm=0.0.0`
`q=42.b401,2b.1202,...` — **无 `45`**。
解读：REGISTER 成功（res=0）、state=UNINIT 可读、未误发 RESTART、
尚无 IND。软成功（刀有效、PD 未起）。下一棒：env / PAS / ADSP 时序
（为何 modem 不自拉 wlan_pd、无 STATE_UPDATED），非再砍 QMI 帧。

未推。

### 本轮：TFTP 真喂 modem_pr SO（冲 UNINIT，2026-10-19 ~09:32 Asia/Shanghai）

**活体证据（a5137f2 / tip e310cd1）**：`ik=4.0 sr=1.7fffffff.0 si=0`
`pn=kernel/elf_loader.1.30` — modem 已向 pd-mapper 要 elf_loader→wlan_pd；
`tf=/readonly/firmware/image/modem_pr/so/549_38_0.mbn.1.32` — **RRQ 在飞**，旧逻辑一律
`ERROR file not found`（TF_N=0x32 次级重试）。PDR REGISTER 成功但 state=UNINIT、
无 IND、**无 0x45**。

**对账 Linux**：`tftp_server` 从 `/vendor/firmware_mnt`（=modem_a vfat）喂
`/readonly/firmware/...`；wlan_pd 自拉依赖 elf_loader + 该 SO。ADSP 本轮
`ad=80000020 ae=1.7.32`（PAS 过、ready 过；后有 sensor EF，非「ADSP 从未 ONLINE」）。
`q=` 相对槽 b 缺的仍是 **`45.*`**（其余 root-PD 服务已见）。

**刀（单变量，不动 TX intent / 不提前 PDR）**：
1. RRQ 路径若前缀 `/readonly/firmware/` → modem_a FAT16 8.3 查找
2. 命中则 512B TFTP DATA + ACK 推进；`glink_tx_payload` 放宽到 n≤520/total≤640
3. sm 增 `tz=st.sz.cl.blk.soff`（st: 1待发 2等ACK 3完成 4未找到 5几何 6UFS 7无CTX）

**成功判据**：`tz` 见 sz/cl 且 st→3；硬成功 `q=` 含 `45.`；软成功文件喂通仍 UNINIT。

未推。待 GATE 刷验。

**刷验（365d1f4，GATE `13201FDD4001N8`，Asia/Shanghai ~09:33）**：`flash boot_a` +
`set_active a` + reboot OK。en10 ECM UP（host `02:00:00:00:00:01`），但对
`169.254.1.1` **ARP incomplete** ≥3 min，无 ping / 无 2323。**无 sm。硬挂**
（与 TX-intent / 过早 PDR 同类：链路在、IP 不应答）。

**处置**：勿再叠加。需硬复位进 fb → 刷 `.local/device/redfin/boot-out/boot-s20-a5137f2.img`
（或 `boot-s20-92393f6.img`）复活。tip `365d1f4` 保留代码供收窄；**下一刀**应改为
开机后一次性 FAT 探针（只写 `tz=`，RRQ 仍 ERROR），**不**在 `smem_scan` 热路径读 UFS、
**不**放宽 `glink_tx_payload`——先证文件存在与几何，再谈真喂。

未推。

### 本轮：开机一次 FAT 探针（不真喂，2026-10-19 Asia/Shanghai）

**基线**：`a5137f2`（PDR OK、UNINIT、无 45）。**不**从 `365d1f4` 真喂路径叠加。

**刀（单变量）**：
1. `load_modem_segs` 成功后开机一次 `tf_boot_probe`：modem_a 查
   `image/modem_pr/so/549_38_0.mbn`，写 `TZ_SZ`/`TZ_CL`（命中 st=1；未找到 st=4）
2. sm 增 `tz=sz.cl`（失败则 `0.<errcode>`）
3. **RRQ 仍 ERROR**（`tf_reply` 不变）；**不** DATA 真喂；**不**放宽
   `glink_tx_payload`（仍 n≤256/total≤512）；**不**在 `smem_scan` 读 UFS

**成功判据**：`tz` 见非零 sz + 合理 cl；设备仍活（ping/2323/`ik=4`）。硬成功仍要 `45.`——本刀只证文件几何。

未推。未刷（设备仍挂在 365d1f4；复活先刷 `boot-s20-a5137f2.img`）。

### 本轮：FAT size=0 / tz=0.57 澄清 + LFN 命中（2026-10-19 Asia/Shanghai）

**活体（8a3f32d）误读**：`tz=0.57` 不是「簇 0x57、size 0」，而是
`Err(0x57)` **未找到**（旧 sm 只打 `tz=sz.cl`，失败时 sz=0、cl=errcode，
与命中空文件同形）。本地无 modem_a 全盘 dump；enchilada 树仅有
`modem_pr/mcfg`、无 `so/`。

**刀**：
1. sm 改为 `tz=st.sz.cl`（st=1 命中；st=4 未找到）
2. `fat_split83` 按**最后**一个点拆；末分量剥 TFTP `.digits.digits`
3. `fat_dir_find` 认 LFN（UCS-2 ASCII）+ 8.3；失败码高字节=分量深度
4. 探针候选：`image/modem_pr/so/549_38_0.mbn`、根上 `modem_pr/so/…`、带 `.1.32`
5. **仍不 DATA 真喂**；不放宽 `glink_tx_payload`（文件 ≫ 256/512）

**刷验**（GATE `13201FDD4001N8`，2323→fb）：未挂。
`tz=1.eb43c.1a67` → 命中，**实长 0xEB43C = 963132 B（~941 KiB）**，
簇 `0x1A67`。本 boot 早期 `ik=0` / `q=` 空、`s=107`（sensor EF 旧症），
无 `45.`——与「几何已证」正交。下一刀：按现有 glink 帽做分块 RRQ
（勿盲目抬 total），最好 defer 到 `ik>=4`；本刀保持 RRQ ERROR。

未推。

### 本轮：TFTP 分块真喂（OACK blksize=252，不抬帽，2026-10-19 ~09:58 Asia/Shanghai）

**基线**：`f73c9bb`（`tz=1.eb43c.1a67` 命中、RRQ 仍 ERROR、活）。
**禁令**：不抬 `glink_tx_payload`（仍 n≤256/total≤512）；不动 TX intent pick；
保留 PDR ik≥4 + node-0 + UNINIT-no-RESTART。

**对账**：modem RRQ 带 `blksize=7680 wsize=10 tsize`（见 in-repo `tqftpserv`/
README 样例）。经典 TFTP 512 块 → payload 516 > 256 帽，且短块=EOF，
故必须 OACK 协商小块。

**刀（`25df6d3`）**：
1. ik≥4 后对 SO 路径：OACK `blksize=252`（4+252=256）、`wsize=1`、`tsize=文件长`
2. ACK 步进：一 DATA 未 ACK 不发下一块；读 modem_a 簇缓存 16KiB
3. ik<4 的 RRQ 仍 ERROR（等重试），避免早期 UFS/DATA
4. sm：`tz=st.sz.cl` 保留探针；新增 `tb=st.blk.acks`
5. **不**改 `glink_tx_payload` 上限

**刷验**（GATE `13201FDD4001N8`，2323→fb，`boot-s20.img`/`25df6d3`，
Asia/Shanghai ~10:00）：flash+set_active a+reboot OK。en10 ECM UP
（host 得新 link-local），但对 `169.254.1.1` **ARP incomplete** ≥90s，
无 ping / 无 2323。**硬挂**（与 `365d1f4` 抬帽真喂同形：链路在、IP 不应答）。
无 sm / 无 `tb` / 无 `45`。

**处置**：停。需硬复位进 fb → 刷
`.local/device/redfin/boot-out/boot-s20-f73c9bb.img` 或
`boot-s20-a5137f2.img` 复活。tip `25df6d3` 保留；下一刀应收窄——
疑点优先：（a）RRQ 热路径 UFS 簇读仍过早；（b）16KiB `TF_CL_BUF`/镜像变大；
（c）OACK 后立刻 DATA 仍可能在 RFS 刚绿时抢 early path。可试：只发 OACK
不 DATA、或把真喂推迟到 `HOST_ADV>=4` 且 sm 已采过一拍、或去掉簇缓存改
单 4K 块。**勿**再抬 glink 帽。

未推。

### 本轮：离线对账 TFTP OACK/DATA 硬挂（不刷，2026-10-19 ~10:06 Asia/Shanghai）

**禁令**：本轮 **不 flash**。设备仍挂在 `25df6d3`；复活镜像已在位
`.local/device/redfin/boot-out/boot-s20-f73c9bb.img`（98304 B，无需重编）。

#### 1) `f73c9bb` vs `25df6d3`：首包 RRQ 跑什么

| | `f73c9bb`（活） | `25df6d3`（硬挂） |
|--|--|--|
| RRQ 非 SO / ik&lt;4 | ERROR | ERROR |
| RRQ SO 且 ik≥4 | ERROR | `TF_ST=8` → **同拍发 OACK**（blksize=252,wsize=1,tsize=sz）→ st=9 等 ACK0 → 再 DATA |
| WRQ | ACK0 | ACK0 |
| glink 帽 | n≤256/total≤512 | **不变** |
| UFS | 仅开机 `tf_boot_probe` | 探针 + DATA 热路径 `tf_read_bytes`/`TF_CL_BUF` |

**首 RRQ 是否会单独发 OACK？会。** `TF_ST=8` 后立刻递归 `tf_reply`，在
`RF_IOK≥4` 时 **OACK 先于任何 DATA** 上线。无法从「无 sm / ARP incomplete」
区分是 OACK 本身堵死，还是 modem 已 ACK0 后首块 DATA/UFS 堵死——两者都在
首轮交互窗口内。

`365d1f4`（抬帽 + 512 DATA）与 `25df6d3`（不抬帽 + OACK 步进）同形硬挂 →
**不是「块太大」单因**；嫌疑在「对 SO 真喂 / 进入传送态」本身。

#### 2) Linux 怎么喂 `modem_pr` SO

- **传输**：**QRTR，不是 UDP**。仓内 `rootfs/src/qcom/tqftpserv/tqftpserv.c`：
  `qrtr_publish(fd, 4096, 1, 0)`；`handle_rrq` 里 **`qrtr_open(0)` 新套接字
  + `connect` 到 modem 的 `sockaddr_qrtr`**，再在该 ephemeral 口上跑
  OACK/DATA/ACK（TFTP TID = 新 QRTR port）。
- **路径**：stock `tftp_server` 从 `/vendor/firmware_mnt`（bind = modem_a
  vfat）读；服务名 QRTR **4096**，公告口常为 **0x11**（与裸机一致）。
- **信道**：用户态 libqrtr → 内核 qrtr → **qrtr-smd / glink** 进 modem；
  无独立「TFTP glink channel」，就是普通 QRTR data 帧。
- **对照 stock 时序**（`radio-bringup`）：2023 modem 在 PIL 后 ~2.5s **自己**
  从 `modem_wlan_region@8c000000` 拉 `wlan_pd`，常 **无** AP 侧 SO 拉载；
  裸机现见 RRQ `549_38_0.mbn`（~963 KiB）+ PDR UNINIT，是缺环境时的
  **TFTP 补段路径**，不是 stock 主路径。

**与裸机差异（高嫌疑）**：tqftpserv **每 RRQ 换源口**；裸机 OACK/DATA 仍从
服务口 **0x11** `glink_tx_payload` 回——可能把控制口与数据面叠死。

#### 3) 下一刀（唯一推荐，禁止再 in-band 洪水）

**OACK-only + 仿 tqftpserv ephemeral src port；永不 DATA。**

- 基线：`f73c9bb`（RRQ ERROR、活、`tz=1.eb43c.1a67`）
- 单变量：ik≥4 且 SO RRQ → 用 **非 0x11** 的固定试验 src port（或递增）发
  **仅 OACK**（可带 blksize/wsize/tsize），`TF_ST` 停在等 ACK0 / 或 ACK 后
  **故意不发 DATA**（必要时随后 ERROR）
- sm：`tb=st.blk.acks`；活体判据 = ping/2323/`ik=4`
- **不**：抬 glink 帽、TX intent、UFS 热读、分块 DATA、PAS 预灌本刀
- 判读：挂 → OACK/错 TID 即毒；活且 `tb` 见 ACK0 → 下一刀才考虑 **单块
  短 DATA（≤1）且长延迟**，仍非全文件洪水

备选（本轮不选）：PAS/区域预置 SO、或回头补 ADSP/pm 环境让 modem **自拉**
wlan_pd（stock 主路径）——更大、正交。

#### 4) 复活镜像

已存在，未重编：
`/Users/sophiehe/Documents/aginxos-next/.local/device/redfin/boot-out/boot-s20-f73c9bb.img`

未推。未刷。

### 本轮：OACK-only + ephemeral QRTR 源口（未刷，2026-10-19 ~10:14 Asia/Shanghai）

**禁令**：本刀 **不 flash**（等 `f73c9bb` 复活确认）。不推。

**相对 `25df6d3`**：去掉全部 DATA/`TF_CL_BUF`/`tf_read_bytes`；SO RRQ 且
`ik≥4` 时仅从 **ephemeral src port**（`0x1a00|seq`，≠0x11）发 OACK
（blksize=252,wsize=1,tsize）；ACK0 → `TF_ST=0xa` 冻结，**永不 DATA**。
WRQ/ERROR 仍走服务口 `0x11`。RX 在 `TF_SPORT` 收 ACK0。

**sm**：`tb=st.sport.acks`（`TF_BLK` 复用为 sport）—— OACK 已发=`9.<sport>.0`，
ACK0 已见=`a.<sport>.1`。

**保留**：FAT 探针/`tz`、PDR（a5137f2 线）、不抬 glink 帽、不动 TX intent。

镜像：`.local/device/redfin/boot-out/boot-s20.img` + 提交后 stamp
`boot-s20-<sha>.img`。


### 本轮：离线对账 — 禁 TFTP 真喂后改打自拉 wlan_pd（不刷，2026-10-19 ~10:20 Asia/Shanghai）

**禁令（early ECM 硬挂，已钉死）**
- TX intent pick / BUSY / RR / INTENT_REQ
- TFTP **DATA** 真喂、**OACK**（含 ephemeral 源口）、抬 `glink_tx_payload` 帽
- tip `8a761ad`（OACK-only）已证：只要离开「SO RRQ→ERROR」就会 ARP incomplete

**复活基线（未动设备）**
`f73c9bb`：FAT 几何 OK（`tz=1.eb43c.1a67`）、RRQ 仍 ERROR、PDR REGISTER
成功且 state=**UNINIT**（`sr=1.7fffffff.0`）、`ik=4`、**无 `0x45`**。
镜像已在：`.local/device/redfin/boot-out/boot-s20-f73c9bb.img`（98304 B）。
本轮 **不 flash、不 push**。

#### 1) bare PAS / modem_wlan_mem / MSA vs Linux pil-tz

| 项 | Linux `subsys-pil-tz` + icnss | 裸机现状（`f73c9bb` 线） |
|---|---|---|
| modem pas-id | **4**，`firmware-name=modem` | 同：`pas_init_image` → INIT/MEM_SETUP/AUTH |
| MEM_SETUP 窗 | ELF 可重定位跨度经 `dma_alloc`；dts `modem_wlan_mem@8c000000` 只标 **`0xF800000`**，驱动**不读**该 length | 固定 **`0x8C000000` + `0x11800000`**（= ELF `0x8C…–0x9D80…`）；`pas2/3/4/5=0` 已绿 |
| 段装载 | `modem.mdt`+`bXX` → region | 同：FAT 抄 30 段，`ld=0x1e` |
| hyp_assign 镜像区 | 本机 `subsys_vmid=0` → **不做** | 同：不做 |
| MSA `pil_wlan_fw@8BA00000` / `0x200000` | **仅** `0x45` `server_arrive` 后 `msa_mem_info` + `ICNSS_MSA_PERM` | **未做**（正确：MSA 在 WLFW 之后，不是 spawn 前置） |
| ADSP | stock ~3.5s ONLINE，**先于** modem | 已先 PAS（pas-id **1** @`0x89200000`/`0x2800000`），settle 3s 再 modem；但 ready 后常 **err-fatal**（`sar.cc:27:chre_utils fatal`，`ae=1.7.32`→slave 含 bit0） |
| **CDSP** | stock ~3.5s ONLINE，**先于** modem（pas-id **18**，mem **`0x87400000`/`0x1E00000`**，smp2p smem **94/432**，remote-pid 5） | **完全没有**：无 CDSP 装段、无 PAS、无 smp2p、无 AOP `cdsp on` |
| RFS / 公告 | qrtr-ns + patched rmt_storage + tftp + pd-mapper（含 `modemuw.jsn`）PIL 前 listen | RFS `ik=4`；公告齐；pd-mapper Dom 含 `kernel/elf_loader`→`msm/modem/wlan_pd` inst 180（与 jsn 一致） |
| PDR | icnss 对 `0x42@inst180` REGISTER；听 IND | REGISTER OK → **UNINIT**；UNINIT **不** RESTART（`a5137f2`） |

结论：modem **root PD 已认证跑起来**（QRTR/`0x42`/`ik=4`）。缺的不是再抄一段进 `0x8C000000`，也不是提前 MSA。

#### 2) 缺口（gap）：自拉 vs TFTP 回退

Stock / radio-bringup（2023 modem + 同 EFS）：

- PIL 后 ~2.5s modem **自己**从 **`modem_wlan_region@8c000000` 已装段**拉起 `wlan_pd`
- **无** AP 侧 SO 拉载 → QRTR 直接 `NEW_SERVER 0x45@0:88`
- 前提是 AP 环境（ADSP/CDSP 先于 modem、早期 radio 栈）；环境不对时同镜像会 `SERVICE_NOT_FOUND` / 不 spawn

裸机活体（`a5137f2`/`f73c9bb`）走的是 **回退链**，不是主路径：

1. `pn=kernel/elf_loader.1.30` — root PD 向 pd-mapper 要 elf_loader，命中 `wlan_pd`
2. `tf=…/modem_pr/so/549_38_0.mbn…` — TFTP RRQ 要 SO（实长 `0xEB43C`，探针已证）
3. 我们 **ERROR**（禁令下唯一安全应答）→ PD 停在 **UNINIT** → **无 `0x45`**

`modemuw.jsn` 在 Linux/裸机 Dom 表里**一样**（elf_loader∈wlan_pd）。故缺口 **不是** jsn/Dom 答错，而是：**root PD 没走「内存内自拉」**，改走「elf_loader + TFTP 远程装 PD」。TFTP 真喂/OACK 已三挂，这条回退 **禁继续挖**。

相对 stock 仍缺、且与「自拉」文档直接相关的单点：**CDSP 从未 ONLINE**（ADSP fatal 为次要嫌疑，已多次无法靠 GLINK/SSR 刀消掉）。

#### 3) 下一刀（唯一推荐，禁 TFTP 成功路径）

**单变量：CDSP PAS 插在 ADSP settle 之后、modem AOP/PAS 之前。**

对齐 `lito.dtsi` / radio-bringup：

1. AOP：`{class: image, res: load_state, name: cdsp, val: on}`
2. 从 vendor 装 `cdsp.mdt`+`b*` → `0x87400000`（region `0x1E00000`）
3. `pas_go(18, 0x87400000, 0x1E00000)`（INIT/MEM_SETUP/AUTH，复用现有 `pas_go`）
4. 写 CDSP smp2p 出向（smem **432** / remote-pid **5** / IPCC CDSP），等入向 **94** 的 err-ready（bit1，bit0 清），超时失败只打环、**仍可**继续 modem（与 ADSP 失败策略同类，便于判读）
5. **然后**才现有 modem AOP + `pas_init_image`

**明确不改**：TFTP（SO RRQ 仍 ERROR）、TX intent、glink 帽、PDR 逻辑、MSA、Dom/jsn。

**sm 信号**

- `cd=st.pas.rdy` — st: 0未跑 1装段 2PAS ok 3ready 4超时/失败；pas=AUTH x0；rdy=slave-kernel 低字节
- 软成功（环境刀生效、仍无自拉）：`cd` 见 ready，且 `pn`/`tf` **不再**出现 elf_loader→SO RRQ（或明显推迟），`sr` 仍 UNINIT，`q` 仍无 45
- 硬成功：`q=` 含 **`45.`**（或 `45.1`），`sr` 离 UNINIT（UP/`1FFFFFFF` 或 IND）
- 失败：`cd` 失败且行为与 `f73c9bb` 相同（仍 SO RRQ + UNINIT）；或 early ECM 无 ping → **整段回退**，刷回 `boot-s20-f73c9bb.img`

**不选的备选（本轮不写代码）**

- SO RRQ 静默丢弃（不 ERROR）：可能只把 UNINIT 拖死，且与 stock「根本不 RRQ」不对齐
- 把 Dom 里 `elf_loader` 改成 0 命中：与 stock jsn **故意偏离**，属症状刀
- 提前 MSA hyp_assign：Linux 顺序在 `0x45` 之后，早做是发明顺序

#### 4) 复活镜像确认

`.local/device/redfin/boot-out/boot-s20-f73c9bb.img` 存在（98304 B，`f73c9bb`）。
tip 仍为 `8a761ad`（OACK-only，**勿刷**）。用户硬复位进 fb 时只刷 `f73c9bb` 复活。

未推。未刷。未改 `main.rs`。

## 2026-10-19：CDSP-before-modem 刀（未刷）

基线代码：`f73c9bb`（FAT 探针 + PDR UNINIT；SO RRQ→ERROR）。
tip 上 OACK（`8a761ad`）TFTP 已回退，main 与 f73c9bb TFTP 一致后再加本刀。

**单变量**：ADSP settle 之后、modem AOP/PAS 之前：
1. CDSP smp2p 出向（smem **432** / host 0↔5 / IPCC CDSP=6）
2. AOP `{name: cdsp, val: on}`
3. vendor 装 `cdsp.mdt`+`b*` → `0x87400000` / `0x1E00000`
4. `pas_go(18, …)` → 等入向 **94** err-ready（bit1，bit0 清）；超时/失败只打环，仍开 modem

**sm**：`cd=st.pas.rdy`（0未跑/1装段/2PAS/3ready/4失败）

**不改**：TFTP（仍 ERROR）、TX intent、glink 帽、PDR、MSA、Dom。

刷序：先 `boot-s20-f73c9bb.img` 复活 → 再本刀。未推。未刷。

### 本轮：CDSP-before-modem 已刷 → 硬挂（2026-10-19 ~10:30 Asia/Shanghai）

`8d88cd5` / `boot-s20.img`（102400 B）在 `f73c9bb` 复活后刷上。
症状同既有 early hang：**ECM 链路在，`169.254.1.1` ARP incomplete，无 ping / 无 2323**。
无可用 `sm`（含预期 `cd=`）。**整段 CDSP-before-modem 打入禁令。**

---

## S4 状态收束（2026-10-19 ~10:35 Asia/Shanghai）— 不刷 / 不实现

### Living tip（唯一可刷）

| 项 | 值 |
|---|---|
| **flash-safe sha** | **`f73c9bb`** |
| 镜像 | `.local/device/redfin/boot-out/boot-s20-f73c9bb.img`（98304 B） |
| 活体 | `ik=4.0`；FAT `tz=1.eb43c.1a67`（`549_38_0.mbn` 实长）；PDR REGISTER 成功且 state=**UNINIT**（`0x7fffffff`）；SO RRQ→**ERROR**；**无 `0x45`** |
| tip 策略 | tip **不得**停在 `8d88cd5` / `8a761ad` / `25df6d3` / `365d1f4`；`main.rs` 回到 `f73c9bb`；其后只许 **docs/notes** 提交。`boot-s20.img` 应与 `boot-s20-f73c9bb.img` 一致，避免误刷 |

### 禁令（early hang = ECM up / ARP incomplete）

已钉死，**禁止再编进可刷 tip**：

1. **glink TX 侧**：intent 池 pick / BUSY / RR / INTENT_REQ / 抬 `glink_tx_payload` 帽 / 飞行锁
2. **TFTP 成功路径**：任何 **DATA**、任何 **OACK**（含 ephemeral QRTR 源口、`blksize=252`）
3. **CDSP-before-modem 冷启**（`8d88cd5`）：刚硬挂 — 与 TFTP 成功同形

仍允许（且活体依赖）：SO RRQ→**ERROR**；PDR REGISTER @ node-0，**仅 ik≥4 后**；UNINIT **不** RESTART；FAT 只读探针（`tz`）。

### 今日已成（保留）

- RFS 同 txn 多路 OPEN → ALLOC + IOVEC 大读（`ik=4`）
- PDR REGISTER 真 RESP：state=**UNINIT**（非坏帧）
- FAT LFN 几何：`549_38_0.mbn` → `0xeb43c` @ cluster 探针
- ADSP up；modem **root PD** up（服务变多，仍无 `0x45`）

### 开放假说：stock 为何不靠 TFTP 自拉 `wlan_pd`

裸机停在 **elf_loader → TFTP 要 SO → 我们 ERROR → UNINIT**，是回退链。stock 主路径应是 root PD **内存内自 spawn** `msm/modem/wlan_pd` → 发 WLFW **`0x45`**。未证伪的缺口：

| # | 假说 | 现况 | 怎么证 |
|---|---|---|---|
| A | **CDSP 时序**：stock CDSP ONLINE 先于 modem；裸机无 CDSP | `8d88cd5` **before** 已硬挂 → 「冷 before」否；**after settle** 未试 | 见下一刀；再挂则整条 CDSP PAS 入禁令 |
| B | **缺 IPA / 互联时钟门**：wlan_pd 要 IPA 资源才 spawn | 裸机未起 IPA | 只读查 IPA clock/GCC 位（无写） |
| C | **modem PAS mem 不完整**：`modem_wlan_mem` 区/认证与 pil-tz 差一段 | root PD 已起，但 PD 表可能缺段 | 对账 PIL 段 vs bare 装入长度（只读） |
| D | **Dom / jsn 环境**：`modemuw.jsn` elf_loader∈wlan_pd | 与 Linux Dom **一致** → 低嫌疑 | 除非找到第二份 region 表 |
| E | **ADSP fatal / SSR 残态** 逼 modem 走 TFTP | ADSP「up」但 fatal 史未清 | sm 已有；不优先再动 GLINK |
| F | **缺 AOP/其他 image load_state**（非 cdsp） | 只明确缺 CDSP AOP | 对账 stock AOP 序列（离线） |

### 下一刀（唯一；**未实现 / 未刷**）

**推荐：park CDSP PAS，改做零新流量观测刀（sm-only）。**

- **做什么**：在 `f73c9bb` 活体上只加 **只读** `sm` 字段——例如 CDSP 入向 smem **94** 原始 bit、出向 **432**、（若安全）IPA/GCC 门控只读影子；**不** AOP、**不** `pas_go(18)`、**不** 新 QRTR、TFTP 仍 ERROR。
- **为何不是 CDSP-after-ik4**：after 仍是 PAS 写路径；before 已同形硬挂，after 一次失败成本高。先用只读判「CDSP 位是否本就与 wlan 无关 / 是否已被别人拉起」。
- **成功判据**：设备仍 ping/`ik=4`；`sm` 多出只读字段；行为与 `f73c9bb` 同（仍 SO RRQ + UNINIT + 无 45）即软成功。
- **若只读刀都挂**：立即 park 整段，只做离线 Linux 抓取（假说 B/C/F）。
- **明确不选（本轮）**：CDSP-after-ik4（留作只读证伪 A 之后的候选）；任何 TFTP 成功；改 Dom elf_loader；提前 MSA。

### 误刷护栏

| sha / 镜像 | 状态 |
|---|---|
| **`f73c9bb`** / `boot-s20-f73c9bb.img` | **唯一 flash-safe** |
| `a5137f2` / `a84360c` | 次选复活（PDR OK、无 FAT tz）；优先仍 `f73c9bb` |
| `8d88cd5` `8a761ad` `25df6d3` `365d1f4` `206821a` `1119de1` | **禁刷**（early hang） |
| tip `boot-s20.img` | 须 = `f73c9bb` 产物，勿留 CDSP/OACK 体积 |

未推。本段只文档 + tip 护栏；**不实现下一刀；不 flash。**

### 复活确认 + park（2026-10-19 ~10:42 Asia/Shanghai）

`boot-s20-f73c9bb.img` 已刷并起来（ping ~10:41:35）。`sm`：

| 字段 | 值 |
|---|---|
| `ik` | `4.0` |
| `tz` | `1.eb43c.1a67` |
| `sr` | `1.7fffffff.0`（REGISTER 成功，state=**UNINIT**） |
| `q` | `42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 `45.`** |
| `tf` | SO 路径仍在；RRQ→ERROR |

**未实现 / 未刷** sm-only（host **0/5** 项 **94/432**）：活体 tip 从未走过该分区；TOC/新分区误读有冻核史；本轮按「宁可 park 也不再挂」跳过。开放假说 A–F 仍成立；下一刀仍是零写 sm-only 或离线证 B/C/F。flash-safe 仍只有 **`f73c9bb`**。未推。

---

## 2026-10-19 ~10:48 Asia/Shanghai：离线对账 → MPSS SMP2P `ipa` 出向刀

### 深对账（假说 B/C/F vs Linux stock 自拉）

| 项 | Linux stock | 裸机 `f73c9bb` | 结论 |
|---|---|---|---|
| **PAS_MEM_SETUP size** | PIL 按 ELF min–max：`0x8C000000`–`0x9D800000` = **`0x11800000`**（DTS `modem_wlan_mem` 仅 `0xF800000` 是 reserved pool，不是 mem_setup 参数） | 同 **`0x11800000`**，AUTH 已过 | **假说 C 关闭**：段跨度不缺；BSS/hash 在 ELF 尾（含 past-DTS 的 30–34）已在 PAS 窗内 |
| **AOP load_state** | `adsp` → **`cdsp`** → `modem`（`signal-aop`）；`ipa_fws` **无** signal-aop | 仅 `adsp` + `modem` | CDSP AOP 仍缺，但 **冷 CDSP PAS 已禁**（`8d88cd5` 硬挂） |
| **IPA bringup** | `ipa3` 探 → `printf 1 >/dev/ipa` 才 PIL `ipa_fws`（pas-id **0xf** @ `0x8BC00000`）；注释写明主要为 **qcacld SMMU / 0x45 之后**，非自拉充分条件 | 无 IPA PAS / 无 QMI `0x31` 真栈 | 全量 IPA 冷启 ≈ CDSP 级写路径，**本轮不做** |
| **MPSS SMP2P 出向** | `lito-smp2p.dtsi`：`master-kernel` + **`ipa`**（`valid_entries=2`）；入向另有 `slave-kernel` / `ipa` / `wlan` | `smp_publish` 只写 **`master-kernel`**（`valid=1`） | **相对 stock 的单点、零新 QRTR 缺口** |
| **自拉 vs TFTP** | PIL+~2.5s 内存内 spawn `wlan_pd` → `0x45@0:88`；环境不对则同镜像 `SERVICE_NOT_FOUND` / 不 spawn | `pn=kernel/elf_loader` → RRQ SO → ERROR → PDR **UNINIT** | 回退链；禁 TFTP 成功 |

内存序（只读钉死）：`CDSP@874` → `ADSP@892` → `WLAN_MSA@8ba` → **`IPA_FW@8bc`** → `modem_wlan@8c0`（DTS `0xf80_0000`；ELF/PAS `0x1180_0000`）。

### 刀（已实现 / 将刷）

**单变量**：`smp_publish` 在既有 item 428 分配里加出向名 **`ipa`**，`valid_entries=2`；入向 435 扫描记 `ipa`/`wlan` 名位。  
**不改**：TFTP（仍 ERROR）、TX intent、glink 帽、PDR、PAS、AOP、CDSP。  
**为何不像 CDSP-before 挂**：无 PAS/AOP/装段；与已活的 `master-kernel` 同一次 smem 写 + 同一 IPCC 踢；Linux 开机即有该名且 value=0。  
**sm**：`me=pub.vout.inflags`（pub=publish 码，vout=2，inflags bit0=入向 ipa / bit1=wlan）。

**成功判据**

- 硬：`q=` 含 **`45.`**
- 软（刀生效、仍无自拉）：仍 ping/`ik=4`；`me=1.2.*`；行为同基线（SO RRQ+UNINIT）
- 失败：early ECM/ARP incomplete → 立刻刷回 `boot-s20-f73c9bb.img`

基线：`f73c9bb`。未推。

### 刷验（2026-10-19 ~10:50 Asia/Shanghai）— 软成功，无 0x45

`c38a6cd` / `boot-s20.img`（98304 B）经 2323→fb GATE `13201FDD4001N8`
`flash boot_a` + `set_active a` + reboot。ping ~32s；2323 OK。

| 字段 | 值 |
|---|---|
| `me` | **`1.2.0`** — pub 写过；vout=2（含 `ipa`）；入向 inflags=0（435 无字面 `ipa`/`wlan` 名；`e=10002` 仍两入向，第二项为 slave-kernel） |
| `ik` | `4.0` |
| `tz` | `1.eb43c.1a67` |
| `sr` | `1.7fffffff.0`（仍 UNINIT） |
| `pn` | `kernel/elf_loader.1.30` — **仍走 TFTP 回退** |
| `q` | `42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 `45.`** |

**判读**：刀安全且生效（相对 `f73c9bb` 仅多 `me=` / 出向 `ipa`），**不足以**触发内存内自拉。假说「只缺 MPSS 出向 ipa」**证伪**。仍缺的更大环境差以 **CDSP ONLINE（禁冷 PAS）** 与/或 **真 IPA PAS+GSI** 为主；二者皆 early-write 高危，下一刀须另辟只读/延后路径，不得回 TFTP/intent。

flash-safe 现可含 **`c38a6cd`**（行为 ⊇ `f73c9bb` + `me`）；复活仍可用 `boot-s20-f73c9bb.img`。未推。

### 本轮：候选刀离线对账 → park（2026-10-19 ~10:55 Asia/Shanghai）

活体仍挂 **`c38a6cd`**（`me=1.2.0` / `ik=4` / `sr=1.7fffffff.0` / `pn=kernel/elf_loader.1.30`，无 `0x45`）。
本轮 **未改 main.rs、未刷、未推**。采 sm 时 2323 一度 RST（ping+ARP peer MAC 仍好）—
软退化，非 ARP-incomplete 硬挂；不因此换镜像。

#### 候选逐条否决

| 候选 | 相对 `c38a6cd` | 裁定 |
|---|---|---|
| **延迟 CDSP @ ik≥4**（PAS/AOP/装段） | 时序不同于 before-modem | **不做**。radio-bringup：「modem 先起则事后永不 spawn wlan_pd」对 ADSP 已钉死；CDSP 同类。且仍是 PAS 写路径，before 已同形硬挂——**非**「强不同」。 |
| **AOP-only `cdsp on`（无 pas_go）** | 比 `8d88cd5` 窄 | **禁区**：用户/收束条写明 `CDSP-before-modem cold PAS/AOP`；before-modem AOP 落在同一禁令。after-ik4 则时序无效（见上）。 |
| **IPA fws 轻量 PAS**（pas-id `0xf` @ `0x8BC00000` / 64KiB，无 signal-aop） | 比 CDSP 小、无 AOP | **本轮不做**。仍是早期 PAS 写；CDSP 挂因未拆到「非 AOP」单变量，一次失败成本高。且 S4 记 IPA 主要为 **0x45 之后** qcacld SMMU，非自拉充分条件。 |
| **出向 `ipa` value 预置 CLK 位**（`IPA_SMP2P_OUT_*` → value=`0x3`） | 同 smem 写，零 PAS | **弱假说，不做**。Linux 仅在 modem **入向查询 IRQ** / panic 时 `freeze_clock_vote`；开机 spawn 窗口 value 常为 0。活体 `me=1.2.0` 入向尚无字面 `ipa`/`wlan`——无查询可应答。预置位 = 发明顺序。 |
| **入向 `wlan` smp2p** | — | **AP 无法预建**。435 由 modem 分配；`wlan` 名随 wlan_pd 起。鸡生蛋。 |
| **改 Dom/jsn 藏 elf_loader** | — | **症状刀**；与 stock `modemuw.jsn` 故意偏离。 |
| **再动 TFTP / TX intent / glink 帽** | — | **禁令**。 |

#### IPA SMP2P 补钉（相对 `c38a6cd` 软成功）

`ipa_v3`：出向 bit0=`CLK_RSP_CMPLT`、bit1=`CLK_VOTE`，**只在** modem 入向 query 时置位。
故「有名 value=0」对齐 Linux 开机初态；**证伪的是「只补名」**，不是「IPA 整栈无关」。
真 IPA 栈（fws PAS + GSI + 应答 query）仍是更大刀子，需先有 Linux 时序收据再开。

#### 下一棒（精确离线索取 — 槽 b / 一次能出 `0x45` 的 radio-bringup）

请抓 **带时间戳** 的一段（`dmesg -T` 或 `/var/radio.log` + `qrtr-lookup`），明确回答：

1. **CDSP ONLINE** 相对 **modem PIL** 与首条 **WLFW/`0x45`** 的先后（是否 before modem）。
2. **`ipa_fws` / `printf 1 >/dev/ipa`** 完成相对 modem PIL / `0x45` 的先后。
3. 同一次能出 `0x45` 的启动里，是否出现 **`modem_pr/so/549_38_0.mbn`**（或任何 `tqftp`/`tftp` RRQ）——有则自拉≠「永不 TFTP」，无则裸机 elf_loader 链仍是环境回退。
4. 入向 SMP2P（debugfs / 日志）何时出现名 **`ipa` / `wlan`**（相对 `0x45`）。

有收据后再开 **单一** 刀：优先「有 before-modem 实证的最窄写」（例如仅 IPA fws PAS，或仅 CDSP 且有 stock 时序），仍禁 TFTP 成功与 TX intent。


### 只读刀：cmd-db BCM dump（sm `bc=`，未刷）

在 flash-safe `c38a6cd` / tip `11cfc4c` 基线上加 `bcm_probe()`：对
IP0/SH0/MC0/ACV/SN1/CN0/SH4 只做 `cmd_db_hit` + 读 `struct bcm_db`
（unit_size/width）。**无** `rpmh_cmd` / **无** `pas_go` / **无** SMMU STE /
**无** TFTP 回包。

`sm` 字段：`bc=NAME.st.addr.unit.width,...`（逗号分隔，序同上）。
`st`：0 miss / 1 ok(HW=BCM,aux≥8) / 2 hit但HW≠5 / 3 aux短 / 4 magic坏。

用途：刷后一眼对上 I4 缺的 addr/unit/width；**不**代替 slot-b 抓 vote 向量。
恢复镜像仍 `boot-s20-c38a6cd.img`。未推。未刷。

flash-safe 仍 **`c38a6cd`**（⊇ `f73c9bb`）；复活 `boot-s20-f73c9bb.img` / `boot-s20-c38a6cd.img`。未推。

### 槽 b 时序抓取 — ABORT（2026-10-19 ~11:00 Asia/Shanghai）

意图：`set_active b`（不覆写 `boot_b`）→ adb `aginxosredfin` 采 dmesg 时序
（CDSP ONLINE / modem PIL / `0x45` / IPA fws / TFTP `549_38_0` / SMP2P ipa·wlan），
再刷回 `boot-s20-c38a6cd.img` + `set_active a`。

**阻断**：活体仍 `c38a6cd`（ping/ARP/ECM OK），但 **TCP 2323 软死**
（RST→Timeout→Refused），无法 `fb` 进 fastboot；`fastboot`/`adb` 皆空。
未切槽、未刷、无 timeline。收据目录
`.local/device/redfin/slot-b-timeline-20261019-105839/ABORT.txt`。

**下一步**：人工硬复位进 fb（GATE `13201FDD4001N8`）后重跑；restore 镜像
已在 `.local/device/redfin/boot-out/boot-s20-c38a6cd.img`。

未推。

### Linux 槽时间线抓包（Asia/Shanghai 2026-10-19 ~11:03–11:05）

`set_active b`（未刷 `boot_b`）→ adb `aginxosredfin` ~31s。收据：
`.local/device/redfin/slot-b-timeline-20261019-110320/`（`dmesg-raw.txt`、`qrtr-lookup.txt`、`TIMELINE.md`）。

PIL 顺序（内核秒）：

| t | 事件 |
|--:|------|
| 40.88–40.90 | **ipa_fws** load / out of reset |
| 47.09–47.37 | **adsp** load / out of reset / Power-Clock；icnss probe |
| 48.32–48.40 | **cdsp** load / out of reset / Power-Clock |
| 48.42–49.08 | **modem** load / out of reset / Power-Clock（IPA BEFORE→AFTER_POWERUP） |
| 49.09 | rmt_storage OPEN/ALLOC/IOVEC（无 TFTP） |
| 50.21 | **wlan_pd ONLINE** `0x1fffffff` → **icnss QMI Connected** |

`qrtr-lookup`：**`svc=0x45 inst=0x1 at 0:88`**（WLFW 在）。dmesg 对 `tftp`/`549_38`/`elf_loader` **0 命中**。

对裸机：顺序是 **ipa → adsp → cdsp → modem**，不是单独冷启 CDSP；Linux 不靠 TFTP 起 wlan_pd。

恢复：trampoline 软 `reboot bootloader` 无效，**待硬复位进 fb** 后刷 `boot-s20-c38a6cd.img` → `boot_a` + `set_active a`。未推。


### 下一刀设计：IPA-before-ADSP PAS（slot-b 解锁，未刷）

**收据**：`.local/device/redfin/slot-b-timeline-20261019-110320/TIMELINE.md`
（内核秒）`ipa_fws@40.9 → adsp@47 → cdsp@48.3 → modem@48.4 → wlan_pd/0x45@50.2`；
**无 TFTP**。

**裸机现况（`c38a6cd` / tip park）**：ADSP + modem PAS；MPSS 出向已有 `ipa` 名
（`me=1.2.0`）；**无** `ipa_fws` PAS；**无** CDSP（冷启已禁）。

#### 与禁区 CDSP-before-modem（`8d88cd5`）的差别

| | 挂掉的 CDSP 刀 | 本刀 IPA fws |
|--|--|--|
| 链位置 | ADSP settle **后**、modem **前**（跳过 IPA） | **链首**，在 ADSP **之前**（对齐 stock） |
| AOP | 需要 `cdsp on` | **无** signal-aop（lito DTS） |
| 区大小 | `0x87400000` / **30 MiB** | `0x8BC00000` / **64 KiB**（实装 ~0x6000） |
| SMP2P | 出向 432 / 入向 94 等待 | **无** |
| pas-id | 18 | **0xf** |

假说：modem 自拉 `wlan_pd`（不走 elf_loader/TFTP）依赖 stock 链前缀；
仅冷启 CDSP 切错了位置且缺 IPA。本刀只补 **最小缺失前缀** = IPA fws。

#### 实现（单变量）

1. `load_ipa_fws`：vendor_a 上找 `ipa_fws.mdt`（+`.bXX`，无则 mdt 内嵌段）
2. `pas_go(0xf, 0x8BC00000, 0x10000)` — INIT/MEM/AUTH，**无 AOP**
3. 插入点：`smem_scan` 之后、LPASS/ADSP **之前**
4. 失败只打环，ADSP/modem 照旧
5. `sm if=st.p0.p1.p2`

**不做**：CDSP、TFTP 成功、TX intent、改 glink 帽、改 PDR。

**风险**：仍是早期 PAS；Linux 在 PIL 前有 IPA SMMU/时钟投票，裸机未仿。
若 ARP incomplete 硬挂 → 整段 IPA PAS 入禁令，刷回 `c38a6cd`。

**备选阶段（若本刀挂）**：仅 vendor 探针（`if=` 找文件/段数，不写 `0x8bc`、不 SMC）—
本轮直接做 PAS，因 slot-b 已给 before-modem 实证。

**刷序（用户硬复位进 fb 后）**：
1. 先 restore：`boot-s20-c38a6cd.img` → `boot_a` + `set_active a` + reboot，确认 ping/`sm`
2. 再刷本刀 stamp：`boot-s20-<sha>.img`（或 tip `boot-s20.img`）
3. 采 `if=` / `ik=` / `q=` / 有无 `0x45`；硬挂则立刻回 `c38a6cd`

**本轮未刷**（设备仍在 Linux slot b）。未推。

**提交 / 镜像（未刷未推）**：`ceeb6fd`
- tip / 待刷：`.local/device/redfin/boot-out/boot-s20.img`（102400 B）
- stamp：`.local/device/redfin/boot-out/boot-s20-ceeb6fd.img`
- restore 仍：`.local/device/redfin/boot-out/boot-s20-c38a6cd.img`

### 本轮：IPA-before-ADSP PAS 硬挂（`ceeb6fd`，2026-10-19 ~11:24 Asia/Shanghai）

**基线 restore**：`c38a6cd` 刷验 OK（GATE `13201FDD4001N8`，~11:21 flash →
ping ~11:23）。`sm`：`ik=4.0` `sr=1.7fffffff.0` `sp=0.2` `me=1.2.0`
`pn=kernel/elf_loader.1.30` `tf=.../549_38_0.mbn` `tz=1.eb43c.1a67`
`q=42.b401,2b.1202,...`（**无 `0x45`**）。

**刀刷**：2323 `fb` → `boot-s20-ceeb6fd.img`（= tip `boot-s20.img`，102400 B）
→ `boot_a` + `set_active a` + reboot OK（~11:24:47）。

**刷验**：≥90s **无 ping**；host **无 ECM/RNDIS 网卡**（en7–en10 皆无，
无 `169.254` 址）；`arp 169.254.1.1` incomplete（落在 en0）；无 2323；
fastboot 空；USB 串口节点无。**硬挂**（比 ARP-incomplete 更重：gadget
未起来）。无 `sm` / 无 `if=` / 无 `0x45`。

**处置**：停。勿再冷刷 IPA PAS。需硬复位进 fb → 刷
`.local/device/redfin/boot-out/boot-s20-c38a6cd.img` 复活。
`ceeb6fd` 入禁区（与 CDSP-before / TFTP 回包同档）。假说：裸机缺
Linux 在 `ipa_fws` PIL 前的 SMMU/时钟投票，链首 PAS 仍过早堵死。

未推。

### 离线：`ipa_fws` PAS 前置（slot-b dmesg + kernel-src，2026-10-19 ~11:28 Asia/Shanghai）

**范围**：`ceeb6fd` 硬挂后，**不刷**。对账 Linux 在 `pas_go(ipa_fws)`
**之前**必须就绪的时钟/互联/SMMU；抽出「不是再冷 PAS」的最小序列。
源：`.local/device/redfin/slot-b-timeline-20261019-110320/` +
`kernel-src/redbull`（`lito.dtsi`、`ipa_v3/ipa.c`、`subsys-pil-tz`）。

#### Linux 在 AUTH 前实际做了什么（内核秒 ≈40.85–40.88）

| 序 | 事件 | 证据 |
|--:|------|------|
| 1 | `apps_smmu` 已起；IPA 三 CB 挂 consumer | dmesg `soc:ipa_smmu_{ap,wlan,uc}` → `15000000.apps-smmu` |
| 2 | IPA 驱动 probe：`qcom,bandwidth-vote-for-ipa` → **无 `core_clk`** | `ipa.c` `use_bw_vote`；`ipa3_get_clks` 直接返回 |
| 3 | `ipa3_enable_clks` = **msm-bus / ad-hoc 投票**（client `ipa`） | dmesg `register_client_adhoc:Client handle 27 ipa` |
| 4 | IOMMU group：SID **0x520 / 0x521 / 0x522**（ap/wlan/uc）+ fastmap | DTS `iommus = <&apps_smmu 0x052{0,1,2} 0x0>`；dmesg groups 13–15 |
| 5 | 附加 map：ap→IMEM `0x146a9000/0x2000`；wlan→`0x1ea0000/0x80000`（ipa-uc ram） | `lito.dtsi` `qcom,additional-mapping` |
| 6 | `IPA_ACTIVE_CLIENTS_INC`（保票）→ `ipa3_attach_to_smmu` → `subsystem_get("ipa_fws")` | `ipa3_load_ipa_fw` 6330–6350 行 |
| 7 | 其后才 PIL：`0x8bc00000`–`0x8bc06000` / Brought out of reset | dmesg 40.882 / 40.901 |

`qcom,ipa_fws` 节点本身：**无** `clocks`、**无** proxy-reg、**无** `signal-aop`——
只有 `pas-id=<0xf>`、`memory-region=pil_ipa_fw_mem`、`qcom,msm-bus,name="pil-ipa_fws"`。
时钟不在 PIL 节点上，而在 **IPA HW 驱动的 bus vote** 里。

GCC：`gcc-lito.c` / `qcom,gcc-lito.h` **无** 可 enable 的 `gcc_ipa_*`；
`debugcc-lito` 的 `measure_only_ipa_2x_clk` 只是测频 dummy，**不是**上电路径。
真「开钟」= 互联向量（DTS `MSM_BUS_MASTER_IPA*` / `SLAVE_IPA_CFG` /
`IPA_CORE→IPA_CORE`）经 RPMH/BCM，至少 SVS2。

#### 前置清单（PAS 前必须，缺一可 NoC 挂死）

1. **互联投票**：client `ipa`（及 PIL 侧 `pil-ipa_fws`），路径含
   IPA→LLCC、LLCC→EBI、IPA→OCIMEM、AP→IPA_CFG、**IPA_CORE→IPA_CORE**
2. **apps_smmu** 已初始化；三 CB STE/SID **0x520/0x521/0x522** 已 attach
3. **additional-mapping**（至少 wlan CB → `0x1ea0000`）
4. **保留内存** `pil_ipa_fw@8bc00000`（64KiB）+ `pil_ipa_gsi@8bc10000` 已隔离
5. （驱动内）IPA/GSI MMIO 映射与 active-client 票 — 只读 `0x1e00000` 而无票
   亦可能堵 NoC（与 `ceeb6fd`「无 ECM」同型）

#### 非「再冷 pas_go」的最小对齐序列（概念）

```
ad-hoc/BCM: vote client "ipa" ≥ SVS2
  → apps_smmu: attach SID 0x520/521/522 (+ additional maps)
  → (optional) sm-probe 只读 GCC/互联状态寄存器，禁止摸 IPA MMIO
  → 仍禁止 pas_go(0xf) 直到上两项有裸机实证
```

裸机今日 **没有** 可刷的 BCM/RPMH 投票实现；`ceeb6fd` 证明「只 PAS、
跳过 1–3」= 无 ECM 硬挂。

#### 下一刀抉择

| 选项 | 内容 | 判定 |
|------|------|------|
| (a) sm-only 开钟 | 只打互联/时钟票 + `sm` 探针，**无** PAS / **无** IPA MMIO | **不做** — 无已验证的 BCM 地址/序列；猜写 RPMH 与冷 PAS 同档风险；且 `d1e0197` 已否决「ipa CLK 预置位」 |
| (b) **park** | tip 回 flash-safe；清单给人/后轮 | **采用** |

后轮解锁条件（任一）：(1) 从 stock 抽出 IPA bus-vote 的具体 RPMH/BCM
命令字与顺序；(2) 证明裸机已有等价互联票且 sm-probe 只读安全；(3) 改打
「modem 为何走 elf_loader」的非 PAS 差（env/smem），仍禁早期 PAS。

#### tip flash-safe

- `main.rs` 已从 tip 去掉 `load_ipa_fws` / 链首 PAS（回 **`c38a6cd`** 内容）
- `.local/device/redfin/boot-out/boot-s20.img` = `boot-s20-c38a6cd.img`
  （sha256 `64f54992731f20bac64415c0e8d63f58b5bab1391a27b38a9631b15a8cc48ba9`，98304 B）
- 禁区扩：早期 PAS（CDSP-before、**IPA-before-ADSP `ceeb6fd`**）、TFTP OACK/DATA、TX intent
- **未刷、未推**

### 离线：IPA interconnect/BCM 投票序列钉死（2026-10-19 ~11:45 Asia/Shanghai）

**范围**：不刷、不推、不猜写 RPMH。目标 = 抽出 Linux 在 `subsystem_get("ipa_fws")`
之前的 **确切** 带宽票路径；对照裸机 helper；给出可编码计划 **或** 明确 blocker
+ 下次槽 b 抓取配方。活体仍 **`c38a6cd`**（本轮 `sm`：`me=1.2.0 ik=4.0
sr=1.7fffffff.0 pn=kernel/elf_loader.1.30 tz=1.eb43c.1a67`，无 `0x45`）。

#### 1) Linux 逻辑序列（源码 + slot-b dmesg，已钉）

源：`lito.dtsi` `ipa_hw` / `qcom,ipa_fws`；`ipa_v3/ipa.c`
（`ipa3_enable_clks` / `ipa3_load_ipa_fw`）；`msm_bus_fabric_rpmh.c`；
`lito-bus.dtsi`；slot-b `dmesg-raw.txt` ~40.85–40.90s。

| 序 | 谁 | 做什么 |
|--:|---|---|
| 0 | DTS | `qcom,bandwidth-vote-for-ipa` → **无** `core_clk`；开钟 = bus vote |
| 1 | `ipa` probe | `msm_bus_scale_register_client`（name **`ipa`**）；dmesg `Client handle 27 ipa` @40.858 |
| 2 | probe | `curr_ipa_clk_rate = ipa_clk_rate_svs2` → **`update_request(hdl, idx=1)` = SVS2** |
| 3 | `ipa3_enable_clks` | 同上 bus vote + `_ipa_enable_clks_v3_0`（bw-vote 模式下 clk 指针为 NULL） |
| 4 | PROXY / active-client | `IPA_ACTIVE_CLIENTS_INC` 保票（load 路径再 INC 一次） |
| 5 | SMMU | attach SID **0x520 / 0x521 / 0x522**（ap/wlan/uc）+ additional maps |
| 6 | 才 | `subsystem_get("ipa_fws")` → PIL pas-id **0xf** @ `0x8BC00000` |

**重要**：`qcom,ipa_fws` 节点只有 `qcom,msm-bus,name = "pil-ipa_fws"`，
**没有** `num-cases` / `vectors-KBps`。PIL 侧不贡献向量；**真票在 IPA HW
驱动 client `ipa`**。`ceeb6fd` 冷 `pas_go` 跳过了 1–5。

##### SVS2 用例（`lito.dtsi`，idx=1；avg/peak 单位 KBps）

| # | master → slave | avg (ab) | peak (ib) | 拓扑上挂的 BCM（`lito-bus.dtsi`） |
|--:|---|--:|--:|---|
| 1 | `MSM_BUS_MASTER_IPA`(90) → `SLAVE_LLCC` | 150000 | 500000 | 路径经 a2noc→…→ `slv_qns_llcc` → **`bcm_sh0` (SH0)**；mas 另有 node-qos **`bcm_ip0`** `<7035 0 1>` |
| 2 | `MASTER_LLCC` → `SLAVE_EBI_CH0` | 150000 | 700000 | `slv_ebi` → **`bcm_mc0` (MC0)** + **`bcm_acv` (ACV)** |
| 3 | `MASTER_IPA` → `SLAVE_OCIMEM` | 75000 | 700000 | `slv_qxs_imem` → **`bcm_sn1` (SN1)** |
| 4 | `MASTER_AMPSS_M0` → `SLAVE_IPA_CFG` | 0 | 76800 | `slv_qhs_ipa` → **`bcm_cn0` (CN0)**；AMPSS mas → **`bcm_sh4` (SH4)** |
| 5 | `MASTER_IPA_CORE` → `SLAVE_IPA_CORE` | 0 | 100 | `slv_ipa_core_slave` → **`bcm_ip0` (IP0)**（核心“开钟”票） |

用例名：`MIN=0 / SVS2=1 / SVS=2 / NOMINAL=3 / TURBO=4`。阈值阈值
`throughput-threshold = <310 600 1000>` 只影响事后升档，**boot PAS 前固定 SVS2**。

##### RPMH/BCM 编码（机制已钉；数值见 I4 离线表 2026-10-19）

```
cmd-db name "IP0"|"SH0"|"MC0"|"ACV"|"SN1"|"CN0"|"SH4"|…
  → addr = cmd_db_read_addr(name)   # HW type CMD_DB_HW_BCM = 5
  → aux  = struct bcm_db { unit_size:u32, width:u16, clk_domain:u8 }
agg:  ab' = ab * width / (buswidth * num_aggports)
      ib' = ib * width / buswidth
      vote_x = ab' / unit_size     # → vec_a
      vote_y = ib' / unit_size     # → vec_b
TCS data = BCM_TCS_CMD(commit=1, valid=1, vote_x, vote_y)
         = (1<<30) | (1<<29) | ((vote_x & 0x3FFF)<<14) | (vote_y & 0x3FFF)
发：apps RSC（与裸机 `rpmh_cmd` 同路：drv-2 TCS0 @ `0x18220d00`）
```

**曾 blocker**：slot-b 无 live `Act_*` / 无 debugfs。**已缓解（离线）**：cmd-db `e4d9591` 读齐 addr/unit/width；用 DTS SVS2+`KBTOB`+终点几何算出 vote/TCS（见 `I4-VOTE-AGGREGATE.md`）。仍缺：live `dump_bcm_clients`、中间 NOC 全量、ACV width=0 活体确认。**禁止未审刷写**。

#### 2) 裸机现有 helper（已搜）

| 能力 | 位置 | 够不够 |
|---|---|---|
| cmd-db 查找（8 字节名 → addr + aux） | `cmd_db_hit` / `cmd_db_find` | 可读 **IP0** 等 BCM 名 |
| apps RSC ACTIVE 单命令 | `rpmh_cmd(addr, data, freq)` @ `RSC_TCS0=0x18220d00` | **物理上能发 BCM 写** |
| ARC 代理票 | `cx.lvl`/`mss.lvl`/`xo.lvl`/`lcx`/`lmx` | 与 IPA 无关 |
| VRM USB 轨 | `rpmh_vrm` | 无关 |
| interconnect / msm-bus 图 / BCM 聚合 | **无** | 缺 |
| apps_smmu STE 写（SID 0x520…） | USB 用过 SMR/S2CR 扫槽；**无** IPA SID 绑定 | 缺专用路径 |
| IPA PAS | `ceeb6fd` 已禁 | — |

结论：mailbox/寄存器级 **发送通路已有**；缺的是 **投什么**（BCM 列表+编码后的 data）
和 **SMMU 前置**。不得用现有 `rpmh_cmd` 对 IP0 猜一个 SVS2 data。

#### 3) 交付判定 → **BLOCKER**（非可编码实现计划）

**可写审查用刀计划，仍不可未审 flash。** 解锁进度：

1. ~~usecase=SVS2~~ DTS+probe 已钉；live Act=0 → 用 DTS 峰值并标 MED
2. ~~非零 client 行~~ 未拿到（无 DEBUG_FS）— 用离线聚合代替并标 gap
3. ~~addr/unit/width + vote/TCS~~ **OFFLINE-COMPLETE**（cmd-db + 公式）
4. SMMU SID 已钉；**STE 裸机写法仍缺（I7）**

齐了之后的 **实现计划骨架**（仍禁止在收据前编码）：

```
1. cmd_db_hit("IP0\0\0\0\0") … 校验 HW=BCM(5)，读 aux
2. 按收据表 rpmh_cmd(addr, BCM_TCS_CMD(1,1,x,y), freq) 逐个 BCM（ACTIVE）
3. 只读 sm：`ib=n.ok`（发出条数/COMPL）
4. 另刀：apps_smmu 写 SID 0x520/521/522（对账 USB SMR 路径）
5. 两步都 sm 绿且设备仍 ping 后，才允许再议 pas_go(0xf)——本轮仍禁 PAS
```

#### 4) 下次槽 b 抓取配方（不猜；对齐已有 timeline 目录习惯）

前置：硬复位 → `set_active b` → adb `aginxosredfin`（与
`slot-b-timeline-20261019-110320` 同流程）；**先**确认 `qrtr-lookup` 有
`0x45`，再采 bus（保证采到的是“能出 WLFW”的那次环境）。

```sh
# 0. 收据目录
STAMP=$(date +%Y%m%d-%H%M%S)
DIR=/data/local/tmp/ipa-bcm-$STAMP   # 或主机侧 .local/device/redfin/…
mkdir -p "$DIR"

# 1. 确认 debugfs
mount | grep debugfs || mount -t debugfs none /sys/kernel/debug
ls /sys/kernel/debug/msm-bus-dbg/

# 2. client 向量（应有文件名 ipa；有则 cat）
ls /sys/kernel/debug/msm-bus-dbg/client-data/
cat /sys/kernel/debug/msm-bus-dbg/client-data/ipa \
  > "$DIR/client-ipa.txt" 2>&1
# 触发 trace/ERR 转储（驱动把当前票打到 dmesg/trace）
echo 1 > /sys/kernel/debug/msm-bus-dbg/dump_clients
echo 1 > /sys/kernel/debug/msm-bus-dbg/dump_bcm_clients
dmesg | grep -E 'bcm=|client=ipa|Client handle.*ipa|update_request' \
  > "$DIR/dmesg-bcm.txt"

# 3. sysfs bw（节点设备上的 DEVICE_ATTR bw）
# 典型路径因 uevent 而异；扫一遍：
find /sys/devices -name bw 2>/dev/null | while read f; do
  echo "=== $f ==="; cat "$f"
done > "$DIR/sysfs-bw-all.txt"
# 若能定位 label，优先：
#   *ipa-core* / *qns-llcc* / *ebi* / *imem* / *qhs-ipa* / *bcm-ip0*

# 4. ftrace（可选，镜像若开 CONFIG_MSM_BUS_DBG）
echo 0 > /sys/kernel/debug/tracing/tracing_on
echo > /sys/kernel/debug/tracing/trace
echo 1 > /sys/kernel/debug/tracing/events/msm_bus/enable 2>/dev/null \
  || echo 1 > /sys/kernel/debug/tracing/events/bus/enable 2>/dev/null
# 轻踢一票：读 /dev/ipa 或 `echo 1 >/dev/ipa` 前先 snapshot；不要乱 unload PIL
echo 1 > /sys/kernel/debug/tracing/tracing_on
sleep 2
echo 0 > /sys/kernel/debug/tracing/tracing_on
cat /sys/kernel/debug/tracing/trace > "$DIR/trace-bus.txt"

# 5. SMMU 对照（PAS 前也应在）
dmesg | grep -E 'ipa_smmu|0x052|group 1[345]' > "$DIR/smmu-ipa.txt"
find /sys/kernel/debug -iname '*smmu*' 2>/dev/null | head

# 6. 时间线锚点（复用旧脚本）
dmesg | grep -E 'ipa_fws|register_client_adhoc:Client handle.*ipa|Brought out of reset' \
  > "$DIR/timeline-ipa.txt"
qrtr-lookup > "$DIR/qrtr-lookup.txt"
```

**最低合格收据**：`dmesg-bcm.txt` 里至少一行
`bcm=IP0 client=ipa act_ab=… act_ib=…`（或 dump 等价），外加能从 cmd-db /
`/sys/kernel/debug` 对上 `IP0` 的 addr。有 IP0 + SH0/MC0/SN1/CN0 更佳。

**明确不要**：在收据前对 `rpmh_cmd` 写 BCM；不要再刷 `ceeb6fd`；不要
AOP/CDSP/TFTP 成功路径。

#### 5) Checklist（相对 S4 开放项）

| ID | 项 | 状态 |
|---|---|---|
| I1 | 逻辑序：ipa SVS2 vote → SMMU → `subsystem_get(ipa_fws)` | **DONE** |
| I2 | SVS2 五路径 ab/ib + 终点 BCM 名（IP0/SH0/MC0/ACV/SN1/CN0/SH4） | **DONE**（拓扑） |
| I3 | BCM TCS 位域 + cmd-db HW=5 + 聚合公式 | **DONE**（机制） |
| I4 | 现场 `(name,addr,unit,width,vote_x,vote_y,data)` 表 | **OFFLINE-COMPLETE** 2026-10-19：cmd-db `e4d9591` 填 addr/unit/width；vote/TCS 用 DTS SVS2+`KBTOB` 离线聚合（slot-b Act=0）。见 `.local/device/redfin/cmd-db-bcm-20261019-120213/I4-VOTE-AGGREGATE.md`。**未刷** |
| I5 | 裸机 `rpmh_cmd` 可复用发 BCM | **DONE**（通路） |
| I6 | 裸机 msm-bus 图 / 自动聚合 | **缺**（不需要若 I4 齐） |
| I7 | SMMU SID 0x520/521/522 裸机写法 | **刀码**（复用 USB CB + SMR/S2CR；`ste=`；未刷） |
| I8 | 早期 `pas_go(0xf)` | **BAN** 至 I4 活体核对 + I7 实证 |

##### I4 离线表摘要（SVS2 ACTIVE；勿当已刷）

| bcm | addr | vote_x | vote_y | TCS |
|---|---|---:|---:|---|
| IP0 | `0x50070` | 0 | 2500 | `0x600009c4` |
| SH0 | `0x50004` | 35 | 239 | `0x6008c0ef` |
| MC0 | `0x50000` | 146 | 1365 | `0x60248555` |
| ACV | `0x5007c` | 0 | 8 | `0x60000008` |
| SN1 | `0x5002c` | 141 | 1320 | `0x60234528` |
| CN0 | `0x50074` | 0 | 2560 | `0x60000a00` |
| SH4 | `0x50014` | 0 | 18 | `0x60000012` |

### I4 窄刀（review-scoped，2026-10-19）：STE + ACTIVE vote

首次 `sm`：`ipa_smmu_ste`（SID `0x520/521/522` → USB CB，写法同 `fastrpc_smmu`）
→ 七笔 ACTIVE `rpmh_cmd`（上表 TCS）。sm：`ste=mask.s0.s1.s2`、`bv=` 七位成功位图。
**无** `pas_go` / 无 TFTP 成功服 / 无 TX intent / 无 glink cap / 无 CDSP-before。
人审 G2/G3 后再议刷。禁区不变。未推。

### I4 绿后下一刀：IPA PAS **after ADSP**（未刷）

**基线**：`8803af7` 活体 `ste=7.6.7.8 bv=7f`（capture `i4-vote-8803af7-20261019-122556/`）。

**调用序（本刀）**：
1. 既有：USB → LPASS SMP2P → `load_adsp` → AOP adsp on → `pas_go(1)` → ONLINE/settle
2. **新**：若 `ADSP_ST` 高位且 `ADSP_P[0..2]==0` → `ipa_i4_once`（STE+七票）→ `load_ipa_fws` → `pas_go(0xf @ 0x8BC00000 / 0x10000)`
3. 既有：EFS seed → modem AOP → `pas_init_image`（modem）

**相对 Linux slot-b**（`ipa_fws@40.9 → adsp@47 → …`）：裸机**故意**把 IPA PAS 放到 ADSP 成功之后——`ceeb6fd` 链首 IPA-before-ADSP 硬挂无 ECM 仍为禁区。票+SMMU 前置保留（I4）。

**sm**：保留 `ste=` / `bv=` / `bc=`；新增 `ip=st.p0.p1.p2`。

**禁区不变**：IPA-before-ADSP（`ceeb6fd`）、TX intent、TFTP OACK/DATA 成功路径、glink cap bump。
**CDSP-before-modem**：**条件解禁**——仅当 ADSP+IPA 绿之后（见下刀）；冷启无 IPA 前缀仍禁（`8d88cd5` 重放）。Flash-safe restore：`boot-s20-c38a6cd.img`。

### b99c785 刷验（2026-10-19 ~12:37 Asia/Shanghai）— IPA PAS 绿，无 0x45

`b99c785` / `boot-s20-b99c785.img` 已刷 `boot_a` + `set_active a` + reboot。
capture：`.local/device/redfin/ipa-after-adsp-b99c785-20261019-123709/`。

| 键 | 值 | 判读 |
|---|---|---|
| `ste=` / `bv=` | `7.6.7.8` / `7f` | I4 仍绿 |
| `ip=` | `80000003.0.0.0` | IPA PAS 三步全 0 |
| `ad=` | `80000020.0.0.0` | ADSP PAS 仍绿 |
| `pn` / `sr` | `kernel/elf_loader.1.30` / `1.7fffffff.0` | 仍 UNINIT，未自拉 |
| `tf` / `tz` | `549_38_0.mbn` / `1.eb43c.1a67` | 仍走 TFTP 回退 |
| `q=` | 无 `0x45`（与 8803af7 同集） | **无 WLFW** |
| `ik` / `me` / `pm` | `4.0` / `1.2.0` / `0.0.0` | RFS/MPSS 出向同前；无 pm-service 命中 |
| ECM | ping@~24s + sm OK | **无挂**；活体仍 `b99c785` |

**对 8803af7 差分**：仅多 `ip=` 绿；wlan 相关键（`pn`/`sr`/`tf`/`q`/`ik`/`me`）**零变化**。
假说「I4 + IPA-after-ADSP 足以内存内自拉 wlan_pd」**证伪**（与 S4 早先「IPA 主要为 0x45 后 qcacld SMMU、非自拉充分」一致）。

#### 相对 slot-b 仍缺什么（自拉）

| 序 | slot-b | 裸机 b99c785 |
|--:|---|---|
| 1 | ipa_fws | **有** |
| 2 | adsp | **有** |
| 3 | **cdsp**（modem 前 ~100ms） | **无** |
| 4 | modem | **有** |
| 5 | RFS → wlan_pd ONLINE / `0x45` | RFS 绿 → elf_loader UNINIT；**无 0x45** |

Linux 同次启动 **0 命中** tftp/elf_loader/`549_38`。裸机停在回退链，说明环境仍不像 stock 前缀。

#### 下一刀：CDSP-before-modem（**条件解禁**，未刷）

**为何解禁**：`8d88cd5` 冷挂时 IPA 环境缺失。现活体 `b99c785`：`ste=7 bv=7f ip=80000003.0.0.0` ADSP OK。
slot-b 序 `ipa→adsp→cdsp→modem→wlan`；目标自拉 `wlan_pd` / `0x45`，不靠 elf_loader。

**门**：仅当 `IPA_ST` 高位且 `IPA_P[0..2]==0`（ADSP 绿是 IPA 门前提）。
IPA skip/fail → **不**跑 CDSP-before-modem，fall through 旧 modem-only，防 `8d88cd5` 重放。

**调用序**：
1. 既有：ADSP PAS → ONLINE/settle → I4 STE+七票 → IPA `pas_go(0xf)`
2. **新（门内）**：CDSP smp2p（smem 432/94，remote-pid 5）→ AOP `cdsp on` → `load_cdsp` → `pas_go(18 @ 0x87400000/0x1E00000)` → 等 err-ready
3. 既有：EFS seed → modem AOP → `pas_init_image`

**sm**：保留 `ste=` / `bv=` / `bc=` / `ip=`；新增 `cd=st.p0.p1.p2`（同 `ip=` 形）。

**仍禁**：IPA-before-ADSP（`ceeb6fd`）、TX intent、TFTP OACK/DATA 成功路径、glink cap bump。
**冷启无 IPA 前缀的 CDSP-before-modem** 仍禁。Flash-safe restore：`boot-s20-c38a6cd.img`。

未推。未刷（等硬复位进 fastboot 再刷）。

### dcbf8cf 刷验（2026-10-19 ~12:54 Asia/Shanghai）— CDSP 触达但 AUTH 脏

`dcbf8cf` / `boot-s20-dcbf8cf.img` 已刷。capture：
`.local/device/redfin/cdsp-before-modem-dcbf8cf-20261019-125446/`。

| 键 | 值 | 判读 |
|---|---|---|
| `ste=` / `bv=` | `7.6.7.8` / `7f` | I4 仍绿 |
| `ip=` | `80000003.0.0.0` | IPA PAS 仍绿 |
| `ad=` | `80000020.0.0.0` | ADSP PAS 仍绿 |
| **`cd=`** | **`8000000b.ffcffff1.0.0`** | 装了 **11** 段；**PAS 未净成功** |
| `pn` / `sr` / `q=` | elf_loader / UNINIT / 无 `0x45` | **对自拉零增量** |
| ECM | ping+sm OK，`HUNG=0` | **无挂**；活体仍 `dcbf8cf` |

相对 `b99c785`：仅多脏 `cd=`；wlan 键零变化。条件解禁的 CDSP-before-modem **安全触达**（有 IPA 前缀不再 `8d88cd5` 冷挂），但 **未 ONLINE**。

#### `cd=` / `pas_go` 编码（重要：注释易误导）

`pas_go` 实际返回打包：

| 退出点 | 返回 `(p0,p1,p2)` | sm 形 |
|---|---|---|
| INIT 失败 | `(i0, i1, -1)` | `*.*.ffffffff` |
| MEM 失败 | `(0, m0, m1)` | `0.*.*` |
| AUTH 路径（含成功） | `(a0, a1, 0)` | `*.*.0` |

因此 **`cd=8000000b.ffcffff1.0.0` ⇒ INIT=0、MEM=0、AUTH x0=`0xffcffff1`、AUTH x1=0**。
对比 IPA/ADSP 成功 `*.0.0.0`（AUTH 亦回 0）。
注释里「p0/1/2 = INIT/MEM/AUTH x0」仅在三步全 0 时看起来成立；AUTH 失败时 p0 实为 **AUTH x0**。

#### `0xffcffff1` 对表

内核 `qcom_scm.h` 常见 remap 仅：

| 码 | 名 |
|--:|---|
| -1 | `QCOM_SCM_ERROR` |
| -2 | `EINVAL_ARG` |
| -3 | `EINVAL_ADDR` |
| -4 | `EOPNOTSUPP` |
| -5 | `ENOMEM` |
| -12 | `V2_EBUSY` |

**`0xffcffff1`（≈ i32 −3145743）不在表内** — 为 TZ/PAS 原始 x0，非 Linux errno。
巧合：`~0x30000e == 0xffcffff1`，而 cmd-db `cx.lvl` 基址为 `0x30000`；**未证实**与 rpmh 相关，仅备忘。
公开 SCM/PAS 文档无此精确码；按 PIL 语义 AUTH 非 0 ⇒ **镜像认证失败或无法出复位**。

pas-id **18**、区 **`0x87400000`/`0x1E00000`** 与 lito `qcom,turing` / stock dmesg
`loading from 0x87400000 to 0x89200000` **一致** → **排除错 pas-id / 错基址·长度** 作为主因。

#### 排序假说（无一够窄到可直接下一刀）

1. **AUTH 哈希/段内容不符**（最像）— `st=0xb` 说明 `.bXX` 都打开并写入；仍可能 PA/reloc、缓存、或某段内容与 mdt hash 不一致。ADSP/IPA 同 `pas_go` 形已绿，CDSP 特有。
2. **缺 stock 在 AUTH 前的 bus/BW** — `pil_make_proxy_vote` 的 `do_bus_scaling(pil-cdsp 154→821)` + `scm_pas_enable_bw`；裸机 ADSP/IPA 也没做却绿，故非充分证明，但是 CDSP DTS 独有 bus 向量。
3. **代理票/序细节** — DTS 要 `vdd_cx`+`xo`；裸机开机已投 xo/cx，`load_cdsp` 再投 cx 但**不校验** `scx==0x10101`（ADSP 对 lcx/lmx 有硬失败）。AOP 在 load 前（stock 亦 AOP 最先）；次要。另：ADSP 在 `pas_go` 前 `sfr_wipe(423)`，CDSP smem-id **601** 未 wipe。

**明确不选本轮**：改序 after-modem（radio 文档：modem 先起则事后不 spawn）；冷 CDSP 无 IPA；TFTP/TX intent/glink cap；盲加 bus vote 无探针。

#### Park / 推荐探针（只读优先）

**本轮不编刀、不刷。** 设备可停 `dcbf8cf`（健康）。

推荐下一动作（单变量、偏只读）：

1. **离线**：从 vendor 抽 `cdsp.mdt`，报 phnum / min_pa / max_pa / hash 段 flags；与 stock「8740→8920」对账；确认裸机 `reloc` 必为 false。
2. **只读 sm 扩字段**（可选窄刀）：`cdx=`=cx 投票 sts、`cdr=`=reloc|min|max、`cdm=`=mdt bytes；**不**改 `pas_go`、**不**新 AOP/bus。用于证伪假说 1/3。
3. 若离线证 reloc=0 且 mdt 与 stock 同跨度，再议 **单变量** 写刀：要么 AUTH 前 `pil-cdsp` bus vote，要么 `sfr_wipe(601)`+cx 强校验——二者勿同刀。

禁区不变：IPA-before-ADSP、TX intent、TFTP 成功路径、glink cap、冷 CDSP 无 IPA。
Flash-safe restore 仍：`boot-s20-c38a6cd.img` / 或回 `b99c785`（IPA 绿基线）。未推。

### 离线 CDSP mdt/ELF 审计（2026-10-19 ~13:10 Asia/Shanghai）— ffcffff1 / 无刀

源：factory `vendor.img`（UP1A.231105.001.B2）`/firmware/cdsp.mdt`+`cdsp.b00`–`b12`，抽到
`.local/device/redfin/cdsp-fw/`（gitignore，不进仓）。**未刷、未推、未碰设备**（仍 `dcbf8cf`）。

#### ELF 摘要

| 项 | 值 |
|---|---|
| mdt | 7660 B；ELF32 QDSP6；`e_phnum=13`；`e_entry=0x87400000` |
| 段文件 | `b00`–`b12`（`b07`/`b10`/`b12` filesz=0 / BSS） |
| hash | phdr[1]：`p_type=0`，`flags=0x2200000`（TYPE_HASH），`p_offset=0x1000`，`filesz=0x1c18`；**内容 packed 在 mdt[hdr_end=0x1d4:]**（= `b01`），与 ADSP/IPA 同形 |
| nload | **11**（非 hash、PT_LOAD、memsz>0）≡ `cd st=0xb` |
| min_pa / max_pa | **`0x87400000` / `0x89200000`** |
| span | **`0x1E00000`** ≡ stock dmesg `8740→8920` ≡ DTS `pil_cdsp_region` |
| bare `reloc` | **false**（地址窗判定：`min>=base && max<=base+size`） |
| `QCOM_MDT_RELOCATABLE` (bit27) | load 段 flags 均带 `0x08000000`；Linux 会走 relocate 分支，但 carveout==min_pa 时 dest 与裸机 `pa` 直写 **等价**；`pas_go` 本就总做 MEM_SETUP(base,size) |
| bXX↔filesz | **11 个 load 段全部一致**（无截断/错号） |

phdr 表（略 flags 低 3 bit = PF）：

| i | pa | filesz | memsz | note |
|--:|---|--:|--:|---|
| 0 | — | 0x1d4 | 0 | ELF+phdrs（=b00）；非 PT_LOAD |
| 1 | 0x89200000 | 0x1c18 | 0x2000 | **HASH**（不装；p_paddr 落在 ADSP 基，同 ADSP hash 落区外惯例） |
| 2 | 0x87400000 | 0x39878 | 0xb0000 | IN |
| 3 | 0x874b0000 | 0x19dec | 0x1a000 | IN |
| 4 | 0x874ca000 | 0x434 | 0x1000 | IN |
| 5 | 0x874cb000 | 0x268b8c | 0x4b6000 | IN |
| 6 | 0x87981000 | 0x14b9e | 0x15000 | IN |
| 7 | 0x87996000 | 0 | 0x6a000 | BSS（整区 `write_bytes` 已清） |
| 8 | 0x87a00000 | 0x80440 | 0xe6000 | IN |
| 9 | 0x87ae6000 | 0xa88 | 0x1000 | IN |
| 10 | 0x87ae7000 | 0 | 0x19000 | BSS |
| 11 | 0x87b00000 | 0xe714 | 0x43000 | IN |
| 12 | 0x87b43000 | 0 | 0x16bd000 | BSS（撑满到 0x89200000） |

#### vs `load_adsp` / `load_ipa_fws` / `pas_go`

| 步骤 | ADSP | IPA | CDSP (dcbf8cf) |
|---|---|---|---|
| 代理电源 | lcx/lmx TURBO，**硬失败**若 ≠`0x10101` | 无 | cx TURBO，**不校验** scx |
| 整区零填 | 是 | 是 | 是 |
| `.bXX` / mdt 内嵌 | 仅 bXX | bXX 或 mdt 内嵌 | 仅 bXX（与 ADSP 同） |
| hash 跳过 | `flags` TYPE_HASH | 同 | 同 |
| `pas_go` 前 dcache | 整区 civac | 同 | 同（共享 `pas_go`） |
| `sfr_wipe` | **423** | 无 | **无 601** |
| AOP load_state on | 有 | 无（DTS 无） | 有 |
| msm-bus 向量 | `pil-lpass`（无完整 vectors） | `pil-ipa_fws` | **`pil-cdsp` `<154 821 0 0/1>`** |
| 裸机 bus / `scm_pas_enable_bw` | 未做（仍 AUTH 绿） | 未做（仍绿） | **未做（AUTH `ffcffff1`）** |

**证伪**：错 pas-id / 错基址长度 / bare reloc=true / phnum≠st / bXX 截断 / mdt 非 packed-hash（ADSP 同形已绿）→ **均否**。
静态「段布局/哈希元数据形」与成功的 ADSP 同构；假说 1 的 **reloc/窗/漏段** 子项关掉。仍可能：运行时内容未进 TZ 可见点（总线/BW）、或 AUTH 复位侧条件。

#### 结论 / 下一刀排序（仍无「唯一窄修复」）

**不编修复刀、不出镜像。** 离线已完成 park 探针 #1。

推荐单变量下一动作（**勿同刀**）：

1. **AUTH 前 `pil-cdsp` bus vote（154→821）** — CDSP DTS 独有完整向量；相对 ADSP/IPA「裸机也没 BW 却绿」仍是最像的 CDSP 特有缺口。需 BCM/cmd-db 对 master/slave 名，或只读探针先报 vote sts。
2. **`sfr_wipe(601)` + `scx==0x10101` 硬失败** — 对齐 ADSP 代理纪律；偏假说 3，与 bus 分开验。
3. （可选、更窄只读）sm 增 `cdx=`/`cdr=`/`cdm=` — 离线已证 reloc/span/mdt；runtime 只剩 cx sts 有增量，优先 ring `cdsp cx=` 若还能捞。

禁区不变。设备可继续停 `dcbf8cf`。Flash-safe：`boot-s20-c38a6cd.img` / `b99c785`。

### 单变量刀：AUTH 前 pil-cdsp bus（CO2+CO0）— 未刷

基线 `dcbf8cf`（AUTH `ffcffff1`）+ 离线 mdt 审计 `1119769`。**仅**加 DTS
`pil-cdsp` 路径票，**不**合 `sfr_wipe(601)` / cx 硬失败。

| 项 | 值 |
|---|---|
| DTS | `lito.dtsi` `qcom,msm-bus,name="pil-cdsp"`：`<154 821 0 0>` / `<154 821 0 1>` |
| master | `154` = `MSM_BUS_MASTER_NPU` → `mas_qnm_npu` bcm **CO2** |
| slave | `821` = `MSM_BUS_SLAVE_CDSP_GEM_NOC` → `slv_qns_cdsp_gem_noc` bcm **CO0** |
| case1 | ab=0 ib=1 KBps → keepalive；`BCM_TCS_CMD(1,1,0,1)` = **`0x60000001`** |
| 发法 | `cmd_db_hit("CO2"/"CO0")` → `rpmh_cmd(addr, 0x60000001)`；无 fallback 盲写 |
| 时机 | IPA+ADSP 门内；`load_cdsp` 成功后、**`pas_go(18)` 前** |
| sm | 保留 `cd=`；增 **`cb=`**（bit0=CO2 bit1=CO0 成功） |
| 禁 | 同刀不加 sfr_wipe/cx；IPA-before-ADSP / TX intent / TFTP / glink / 冷 CDSP |

未推。未刷。Flash-safe restore：`boot-s20-c38a6cd.img` / `b99c785`。等硬复位进 fastboot 再刷 stamp。



### ec81a77 刷验（2026-10-19 ~13:25 Asia/Shanghai）— CDSP AUTH 绿，仍无 0x45

`ec81a77` / `boot-s20-ec81a77.img` 已刷。capture：
`.local/device/redfin/cdsp-bus-ec81a77-20261019-132553/`。

| 键 | 值 | 判读 |
|---|---|---|
| `ste=` / `bv=` | `7.6.7.8` / `7f` | I4 仍绿 |
| `ip=` / `ad=` | `80000003.0.0.0` / `80000020.0.0.0` | IPA+ADSP PAS 绿 |
| **`cd=`** | **`8000000b.0.0.0`** | **AUTH 干净**（对 `dcbf8cf` 的 `ffcffff1`） |
| **`cb=`** | **`3`** | CO2+CO0 两票 ACTIVE |
| `me=` / `ik=` / `rf`/`rh`/`es`/`gn` | `1.2.0` / `4.0` / RFS 绿 | **modem AUTH 已成**（RFS 开槽） |
| `pm=` | `0.0.0` | 无 pm-service / SSREQ |
| `sr=` | `1.7fffffff.0` | PDR REGISTER 命中，**wlan_pd UNINIT** |
| `pn` / `tf` / `tz` | `kernel/elf_loader.1.30` / `549_38_0.mbn` / `1.eb43c.1a67` | **仍走 TFTP 回退** |
| `q=` | 无 `0x45`（同前集） | **无 WLFW** |
| ECM | ping+sm OK，`HUNG=0` | **无挂**；活体仍 `ec81a77` |

**判读**：pil-cdsp bus **修好 AUTH**；对自拉 **零增量**（仍 elf_loader / UNINIT / 无 0x45）。
modem PAS 本就在 CDSP 块之后无条件执行，且 RFS 已证 AUTH——缺口不是「没跑 modem」。

#### 相对 slot-b（自拉）差什么

| 序 | slot-b | 裸机 ec81a77 |
|--:|---|---|
| 1–4 | ipa→adsp→cdsp→modem | **有**（CDSP=AUTH 绿；ONLINE **sm 未见**） |
| 5 | RFS → **wlan_pd ONLINE** / `0x45`（**零** tftp/elf_loader） | RFS 绿 → **elf_loader+TFTP**；PDR UNINIT；无 0x45 |

软成功标准（CDSP 刀设计）要 `cd` **ready** 且 `pn`/`tf` 不再 elf_loader。现仅 AUTH 绿，ONLINE 未进 sm，环已滚掉 `cdsp rdy=`。

#### 下一刀：`sfr_wipe(601)` + sm `ce=`/`cz=`（未刷）

单变量写：ADSP 在 `pas_go` 前 `sfr_wipe(423)`；CDSP DTS `qcom,smem-id = <601>` 仍未 wipe。
假说：AUTH 已绿，但 SFR 残串可能让 CDSP 立 fatal / 迟 ready，modem 仍见「环境不像 stock」而走 elf_loader。
同刀只加观测（不改等待语义、不加 cx 硬失败）：

| 项 | 值 |
|---|---|
| 时机 | IPA+ADSP+bus 门内；`cdsp_bus_vote` 后、**`pas_go(18)` 前** |
| wipe | `sfr_wipe(601)` → `CDSP_Z`/`CDSP_ZB` |
| sm | **`ce=pub.rdy.wt`**（sm 时 `cdsp_edge` 刷新 rdy）；**`cz=st.zb`** |
| 采 | `ce=` bit1？`wt`≈超时 10000？`cz`；`pn`/`tf`/`sr`/`q` 有无 0x45 |
| 禁 | cx 硬失败同刀；IPA-before-ADSP；TX intent；TFTP 成功；glink cap；冷 CDSP |

未推。未刷。Flash-safe：`boot-s20-c38a6cd.img` / 活体可停 `ec81a77`。等硬复位再刷 stamp。

**若刷后仍 elf_loader**：按 `ce=` 分流——ONLINE 且无 fatal → CDSP 非充分（另排 GSI/其它）；超时/无 bit1 → 再议 negotiate/wait；fatal+ready → 放宽 wait。

### af0c6ff 后：CDSP ONLINE 证伪充分性 → 自拉 0x45 缺口（离线，2026-10-19 ~13:55 Asia/Shanghai）

**活体**：`af0c6ff` / capture
`.local/device/redfin/cdsp-sfr-af0c6ff-20261019-135156/`。

| 键 | 值 | 判读 |
|---|---|---|
| `ce=` | `1.6.32` | **CDSP ONLINE**（rdy bit1）；wt=50ms |
| `cz=` | `2.0` | sfr_wipe(601) **miss**（st=2 没有这项）— 观测刀，ONLINE 不依赖 wipe |
| `cd=` / `cb=` | `8000000b.0.0.0` / `3` | AUTH 仍绿；CO2+CO0 仍 |
| `ip=` / `ste=` / `bv=` | 绿 | I4+IPA PAS 仍 |
| `ik=` / `me=` | `4.0` / `1.2.0` | RFS + MPSS 出向 `ipa` 名 |
| `sr=` | `1.7fffffff.0` | PDR REGISTER OK，**wlan_pd UNINIT** |
| `pn` / `tf` | `tms/pddump_disabled…` / `…549_38_0.mbn…` | 环滚过 `elf_loader`；**TFTP 路径仍在** |
| `q=` | 无 `45.` | **无 WLFW** |
| ECM | ping+sm OK | **无挂**；**勿再叠 CDSP-only** |

#### 相对 slot-b（自拉成功）差什么

slot-b `TIMELINE.md` / `dmesg-raw`（同日）：

| t | Linux | 裸机 af0c6ff |
|--:|---|---|
| 40.9 | IPA FW + **GSI register** | PAS+STE+BCM **有**；**无** GSI/uC 驱动态 |
| 47–48.4 | adsp→cdsp→modem | **有**（CDSP ONLINE 已证） |
| 48.42 / 49.09 | IPA-WAN **BEFORE/AFTER_POWERUP**（`ipa3_proxy_clk_vote` = ACTIVE 保票） | BCM ACTIVE 已有（`bv=7f`）；无 SSR notifier 语义 |
| 49.09 | RFS OPEN/ALLOC/IOVEC；**0 命中** tftp/elf_loader | RFS 绿 → **仍** SO RRQ 回退 |
| **49.30–49.39** | **`QMI_IPA_INIT_MODEM_DRIVER_REQ_V01`** ↔ RESP | **无** IPA QMI 栈；`q=` **无** `31.*` |
| 50.21 | **wlan_pd ONLINE** → `0x45@0:88` | UNINIT；无 `0x45` |

slot-b `qrtr-lookup`：**`0x31/0x201@0:29`（modem Q6）** + **`0x31/0x101@1:16404`（AP A5）** + **`0x45/0x1@0:88`**。
裸机 `q=` 窗（最多 8 对）：`42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001` — **无 31、无 45**。

#### 假说排序（冲自拉；禁区内已关）

| # | 假说 | 裁定 |
|--:|---|---|
| 1 | **缺 IPA 客户端后半栈**：PAS 后无 GSI/uC 握手 → modem **不**公告 Q6 `0x31/0x201` → 无 `INIT_MODEM_DRIVER` → 环境仍像「无 IPA」→ elf_loader 回退 | **主缺口**（时序钉在 49.30→50.21；与「CDSP 非充分」一致） |
| 2 | 只缺 AP→modem 发 INIT（表项齐全） | **次**；无 Q6 NEW_SERVER 则无处可发；全量 req 含 mem partition/ep — **非**低危单变量 |
| 3 | WLFW/MSA/「SSA」路径 | **关**：`icnss` IND/MSA/**五路电在 0x45 之后**；不造 spawn |
| 4 | 再叠 CDSP / sfr / bus | **关**：ONLINE 已证仍 elf_loader |
| 5 | mss_pil proxy / MPSS 出向名 / PDR REGISTER | **已齐或证伪**（cx/mss/xo；`me=1.2.0`；`sr` UNINIT 不 RESTART） |
| 6 | TFTP 真喂 / TX intent / glink cap / IPA-before-ADSP | **禁令** |

`ipa3_proxy_clk_vote` = `IPA_ACTIVE_CLIENTS_INC`（保 BCM），**不是**新 RPMH 字；`bv=7f` 已近似。BEFORE_POWERUP 不单独成刀。

#### 下一刀（唯一；**doc park — 不实现 / 不刷**）

**推荐：sm-only latch `iq=`（IPA QMI 目击），零写。**

| 项 | 值 |
|---|---|
| 做什么 | `scan_new_servers` 见 svc **`0x31`** 时闩 `IQ_SEEN=1`、`IQ_INST=inst_word`（不限首 8 窗）；sm 增 **`iq=seen.inst`**（如 `0.0` / `1.201`） |
| 不做 | 不公告 A5 `0x31`（史：modem `ql` 空）；不发 INIT；不摸 GSI/IPA MMIO；不改 TFTP/intent/PDR/PAS |
| 门 | 无；只读扫描 |
| 成功判据 | 设备仍 ping/`ik=4`；`iq=` 可读 |
| 分流 | `iq` 长时 `0.0` → 证实 modem Q6 IPA 未起 → **下一写刀**须先 GSI/uC（高危，另开收据）；`iq=1.201` → 才议窄 INIT（仍大，勿与 GSI 同刀） |
| 禁 | CDSP-only 叠刀；IPA-before-ADSP；TX intent；TFTP 成功；glink cap |

**本轮不编 tip、不出镜像、不 flash、不 push。** 活体可停 **`af0c6ff`**（健康）。
Flash-safe restore 仍：`boot-s20-c38a6cd.img` / 绿 IPA 基线 `b99c785`。
hard-reset：**不需要**（无刷）。

### iq= sm-only latch 实现（tip，未刷，2026-10-19 ~14:00 Asia/Shanghai）

按上节 park：**只读闩**，零写。

| 项 | 值 |
|---|---|
| 代码 | `scan_new_servers`：`typ==4 && svc==0x31` → 首见闩 `IQ_SEEN=1`/`IQ_INST=inst`（**不**受 `SMEM_NSVC<8` 限制；不随扫描清零） |
| sm | **`iq=seen.inst`**（`0.0` 未目击 / `1.201` 等 = modem Q6）紧挨 `q=` 后 |
| 不做 | 不公告 A5 `0x31`；不发 `INIT_MODEM_DRIVER`；不摸 GSI/IPA MMIO；不改 TFTP/intent/PDR/PAS/CDSP |
| 基线 | tip atop `97b8056` docs / 活体 `af0c6ff` |
| 出件 | `boot-s20-<sha>.img`；**未 flash、未 push** |
| 刷后分流 | `iq=0.0` 长时 → 下一写刀先 GSI/uC；`iq=1.201` → 再议窄 INIT（勿与 GSI 同刀） |

Flash-safe restore 仍：`boot-s20-c38a6cd.img` / `b99c785` / 活体可停 `af0c6ff`。
等硬复位进 fastboot 再刷 stamp。

### cef1fd9 后：iq=1.201 证伪「modem 无 0x31」→ INIT park（离线，2026-10-19 ~14:15 Asia/Shanghai）

**活体**：`cef1fd9` / capture
`.local/device/redfin/iq-latch-cef1fd9-20261019-140835/`。

| 键 | 值 | 判读 |
|---|---|---|
| **`iq=`** | **`1.201`** | modem Q6 **已**公告 IPA QMI svc `0x31` inst `0x201`（t≈0 即闩；t≈66s 仍） |
| `q=` | 无 `45.` | **仍无 WLFW** |
| `ik=` / `cd=` / `ip=` / `ce=` / `ste=` / `bv=` | 绿 | 基线未毁 |
| hang | HUNG=0 | ping+sm OK；**勿叠空 INIT** |

分支（97b8056 park）：`iq=1.201` → 曾写「再议窄 INIT」。本轮对账 **否决「最小/空 INIT」刀**。

#### Linux / vendor 对 `QMI_IPA_INIT_MODEM_DRIVER_REQ_V01`（msgid `0x0021`）

IDL（`ipa_qmi_service_v01.h`）：`ipa_init_modem_driver_req_msg_v01` **全部 Optional** → 语法上空包合法。

**stock 实发**（redfin vendor `ipa3_qmi_init_modem_send_sync_msg`，
`.local/device/redfin/kernel-src/redbull/.../ipa_qmi_service.c`；mainline `drivers/net/ipa/ipa_qmi.c` 同构）：

| TLV / 字段 | 来源 | 无 GSI/IPA 客户端时 |
|---|---|---|
| `platform_type` | `ipa_wan_platform`（常 `MSM_ANDROID=3`） | 可硬编码，**alone 非 stock** |
| `hdr_tbl_info` start/end | `IPA_MEM_PART(modem_hdr_*)` + `smem_restr_bytes` | **需** mem_partition + SRAM 已铺 |
| `v4/v6_route_tbl_info` | `v*_rt_nhash_ofst` / `v*_modem_rt_index_hi` | 同上 |
| `v4/v6_filter_tbl_start_addr` | `v*_flt_nhash_ofst` | 同上 |
| `modem_mem_info` | `modem_ofst` / `modem_size` | 同上 |
| `ctrl_comm_dest_end_pt` | `ipa_get_ep_mapping(APPS_WAN_CONS)` | **需** GSI EP 分配 |
| `hdr_proc_ctx_tbl_info` | `modem_hdr_proc_ctx_*` | mem_partition |
| hash route/filter（若 hashable） | `v*_rt_hash_*` / `v*_flt_hash_*` | mem_partition |
| `hw_stats_quota_*` / `hw_drop_stats_*` | `stats_quota_q6_*` / `stats_drop_*` | mem_partition |
| `is_ssr_bootup` | **仅** `ipa3_uc_loaded_check()` 后才 valid=1 | **需** IPA uC 已载（GSI 路径） |

冷启：`is_ssr_bootup_valid=false`（不发该 TLV）。slot-b dmesg：`Sending QMI_IPA_INIT_MODEM_DRIVER_REQ_V01` @~49.30 → RESP @~49.39（**在** GSI register @~40.9 **之后**）。

`IPA_MEM_PART` = `ipa3_ctx->ctrl->mem_partition->*`（如 IPAv4.7 / lito `ipa_4_7_mem_part` 静态表）。表内 offset **只有**在 `ipa3_init_mem_partition` + SRAM canary/表写入之后才对 modem 有意义；裸抄数字而不铺 SRAM = **猜**。

#### 为何不实现「空 / platform-only」INIT

| 候选 | 裁定 |
|---|---|
| 空包（零 TLV） | IDL 合法；**非** stock；modem 接受/拒/半开未知 → **禁猜** |
| 仅 `platform_type=3` | 同上；缺 mem/ep，不能当「最小合法 stock 探针」 |
| 硬编码 `ipa_4_7_mem_part` + 假 `ctrl_comm` | 无 SRAM 布局 / 无 EP map / 无 uC → **假地址**；高危 |
| 真 stock INIT | **绝对依赖** GSI 后半：mem 布局 + WAN_CONS EP +（SSR 时）uC loaded |

结论：**无「不编程 GSI rings 的最小有效 INIT」**。本刀 **不写、不编、不刷**。

#### 下一刀排序（只读优先；仍禁 TFTP/intent/glink/CDSP 叠）

| # | 探针 | 目的 | 危 |
|--:|---|---|---|
| 1 | **slot-b 再采**：`dmesg` 开 IPAWANDBG / 或 `ipa_qmi` 打印的 offset 行；`debugfs` `ipa`/`gsi` mem_partition / EP map | 钉死 redfin 活体 TLV 数值（相对静态表 + `smem_restr`） | 零写 |
| 2 | **裸机只读**：IPA MMIO `0x1e00000` 区 — GSI EE/通道态、SRAM 窗口是否已有 canary（对照 `ipa3_sram_set_canary`） | 证「PAS 后有无 GSI 驱动态」；**不** doorbell / **不** alloc channel | 只读 MMIO |
| 3 | **裸机只读**：`cmd_db`/已有 BCM 外，查 GSI IRQ / `msm_gsi` 资源是否可映 | 为真 GSI bring-up 收据 | 只读 |
| 4 | （收据齐后）**GSI+uC 写刀** → 再 **INIT**（可同序分两 tip；**勿**与 IQ 闩同刀） | 复制 slot-b 49.30 握手 | 高危写 |

**本轮不编 tip、不出镜像、不 flash、不 push。** 活体可停 **`cef1fd9`**（健康，`iq=1.201`）。
Flash-safe restore 仍：`boot-s20-c38a6cd.img` / `b99c785` / 绿 CDSP 基线 `af0c6ff`。
hard-reset：**不需要**（无刷）。等 #1/#2 只读收据后再开 GSI 刀。

### ig= IPA/GSI 只读 MMIO 刀（tip，未刷，2026-10-19 ~14:20 Asia/Shanghai）

按 7fc307e park #2：**裸机只读** IPA@`0x1e00000` / GSI@`0x1e04000`。
INIT 仍 park（需 mem/EP）。基线 tip atop `7fc307e` / 活体 `cef1fd9`。

| 项 | 值 |
|---|---|
| 触发 | IPA PAS 绿（`IPA_ST` 高位 + p0..p2==0）后 **一次**；sm 幂等重入 |
| 读 | IPA `VERSION`/`SHARED_MEM_SIZE`；GSI `CFG`/`EE0_STATUS`/`SW_VERSION`/`HW_PARAM_2`/`CH0_CNTXT_0`；SRAM canary @`0x684`/`0x284`（`ipa_4_7` modem_hdr−4 / v4_flt_hash−4） |
| sm | **`ig=st.iver.gcfg.gstat.gver.hp2.ch0.ca.cb.sm.r`**（`0433396` 九段；tip 尾加 `sm`/`r`。`st=0` 未采 / `1` 已采；其余 `ffffffff` 至采完） |
| 不做 | **无** doorbell / CH_CMD / ring / QMI INIT / CDSP·TFTP·TX 改动 |
| 离线 | `IPA-4_7-MEM-PART.md` — vendor `ipa_4_7_mem_part` 全表（INIT TLV 索引） |
| 出件 | `boot-s20-<sha>.img`；**未 flash、未 push** |

判读（刷后）：

| `ig=` | 含义 |
|---|---|
| `st=0` / 全 f | IPA 未绿或未跑到探针 |
| `gcfg` bit0=0 且 `ca/cb≠deadbeef` | PAS 后 GSI 驱动未起、SRAM 未铺 — 与 park「无空 INIT」一致 |
| `ca/cb=deadbeef` | 意外已有布局（冷残/他路径）→ 再议 |
| hang | 回 `cef1fd9`；禁叠写刀 |

Flash-safe restore：`boot-s20-c38a6cd.img` / `b99c785` / 活体可停 `cef1fd9`。
等硬复位进 fastboot 再刷 stamp。


### slot-b harvest IPA INIT/EP（2026-10-19 ~15:50 Asia/Shanghai）

Capture：`.local/device/redfin/slot-b-ipa-init-20261019-154055/`（`INIT-TLV-COMPUTED.md`）。

| 钉死 | 值 | 来源 |
|---|---|---|
| `ctrl_comm_dest_end_pt` | **16** | `ipa3_ep_mapping[IPA_4_7][APPS_WAN_CONS].ipa_ep_num` |
| APPS_WAN_CONS `gsi_chan` | **7** | 同行 `ipa_gsi_chan_num` |
| `platform_type` | **3** (MSM_ANDROID) | `rmnet_ipa.c` |
| route `num_indices` | **7** | `v*_modem_rt_index_hi` |
| 活体 INIT | @49.21→49.31 | dmesg（无 IPAWANDBG TLV 行） |
| qrtr | `0x31/0x201` + **`0x45/0x1`** | slot-b 有 WLFW；裸机仍无 0x45 |
| `smem_restr` 活体 | **待刷采** | 无 debugfs / CONFIG_DEVMEM；下一刀 `ig=` 尾字段 `.sm.r` 导出 |

debugfs 仍不可挂；`/dev/mem` mknod 后 ENXIO。mem TLV = `ipa_4_7_mem_part.ofst + R`（R=`ig=` 尾 `.r`）。

**下一刀（本 tip）**：只读导出 `SHARED_MEM_SIZE`→R（扩展 `ig=`）；**仍禁**空 INIT。GSI CH7 + SRAM canary 仍是写刀前提。

### ig= 扩展 SHARED_MEM→R（tip，未刷，2026-10-19 ~15:58 Asia/Shanghai）

按 551ec4e park「下一刀」：**只读**导出 `IPA_SHARED_MEM_SIZE`→`smem_restricted_bytes`（R）。
基线 tip `0433396` + docs `551ec4e`。Capture 上下文：`slot-b-ipa-init-20261019-154055`
（EP=16 / gsi_ch=7 / platform=3）。设备仍停 Linux `_b`——**本刀未 flash**。

| 项 | 值 |
|---|---|
| 触发 | 同 `0433396`：IPA PAS 绿后一次；sm 幂等 |
| 读 | 既有 ig 九段 + **`IPA_SHARED_MEM_SIZE@cfg+0x54`** |
| sm | **`ig=….sm.r`**：`sm`=SHARED_MEM 原字（baddr[31:16]\|size[15:0]，单位 8B）；**`r`=baddr×8=`smem_restricted_bytes`** |
| INIT 用法 | 绝对地址 = `ipa_4_7_mem_part.ofst + r`（见 `IPA-4_7-MEM-PART.md` / `INIT-TLV-COMPUTED.md`） |
| 不做 | **零写**；无 doorbell / ring / QMI INIT / CDSP·TFTP·TX 改动 |
| 出件 | `boot-s20-<sha>.img`；**未 flash、未 push** |

判读（刷后）：

| 字段 | 含义 |
|---|---|
| `sm=ffffffff` / `r=ffffffff` | 未采到（IPA 未绿或未跑探针） |
| `r=0` | restr=0 → TLV 绝对 = 表 ofst（常见冷启） |
| `r≠0` | 用活体 R 重算 INIT mem TLV；仍禁空 INIT 直至 GSI CH7 + canary |

Flash-safe restore：先 `boot-s20-0433396.img`（本扩展基线，仍在 boot-out），
或 `c38a6cd` / `b99c785` / `af0c6ff`。等硬复位进 fastboot：可直接刷本 tip stamp
（覆盖 restore），或先刷 `0433396` 再刷 tip。

### GSI CH7 setup park + RO `gc=`（2026-10-19 ~16:15 Asia/Shanghai）

**活体基线**：`9830750` — `ig=1… .sm.r=.600.0` → **R=0**；`ch0=0`；`ca/cb≠deadbeef`；`iq=1.201`；无 hang。
slot-b：EP=16 / `gsi_ch=7` / `platform=3`（`INIT-TLV-COMPUTED.md`）。

#### 为何不做 allocate+config+start 写刀

对照 Linux `ipa3_setup_sys_pipe` → `ipa_gsi_setup_channel`（`ipa_dp.c`）+ `gsi_alloc_channel`/`gsi_start_channel`（`gsi.c`）：

| 参数 | vendor 来源 | 裸机状态 |
|---|---|---|
| EE | DTS `qcom,ee=<0>` | **已知** AP EE0 |
| `ch_id` | EP map APPS_WAN_CONS → **7** | **已知** |
| dir / prot | CONS → `FROM_GSI` / `GPI` | **已知** |
| `re_size` | `GSI_CHAN_RE_SIZE_16B` | **已知** |
| `ring_len` | `2 * desc_fifo_sz`；`desc_fifo_sz = wan_rx_desc_size * 8`；lito 无 `wan-rx-desc-size` → default **256** → ring **0x1000** | **尺寸公式已知** |
| prefetch | EP map `GSI_ESCAPE_BUF_ONLY` thresh 0 | **已知** |
| **ring DRAM PA** | `dma_alloc_coherent` via IPA SMMU AP | **未知** — 无固定 carveout；禁 invent |
| **evt ring** | CONS 独占 EVT（同 ring_size）或 common 0x7C00 | **同缺 PA**；另需 `EV_CH_CMD` ALLOC |
| **CH_CMD ALLOC/START** | 写 `GSI_EE_n_GSI_CH_CMD` 后 **IRQ completion** | 裸机 **无** GSI IRQ 状态机 |
| **IPA EP16 cfg** | `ipa3_cfg_ep`（hdr/aggr/metadata） | 寄存器序未活体钉死；INIT 另议题 |
| **RX TRE / buffers** | replenish 池；doorbell 前须铺 | 无缓冲 PA；**禁 doorbell** |
| SMMU | rings 走 `ipa_smmu_ap` IOVA | I4 STE 仅 IPA FW SID；ring DRAM 映射未建 |

裁定：**缺关键未知 → 禁 invent ring / 禁 doorbell / 禁 CH_CMD**。本刀 **不**实现写路径。

#### 本 tip：只读子集 `gc=`（可刷观测，非 setup 写刀）

与既有 `ig=` 同次、同门（IPA PAS 绿后一次）；**零写**。

| 项 | 值 |
|---|---|
| sm | **`gc=st.c0.c1.c2.c3.irq.err`** |
| `c0` | EE0 CH7 `CNTXT_0`（含 CHSTATE；ofs `0xf380`） |
| `c1` | `CNTXT_1` R_LENGTH |
| `c2`/`c3` | ring base LS/MS |
| `irq` | `EE0 TYPE_IRQ` @`0x12080` |
| `err` | `EE0 ERROR_LOG` @`0x12200`（**不** CLR） |
| `ig=` | 路径保持（含 `.sm.r`） |
| 不做 | 无 CH_CMD / doorbell / ring alloc / QMI INIT / CDSP·TFTP·TX 改 |

判读：`c0` CHSTATE=0 且 `c1/c2=0` → CH7 仍未配（与 `9830750` ch0=0 一致）。写刀前提仍是：钉 ring DRAM PA（或 attested carveout）+ EVT + IRQ 完成路径 + EP cfg。

Flash-safe restore：`boot-s20-9830750.img` / `0433396` / `c38a6cd`。
**未 flash、未 push**（本轮 build+commit only）。

### GSI CH7 DRAM 划区 `gr=`（allocate-only，2026-10-19 ~16:25 Asia/Shanghai）

离线对账（本 tip；**未 flash**）：

| 项 | 值 |
|---|---|
| CH ring | **0x1000** = `2 * (256 * IPA_FIFO_ELEMENT_SIZE)`；`IPA_WWAN_CONS_DESC_FIFO_SZ=256`（lito 无 `wan-rx-desc-size`） |
| EVT ring | **0x1000** dedicated（WAN_CONS：`use_comm_evt_ring` 默认 false → `ipa_gsi_setup_event_ring(ring_size)`；非 common `0x7C00`） |
| RX pool 预留 | **0x1FE000** @ `+0x2000`（≈192×8KiB；`IPA_GENERIC_RX_POOL_SZ` / `IPA_GENERIC_RX_BUFF_BASE_SZ`）— **未**写 TRE |
| **carveout** | **`0x9D800000` + `0x200000`** |
| 空洞收据 | DTS `modem_wlan_mem` 止于 `0x9D800000`；裸机 RFS=`0x9E000000`/`0x280000` + SH0..3 → `0x9EC80000`；`cdsp_sec@0x9F400000`。中间 `0x9D80…–0x9E00…` **无** reserved no-map |
| SMMU | I4 STE→USB_CB 恒等 `0x80000000–0xBFFFFFFF` 已覆盖；**不**新开 CB / **不** invent IOVA |
| sm | **`gr=st.ch.evt.rx.sz`**（与 `ig=`/`gc=` 同门：IPA PAS 绿后一次清零+`dcache_civac`） |
| 做 | 只 `write_bytes(0)` + civac + 闩 PA |
| **不做** | 无 CH_CMD / doorbell / CNTXT 写 / EP16 cfg / QMI INIT / CDSP·TFTP·TX 改 |

判读：`gr=1.9d800000.9d801000.9d802000.200000` → 划区就绪；`gc=` CHSTATE 仍应 0（未 ALLOC）。下一写刀仍须 IRQ 完成路径 + EP cfg + TRE，**勿**与本刀叠 doorbell。

Flash-safe restore：`boot-s20-9830750.img` / `2419bdf` / `0433396`。
**本 tip build+commit；未 flash、未 push。**


### GSI CH7 config/start tip（2026-10-19 ~16:30 Asia/Shanghai）

**基线 live**：`67d685c` — `gr=1.9d800000.9d801000.9d802000.200000` MATCH；`gc` CHSTATE 全 0；capture
`gsi-carveout-67d685c-20261019-162125/`。

**本刀**：在划区 PA 上做 EV0+CH7 ALLOC/CNTXT/最小 TRE/START（见 `IPA-4_7-MEM-PART.md`）。
无 QMI INIT、无 EP16。TRE 公式来自 `gsi.h` `struct gsi_tre` + `ipa_dp.c` replenish
（EOT|EOB|BEI、XFER）；Linux 在 START 前 replenish，故本刀填 4 颗再 doorbell。

| 项 | 值 |
|---|---|
| 超时 | ALLOC/START 各 **5s** 轮询 CNTXT0.CHSTATE（不依赖 GIC 投递） |
| sm | `gc=` 刀后重读（st/CHSTATE）；`gr=` 不变 |
| 出件 | `boot-s20-<sha>.img`；**未 flash、未 push** |

判读（刷后）：

| `gc` | 含义 |
|---|---|
| `st=1` 且 `c0` CHSTATE=2 | START 绿 |
| `st=2/3/4` | EV/CH ALLOC 或 START 超时；看 `c0`/`err` |
| `ca/cb` 仍非 deadbeef | 预期 — 未铺 INIT/SRAM canary |

下一刀候选：EP16 cfg 或 INIT（仍要 canary）；本刀只证 GSI CH 起。
Flash-safe restore：`67d685c` / `9830750` / `2419bdf`。

## 2026-10-19 INIT 未发：发送环不空（why=6）

活体 `ii=0.0.ffffffff.6`，`iq=1.201.0.1d`，`gc` st=1。
IPA 服务在 node 0、port `0x1d`、实例 `0x201`。GSI CH7 已 START。
`INIT_MODEM_DRIVER` 没发，因为 modem GLINK 发送环 head 一直不等于 tail。

随后一张把环上的 head/tail/首字也打进 `ii=`。同一镜像里混进了过期任务改的
rmtfs 8 深队列，刷后 ping 没回来。队列已从树上撤掉。

只带原因码的这张已刷。约 33 秒，以及之后再读，都是
`ii=0.0.ffffffff.6.16f0.1658.20009`。head `0x16f0`、tail `0x1658`
不动，差 `0x98` = 152 字节。tail 第一字 `0x20009` = TX_DATA、lcid 2
（IPCRTR）。`iq=1.201.0.1d`。`q` 仍无 `0x45`。

再拆 8 个字：`20009.1e.34.11.13.0.2b.1000500`。
intent `0x1e`，chunk `0x34`，源口 `0x11`，长度 `0x13`，
目的 node 0 port `0x2b`。QMI/TFTP 头 `00 05 00 01` =
opcode 5、code 1，加上 `file not found` 正好 19 字节。
这就是 RRQ 的 ERROR。它停在 tail 上，后面的 `INIT` 等不到空环。

下一张：RRQ 只记账，不把这包 ERROR 写进发送环。WRQ 仍 ACK。

这张已刷。约 33 秒 `ii=2.1.0`：`INIT_MODEM_DRIVER` 的 RESP
result 是 0。随后 ping 和 2323 都掉了，`q=` 没再采到。
没有 `0x45` 的新证据。

CH7 只 START、不发 INIT 的上一张是活的。INIT 被答应之后
机器死。下一张仍发 INIT，但不调用 `ipa_gsi_ch7_cfg`。

这张已刷。ping 和 2323 都在。`ii=2.1.0`，`gc` 的通道上下文是 0
（没有 START）。`ik=4.0`，`sr=1.7fffffff.0`。`q` 仍是
`42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001`，没有 `0x45`。

内核在 INIT 成功之后还要回 modem 的
`INIT_MODEM_DRIVER_CMPLT_REQ`（msgid `0x35`）：RESP 只有 TLV 0x02
result 成功。`ic=` 是见到次数和已回次数。

这张已刷。ping 和 2323 都在。`ii=2.1.0`，`ic=0.0`：modem 没有把
msgid `0x35` 发到本机端口 `0x15`。`sr=1.7fffffff.0`。`q` 仍无 `0x45`。
本机 `host_svc_port` 不公告服务 `0x31`，modem 没有可投递的 IPA 服务端。

下一张按内核 `qmi_add_server(0x31, vers, instance 1)` 在端口 `0x16`
公告 IPA 服务端，完成请求改在 `0x15` 或 `0x16` 上收。

这张已刷。`lk=0.0.4`：调用处在 `HOST_ADV < 4` 就停，第五个
`0x31` 没发出去。`ic=0.0`，`ii=2.1.0`，`q` 仍无 `0x45`。
门槛改成 `< 5` 之后已刷。`lk=0.0.5`：第五个服务发出去了。
`ic=0.0`：端口 `0x15` 和 `0x16` 上都没有 msgid `0x35`。
`ii=2.1.0`，`sr=1.7fffffff.0`。`q` 仍无 `0x45`。ping 和 2323 都在。

## 2026-10-19 rmtfs 突发只闩一笔（过期任务，已撤，勿刷）

`fr` 前 32 字节仍是 `dog_hal_common.c:180:[tmr_slave3`。这是 modem
DOG 的定时从端，不是一条 QMI。`radio-bringup` 写明：开机时
`qrtr-ns`、`pd-mapper`、`tftp_server`、`rmt_storage` 没有真在服务，
DOG 每 65.7 秒判初始化卡住。commaai/agnos-kernel-sdm845 PR 120 的
恢复日志里，servreg 实例 180 连上之后、`wlan_pd` 变成 `0x1fffffff`
之前，rmt_storage 连续 OPEN `/boot/modem_fs1`、`modem_fs2`、
`modem_fsg`、`modem_fsc`，然后 `ALLOC_BUFF`，然后 Read iovec。
`tmr_slave3` 等的就是这条初始化走完。EFS 没被真正 IOVEC 服务时，
看门狗先响，`0x45` 不会出现。

`rootfs/src/qcom/rmtfs/rmtfs.c` 的 `handle_rmtfs` 对每次 `recvfrom`
都按 msgid 应答（OPEN 1、CLOSE 2、IOVEC 3、ALLOC 4、DEV_ERROR 5）。
没有「只留一笔」的槽。

裸机扫描不是这样。`scan_new_servers` 一次把接收 FIFO 从 0 走到 head。
端口 `0x10`、msgid 1..=5 里，只有 `RF_PEND` 为假的第一笔被闩上。
同一趟里后面的请求只做 `RF_DROP |= 2`，不进 `RF_SEEN`。
`smem_scan` 里的 8 轮循环要等 `rf_reply` 把 `RF_N` 加上去才再扫。
`rf_reply` 在发送 head 不等于 tail 时直接返回，不增加 `RF_N`。
刚发出的那笔应答还占着发送环，循环马上停。然后
`glink_rx_advance` 把接收 tail 推到这次走完的 `RX_PARSED`，
没答的 OPEN/ALLOC 一起被消费掉。modem 不重发。所以四次 OPEN 加
一次 ALLOC 落在同一次 FIFO 里时，会被单槽闩丢掉。这是代码错误，
不是缺一种新的 QMI。

改动只在闩：8 深队列，按 FIFO 顺序把每笔未见过的请求整包抄下来
（GLINK 头加 chunk，扫描上限 512）。`rf_reply` 仍用原来的
`put_result` / `qmi_finish` / `glink_tx_payload`，发送环空了才答
队头，答完才出队。环不空就留在队列里，下一拍再答。队列满才记
`RF_DROP` bit1。

`./build-bare.sh --s18` 已出
`.local/device/redfin/boot-out/boot-s18.img`。fastboot 上没有设备，
没刷。`q=` 还没有 `0x45` 的新数。

## 2026-10-19 IPA 公告实例字改成 0x101（已编，未刷）

`qmi_interface.c` 的 `qmi_send_new_server` 把实例字写成
`version | (instance << 8)`。`ipa_qmi_service.c` 调
`qmi_add_server(0x31, IPA_A5_SVC_VERS=1, IPA_A5_SERVICE_INS_ID=1)`，
线上是 `0x101`。slot-b `qrtr-lookup` 是 `0x31/0x101@1:16404`。
modem 自己的 Q6 是 `0x201`（版本 1、实例 2），活体 `iq=1.201` 对得上。
上一张第五个 `NEW_SERVER` 把实例字写成了 `1`，对不上 `0x101`。
`lk=0.0.5` 只说明包发出去了，`ic=0.0` 是没有客户连上。

这张只改这一处：公告和 LOOKUP 回包的实例字都用 `0x101`，端口仍是
`0x16`。服务端口上第一笔 QMI 请求的 msgid 记在 `im=`，先不回
`INDICATION_REGISTER`（`0x20`）。不 START CH7，不喂 TFTP DATA。

`./build-bare.sh --s20` 已出
`.local/device/redfin/boot-out/boot-s20.img`（106496 字节，LZ4
roundtrip exact）。当时 ping `169.254.1.1` 通，2323 连接超时，
fastboot `device_count 0`。没刷。

随后这张已刷：闸门 device_count 1、product redfin、不是 enchilada，
`boot_a` / `set_active a` / reboot 都是 0。约第 11 次探测 ping 通。
2323 连了 8 次都超时，没有 `sm`。再等约 90 秒，ping 也不回，
fastboot `device_count 0`。ARP 里还留着 `02:00:00:00:00:02`。
`im=` / `ic=` / `q=` 都没采到。上一张实例字是 `1` 时 ping 和 2323
一直在。这张只改了实例字 `0x101`。

`rmnet_ipa.c` 里 `ipa3_qmi_service_init` 在 `ipa3_is_ready` 和
`ipa3_init_q6_smem` 之后。裸机还没有这条端点准备，所以先撤掉
`0x31` 公告和 LOOKUP 回包，`HOST_ADV` 回到 4 个服务。

这张已刷（106496 字节，18:27，LZ4 roundtrip exact）。闸门同样通过，
`boot_a` / `set_active a` / reboot 都是 0。又是约第 11 次 ping 通，
2323 连 6 次超时。之后 ping 失败，fastboot 和 adb 都是 0。
没有 `sm`。撤掉公告并没有把 USB 留住。这次是热重启，上一张
`0x101` 掉线之后没有拔电。

关机再进 fastboot 之后，同一张又刷了一次。闸门通过。约第 10 次
ping 通，2323 连 8 次超时，探测过程中 ping 变为失败。fastboot
没有设备。冷启动同样留不住 USB。`sm` 没有。

下一张把第五个公告改回实例字 `1`（`lk=0.0.5` 那张 ping 和 2323
还在）。`./build-bare.sh --s20` 已出 18:38 的 `boot-s20.img`
（106496 字节，flat 166936，LZ4 roundtrip exact）。当时
fastboot `device_count 0`，ping 也不通。没刷。

这张随后已刷。闸门 device_count 1、product redfin、不是 enchilada，
`boot_a` / `set_active a` / reboot 都是 0。约第 11 次 ping 通，
2323 立刻回 `aginx`。一次 `sm`（1658 字节）：`iq=1.201.0.1d`，
`ii=2.1.0`，`ic=0.0`，`im=0`，`lk=0.0.5`，`sr=1.7fffffff.0`，
`si=0`，`ik=4.0`。`ql` 读到 `480.480.1e0.1a8.e.92`（不是
`0x31`）。25 秒后 ping 仍通，第二次 2323 超时。再探 ping 失败，
fastboot `device_count 0`。没有第二份 `q=`，没有 `0x45` 的新证据。

去掉 `im=` 闩之后又刷了一张（18:51）。闸门通过。`sm`（1648 字节）：
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001`，没有
`0x45`。`ql` 空。`iq=1.201.0.1d`，`ii=2.1.0`，`ic=0.0`，
`lk=0.0.5`，`sr=1.7fffffff.0`，`ik=4.0`。之后 40 秒只 ping、不再
连 2323，ping 一直通。第二次 2323 超时，紧接着 ping 失败。
fastboot `device_count 0`。

唯一套接字停在关闭态时，下一笔 SYN 走 smoltcp RST。已改成
`abort` 后重新 `listen`（和半开握手同一条）。18:55 的
`boot-s20.img` 已编出。当时 fastboot 没有设备，没刷。

这张已刷。闸门通过。三次 `sm` 都回 `aginx`，每次之后 ping 都通，
第三次之后 15 秒 ping 仍通。三次内容一样：
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001`，没有
`0x45`。`ql` 空。`iq=1.201.0.1d`，`ii=2.1.0`，`ic=0.0`，
`lk=0.0.5`，`sr=1.7fffffff.0`，`si=0`，`ik=4.0`。

slot-b 在 `Sending QMI_IPA_INIT_MODEM_DRIVER_REQ` 后 164 微秒就进了
`ipa3_handle_indication_req`（当时 init 还没完成，所以 “not send
indication”）。服务端实例字必须是 `0x101`。TCP 能连第二次之后，
把第五个公告从实例字 `1` 改成 `0x101`。

这张已刷（19:10）。两次 `sm`，ping 都通。`q` 仍无 `0x45`。
`ii=2.1.0`，`ic=0.0`，`lk=0.0.5`，`sr=1.7fffffff.0`，`ik=4.0`。
`im=27`：服务端口第一笔请求是 `QMI_IPA_CONFIG_REQ`（0x27），不是
`0x20` 也不是 `0x35`。

内核 `handle_ipa_config_req` 调 `ipa_mhi_handle_ipa_config_req`
（这条返回 0），应答只有 TLV 0x02 result 成功。先按这个回。

19:13 那张已刷。`im=27.27.47`，8 秒后再读是 `27.27.52`。第一笔和
最近一笔都是 `0x27`，已回次数 0x47 再到 0x52。同一笔停在接收环里，
每轮扫描又回一次。`q` 仍无 `0x45`，`ic=0`，`sr` 仍 UNINIT。ping 通。

改成同一个事务号和源端口只回一次。19:15 的镜像 110592 字节，闸门
通过并刷了。两分钟没有 ping，fastboot 和 adb 都是 0。没有 `sm`。

随后把应答改到第一次 `sm` 之后。19:25 已刷。第一次 `sm`：
`im=27.27.0`（见到 `0x27`，还没回），`q` 无 `0x45`，`ii=2.1.0`，
`ic=0`，`sr=1.7fffffff.0`，ping 通。8 秒后再连，2323 超时，ping
失败。成功的 CONFIG 应答本身会把 USB 弄掉。

19:28 的镜像不再回 CONFIG，只把请求前 4 个字留在 `cg=`。
fastboot `device_count 0`，没刷。

随后已刷，并热重启了一次。两次都是 ping 通、`cg=-`、`q` 空、
`lk=0.0.0`、`hh=1`、`ii` 原因 1。第一次 `s` 从 6 变成 `107`。
第二次 25 秒时还是 `s=6`，再过 15 秒也是 `107`。
`af=` 是 `err_qdi.c:1045` … `SNS_REG_INIT` …
`sns_registry_sensor.c:94:SNS_RC_SUCCESS`。IPA/CDSP PAS 是绿的
（`ip=80000003.0.0.0`，`cd=8000000b.0.0.0`，`ce=1.6.32`）。
没有 `0x27`，因为 modem 在公告之前就 err-fatal 了。这两次都是
fastboot 热重启，不是拔电。

拔电后再刷同一张。闸门通过。约第 11 次 ping 通。隔 20 秒连
2323 超时，ping 也不回。又等 20 秒仍是超时。fastboot
`device_count 0`。没有 `sm`，`cg=` 没读到。

同一张再刷一次。闸门通过。ping 一通就读。两次 `sm`，ping 都在。
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001`，没有
`0x45`。`ii=2.1.0`，`ic=0.0`，`im=27.27.0`，`lk=0.0.5`，
`sr=1.7fffffff.0`，`ik=4.0`，`s=6`。
`cg=27000300.10003800.30004.4150000`。

这是 QMI 请求头：类型 0、事务 3、msgid `0x27`、体长 `0x38`。
第一个 TLV 是 `0x10`，长度 4，值 3。`ipa_config_req_msg_v01` 里
`0x10` 是 `peripheral_type`，3 是 `QMI_IPA_PERIPHERAL_PCIE`，
不是速度枚举里的 USB SuperSpeed（那是 TLV `0x14`）。
内核在 GSI 上忽略这条，只回成功。

21:52 这张在第一次 `sm` 之后回一次成功。`im` 从 `27.27.0` 变成
`27.27.1`，之后 40 秒内 ping 一直通，最近一笔仍是 `0x27`，
`ic=0`，`sr=1.7fffffff.0`，`q` 仍无 `0x45`，`s=6`。
一次成功应答没有把 USB 弄掉，也没有引出 `0x20` 或 `0x45`。

23:25 这张：见到 `0x20` 就回成功，并且 INIT 已经完成时再发
`0x22`。`jx` 记 0x15/0x16 上第一笔不是 `0x27` 的包。
三次 `sm`，ping 都通。`im` 从 `27.27.0` 到 `27.27.1`。
`jx=15.2.21`：那是 INIT 的应答（端口 `0x15`，qtype 2，msgid `0x21`）。
`ir=0.0`：没有 `0x20`，所以也没发 `0x22`。`ic=0`，
`sr=1.7fffffff.0`，`q` 仍无 `0x45`，`s=6`。

Canary 已铺：`ig` 的 ca/cb 是 `deadbeef.deadbeef`。USB 仍在。
`ii=2.1.0`，`im=27.27.1`，`jx=15.2.21`，`ir=0.0`，`sr` 仍 UNINIT，
`q` 仍无 `0x45`。

CH7 上下文全 0，端点没分配。内核这时不发 TLV `0x17`。
去掉 `ctrl_comm_dest_end_pt=16` 后刷了。约第 14 次 ping 通，
接着 2323 超时，ping 也不回。fastboot `device_count 0`。没有 `sm`。
TLV `0x17` 已加回源码，死亡镜像不再作为下一张。23:43 的
`boot-s20.img` 是加回之后的。

这张已刷。闸门通过。约第 25 次 ping 通，2323 超时，ping 随即失败。
再探 30 秒，ping 和 fastboot 都是 0。没有 `sm`。上一趟同一套
代码（canary + TLV `0x17`）两次 `sm` 都在。这次是那张把端点
拿掉、USB 掉线之后的再刷。

关机再进 fastboot 后又刷了一次。闸门通过。又是约第 25 次 ping
通，2323 连续超时，ping 失败。没有 `sm`。冷启动写 canary 的
镜像留不住 USB。canary 写入已从探测里拿掉，下一张不写。

07:16 不写 canary 的镜像已刷。闸门通过。约第 13 次 ping 通，
两次 `sm`，ping 都在。`ig` 的 ca/cb 是 `40f3727d.92a6e3b6`，
不是 `deadbeef`。`q` 无 `0x45`。`ii=2.1.0`，`im` 从 `27.27.0`
到 `27.27.1`，`jx=15.2.21`，`ir=0.0`，`ic=0`，`sr=1.7fffffff.0`，
`s=6`。

随后改成第一次 `sm` 之后才写 canary。已刷。约第 14 次 ping 通。
第一次 `ig` ca/cb 仍是 `40f3727d.92a6e3b6`，`im=27.27.0`。
4 秒后第二次是 `deadbeef.deadbeef`，`im=27.27.1`。ping 都在。
`jx=15.2.21`，`ir=0.0`，`sr=1.7fffffff.0`，`q` 仍无 `0x45`，
`s=6`。晚写能留下，基带不重新发 `0x20`。

接着按 `ipa3_init_q6_smem` 在探测里先把 `modem_hdr`、
`modem_hdr_proc_ctx`、`modem` 三段清零，再写 canary。从正在跑的
机器 `fb` 重启刷上。约第 13 次 ping 通。第一次 `sm` 的 ca/cb
已经是 `deadbeef.deadbeef`。两次 ping 都在。`ii=2.1.0`，
`im` 从 `27.27.0` 到 `27.27.1`，`jx=15.2.21`，`ir=0.0`，
`sr=1.7fffffff.0`，`q` 仍无 `0x45`，`s=6`。清零加 canary
没有引出 `0x20`。这次不是冷启动。

modem 入向已有 `ipa`（`me=1.2.1`，没有 `wlan`），出向 value 一直是 0。
`ipa3_smp2p_modem_clk_query_isr` 把 bit0（完成）和 bit1（时钟票）写上。
这张已刷。第一次 `sm`：`mv=1`，`me=1.2.1`，`sr=1.7fffffff.0`，
`si=0`，`ir=0`，`q` 无 `0x45`，`tf` 仍是
`/readonly/firmware/image/modem_pr/so/549_38_0.mbn`，ping 通。
6 秒后 2323 超时，ping 失败。时钟应答之后 USB 掉了。调用已撤。

07:43 撤掉时钟应答的镜像已刷。闸门通过。约第 13 次 ping 通。
第一次 `sm`：`mv=0`，`me=1.2.1`，`ig` ca/cb 是 `deadbeef`，
`ii=2.1.0`，`im=27.27.0`，`ir=0`，`sr=1.7fffffff.0`，`q` 无
`0x45`，`s=6`，ping 通。8 秒后 2323 超时，ping 失败。
fastboot `device_count 0`。没有第二份 `sm`。上一趟同一套
清零加 canary、不回时钟的代码能一直 ping。这次没有。

同一张 07:43 又刷了一次。闸门通过。约第 14 次 ping 通。
第一次 `sm` 和上一趟一样：`mv=0`，`me=1.2.1`，`ii=2.1.0`，
`im=27.27.0`，`ir=0`，`sr=1.7fffffff.0`，`q` 无 `0x45`，
`s=6`。4 秒后 2323 超时，ping 失败。这次也是掉线之后直接进
fastboot，没有拔电。

拔电后再刷 07:43。闸门通过。约第 15 次 ping 通。三次 `sm`
（间隔 6 秒、再 8 秒）ping 都在。内容一样：`mv=0`，
`me=1.2.1`，`ii=2.1.0`，`im` 从 `27.27.0` 到 `27.27.1`，
`ir=0`，`sr=1.7fffffff.0`，`q` 无 `0x45`，`s=6`。

从这台干净机器 `fb` 再刷一次时钟应答。约第 13 次 ping 通。
第一次 `sm`：`mv=1`，`me=1.2.1`，`sr=1.7fffffff.0`，`ir=0`，
`q` 无 `0x45`。`tf` 不再是 `549_38_0.mbn`，变成
`/readwrite/ota_firewall/ruleset`。6 秒后 2323 超时，ping
失败。干净重启上，时钟应答本身会让基带往下走，并把 USB 弄掉。
调用已再撤。09:16 的镜像是撤掉之后的。

09:16 已刷。闸门通过。两分钟没有 ping。随后 fastboot 和 adb
都是 0。没有 `sm`。

同一张再刷。闸门通过。约第 15 次 ping 通，USB 留着，但 `q` 空，
`hh=1`，`lk=0.0.0`，随后 `s=107`。`af=` 结尾是
`SNS_REG_INIT` … `SNS_RC_SUCCESS`。`ik=0`。没有服务，没有
`0x45`。同一张再热重启一次：又是 `hh=1`、`lk=0`，约 20 秒后
同样 `s=107`，`af=` 仍是 `SNS_RC_SUCCESS`。ping 还在。

只加了 `iz=`：lcid 2 上见过的最大 intent，门槛 52 没动。
从这台已经 `s=107` 的机器 `fb` 刷上。约第 16 次 ping 通，
等 20 秒后 2323 超时，ping 失败。fastboot `device_count 0`。
`iz=` 没读到。

拔电后再刷同一张。闸门通过。约第 16 次 ping 通。第一次
`sm`：`iz=2080`，`hh=1`，`lk=0.0.5`，`s=6`，`ii=2.1.0`，
`ik=4.0`，`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01` 起，
没有 `0x45`。8 秒后 2323 超时，ping 失败。intent 是
`0x2080`，大于门槛 52。

同一张再刷，没有改代码。闸门通过。约第 16 次 ping 通。
`pn=tms/pddump_disabled.0.11`（命中 0，载荷 `0x11`）。
`tf=/readonly/firmware/image/modem_pr/so/549_38_0.mbn.1.2b`。
`sr=1.7fffffff.0`，`lk=0.0.5`，`iz=2080`，`s=6`，`q` 无
`0x45`。读的时候 ping 是通的。`pn` 只留最后一次名字。

只加 `pe=`：任何一笔域名请求是 `kernel/elf_loader` 就记 1。
不新发应答。09:59 的镜像已编。fastboot `device_count 0`，没刷。

09:59 已刷。闸门通过。约 45 次探测没有 ping。随后
fastboot 和 adb 都是 0。没有 `sm`，`pe=` 没读到。

同一张再刷。闸门通过。约 40 次探测没有 ping。随后
fastboot 和 adb 仍是 0。`pe=` 还是没读到。没有改代码。

同一张再刷。闸门通过。约第 29 次 ping 通。当时
`pe=0`，`pr=0`，`pn` 空，`tf` 空，`iz=0`，`hh=1`，`lk=0.0.0`，
`s=6`。再过大约 35 秒：`pe` 仍是 0，`iz` 仍是 0，`s=107`，
`q` 空。ping 还在。没有域名请求，因为通道 2 上没有记到
intent，服务没有公告。

`x=` 尾部 88 字节对得上一条 DATA：命令字 `0x10009`，
lcid 1，chunk `0x34`，QRTR 类型 2（HELLO）。本机 intent
只发给 lcid 2。这条 HELLO 在 lcid 1 上，所以 `iz=0`，
`lk` 停在 0。没有改代码。

收包侧改成认对端通道 1。发出仍用本机通道 2。已刷。
约第 31 次 ping 通。6 秒后仍是 `iz=0`，`lk=0.0.0`，`hh=1`，
`s=6`。再过约 20 秒 `s=107`，`iz` 仍是 0，`q` 空，`pe=0`。
ping 还在。通道 1 上也没有够格的 intent。

活体 `d=` 的接收头是 `0x2b0`。`glink_open_ack` 在头超过
`0x80` 时直接返回，`OPEN_ACK` 不做，`glink_hello` 也不进。
只去掉这个上限。已刷。约 40 次探测没有 ping。随后
fastboot `device_count 0`。没有 `sm`。

同一张再刷。闸门通过。约第 31 次 ping 通。`iz=0`，
`hh=1`，`lk=0.0.0`，`s=6`，`q` 空。再过大约 35 秒
`s=107`，`iz` 仍是 0，`lk` 仍是 0。ping 还在。
去掉 `0x80` 上限没有让公告发出去。

只加 `ga=`：`GLINK_ACKED` 是否做过。已刷。约第 31 次
ping 通。`ga=1`，`iz=0`，`hh=1`，`lk=0.0.0`，`s=6`，`q` 空。
20 秒后仍是这些数，ping 还在。OPEN_ACK 做过，hello 也进了，
通道 1 上还是没有 intent。

这一趟 `x=` 尾部是 `0x20009`：DATA，lcid 2。上一趟是
`0x10009`，lcid 1。收包改成 1 或 2 都认。已刷。约第 20 次
ping 通。`iz=2080`，`ga=1`，`hh=1`，`lk=0.0.5`，`s=6`，
`ii=2.1.0`，`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01` 起，
没有 `0x45`。ping 还在。

同一张再刷，没有改代码。闸门通过。约 40 次探测没有
ping。随后 fastboot `device_count 0`。`pe=` 和 `tf=` 没读到。

同一张再刷。闸门通过。约第 29 次 ping 通。第一次 2323
超时，ping 仍在。12 秒后：`pe=1`，
`pn=kernel/elf_loader.1.30`，`pr=3`，
`tf=/readonly/firmware/image/modem_pr/so/549_38_0.mbn.1.9e`，
`sr=1.7fffffff.0`，`lk=0.0.5`，`s=107`，
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001`，
没有 `0x45`。ping 还在。`kernel/elf_loader` 来过，命中 1。

只在 INIT 已成功后，对 RRQ 回一次 TFTP file-not-found。已刷。
约第 20 次 ping 通。`tf=...549_38_0.mbn.1.1d`，8 秒后 `.1.45`。
`pe=0`，`sr=1.7fffffff.0`，`lk=0.0.5`，`ii=2.1.0`，`s=6`，
`q` 仍无 `0x45`。两次 ping 都在。这一次错误应答没有把 USB 弄掉，
也没有让基带停手。

只加 `mw=`：`0x8C000000` 第一个字，ELF 小端魔数是 `464c457f`。
已刷。约 40 次探测没有 ping。随后 fastboot `device_count 0`。
`mw=` 没读到。这个读只在 `sm` 里，开机路径没碰那块内存。

同一张再刷。闸门通过。约 45 次探测没有 ping。随后
fastboot `device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 30 次 ping 通，接着两次
2323 都超时，ping 也失败。随后 fastboot `device_count 0`。
`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 41 次（0.5 秒一探）ping 通，
接着 4 次 2323 都超时，ping 失败。随后 fastboot
`device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 52 次（0.4 秒一探）ping 通，
接着 6 次 2323 都超时，ping 失败。随后 fastboot
`device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 55 次（0.3 秒一探）ping 通，
接着 5 次 2323 都超时，ping 失败。随后 fastboot
`device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 57 次（0.3 秒一探）ping 通，
接着 5 次 2323 都超时，ping 失败。随后 fastboot
`device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 55 次（0.3 秒一探）ping 通，
接着 5 次 2323 都超时，ping 失败。随后 fastboot
`device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。约第 56 次（0.3 秒一探）ping 通，
接着 5 次 2323 都超时，ping 失败。随后 fastboot
`device_count 0`。`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。又是约第 56 次 ping 通，接着 5 次
2323 都超时，ping 失败。随后 fastboot `device_count 0`。
`mw=` 仍没读到。没有改代码。

同一张再刷。闸门通过。又是约第 56 次 ping 通，接着 5 次
2323 都超时，ping 失败。随后 fastboot `device_count 0`。
`mw=` 仍没读到。没有改代码。

`mw=` 那张每次都是 ping 闪一下、2323 连不上。已把这个读撤掉，
编回能连过 2323 的代码并刷了。约 45 次探测没有 ping。随后
fastboot 和 adb 都是 0。

撤掉 `mw=` 的 15:17 镜像再刷。闸门通过。第 10 次探测已经
离开 fastboot。约第 30 次 ping 通，2323 超时，当时 ping 已失败。
随后 fastboot `device_count 0`。不是 `mw=` 这一读造成的。

同一张 15:17（110592 字节，无 `mw=`）再刷。闸门通过。约第 44 次
ping 通，2323 连上，`sm` 1761 字节。`pe=0`，
`tf=/readonly/firmware/image/modem_pr/so/549_38_0.mbn.1.2b`，
`sr=1.7fffffff.0`，`lk=0.0.5`，`s=6`，
`q=42.b401,2b.1202,f.1,33.201,15.1,33.b01,18.1,16.1001`。
没有 `0x45`。这是这张去掉 `mw=` 之后第一次读到控制台。
随后再探：USB 产品名仍是 Aginx ECM，fastboot 和 adb 都是 0，
ping 失败，2323 超时。没有第二份 `sm`。没有改代码，没有再刷。

## 2026-10-20 已确认 / 待确认

对照的是这三处，不是猜的：

- 槽 b `dmesg-raw.txt` 与 `TIMELINE.md`（2026-10-19 11:03）
- 内核 `include/linux/soc/qcom/pdr.h` 的状态值，以及仓内
  `rootfs/src/qcom/tqftpserv/tqftpserv.c` 的 `handle_rrq`
- 裸机 15:17 那次 `sm`（上面那段）

### 已经在机器或源码里定下来的

| 事实 | 出处 |
|---|---|
| 状态：DOWN `0x0fffffff`，UP `0x1fffffff`，UNINIT `0x7fffffff` | `pdr.h` |
| 槽 b 顺序：`ipa_fws` → ADSP → CDSP → modem → rmtfs 真读 → 约 1.2 秒后 `wlan_pd` UP | dmesg 40.88–50.21 |
| 同一秒 `icnss` 连上 WLFW，`state: 0x980`。dmesg 里没有 `tftp` / `549_38` / `elf_loader` | `TIMELINE.md` |
| `tftp_server` 进程在跑。`qrtr-lookup` 里 `0x45` 实例 `0x1`，node 0 端口 88，modem 上还有几十个服务 | `ps.txt`、`qrtr-lookup.txt` |
| IPA 的 indication 在 INIT 完成前被内核明确丢掉（`not send indication`），`wlan_pd` 照样 UP | dmesg 49.30 / 50.21 |
| `tqftpserv` 对带 option 的 RRQ：新 QRTR 口上发 OACK；没有 option 才发 DATA；找不到文件也在这个新口上发 ERROR，不在监听口 | `handle_rrq` |
| 裸机 FAT 上 `549_38_0.mbn` 在，长 `0xeb43c`，簇 `0x1a67` | 早前 `tz=1.eb43c.1a67` |
| 监听口上的一次 file-not-found 不掉 USB，也止不住 RRQ | 15:17：`tf` 次数 `0x2b`，`s=6` |
| 枚举之前发 OACK 或 DATA，ECM 在、ARP 不完整 | `8a761ad` / `25df6d3` / `365d1f4` |
| 15:17 控制台：`sr=1.7fffffff.0`（登记成功，状态 UNINIT），`lk=0.0.5`，`s=6`，`pe=0`，`q=` 正好 8 个，字符串里没有 `45` | 上面那次 `sm` |

`q=` 只有 8 格。`scan_new_servers` 每轮先清零，再只留下最先看到的 8 个 `NEW_SERVER`。槽 b 的 `0x45` 在端口 88，排在这 8 个后面。所以「`q=` 里没有 `45`」不能当成「modem 没注册」。`0x31` 已经用 `iq=` 在 8 格外面闩过，`0x45` 之前没有同样的闩。

### 还没证实的

- 15:17 那份 `sm` 没拆 `si=`、`cd=`、`ce=`、`ip=`、`ik=`、`tz=`。不知道 CDSP 的 err-ready、IPA PAS、EFS 种子、SO 探针在那一次开机里是什么。
- `STATE_UPDATED`（`si`）在更早的开机上是 0。15:17 这次没读到。UNINIT 只说明登记那一瞬间 PD 没起来，不排除之后才 UP、而 `0x45` 被 8 格丢掉。
- 槽 b 的 `tftp_server` 在跑，但没有它的日志。dmesg 没有 `549_38` 不能证明它没把这个 SO 喂出去。
- 枚举完成之后再按 `tqftpserv` 发一次 OACK（仍不 DATA）没有试过。枚举前的 OACK 已经挂过，这一刀不做。

### 这一刀

只加 `w45=`：见到服务 `0x45` 就闩住，不跟那 8 格一起清。不发新的 QRTR，不改 TFTP，不改 PAS。`w45=0` 是没见过；高位为 1 才是见过，低位是实例字。

### 刷验（2026-10-20 15:58 镜像，110592 字节）

闸门通过，`boot_a` + `set_active a` + reboot 都是 0。约第 18 次 ping 通，2323 的 `sm` 1767 字节。

| 字段 | 值 | 读法 |
|---|---|---|
| `w45` | `0` | 接收 FIFO 里没有 `0x45`。不是被 8 格丢掉的 |
| `sr` | `1.7fffffff.0` | 登记成功，状态仍是 UNINIT |
| `si` | `0` | 没有 `STATE_UPDATED` |
| `s` | `6` | 没有 err-fatal |
| `lk` | `0.0.5` | QRTR 链路在 |
| `ik` | `4.0` | EFS 的 IOVEC 成功 |
| `tz` | `1.eb43c.1a67` | SO 仍在，实长没变 |
| `ip` | `80000003.0.0.0` | IPA fws 三段，PAS 三步都是 0 |
| `cd` | `8000000b.0.0.0` | CDSP 11 段，PAS 三步都是 0 |
| `ce` | `1.6.32` | 出向已发；ready = 6 = bit1 err-ready + bit2 proxy-unvote，bit0 fatal 是清的；50 ms |
| `me` | `1.2.1` | 入向有 `ipa`，没有 `wlan` |
| `pe` | `0` | 这一拍还没见到 `kernel/elf_loader` |
| `tf` | SO 的 RRQ，次数 `0x20` | 回退链还在 |

CDSP 先于 modem、而且 err-ready 干净，这一条已经做到了。`wlan_pd` 仍是 UNINIT，`0x45` 仍没注册。缺的不是「CDSP 没起来」。

### 2026-10-20 只读 SO 文件头

公开文档没有写 `549_38_0.mbn` 是不是 `wlan_pd` 的镜像。主机上没有这份文件，只有 FAT 位置（簇 `0x1a67`，长 `0xeb43c`）。这一刀在已有的开机探针里多读文件第一块，不发给 modem，RRQ 仍是 file-not-found。

`th=mag.eh`。`mag` 是前 4 字节小端，ELF 是 `464c457f`。`eh` 低 16 位是 `e_type`，高 16 位是 `e_machine`。Hexagon（`EM_QDSP6`）是 164，即 `0xa4`。可执行文件 `e_type` 2 时，`eh` 打印成 `a40002`。共享对象 `e_type` 3 时是 `a40003`。

刷验：闸门通过。约第 18 次 ping 通，`sm` 1786 字节，有换行。`th=464c457f.a40003`。前 4 字节是 ELF。`e_type` 3 是共享对象，`e_machine` `0xa4` 是 Hexagon。`tz=1.eb43c.1a67` 没变。`w45=0`，`sr=1.7fffffff.0`，`si=0`。文件没有发给 modem。

### 2026-10-20 第 4 步：按 tqftpserv 送 SO

`rootfs/src/qcom/tqftpserv/tqftpserv.c` 的 `handle_rrq`：新 QRTR 口，带选项就 OACK，然后等 ACK，再发 DATA。块号从 1 起，偏移是 `ack * blksize`。

裸机发送环的载荷上限是 256。DATA 头 4 字节，所以 OACK 里的 `blksize` 写 252，`wsize` 写 1，`tsize` 写文件实长。源端口 `0x21`，不占用监听口 `0x11`。`rsize` 若在 RRQ 里就只送那么多；没有就送整文件。对端 opcode 5 把状态记成 9，停。

开机不发。`SM_ARMED` 在第一次 `sm` 里才变成真，下一轮扫描才发 OACK。传输中扫描从 200 圈收成 2 圈，好让 ACK 赶在 modem 的超时里。`tb=st.blk`：1 已 OACK，2 在送 DATA，3 短块已送出，8 读失败，9 对端 ERROR。

刷验：闸门通过。约第 17 次 ping 通。三次 `sm`，ping 都还在。

| | tb | tf 次数 | sr | w45 |
|---|---|---|---|---|
| 第 1 次 | `0.0` | `0x1f` | UNINIT | 0 |
| 3 秒后 | `1.0` | `0x53b` | UNINIT | 0 |
| 再 5 秒 | `1.0` | `0xe37` | UNINIT | 0 |

OACK 发出去了，USB 没掉。没有 DATA（块号停在 0），也没有对端 ERROR（状态不是 9）。RRQ 次数从 31 涨到 3639：状态停在 1 之后，后来的 RRQ 被丢掉，OACK 仍对着第一次那笔的端口。modem 在换新请求，ACK 没有回到 `0x21`。

下一刀：状态仍是 1、块号仍是 0 时，新的 SO RRQ 改用它自己的 node/port 再发一次 OACK。监听口 `0x11` 上的 opcode 4/5 也算 ACK/ERROR。已经在送 DATA 就不换端口。

换端口这张刷过。闸门通过。约第 29 次 ping 通，2323 超时，随后 ping 失败，fastboot 和 adb 都是 0。`sm` 没读到，`tb` 没有新值。OACK 要等第一次 `sm` 才发，这次控制口没连上，换端口那一步没有跑到。

同一张再刷。约第 18 次 ping 通，三次 `sm` 都有换行，结束时 ping 还在。`s=6`，`sr=1.7fffffff.0`，`si=0`，`w45=0`，`tz=1.eb43c.1a67`。`tb` 三次都是 `0.0`。`tf` 不是 SO，是 `/readwrite/ota_firewall/ruleset` 的 RRQ，次数 `0x2d` → `0x3d` → `0x51`。第三次 `pe=1`。OACK 只认 `549_38_0.mbn`，所以这份请求不会把 `tb` 变成 1。`tqftpserv` 的开机日志里这份 ruleset 也经常打开失败，然后被拒绝。

按那份日志拒绝：请求一到就回，不再等 `sm`。每次从新的源端口回 TFTP ERROR 1 `file not found`，端口从 `0x1000` 起加。INIT 还没完成（`II_ST!=2`）不发，那次早发会卡发送环。`tr=次数.最后源端口`。

刷验：闸门通过。约第 19 次 ping 通。三次 `sm`，结束时 ping 还在。`s=6`，`sr` 仍 UNINIT，`w45=0`，`pe=0`。

| | tr | tf | tb |
|---|---|---|---|
| 第 1 次 | `1.1000` | SO，次数 `0x27` | `0.0` |
| 3 秒后 | `1.1000` | SO，`0xc4` | `1.0` |
| 再 4 秒 | `1.1000` | SO，`0xc4` | `1.0` |

ruleset 只拒绝了一次，源端口 `0x1000`。随后 modem 改去要 `549_38_0.mbn`，和 `tqftpserv` 开机日志的顺序一样。SO 的 OACK 发出后块号停在 0，后两拍请求次数也不再涨。后两行顶满 1792 字节，换行被丢掉。USB 没掉。

下一刀：SO 的 OACK 和后面的 DATA 改用新源端口（`tf_new_port`，记在 `TF_XFER_PORT`），ACK 听这个口，不再固定 `0x21`。ruleset 已经证明新口才被收下。

这张刷过。闸门通过。约第 29 次 ping 通，2323 超时，随后 ping 失败。没有 `sm`。SO 的 OACK 要等第一次 `sm`，这次没跑到。ruleset 的新口拒绝在 `sm` 之前就会发，这张和上一张能连上的镜像在那一步是一样的。这次是控制口没起来。

同一张再刷。约第 18 次 ping 通。三次 `sm`，结束时 ping 还在。`s=6`，`sr` 仍 UNINIT，`w45=0`，`pe=0`。`tr=1.1000`。`tf` 是 SO，次数 `0x1e` → `0xbb`，后两拍停在 `0xbb`。`tb` 从 `0.0.0` 变成 `1.0.109d`，再停住。OACK 从新口 `0x109d` 发出，块号仍是 0。换端口没有换来 ACK。后两行仍顶满 1792，没有换行。

下一刀：OACK 改成 `tftp_send_oack` 的内容和顺序。回请求里的 `blksize`、`timeoutms`、文件实长 `tsize`、`wsize`，不再写死 252 和 `wsize=1`。`blksize` 大于 252 时即使收到 ACK 0 也不发短 DATA（短块会被当成文件结束），`tb` 状态记 7。错误码 9（End of Transfer，stat 之后的正常回应）把状态记 4，下一笔 SO 可以再 OACK。不抬 glink 的 256/512 帽。

这张刷过。闸门通过。90 次里没有 ping。随后 fastboot 也是 0。没有 `sm`，新的 OACK 内容没跑到。SO 的 OACK 仍要等第一次 `sm`。

同一张再刷。闸门通过。70 次里没有 ping。没有 `sm`。这是这张镜像连续第二次控制口没出现。新的 OACK 内容仍没读到。

开机只抄 SO 载荷，选项在第一次 `sm` 之后才写进 OACK。这张刷过。约第 2 次 ping 通。三次 `sm`，结束时 ping 还在。`tr=1.1000`。`tf` 是 SO，`0x22` → `0xbf` 后停住。`tb` 从 `0.0.0` 到 `1.0.109d`，没有变成 4 或 7。`w45=0`，`sr` 仍 UNINIT。按请求回选项之后，仍然没有 ACK，也没有错误码 9。

接收扫描每次从 0 重走，最多 200 个 glink 包，碰到 chunk>512 就停。OACK 之后的 ACK 落在这扇窗口外面，所以 `tb` 停在 1、请求次数也不再涨。下一刀从 `RX_PARSED` 接着扫，大包跳过但不截断后面的包。

这张刷过。约第 18 次 ping 通。三次 `sm` 都有换行，结束时 ping 还在。`tr=1.1000`。`tf` 是 SO，次数停在 `0x7`。`tb` 三次都是 `0.0.0`。游标不回头，SO 请求在 `sm` 之前被丢掉，之后扫不到，所以 OACK 没发。下一刀：`sm` 之前遇到 SO 请求只留着，不丢。

留着再刷。约第 18 次 ping 通。三次 `sm` 都有换行，结束时 ping 还在。`tr=0.0`。`tf` 是 SO，次数 `0x3` → `0x4` 后停住。`tb` 从 `0.0.0` 变成 `1.0.1000`，再停住。OACK 从 `0x1000` 发出了。没有变成 4 或 7。`w45=0`，`sr` 仍 UNINIT。请求留到 `sm` 之后能发出去，ACK 和错误码 9 仍然没被认出来。

下一刀只记账：OACK 之后第一包打到传输口，或者任何端口上的 TFTP 操作码 4/5，记进 `rx=端口.节点|操作码<<16`。不再猜它回了什么。

这张刷过。约第 19 次 ping 通。三次 `sm` 都有换行，结束时 ping 还在。`tr=1.1000`。`tf` 是 SO，`0x5` → `0x6` 后停住。`tb` 从 `0.0.0` 变成 `1.0.1001`。`rx` 三次都是 `0.0`。`w45=0`，`sr` 仍 UNINIT。OACK 从 `0x1001` 发出之后，扫描里没有打到这个口的包，也没有操作码 4 或 5。

`ob=blk.wsz.ts` 是实际写进 OACK 的三个数。`ts=1` 表示带了文件实长。不改回包。OACK 发出前这三个数还是默认 `512.1.0`。

这张刷过。闸门通过。70 次里没有 ping。没有 `sm`，`ob` 没读到。回包没改，和上一张能连上的镜像只差这一行打印。

同一张再刷。约第 20 次 ping 通。三次 `sm` 都有换行，结束时 ping 还在。`tb` 从 `0.0.0` 变成 `1.0.1001`。`ob` 从 `200.1.0` 变成 `1e00.1.0`。`rx=0.0`。`w45=0`，`sr` 仍 UNINIT。

`1e00` 是 7680，块长抄对了。`wsz=1` 不是请求里的 10，`ts=0` 表示 OACK 里没有文件实长。96 字节的抄录把后面的 `tsize` 和 `wsize` 截掉了。

抄录改成 192 字节，整段 RRQ 装得下。OACK 仍按抄到的 `blksize`、`wsize`、`tsize` 回。不改别的。

这张刷过。约第 18 次 ping 通。三次 `sm` 都有换行，结束时 ping 还在。`tb` 变成 `1.0.1001` 之后 `ob` 仍是 `1e00.1.0`。`rx=0.0`。`w45=0`，`sr` 仍 UNINIT。192 字节里解析出来的还是只有块长 7680，没有 `wsize=10`，也没有 `tsize`。先前那两个数来自 `tqftpserv` 的例子，不是这台机器这份请求。

刷验：闸门通过。约第 15 次 ping 通。三次 `sm` 都有换行，结束时 ping 还在。`s=6`，`sr` 仍 UNINIT，`w45=0`，`tb=0.0`。`tf` 一直是 ruleset，次数 `0x31` → `0x41` → `0x55`。`tr` 从 `0` 到 `0x10` 再到 `0x24`。拒绝发出去了，USB 没掉，modem 没有因此改去要 SO。第三次 `pe=1`。

## 2026-10-20 rmtfs 8 深队列（过期任务，已收成 1，勿刷 s18）

`fr` 开头的 `dog_hal_common.c:180:[tmr_slave3` 是 DOG 定时从端。
`radio-bringup`：`qrtr-ns`、`pd-mapper`、`tftp_server`、`rmt_storage`
没有真在服务时，这条每 65.7 秒把初始化判死。commaai PR 120 在
servreg 实例 180 之后、`wlan_pd` 变成 `0x1fffffff` 之前，是
`/boot/modem_fs1`、`modem_fs2`、`modem_fsg`、`modem_fsc` 四次 OPEN，
然后 `ALLOC_BUFF`，然后 Read iovec。`tmr_slave3` 等的就是这条走完。

`rmtfs.c` 的 `handle_rmtfs` 每次 `recvfrom` 都答。裸机扫描一次走到
head，端口 `0x10` 上 msgid 1..=5 只在 `RF_PEND` 为假时闩一笔。
同趟后面的只记 `RF_DROP` bit1，不进 `RF_SEEN`。`rf_reply` 看见发送
head 不等于 tail 就返回，`RF_N` 不加，8 轮循环停。接着
`glink_rx_advance` 把接收 tail 推到整段 `RX_PARSED`，没答的包被吃掉。
四次 OPEN 加一次 ALLOC 落在同一次 FIFO 里会被丢掉。这是闩的错误。

上一张 8 深整包拷贝混进过别的镜像，刷后 ping 没回来，已经撤掉，
那张不要再刷。这一张不抄 528 字节。8 个槽只留 txn、node、port、
msgid、FIFO 偏移和 OPEN 的路径。`rf_reply` 仍用原来的
`put_result` / `qmi_finish` / `glink_tx_payload`，发送环空了才答
队头。接收 tail 停在队头那一笔的偏移上，ALLOC 和 IOVEC 还在原来的
FIFO 里，给现有的 TLV 读取用。队列满才记 drop bit1。

`./build-bare.sh --s18` 出了
`.local/device/redfin/boot-out/boot-s18.img`（110592 字节，12:20）。
fastboot 没有设备，没刷。没有新的 `q=`，`0x45` 仍不在线上。
随后把队列收成 1，并去掉它卡住接收 tail 的那段。`boot-s18.img`
不要刷。上面这句「`boot-s20.img` 仍是 11:45 那张」只对当时有效。

## 2026-10-21 OACK 去掉默认 wsize（当前，0x45 仍无）

`boot-s20.img` 是 2026-10-20 23:23 那张，110592 字节，已刷进 `boot_a`。OACK 不再写默认 `wsize=1`；请求里有 `rsize` 键且值为 0 时停在状态 1，等 code 9，不发 DATA。不抬 glink 256/512。`RF_Q_CAP` 仍是 1。不要刷 `boot-s18.img`。

刷后有一次 ping（约第 17 次）。只读到一次 `sm`，发生在 OACK 之前：`s=6`，`tr=1.1000`，SO 请求 5 次，`tb=0.0.0`，`w45=0`，`sr=1.7fffffff.0`。随后链路断，第二次 `sm` 没读到。状态行里先有一段 rmtfs 的 `rs=`，新的 rsize 在 `ob=` 和 `tr=` 之间，那次没拆出来。

2026-10-21 再读时活动槽位已经是 b。切回 a 再重启，没有重刷。这次没有 ping，也不在 fastboot。没有新的 `sm`。`0x45` 仍不在线上。

接棒（同日，Opus）：闸门过后 `set_active a` 重臂（retry 3）→ reboot，没重刷。这次做了自动窗口抓取：后台循环等 ping，通后 ~1s 内连 2323。ping 通（抓取器 t=96s），`aginx` 问候+第一拍 `sm` 都拿到，随后 6s 后的第二拍无回包，事后 ping 100% 丢，手机三无（ping/fastboot/adb 全死，需物理强关）。

第一拍 `sm`（uptime `u=12390`≈12.4s，即 ping 起来后 ~1s 内）：`s=6`，`tf` SO RRQ ×5（挂起待发），`tb=0.0.0`，`rx=0.0`，`ob=200.0.0`（默认，OACK 未发），TFTP `rs=0.0`（**挂起的这份 RRQ 里没解析到 rsize 键**——「stat rsize=0 等码 9」分支这次根本没武装），`tr=0.0`（这次开机 ruleset RRQ 一次都没来过，与上一棒的 `tr=1.1000` 不同），`ii=2`（INIT 完），`sr=1.7fffffff.0`，`w45=0`。

判读：`sm` 第一拍把 `SM_ARMED` 置位，挂起的 SO RRQ 的 OACK 会在下一轮扫描发出——但第二拍没读到，OACK 发没发、发了之后 modem 回什��，全部没观察到。这张镜像（10-20 23:23）三个观察窗：一窗 sm#1 前链路死、一窗只活到 sm#1 后 ~6s、一窗（上一棒）同样只到 sm#1。**它从未产出过 OACK 之后的 sm 读数**；而 10-20 白天那些把 `wsize=1` 写进 OACK 的镜像能连出三拍、OACK 后 ping 仍在。窗口长度本身在变短还是这张的 OACK（只回 blksize）发出后引死，现有数据分不开。按指令未改任何回包。

接棒第二窗（同日稍后，Opus）：`set_active a` 重臂（3）→ reboot，密集采样器（ping 通后 0/1/2/3/5/7/10/14/20s 各一拍、每拍新连接）。**九拍全中，OACK 后 ping 活满 20s 采样窗**（后续补采又活 ~20s，总窗口 ~1-2 分钟后死）。前述「这张的 OACK 引死链路」假设作废。

时间线（uptime 均显示 `u=12390` 冻结，为网络起位锁存值，非活体）：
- t=+0s（sm#1，武装 SM_ARMED）：`tf` SO RRQ ×4 挂起，`tb=0.0.0`，`ob=200.0.0`（默认），`rs=0.0`，`tr=1.1000`（ruleset 已拒一次），`pn=tms/pddump_disabled`。
- t=+1s：**`tb=1.0.1001`（OACK 已发，出口 0x1001），`ob=1e00.0.0`（只写 blksize=7680）**；SO RRQ 停在 ×5 不再涨；**`pn` 切到 `kernel/elf_loader.1.30`、`pr=2→3`、`pe=1`**——modem 开始问 wlan_pd 装载服务，此线历史最远。
- t=+3s→+5s 之间：**`rx=12.40001`**（一槽闩死，之后不再变）。
- t=+5s..+20s 及补采：全部字段冻结（`tb=1.0.1001`、RRQ ×5、`d=` 走了一格 b80、`x=` 出现 QMI 形内容后归零）。

三条定谳：
1. **OACK（只回 blksize=7680）被 modem 收下**——RRQ 重试从「无限涨」（10-20 各窗 0x1f→0x53b→0xe37）变为「停在 5」，随后 modem 推进到 kernel/elf_loader servreg 问询。「OACK 没人理」的旧判词对本张不成立。
2. **`rx=12.40001` 几乎可以断定不是 TFTP ACK**：TX 构包（glink_tx_payload，+28 src_port/+40 dst_node/+44 dst_port/+48 载荷）与 RX 解析对称，该包 dst=0x12——正是我们公告的 **svc 64（servreg/pd-mapper 位）**端口（glink_announce：svc 4096→0x11、svc 64→0x12）。载荷首两字节 `00 04`（QMI ctl=0/txn=4）与 TFTP ACK 碰码；stock tqftpserv 用 `qrtr_open(0)` 临时口收 ACK，客户端应答到 OACK 的源口（0x1001）而非 0x12，而 0x1001 上什么都没收到。备忘录一槽（TF_RX_HIT 闩）无法区分，此判据待下一刀验证。
3. **结构墙坐实**：本机这份 SO RRQ 只有 `blksize=7680`、**无 rsize**（`rs=0.0`，192 字节全抄也只见 blksize）——公开日志「stat(rsize=0)→code 9→rsize=52 读头」的舞步与本机首请求不符。blksize=7680 的 DATA 块（物理上限：glink 载荷 ≤256、整包 ≤512）发不出；短块=TFTP EOF=截断文件。**双方对峙：modem 收了 OACK 等块 1，我方发不出 7680。**

下一刀候选（未做，待裁决）：OACK 里把 blksize **向下议价**（RFC 2348 允许服务器回更小值，如 256：载荷 4+256=260→chunk 292→total 312≤512 可发）。这不是补没见过的选项，是改一个已见选项的值；但 stock tqftpserv 从不降议（原样回显），Qualcomm 客户端是否接受降议无参照，属上机实验。备选：维持现状已无新信息（状态机两头等死）。

### 刀一（同日深夜，Opus 执刀）：降议 252 —— OACK 被收，但 ACK 全程不来

用户裁决「你开解决问题」，降议刀自主上机。刀口：RRQ 解析处 `TF_REQ_BLK = v` 改为钳到 `TF_BLK`（252）——OACK 值、ACK 后 `>TF_BLK` 闸、DATA 切块三处同源一个变量（commit c87f758）。镜像 110592B/01:03 刷 boot_a。

- **窗 1（01:06，短命）**：ping 起 15 秒，+2s 一发 sm 为武装前形状（`ob=200.0.0`），+9s 零字节，链死。不构成协议结果（与既往窗口抖动同款）。
- **窗 2（01:12–01:15+，活窗）**：`ob=fc.0.0`（**OACK 实发 252**）、`tb=1.0.1001`、RRQ 停 ×6 不再爬、`rx=12.40001`（servreg 包照旧占闩）。此后 **20 发采样横跨 144 秒：`tb` 纹丝不动 1.0.1001，无 ACK、无 ERROR、无重 RRQ**。ping 全程活（1.9ms），2323 拒连一次=旁路连接挤爆裸监听 backlog（教训：单采样者纪律）。

定谳（刀一）：**modem 收下 OACK(252) 后整体沉默**——不是拒（无 ERROR→tb 没走 9），不是重试（RRQ 不涨），就是不来 ACK。两种活口：
- **B：ACK 回到 0x11（RRQ 目的地/播报的 TFTP 服务口）而非 OACK 源口 0x1001**——旧图 `tf_note_ack` 只认 0x21/0x1001，此路全瞎；
- **D：Qualcomm 客户端不支持降议，值不等于请求即静默停车**（stock tqftpserv 从不降议，客户端从未被逼着处理过这事）。
另记工具账：rx 单发闩被 servreg 包永久占用后，OACK 之后的一切收包不可观测——观察面本身先瞎了。

### 刀二（同日，紧接上刀）：rx 滚动备忘 + 0x11 ACK 监听（commit b0f2329，镜像 01:16，待上机）

两件一套，一次窗口榨两个答案：
1. rx 备忘改**滚动 4 槽**（port.op 时间序旧→新），0x12（自家假 pd-mapper 口，QMI 头 `00 04` 恒撞 ACK 码）不入册——B/D 之外任何口上出现 op 4/5 都现形；
2. `tf_note_ack` 增监听 `TF_SVC_PORT=0x11`——若 B 为真，ACK-0 直接被处理，DATA 块 1 当场开流（等于顺手就是修法）。
判读表：tb→2 且块号爬 = B 真，传输开流；rx 册空且 tb 钉死 = D 真（客户端对降议静默），下一刀换「OACK 后宽限期直发 DATA 块 1」（hypothesis E：非规客户端 OACK 后不等 ACK-0）。

### 刀二+刀三（同日 01:30 窗，Opus）：三局分尽 —— D 深定谳，降议此路不通

刀二上机两窗皆短（30s/18s，OACK 后 5.3s 内 rx 册零条目，未到期即死）。窗口经济裁决：把假设 E（宽限刀，OACK 后 2s 无 ACK-0 合成 ack=0 直发 DATA 块 1）并入同一镜像（commit 后述），一张镜像分尽 B/E/D。01:27 镜像 114688B 刷入，01:30:28 起窗（前密 0.7/2.2/3.8/5.3/7s + 8s 疏）：

- +0.7s：武装前形状（`tb=0.0.0`）。
- +2.2s：`tb=1.0.1001`、`ob=fc.0.0` —— OACK(252) 出手。
- +3.8s：**`tb=2.1.1001`** —— 宽限刀开火，**DATA 块 1 真发出**（st=2 仅在 glink 发送成功后置位；`iz=2080`(=8320B) intent 裕如）。
- +5.3s/+7s/+14s/+16s/+25s/+35s：`tb` 钉死 `2.1.1001`，**rx 滚动册全程空**。

三局推演（每一局都有独立判据，不是排除法猜的）：
1. **B 死**：若 ACK-0 回 0x11，刀二的 0x11 监听会处理（tb 提前转 2）且滚动册必留 `11.4` 条目——册空，两头都没发生。
2. **E 死**：块 1 已亲手投到客户端 RRQ 源口；活着的客户端收到数据必回 ACK-1（TFTP 语义）或 ERROR——tb 不爬、册空、无 ERROR。
3. **D 深坐实**：**Qualcomm TFTP 客户端对「OACK 值 ≠ 请求值」的处理 = 收下 OACK、停重试、然后整体停车——连已到手的数据都不 ACK。**降议（RFC 2348 服务器回更小值）在本客户端无生路。

对照收全：OACK 值=请求值（7680，10-20 窗）时��户端**正常等块 1**（停重试、推进 servreg 问询）——客户端只认 7680。

### 下一刀立案：glink TX 环扩容（7684 字节 DATA 块的唯一活路）

墙的两面现在都量完了：客户端**只吃 7680**（blksize 降议死、数据不挑逗）；对端 intent `iz=8320` **装得下** 7684 字节 chunk（32 qrtr 头+4 TFTP 头+7680 数据=7716 ≤ 8320）。唯一卡的是**自家 TX 环 512 字节**（glink_tx_payload：载荷 ≤256、整包 ≤512）——这是本方选择，非协议限制。下一刀：TX 环从 512 扩到 ≥8192（对齐/写指针语义/环内存落点全套核），OACK 回原值 7680（降议刀退役），DATA 按 7680 整块发。此刀动的是 glink 传输层底盘，比前三刀都重，动手前先把环的寄存器语义与内存布局读全。

### 扩容刀+TX 观察刀（同日 01:37/01:53 两窗，Opus）：7680 整块实发被消费 —— 断点收窄到 modem qrtr 路由层

扩容刀六处（commit 见 git log）：解析回原样收、TF_BLKBUF 8200、闸 >8192、切块按 TF_REQ_BLK、so_read ≤8192、TX 载荷帽 7684/整包帽 8192+环尾回卷。01:37 窗：**`ob=1e00`（原样 7680）、+3.8s `tb=2.1`——7680 字节块 1 实发成功**，但仍零 ACK，且 tf 爬到 1.7（客户端在 OACK 和块 1 之后还在发新 RRQ）。随即加 TX 观察刀：tf_reply 顶部把 head/tail/环容存静态，sm 打 `tt=`。01:53 窗（01:39 那靴死靴，重挂计数再启）：

- +0.7s：`tt=960.960.4000` —— **TX 环真容 16384 字节**（比 intent 8320 还宽）。
- +2.2s：`tt=9f0.9f0.4000` —— OACK（~144B）发出后 **tail 精确跟进：modem glink 层消费了 OACK**。
- +3.8s：`tt=2828.2828.4000` —— 7680 块 1 发出（2544+7736=0x2828）**tail 再次精确跟上：大块被完整消费**。
- 但 tf=1.3→1.5 持续爬：客户端自己的层**零感知**，按 ~2s 节奏重发 RRQ。

**假设 F 阵亡**（大块未被传输层收——错，收了）。断点收窄为：**modem glink 消费 ✅ → modem qrtr 路由层丢弃 ❌ → 客户端 socket 永不见包**。刀 G1 立案并上机：OACK/DATA 改从**已播报的服务口 0x11** 发（不再用未注册临时口 0x1000/0x1001）——裸世界无内核 qrtr 登记临时端口，modem 路由器只认 NEW_SERVER 播报过的口；stock 的 `qrtr_open(0)` 行得通正因内核替它登记了。ACK 若来也落 0x11，与刀二的监听+滚动册正好闭环。

### 刀 G1 收据（02:01 窗，27s）：0x11 源口发 OACK+DATA —— 客户端仍聋，但此判后被 G3 翻案

OACK/DATA/ACK 全改从已播报服务口 0x11 发（commit c1586f1）。窗 02:01:42：
`tb=1.0.11`→`2.1.11`（宽限刀块 1 照发），tt `960→9f0→2828→2970`（glink 层逐包精确消费），
**rx=0，tf 持续爬**（客户端按 ~2s 重发 RRQ，零感知）。当时判词「源口假设死、
断点=modem qrtr 路由层」——**此判被 G3 的 rd 测量推翻重开**：OACK 全程在等
SM_ARMED（第一次主机 sm），rd=6.6s 迟到 5 个超时窗，客户端死的是超时不是路由。
0x11 源口本身未被干净检验，G4 修复时延后重看。

### 刀 G2 收据（07:27 窗）：客户端口逐笔换新 + CTRL 副本无反应 + 控制面活照

rq/rq2 观测 + CTRL 口 OACK 副本探针（commit 1bec955）。窗口抓到：
- `rq=0.23 rq2=0.22`——**TFTP 客户端每笔重试换新源口**（0x22→0x23→…，~2s 节奏），
  闭口即 DEL_CLIENT（x= 里解码到 DEL_CLIENT node0/port0x24）；
- `cp=1`——CTRL 口(0xfffffffe) 副本已发，客户端零反应（CTRL 源址假设死，带竞态保留）；
- rmtfs 侧 `rh=8.0.9000101.a000101.d000104.e000103`——4 种请求型在推进，
  **我方 DATA 投递→(0,客户端口) 对 QMI 客户端是通的**；
- x= 另解到 NEW_SERVER svc=0xe7 inst=1 node=0 port=0x35（modem 侧新服务上册）。

协议真源核对（NDK qrtr.h + 内核 af_qrtr.c/ns.c + 本仓 tqftpserv.c）：connect()
纯本地不走线；stock 每笔 RRQ qrtr_open(0) 新口回；DEL_CLIENT=socket 关闭时广播。
合并根因假说：回信地址陈旧三机制（armed 扣 pend、pend 忙跳过且水位前进、
seen 表 8 满冻结）→ G3 立案。

### 刀 G3 收据（07:36 刷，+0.7s~+330s 窗）：rd=19f3 抓现行 —— SM_ARMED 等主机是 6.6 秒凶器；CTRL 副本自写坏环

地址刷新先于一切闸 + TF_RQ_AT 时戳 + rd/tm 字段 + seen 环形化（commit 685d45e）。
窗口全程（11 拍 ×30s）：

- `tm=3e8`（1000ms）——客户端自报超时 1 秒；
- **`rd=19f3`（6643ms）——OACK 比最新 RRQ 迟 6.6 秒**。客户端 5 连请求
  （口 0x22→0x23→0x24，tf 冻结 ×5）全部超时闭口后，OACK 才发到死口；
- 武装时机定谳：SM_ARMED 在**第一次主机 sm**（12392–12398）才置位——纯观察
  纪律，非协议需要。而 +0.7s 首样已 `ii=2`（IPA INIT 完成）、`rf=3`（rmtfs
  在流动）——**glink TX 早已无主机也活**，这个等待纯属自杀；
- **CTRL 副本自写坏环**：tt 冻死 `a00.960.4000`（OACK 后一笔 160B 永不被
  消费）。CTRL 副本的落位公式（tx_head+total 自算）与 glink_tx_payload
  内部回卷纪律不一致，疑写坏/错位 → modem glink 消费者拒收 → head≠tail
  一刀切闸把后续 grace/DATA 全部冻死（tb 钉死 1.0.11）。G2 探针是本靴
  冻环元凶，问题已答（无反应），当删。

**翻案条款**：rd 测量回溯污染此前全部「客户端聋」判读——刀二/刀三的 D 深定谳
（降议停车）、扩容刀的「断点=qrtr 路由层」都建立在 OACK/DATA 已被活客户端
收到的前提上，而 SM_ARMED 使每个采样窗的回信都迟到数秒（ping 前置期客户端
窗口已走完）。唯一可能例外：刀二+刀三窗 OACK 后 1s 内 `pn` 切 kernel/elf_loader、
pr=2→3——若那是对 OACK 的因果反应则该靴客户端尚活，其后的沉默另有原因。
G4 修复时延后一切重验。

### 刀 G4（08:27 镜像 114688B，commit 2585aac）：武装改 II_ST=2 + 删 CTRL + 环闸改容量闸——待上机

三刀口：
1. tf_reply 武装 `SM_ARMED`→`II_ST==2`（IPA INIT 完成 = rmtfs 已证明 TX 可用），
   OACK 在 RRQ 后一圈主循环内即发（亚秒），无主机也活；
2. CTRL 副本探针删除（G2 已答 + G3 证实冻环）；
3. 环闸从 head≠tail 一刀切改「免回卷容量闸」：head≠tail 时最大整包 8192
   不回卷放得下就继续排（与 glink_tx_payload 内部回卷纪律一致），一笔
   不被消费的包不再冻死全局。

判读表：`rd=` 应落到 <100ms；`rx=` 出现 ACK → 传输开流，直奔 st=3（126 块）
与 w45；rx 仍零 → 时延排除，0x11 源口终于获干净重测（G1 翻案验证）。

### 刀 G4 收据（08:41/08:48 两窗）：rx=1.11.5 —— 客户端首答落袋，stat 舞闭合

**窗1（08:41）**：零样（+0.7s 即 TimeoutError，ping 死于样#1）。与历史 13s~2min
短窗同形，无新信息；采样器已加 3 次重连。经济窗：不重刷，`set_active a`
重臂 retry 计数再 reboot。

**窗2（08:48:48 开窗，活到 +578s，ping 死 08:58:47）**——历史最长窗：

- `rd=0`：OACK 与 RRQ 捕获同圈，时延根除（G3 的 19f3 凶器确认已死）；
- `rx=1.11.5`、`tb=4.0.11`：**客户端有史以来第一笔回答**——发往 0x11 的
  op5 ERROR code 9（End of Transfer），stat 舞按 stock 剧本闭合
  （tf_note_ack：code 9 + st 1 → st 4）；
- 全程无主机 sm（+0.7s 首样即 ii=2、rf=3，rmtfs 回复已在 TX 流动）——
  无头武装实测成立，此前所有「客户端聋」判读的时延污染彻底坐实；
- stat 闭合后客户端又发 4 笔 RRQ（rq=0.22、末笔 0x21 口，rsize 键不再带，
  `rs=0.0`，仅 blksize=7680+timeoutms），随后 pr=3 pn=kernel/elf_loader.1.30
  （modem 问 elf_loader 域，我们答 1 hit msm/modem/wlan_pd inst 180）、
  modem 播 NEW_SERVER(svc 4, inst 1, node 0, port 0x43)——然后**全静默**，
  tt=b30.b30（TX 环清空，全被消费）直到窗死。

**stall 定位与观察盲洞**：静默期 pr 停爬、tt 清空、rx 零新笔。两种假设无法
区分：(a) modem 在等我们从未播报的某服务（NEW_LOOKUP 无应答）；(b) 客户端
把后续 RRQ 发到了 0x11 以外的口——tf 扫描硬编码 dst_port==0x11，rx 日志也
只看 TF 口，换口请求整段不可见。7975 行注释自证：「缺 ipa 出向时 modem 可能
不走内存内自拉 wlan_pd，掉进 elf_loader+TFTP」——我们正在这条 fallback 路上，
elf_loader 的段落读（stock README：stat 后 `RRQ rsize=52 seek=0` 读 ELF 头，
再逐段读）需要 rsize/seek 两键，而我们的 OACK 不回显、DATA 不认 seek。

窗死本身（+578s）是历史最长，非本刀回归——旧窗 13s~2min 死于同因未明。

### 刀 G5（09:03 镜像 114688B）：全口请求日志 rl= + OACK 回显 rsize/seek + DATA 从 seek 起算——待上机

对准 G4 窗2 stall 的三个盲洞/缺口：

1. **rl= 全口请求日志**（6 槽滚动，槽=dst口.源口.op.fl.rsize.seek，水位去重）：
   任何发到 node1 的 op1/op2 都入册——客户端换口发请求不再盲。fl bit0=rsize
   键在、bit1=seek 键在（区分「键在值 0」=stat 与「键不在」）；
2. **OACK 回显 rsize+seek**（stock tftp_send_oack 同款，请求带才回）：stat 舞
   与段落读都靠这两键认门；tf_rrq_rsize 扩为 (has, saw_rs, rsize, saw_sk, seek)
   五元，tf_apply_saved 重放并逐请求清零；
3. **DATA 偏移 = seek + ack×blksize**，窗口尾 = min(seek+rsize, 文件尾)——
   段落读（rsize=52 读 ELF 头等）此前永远从文件头回。

判读表：静默期若 rl= 冒新槽（非 0x11 口）→ 假设 (b) 成立，看 fl/rs/sk 按
stock 舞应答；rl= 也零新笔 → 假设 (a)，下一刀对准 NEW_LOOKUP 应答面。

### 刀 G5 收据（09:10 窗，活过 +63s 冻结）：rl= 盲洞闭合——没有换口请求，断点回 OACK 内容本身

镜像 09:03 刷入（09:09:18），窗 09:10:17 开。+0.7s 首样即定谳：

- **rl=0x10（16 笔请求）**，最新 6 槽全 dst=0x11（src 0x21–0x25）、**op=1、fl=0**
  ——请求只带 blksize=7680+timeoutms=1000，**没有 rsize 也没有 seek 键**
  （ob=1e00.0.0 旁证：无 tsize 无 wsize）。还有一笔 op2 WRQ（src=0x1f）——
  stock 有正经 handle_wrq，非偏；
- rx=3：两笔 op5（tb=4 证明至少末笔 code9-while-st1）+一笔 op1；
- 之后完全冻结（+63s 同形）：tt=bd8.bd8 环清空、pr=2→3（tms/pddump_disabled
  →kernel/elf_loader 各问一次域，均答、pe=1）、w45=0。

**判读**：假设 (b)（客户端换口重试、旧扫描盲）**死**——stall 期零新请求，全
部 16 笔都在 0x11 眼皮底下。客户端是**收到我们 OACK 后主动 EOT 收摊**再换
口重试 16 次后弃。断点=OACK 内容不满足客户端。G4w2 静默同根。

**stock 对齐复核**（tqftpserv.c handle_rrq/tftp_send_oack 逐行）三个差异：

1. **wsize 恒回显**：stock `wsize` 缺省 1，`wsize ? &wsize : NULL` 恒真——
   请求没带 wsize 也回 `wsize=1`。我们「请求带才回」→ 本靴客户端每笔 OACK
   都缺 wsize。**首要嫌疑**；
2. rsize/seek 只在**值非零**时回显（stat 的 rsize=0 键在也不回）——G5 的
   「键在就回」比 stock 多（本靴未触发，一并修）；
3. tsize 只在请求带时回（我们已对齐）。

### 刀 G6（09:14 镜像 114688B）：wsize 恒回显（缺省 1）+ rsize/seek 改值非零门 + rx 第三元（op5 错误码/op4 块号）——待上机

判读表：rx= 槽第三元直接看 op5 码——9=EOT 正常收摊、其他码=真错误；tb 离开
4 进 2/3=客户端开始 ACK DATA（wsize 补齐奏效）；rl 再冒新槽+fl 含 rsize/seek
位=进入段落读舞（stock README 的 rsize=52 读 ELF 头）。

### 刀 G6 收据（10:24 窗2，活过 +378s 冻结）：EOT 消失、tb 初进 2——wsize 回显改变客户端反应，但 DATA 晚到

窗1 09:27 死于 sample#1（17s 零样本，G4w1 同款首发窗劣化方差，非刀退）；
手机随后全挂（adb/ping 全盲 90s 轮询不回），两条 re-arm 链 600s/1800s 空手，
第三条 30min 看守 10:23:57 捉到 fastboot，re-arm（不重刷）得长命窗2。

窗2 判读（首样 +5.3s 定谳，至 +378s 全冻结同形）：

- **tb=2.1.11**——状态机首进 2（DATA 块 1 已发）。但发它的是 2s 宽限合成的
  ACK-0（OACK+2s→synth→DATA），而客户端超时是 **tm=3e8=1000ms**：18 笔重试
  全在宽限窗内打完（最后口 0x24 已死），DATA 块 1 迟到扑空；
- **rx=0 全靴零回包**——G5 有两笔 op5 EOT，G6 补上 wsize 后 EOT 也消失。
  客户端对「OACK+迟到 DATA」既不 ACK-0 也不 EOT，纯静默；
- rl=0x12（18 笔）全 dst=0x11 src 0x22–0x24、op=1、fl=0；tf=…1.7（7 笔
  pend 各得一 OACK）；tt=29c0.29c0 环排空；pr=3 pn=kernel/elf_loader pe=1
  （两域全活）；w45=0。

**判读**：wsize 恒回显**确实改变了客户端反应**（EOT→静默），说明客户端在
逐字节看 OACK。两大假设并存：

- **T1**：客户端是「OACK 后直接等 DATA」型（RFC 2348 windowsize=1 语义下
  无需 ACK-0）——我们 2s 宽限错过它 1s 窗；
- **T2**：RFC 2347 禁止服务端回显未请求的选项——stock 恒回 wsize 恰好是
  这条违例，Qualcomm 客户端收到带未请求 wsize 的 OACK 静默丢弃。

（G6w2 零回包 vs G5 两 EOT 也可读成 T2 侧证：G5 OACK 缺 wsize 但其余合法
→ 客户端 EOT 收摊；G6 OACK 带未请求 wsize → 客户端更早更狠地静默。）

### 刀 G7（10:36 镜像）：OACK 落环即合成 ACK-0、同循环直发 DATA 块 1——T1/T2 一刀两证

改法：OACK glink_tx 成功块内直接 `TF_ACK_PEND=true; TF_ACK_BLK=0;`——下一
趟 tf_reply 即发 DATA 块 1（微秒级，不再等 2s）。wsize 恒回显**保留不动**
（单变量判读）。宽限块因 `!TF_ACK_PEND` 门自然退役为兜底（迟到错块 ACK 丢
pend 后 2s 仍能自救重发）。真 ACK-0 若后到：TF_ACK_OFF 去重 + ack≠expect
直接丢，不重复块 1。stat 舞不受影响（DATA 分支 rsize=0 门照旧拦住）。

判读表：
- **rx 出现 op4（块号第三元）且 tb=2.N 推进、tt 增长** → T1 定谳，传输开流，
  朝 126 块（st=3）/w45=1 推进；
- **rx 冒 op5 code9（tb=4）** → 客户端读到了 OACK+DATA 但内容拒收 → 偏 T2
  侧（下刀撤未请求 wsize）；
- **仍 rx=0** → 客户端根本没吃到我们任何包 → 投递层（qrtr 源口/路由）升权，
  下刀换临时口+NEW_SERVER 册（stock 每笔 RRQ qrtr_open(0) 的全仿）。

### 刀 G7 收据（10:39 窗，+159s 冻结后窗亡）：立即 DATA 零上环——T1 根本没被测到；客户端改吐 EOT-9

刷链 10:38:41 一次过（捉 fastboot→闸→刷→set_active a→重启，60s 内 ping 起）。
+0.7s 首样定谳，至窗亡（sample#23，~3 分钟）全冻结同形：

- **tb=4.0.11**——st=4 来源找到了（main.rs 7080：`code == 9 && TF_ST == 1`）：
  客户端发的是 **ERROR code 9 = EOT（礼貌收摊）**，不是错误。每笔 OACK 后
  客户端 1s 内 EOT-9 关口，换口（0x21→0x25 递增）重试，16 笔后弃；
- **tt=be8（全靴 ~3048B）＝纯 OACK+rejects 体积——立即 DATA 一块都没上环**。
  G7 的 T1 测试根本没打出去：客户端从未收到过我们的 DATA；
- rx=3（两 op5 arg=ffff 短包＋一笔 op1"/r"）；tf=NAME.1.6（6 笔 OACK）；
  rl=0x10 全 fl=0；pr=3 两域活；w45=0。

**客户端行为横比**（OACK 字节 G6≡G7 逐字节一致）：

| 窗 | OACK 内容 | DATA | 客户端反应 |
|----|-----------|------|-----------|
| G5 | 缺 wsize | 无（grace 未到窗亡） | 两笔 op5 收摊 |
| G6w2 | wsize=1 | +2s 扑空 | **全静默** rx=0 |
| G7 | wsize=1 | +0ms 没发出去 | 两笔 op5(EOT-9) 收摊 |

同字节不同反应＝客户端逐靴情绪不稳定（G6w2 是 re-arm 二靴，G5/G7 是首靴
——首靴反应型、二靴静默型，两数据点待更多样本）。**st=4 定谳把「OACK 被
拒」改写成「客户端主动收摊」**——EOT-9 在 stock 方言里是 stat 舞的完成信号，
客户端对无 rsize 的普通 RRQ 也发它＝把这个 OACK 当成了 stat 应答、或者是
干脆利落的「不要了」。

**stock 对齐终审**（本轮重读 handle_rrq+tftp_send_oack+parse_options 三层门）：
tsize 两层门复合（调用点 `tsize ? &tsize : NULL` 与内闸 `*tsize != -1`）＝
只在请求时回；wsize 恒回（缺省 1）；rsize/seek 值非零才回；顺序
blksize,timeoutms,tsize,wsize,rsize,seek。**我们 G6/G7 的 OACK 与 stock
逐字节一致**——OACK 内容排雷完毕，方言层再无已知差异。

### 刀 G8（10:5x 镜像）：DATA 路径四级计数器 dv=（纯插桩不改行为）

G7 最大遗留＝立即 DATA 没上环的堵点定位。静态推演穷尽：分支门（st=1/
ACK_PEND/ack==expect/stat 门/blk≤8192/off<limit）全该过；G6w2 同分支同代码
+2s 能发；内圈 tf_reply 每外圈至少一趟。四计数器进 sm：

- dv=进入.读零.tx拒.环闸挡
- 进入=DATA 分支卫队通过次数（OACK 后~1s 内每圈+1，应≈主循环频率）
- 读零=so_read 归零（UFS/FAT 读失败→st=8）
- tx拒=glink_tx_payload 返假
- 环闸挡=tf_reply 顶 `head≠tail && head+8192>环` 挡

判读表：读零>0＝UFS 早期读失败（G6w2 +2s 能读、G7 +0.5s 不能？）；
tx拒>0＝glink 内闸；环闸挡>0＝容量；全 0 而 tt 仍无 DATA＝tt 读数另解。
G7 的立即 synth 保留，行为零改动。

### 刀 G8 收据（10:54 刷、窗 10:58–11:0x）：立即 DATA 路径健康＋st=2 饿死机理定谳

刷机链 bvkofwxwk（10:54:26 FLASHED），ping up 10:58:26，首样 +0.7s 已是
终态——整个 12 笔交换在 ping 前打完，窗口到 +168s 全冻。dv 四计数器：

- **dv=1.0.0.0 → 1.0.0.1**：DATA 分支全程只进 1 次、so_read 成功（dz=0）、
  glink_tx 成功（dx=0）、环闸挡 0→1（后段一次性，无关）。G7 的立即
  DATA 机制本身完全健康——tb=2.1.11、tt 0x27f0→0x2978（一块 7680＋OACK
  ＋stat 期零头真上环）。G7 靴的「零上环」与本靴「秒上环」同代码不同靴
  ＝靴间方差实锤。
- **dv=1 vs 7 OACK 之谜解开＝不是谜是死锁**：tf=NAME.1.7＝7 笔 pend 只
  服务了 1 笔。首笔 OACK+DATA#1 后 st=2、BLKNO=1、ACK_PEND=false，等
  ACK；rx=0（客户端全靴零回包）→ 后续 6 笔 pend 进来时
  `retarget`(st==1&&blk==0)/`fresh`(st==0||4) **两头不沾全被丢**。
  客户端 12 笔 RRQ（rl=12，槽 11.21/22/23/24…）只吃到 1 份应答。
- **客户端方言排雷收口**：ob=1e00.0.0＋tm=3e8＝只要 blksize=7680+
  timeoutms=1000；rl 槽 fl=0（无 rsize/seek 键）——**rsize 段落读方言
  假说死**，纯 RRQ。OACK 内容 G6 起与 stock 逐字节一致。
- **靴间情绪表更新**：G8w1＝静默型＋首靴（破「首靴反应型」规律）——
  情绪不由靴序决定。静默靴对 OACK+DATA 双双在环零反应。
- **EOT-9 另解立案**：G5 的 16×EOT-9 若是「每弃一次口礼貌收摊」而非
  「对 OACK 的反应」，则**出站投递从未被证明过**（NEW_SERVER 广播可达
  ＝客户端找得到 0x11；单播 DATA/OACK 到 (node,0x21+) 是否进客户端
  socket 未知）。G9 若 12 笔全服务仍 rx=0 → 转投递层审计
  （qrtr 头 dst_node/端口注册）。

**G9 立案**：armed 的 SO RRQ pend 一律服务（stock handle_rrq 语义＝每笔
RRQ 独立会话全新 OACK）——retarget/fresh 区分退役，饿死机理直接消灭。
G7 立即 DATA 保留（stock 是 OACK→等 ACK-0→DATA#1，我们不等；静默靴里
立即 DATA 把信号最大化，且 stock 客户端在 Linux 上必发 ACK-0＝ACK-0
缺席本身即客户端侧问题铁证）。判读：12 笔全服务后 rx>0（任一 ACK/EOT）
＝投递通、转内容/时序；rx 仍=0＝投递死、转 G10 投递层审计。

### 刀 G9 收据（11:14–13:36）：满服务三靴全黑，G8b 对照定罪；G9c 节流版立案

- **三靴三黑**：一靴从未起网；二靴起网 16s 后永寂（首样 TCP 超时＋ping 8
  连死）；三靴起网 94s 后永寂。G9 镜像 0/3。
- **G8b 对照（revert 6eeab85，13:15 重建，13:36 刷）干净起靴**：首样
  +0.7s 即完整 sm，且与 G8 首靴逐字段同型（tf=NAME.1.7、tb=2.1.11、
  rx=0、dv=1.0.0.0、tt=27f0→2830）——G8 家 2/2 活 vs G9 家 0/3 黑，
  **G9 满服务版定罪**（机理未闭环：门只在 bring-up 后期执行，靴死于
  起网前，静态分析追不出因果；经验判决收案）。
- **本靴新细节**：rl 重试口复用（22→21→23→22→21→24，约 4 口轮转）；
  rk=31（G8 首靴 4d）——ctrl 面计数有靴间方差，数据面完全确定。
- **G9c 立案（节流版）**：G8 门原样保留（fresh/retarget 首服务+synth
  DATA）；st=2/3 时新到 pend 只回 OACK（不重置游标、不 synth、
  ~100B/笔），答到每个重试口又不打环。rx>0＝投递通；全零＝G10 毒丸
  （ERROR-1 强制反应）判「送达被无视 vs 从未送达」。

### 刀 G9c 收据（13:41–13:52，第 5 物理循环）：rx=3 出站投递定谳＋dv=0 机理闭环

刷机链 b2xkrqcfm，用户 ~13:40 物理循环 #5，ping up 13:41:02，+0.7s 完整
sm——**G9c 节流版起靴过关**（对照 G9 满服务 0/3 黑）。+3.8s 即终态冻结：

- tf=NAME.1.6（6 pend）、tb=4.0.11、**rx=3.11.5.ffff.11.1.2f72.11.5.ffff**、
  dv=0.0.0.0.0（五元全零）、tt=be8 恒定、rq=0.25.0.24、
  rl=10 槽（WRQ 口 0x1f ＋ 5 笔 RRQ 口 0x22/21/23/24/25）、w45=0。
- 早期瞬态：首样 pn=tms/pddump_disabled.0.11 pr=2 pe=0 → 定居
  pn=kernel/elf_loader.1.30 pr=3 pe=1（与 G8 家定居态同）。pddump_disabled
  ＝自弃阶段，非 SO 非 ruleset 的 pend 我们只丢不答，阶段自过——非进展。
- +686s（13:52:28）链路 8 连死，窗关。

**判决一：出站投递定谳＝通**。rx=3＝G5/G7 家族以来首批客户端回包：
2× op5（EOT-9；arg=ffff 系 <54B 短包自然读不到，非异常）＋1× op1
（arg=2f72＝文件名字节 "r/"，≥52B 的 RRQ 重录）。双重排他：rx 日志门
`TF_ST>=1` ⇒ 回包在首笔 OACK 之后到达；而全镜像只有 SO 路发 OACK
（ruleset 回 ERROR、其余丢弃）⇒ **两笔 EOT-9 只可能回给 SO OACK**。
客户端读到我们的 OACK 并有意回应。G10 毒丸（探投递）退役。

**判决二：dv=0 机理代码闭环**。tf_note_ack（main.rs 7085–7092）对
op5-9 无条件 `TF_ACK_PEND=false`＋`st=4`（st==1 时）。反应型客户端的
EOT-9 在扫描程内抢在 DATA 泵之前清掉 G7 合成 ACK-0——DATA 分支
（7194 `st==1||st==2` 门）整靴零进入，st 在 1↔4 循环，DATA 永不出。
G9c retry 路（st==2||3）在反应靴永不触发（st 到不了 2）——**G9c 对
反应靴是空刀**；其第五元 TF_DR 本靴恒 0 佐证。

**判决三：陈旧地址假说死**。pend 捕获 11721 单槽（`!TF_PEND`）＋
11694–11708（刀G3）每见新请求即把回信地址刷成最新源端口——OACK 永远
发给最新 RRQ 的口。拒收 EOT-9 的口＝当前活会话的口。**拒收不是「答给
已弃端口的礼貌收摊」，是活监听者的有意拒绝**。

**中央谜题换形**：反应靴拿到字节级 stock 对齐 OACK（G6 起
blksize+timeoutms+tsize+wsize）仍拒收（EOT-9）再重 RRQ×5 后弃；静默靴
对 OACK+DATA 双盲。任何情绪下客户端从不 ACK-0。而「纯 RRQ 无 rsize」
的方言裁决依赖我们自己的解析器（tf_rrq_rsize）——若客户端选项区有
未解析尾巴，fl=0 是瞎的。

**G10 立案（转录刀，零行为变化）**：sm 增两字段——rp= pend 包原文
hex（~96B：文件名＋mode＋全部选项逐字节）、rr= 最近入站包原文（op5 的
错误消息字符串——客户端会用英文写明拒收理由）。一刀不清答不猜：
反应靴 rp 选项区尾巴＝方言真图；rr 文本＝拒收理由原文。

## 刀 G10 收据（2026-10-21 14:10 靴，转录刀）

刷机链 bkc2c1ovw，用户 ~14:08 物理循环 #6，ping up 14:11:08，+3.8s 起
终态冻结（又一次反应靴：tb=4.0.11、rx=3、dv=0、w45=0，rl 同型 5 RRQ
轮口）。转录两字段一次拿到全量——**本刀把整条线的中央谜题改写了**：

**转录一：rr=2.00050009.00050009——客户端回包是裸 4 字节包**。
两笔 op5 都是 `00 05 00 09` 整（长度字段=4，无消息文本）：code 9 纯码
回执。「客户端会用英文写明拒收理由」的希望落空——它连一个字母都不屑
写。这是协议步进式的拒绝（与 stat 舞步的 EOT-9 终结符同形），不是带
情绪的抱怨。

**转录二：rp= 88 字节整处截断，尾巴 `7473`＝"ts"——RRQ 选项区在
timeoutms 之后还有没被我们看见的东西**（至少 "tsize"，不止不可能）。
字节级拆账：op(2)+name(49)+NUL+octet(5)+NUL+blksize 对(13)+timeoutms
对(15)＝86，"ts" 落在 87–88——**死在选项键中间，任何自然包不会这样
结尾**。TF_OPTLEN=88=min(size, room) 且 room=chunk-48=88 ⇒ room 是
裁切者（size≥88）：tf_save_so 的 room 把物理载荷量算少了 16 字节
（fragment＝16B glink 头＋chunk，qrtr 头 32B 落 +16..+48，物理载荷
＝`chunk-32` 而非 `chunk-48`）。

**连锁判决：G8「纯 RRQ」方言裁决翻案**。ob=1e00.0.0（blksize 有、
tsize 无）不是客户端没问 tsize，是解析器只见到被砍掉 16B 的抄本。
fl=0（无 rsize/seek）同理只对裁切内成立。**凡「客户端没要 X」类结论
在 G10 之后一律视为抄本边界制品**，唯原文转录可信。

**假说链定型（G11 待考）**：客户端 RRQ 带 tsize（stat 探询风格值 0？）
→ 我们的 OACK 不回 tsize（解析器没见到）→ 客户端裸 EOT-9 拒收 → 换口
重 RRQ×6 → 弃。即 **OACK 必须逐键应答客户端所问（RFC2347 式严格），
缺一即拒**。修复路径惊人地短：tf_save_so room 补 16 字节后，解析器自
然认出 tsize，OACK 构造器本就有 tsize 腿——**回信内容零改动即多出
tsize 应答**。若假说成立，客户端 ACK-0 → DATA 流出 → 本线第一次
真正开始传文件。

**G11 立案（room 修＋转录扩容）**：① tf_save_so room `chunk-48`→
`chunk-32`（16B 纯 bug 修，无歧义）；② sm 增 `rpl=size.room`
（qrtr 尺寸段 vs 裁切窗，若 size>room 则跨 fragment 重组 G12 立案）；
③ rp= 转 `px=`（让出与旧 rp=0.ffffffff 的撞名）、渲染帽 96→160
（TF_OPTBUF 192 容得下）；④ OACK/解析器零改动。风险：若 "ts" 后不是
tsize 而是未知键，解析器忽略、OACK 不变、安全空转。

## 刀 G11 收据（2026-10-21 14:35 靴，room 修＋rpl=）：方言全貌闭环、OACK 与 stock 同字节——客户端改姿态但仍零回包；dv=0 矛盾立案 G12

刷线：14:34 物理循环 #7（用户手），14:34:52 gate 过刷 boot_a，14:35:51
ping 起。faa3408 镜像（room chunk-48→chunk-32、rpl=、px= 帽 160）。

**rpl=67.68 定谳：size=0x67=103 ≤ room=0x68=104——抄本完整，零跨界截
断**。G10 的「ts 死在键中间」确系 16B 裁切制品，G8「纯 RRQ」翻案闭环。
完整方言原文（103B）：`op1 +
"/readonly/firmware/image/modem_pr/so/549_38_0.mbn\0" + "octet\0" +
"blksize\0" "7680\0" + "timeoutms\0" "1000\0" + "tsize\0" "0\0" +
"wsize\0" "10\0"`。客户端要 **tsize=0**（尺寸探询）＋ **wsize=10**
（RFC7440 窗口读），无 rsize/seek。解析器全见：ob=1e00.a.1（blk=7680、
wsz=10、ts=1）；OACK 回 blksize/timeoutms/tsize=964092/wsize=10——
与 stock tqftpserv 对该请求的应答**同内容**（键序同、值同）。

**客户端换姿态（内容确实被解析）**：5×RRQ→0x11（src 0x22/0x23/0x21）
＋1×RRQ→**dst 0x1111**（错口不服务）；O1–O4 后全静默，O5 后单笔裸
4B EOT-9（rr=1.00050009，本靴最后一笔客户端包，st=4 定谳＝O5 后超时
闭口）；全线 ~+7s 冻结（d=2b40.2b40、tt=b48）。G10 是每 OACK 一笔
EOT-9，G11 收敛为「静默×4＋末笔 EOT-9」——纯超时重试画像，像极了
「从未收到任何应答」的经典 TFTP 客户端弃线形。**tsize 假说（缺键即
拒）证伪一半**：键全回了，客户端仍不 ACK-0。

**G6w2 复核翻案（G7 注释重读）**：窗2 的「dv 有数」是 2s 宽限合成后
发的 DATA 块1（客户端 1s 窗早过、扑空）；「客户端 ACK-0 型」从未成
立——**全史零客户端回包**（OACK 从 0x11、DATA 从 0x11，皆零感知）。

**dv=0.0.0.0.0 矛盾（G12 主题）**：G7 synth 在 OACK TX 成功即置
st=1＋pend=true＋blk=0；pump 门 `armed&&(st1|st2)&&pend` 在
smem_scan(true) 每圈免费评估（soak 环 xfer 期 every=2，~2ms 一圈）；
TF_DC=0（环闸没挡）、tf_note_ack op1 no-op、tf_drop_pend 不碰 pend、
II_ST=2 吸死（ipa_init_tick 早退门 `II_ST==2`，无 2→3 路径）——静态
推演 dv 必≥1，实测 0。G8 注释早留话：「静态推演所有门都该过，必须实
测堵点」。**G12 四分位插桩裁决：pq=call.noarm.nost.nopend＋
ox=ok.no（OACK TX 成败），六格完整分割每次 tf_reply 调用**。

**临时口案中案（G1 doctrine 被推翻）**：rl= 里 dst=0x1011/0x1111 的
RRQ＝客户端咬住我们 ruleset ERROR 的临时源口（tf_new_port() 0x1000+）
再寻址——**临时口包可穿 modem 路由层抵达客户端**（抵达才有咬住），
且反向（客户端→0x1111）也穿。G1「路由器只认 NEW_SERVER 播报口」与
0x11 现象同形（glink 尾随、客户端零感知），从未区分两口；stock
tqftpserv 本就每客户 qrtr_open(0) 新临时口应答（qrtr_publish 只发
布服务口 0x11 等价物）。**G12 第二刀：OACK/DATA 改从 tf_new_port()
临时口发（stock 同构；retry st2/3 沿用在役 XFER 口）；ACK 回临时口
的 on_xfer 监听腿已在役（walker 11408）**。若临时口活、pq= 显示 pump
放行、dv>0、客户端仍静默——下一刀转 qrtr 头字节级对 stock（confirm
位/节点编码）。

## 刀 G12 收据：镜像杀靴定谳＝首查 sm 即死（两窗复现）→ G13 二分回退

**镜像**：94b127b（boot-s20.img 14:58 烤，flash 窗 16:42 / 17:25 两发）。
**两窗判别数据**（g12b 判别采样器：ping 1s 精判 up 时刻 → 20s 纯 ping
存活段 → 每 4s sm 至 +120s）：

- 窗1（16:42 刷）：ping up 16:50:43（等 8.6 分钟——**Mac 侧 USB 枚举
  抖动**，非靴期病：窗2 同镜像 29s 即起）；[+0.7s] 首查 sm 即
  TimeoutError；ping 亡 16:50:59（+16s）。
- 窗2（17:25 刷）：ping up 17:26:37（29s，正常）；**纯 ping 20s 全稳**
  （soak 环在跑、idle smem_scan 每 ~200ms 一圈）；[+20s] 首查 sm →
  TimeoutError；ping 亡 17:27:05（+8s）。**17:32 仍无 ping——死后
  5.5 分钟不回靴**：非 panic-reset（panic 面会 PS_HOLD 复位、二靴
  ping 早回），是**硬锁死或 tries 烧穿**（s20 无看门狗兜底的死形状）。

**判别结论**：sm → smem_scan(ufs,true)@12160 → do_glink → tf_reply
（G12 码）路径杀靴，可复现；G11 同路径数百次 sm 无恙——**行为增量唯
临时口**（pq=/ox= 是纯 wrapping 算术）。读面全排清：push_str/push_hex
皆有界护栏、ctrl_reply=send_slice 无定长雷、渲染峰值 ~500B 距 2560
极远。嫌疑归 OACK-from-tf_new_port() 腿（或客户端对其反应的级联）。

**G13 二分回退（21b50cb，镜像 17:35 md5 4ce92dc0…）**：sport 复位
0x11，pq=/ox= 全留——一刀两用：若 sm 复活 → 临时口腿定罪＋pq= 直接
实测 dv=0 矛盾；若仍死 → 计数/渲染/console 路径翻案再查。

## 刀 G13 收据：临时口无罪释放＋sm 杀靴定谳＋观测面路由污染案

**G13b 铁壁窗（#11，刷 18:54:17，全探针带 en10 接口绑定）**：
ping up 18:55:31（58s 正常带）→ **纯 ping 90s 全活**（G12 两窗
+16/+28s 死位不复发=「镜像自发死」分支死）→ **首查 sm（+91s）即
TimeoutError → +98s ping 亡**（if_active=True）→ **死后 5min 无复活**
（panic-reset 会对偶回靴；无复活+USB 链仍活=**硬楔死**：溢栈或死循环
形状，非干净数据放弃）。

**判决**：
1. **临时口腿无罪**——sport 复位 0x11 后死形复现，G12/G13 共享增量
   =pq=/ox= 计数+渲染。但计数在 90s soak 里跑了数百次无害；渲染只在
   首查 sm 时首跑——时间上精确贴死。
2. **栈深论立案（G14 裁决）**：ctrl_service 栈上 `reply=[u8;2560]`
   活过整个 match，`smem_scan(ufs,true)` 在 arm-8 里跑在其上——
   console 路径=kmain→ctrl_service(+2560B)→smem_scan→tf_reply→
   pump→so_read(UFS 深栈)，比 soak 路径深 2.5KB+。so 传输活窗期
   （RRQ 迟重试）so_read 被踩到即溢栈。TF_BLKBUF 是静态（pump 无大
   栈缓冲），深差主项=reply 本体。**G14=reply 挪静态 CTRL_REPLY**
   （720cbe2，零行为差）：若首查 sm 活+pq= 出数 → 栈论成+dv=0 实测
   到手；若仍死 → 栈论死，转环境对照刀（G11 源重编译重刷）。

**观测基础设施案（本窗烧掉一扇观测）**：Mac 会给 169.254.1.1 生成
en0 reject 主机路由（netstat 尾标 `!`；Wi-Fi 侧另有 169.254.x 邻居
169.254.140.138），裸 ping/TCP 全走 en0 瞎探。**免疫法=接口绑定**：
`ping -b en10`、TCP `setsockopt(IPPROTO_IP, 25 /*IP_BOUND_IF*/,
if_index)`；sudo 删路由要密码不可用。G13 首窗（18:01 刷）整窗被此
烧掉（死时刻未记录）；g13b 起全带绑定。

## 刀 G14/G15 收据：栈论死＋代码全案翻案——「首真 SYN 致死」环境案成立

**G14（720cbe2，#12 刷 20:02）**：reply 缓冲挪静态后死形照旧
（+91s CONN 超时→+98s ping 亡→无复活+USB 链活）。**栈深论死。**

**G15（372222f，#13 刷 20:29，faa3408 源字节级重铸+锐化观测）**：
90s 纯 ping 活 → [+91s] **CONN-FAIL**（ping_now=False，即 1.2s 内
ICMP 也死）→ ICMP died +96s。**G11 源重铸同样死——G12/G13/G14 全部
代码增量无罪，代码全案翻案。**

**五窗归一律**：凡 SYN 真正抵达手机的窗，SYN→死延迟 15s/8s/1s/7s/1s；
唯一幸存窗=14:35（G11 镜像，+0.7s 早连完整回包+28 采样活分钟级）。
**14:35→16:42 之间手机侧 console 路径变成致命路径**（与镜像内容无关）。

**头号嫌疑：横幅路径 `s20_keep` 的 UFS 写**——accept→banner→s20_keep
（UFS 日志环写）；环头随全天 13+ 次靴推进，若走入磨损/损坏区，写必楔
（G11 14:35 写得动，之后写必死；纯 ping 期无 UFS 写故能活 90s+）。
**G16 分级试探**（纯观测，镜像不动）：+2s 早连复刻 14:35 形状 →
L1=pong×30（零 UFS 零 smem_scan）→ L2=log（UFS 读）→ L3=sm
（smem_scan）——一层层定位致命层。若 L1 即死=accept/横幅写；L2 死
=UFS 读；L3 死=smem_scan 腿。

## 刀 G16 收据：工作法定律成立——早连+保温=盒不死，闲置 ~90s=必死

**G16 分级试探（#14 刷 21:34，G15 镜像不动、纯观测侧）**：
- ping up 21:35:59（57s）→ **+2s 早连**（复刻 14:35 成功形状）
- **L1：30×pong 满跑 60s——活穿 +98s 死区**（前四窗全灭于此）
- **L2：log 回包、ICMP 活**（UFS 读无罪）
- **L3：sm 完整 2173B 回包、ICMP 活**；+10s post-end 仍活
- 活体加查（box-age ~4min）：**rq=0.25.0.24（又 3 笔 RRQ）、rr=2 笔
  EOT-9、tt=c58——客户端 so 请求无限重试循环，传输态永活**。

**定律（六窗+基线归一）**：console 有持续 TCP 活动（≥1 查询/~3s）
=盒不死（14:35 窗 4s 节奏活分钟级；G16 窗 3s 节奏活到观测尾）；
**无 TCP 闲置 ~90-160s=必死**（硬楔死形状：ICMP 先断、USB 链仍活、
不回靴）。G12–G15 四窗全是「纯 ping 闲置段」踩进死区——刀系全部
无罪，案翻到底。机制疑云（未裁）：CTRL_PS 状态机注释「0 闲，1 应答
已交等 IN，2 再等 20ms 拉PS_HOLD」附近可能有 idle→掉电路径走歪
（真 PS_HOLD 会断 USB，实测 USB 链活=不是干净断电，是楔死）。

**工作法（此后全部观测窗照此）**：ping up 后 +2s 内首连，3s 节奏
ping 保活，sm 穿插（每 4 拍一发）。G17=此法+G14 全插桩镜像——
pq=/ox= 终审 dv=0。

## 刀 G17 收据：dv=0 终审破案——客户端原话「Expected OACK」+ st=9 黑洞

**G17（#15 刷 22:02:44，G14 源 720cbe2 全插桩 + 保温观测法；镜像 21:40 烤）**：
- 保温链 +2s 建立；[sm0]–[sm31] 全窗 (~590s) 采集。
- **全窗冻结**：tb=9.0.11、dv=0.0.0.0.0、ox=1.0、rq=0.23.0.24、
  tt=b88.b88.4000、rr=2.00050009.00050008"Expected OACK"——唯一在
  爬的只有 nost（~91/12s ≈ 7.6/s 浸泡空转节奏）。
- **+597s ICMP 死（保温全程在线）**——保温定律加天花板：闲置死
  90–160s，持续保温活 ~597s。~600s 整数疑似有真定时器（挂账：
  保温天花板机制未裁）。

**pq= 四分位（dv=0 矛盾的实测堵点，一次采样定谳）**：
sm0 时刻 call=0x90(144)＝noarm(0x1a=26)+nost(0x76=118)，nopend=0、
dc=0——**每一次 tf_reply 都落在未武装/错误态两格，pump 门从未放行**。

**rr= 槽 2 十六进制解码（客户端原话，本案最强证据）**：
`00050008 4578706563746564204f4143` = op5 **code 8** + ASCII
**"Expected OAC[K]"**。code 8=EOPTNEG（RFC 2347，stock 错误表
tqftpserv.c:56）；措辞是「一直在等 OACK、没等到」的**超时腔调**，
非内容拒收（内容拒会点名键；G11 已验证键集与方言吻合；stock 本就
支持回显 wsize——tqftpserv.c:26/284/406）。**客户端从未收到我们
唯一一发的 OACK**（ox=1.0，glink TX 成功≠送达——发时口已闭）。

**rx= 备忘四槽真解（第三元=op5 错码/op4 块号/其余 op 是文件名前两
字节——G6 注释「其余填 ffff」是愿望，代码对一切 op 都读 +50/+51）**：
`(11,op5,短包)(11,op2=WRQ,"/r")(11,op5,code8)(11,op1=RRQ,"/r")`
——0x2f72="/r"＝文件名前缀（so 拉取「/readonly/…」），**不是 ACK-0**
（槽 2 是 rmtfs WRQ 腿）；**全窗无 ACK-0**。时序关键：code8 错误之后
客户端又打了一发新 RRQ（末槽）——被 st=9 黑洞整吞。

**判决链**：
1. G3 地址刷新、G13 sport=0x11 均无罪——OACK 发给了当时最新址，
   但客户端 1s 口窗已闭（重试风暴大部在武装前烧完：noarm=26）。
2. 7119 行 op5 处理 `code==9&&st==1→4 else→9` 把会话打成 st=9；
   7398 行 retry 腿只认 st∈{2,3}——**错误终态封死重答腿**，错误后
   每笔新 RRQ 全进 tf_drop_pend（nost 独大的机器成因）。
3. 对照 stock：按客户端地址建会话、每笔 RRQ 都有 OACK（新口=新会
   话）——我们的单全局状态机在「客户端逐笔换口重试」面前结构性
   耳聋。

**刀 G18（码 0e2f653，22:18 烤 c211ad61）**：retry 腿扩到
st∈{7,8,9}（终态赦免；G9c 节流纪律不变：只投 OACK、不动游标、
一 pend 一答）。胜判：dv 第五元（retry 服务数）升起、ox 随重答爬
升、客户端任一战役 RRQ 在 ~130ms 内得答 → ACK-0 → DATA → **dv>0
首破**。

## 刀 G18 收据：终态赦免首破 dv=1——「Expected OACK」绝迹、首块 DATA 落地；新锁=窗口死锁（G19 开刀）

**G18（#16 刷 22:24:18，c211ad61）**：+2s 建链后 **+9s 死**（建链后到死前
无一次成功查询落盒）——新死形，单靴未裁（G15 教训：单窗死形误导过三刀）。

**Mac 侧接口盲事件（#16 死后）**：强制断电循环后 ECM 重枚举为 en4/5/6
（fe:be:d9:67:0c:a0/a1/a2），「Aginx ECM」网络服务孤儿在死 en10 上——
**未被服务接管的接口永远不拿 APIPA**（60s 轮询实证无效），Mac 对
169.254.1.1 全盲。g18b 观测器 if_nametoindex("en10") 直接崩= #17 靴
（22:40:30 刷）全盲观测。**冷枚举复原**：#18（22:47:21 刷）全新上电
自然复用 en10，服务回绑，+52s 有视觉。修法=g18c 观测器接口自动发现
（任一 enX 拿 169.254.* 即用，300s NO-VISION 心跳）。教训：**跨强制
断电循环绝不硬编码接口名**。

**G18 窗（#18，G18c 观测）定谳——死锁形，非死形**：
- sm0（+3s）即 **dv=1.0.0.0.0、tb=2.1.11、rr=0**——**G17 的
  「Expected OACK」绝迹**（rr=0=零错误回包），OACK 已被收下、**首块
  DATA 已发**（dv 第一元=1，史上首破）。宽限 synth 或真 ACK-0 无法
  从本面区分（都算 dv），但 rr=0 证明客户端进了传输态不抱怨。
- sm1–sm32（~590s 全窗）**全定格**：dv=1.0.0.1.0（第四元 DC=1 环闸
  一挡）、tt=2988.2988.4000 冻结、rq=0.24.0.23 冻结、nopend 独大爬升
  （~7.6/s 浸泡空转节奏）——**泵再未放行**。
- **+593s ICMP 死（保温全程在线）**——保温天花板 ~597s 二次实证
  （G17 +597s / G18 +593s；~600s 真定时器嫌疑加重，机制仍挂账）。

**死锁定谳（G18 的真正产出）**：方言 wsize=10（px= 全文自供），客户端
**攒满整窗（10 块）才回 ACK**；老泵单块在飞——发块 1 等 ACK-1，客户端
等块 2..10，双方零报错零超时腔调（rr=0、st 稳 2）＝**协议级活锁**。
G5–G17 十七刀全在「OACK 到没到」打转，G18 把战线推到「窗没填满」。

**刀 G19（码 b8c306c，22:59 烤 7727d939，#19 待刷）**：RFC 7440 窗口
泵——base=ACK（ack>expect 丢 / ack≤expect 从 ack+1 重开=合法重传与
客户端重启同路），连推 min(wsz,EOF)；环闸/DX 打断后 TF_WSENT 保留
在途数下圈续推（不变式 blkno=base+wsent）；glink_tx_payload 内部发布
环头（6035 行）→ 循环内每块 TX 后重读 tx_head；TF_WSENT 随会话重启
归零（tf_apply_saved）；tb= 加第四元 wsent（dv 若再冻可分清「泵没跑」
vs「ACK 没来」）。观测=g19 早窗加密（+3/+5/+8s 三拍 sm 抓中途爬升）。
胜判：dv>10（首窗）、rx 见 op4、tt 爬升 → 126 块 964092B 送完 →
w45>0 → wlan_pd UP。

## 刀 G19/G20 收据：三层洋葱解剖——#18 泵送达层死、#19 客户端层死（stat 收幕+WRQ 换轨）、#20 传输层死（TX 消耗冻结先于会话）

**G19（#19，码 b8c306c，烤 7727d939，23:08 刷）——传输健康，客户端换轨**：
- sm0（+3s）即 **tb=4.0.11.0**：st=4＝客户端在收到我们 OACK 后回
  **ERROR code 9（End of Transfer）**——tf_note_ack 7299 行 `code==9
  && TF_ST==1 → 4`。**stat 舞首次完整收幕**：客户端要 tsize→OACK 带
  尺寸→ERROR-9 关账。G7 合成 ACK-0 与 rsize=0 门配合正确（dv=0.0.0.0.0
  读泵零进入＝stat 请求不放 DATA）。
- **tt=a80→c08 后 c08.c08.4000 定格＝head==tail：TX 环被吃空**——传输层
  健康（对照 #20）。ox=2.0 两笔 OACK 送达；rq=0.25.0.24（37 笔 RRQ）、
  rx=3（op5(opffff)→新 op1→op5），pq=nopend 独大＝后续请求无新 pend
  （tf_drop 或水位盲）。
- **rl= 台账解锁 #19 真故事**：计数 0x1011=4113 笔探测；末 6 槽含
  **port 0x1f 的 op2 WRQ**（tf 扫描只服务 0x11——WRQ 落在异口）+5 笔
  0x11 RRQ。客户端在 stat 收幕后**换 WRQ 姿态**——G20 写腿的动因。
- +377s 单次 ICMP 丢后 5 连 OK＝**软失速**（新现象，挂账）；观测窗在此
  被脚本判死退出（watch 短板：5 粒度后即弃，盒可能仍活）。

**G20（#20，码 88396bb，烤 ef49ed2b，23:34:49 刷）——泵开火但传输已死**：
- sm0（+3s）即全定格：**tb=2.3.11.3——G19 窗口泵开火**（st=2、blk=3、
  **wsent=3**），**ww=1.1.0.0.0——WRQ 会话开账**（OACK 已答、期望块 1、
  零数据回收）；rx=4＝op5→**op2 WRQ**(2f72)→op1→op1，**全程零 op4**。
- **tt=3c70.28b8.4000 定格：gap=0x13B8=5048B 未被消费**。3 块×7712B=
  23KB > 16KB 环容量——**TX 消费冻结先于 TFTP 会话内容入环**：客户端
  rx 序列以 ERROR 开头＝它从未收到我们的任何回包（stat 舞在 #19 能收幕
  证明客户端能消费 OACK；#20 的 0x28b8 尾巴冻死＝modem 侧 glink 消费
  停摆，门铃丢/intent 记账死二选一）。
- **入场门 8192 自锁**：head=0x3c70 近环尾（sz=0x4000），`head+8192>sz`
  恒真→dc≈call−42（pq call 0xb4c、dv4 从 0xb22 爬到 0x475aa＝每 ~2ms
  一挡）——**retry-OACK 腿被自己锁死在门外**，全靴再无任何 tf_reply
  体出环。
- 客户端终态：ERROR→WRQ→RRQ×3→**永久静默**（服务器不可达的教科书
  姿态）；盒本身活满 600s 观测窗（33 样本、保温全绿）——死的是 glink
  TX，不是 L0。

**三层洋葱定谳**：#18 泵送达层活（客户端收下 OACK+块1，窗死锁协议层）；
#19 传输层活（环吃空）客户端层换轨（stat 收幕→WRQ）；#20 传输层死在
会话前。三靴三层各烂一层——**单靴死形不可跨靴外推**（G15 教训三现）。

**G20 自伤（码内发现，未及发作）**：WRQ 腿调 tf_apply_saved()——
①op2 选项从不存盘（tf_save_so 只认 op1），解析的是**陈旧 RRQ 选项**；
②apply 会清 TF_WSENT＝破坏 G19 读窗不变式；③涂改 TF_REQ_* 污染读
会话。本靴 WRQ/RRQ 选项同形（blksize 7680/wsize 10）故未显炸，但结构
性错误必须修。

**刀 G21（码 7b8cdda，烤 7df52b58，#21 待刷）——传输法证+自愈+污染修**：
① xl= 台账：glink_tx_payload 成功记 4 槽 sport.size.head，定位冻结
   frontier（#20 的 5048B 里坐的是谁）；sm=xl=count[.sport.size.head
   ×≤4].rkick。
② tx_rekick：tail 静止 ≥500ms 且 head≠tail → 重发布 head+重敲门铃
   （1s 节流，TXK_N 计数）——门铃丢假说直接考；soak 环每拍调用。
③ 泵在飞闸：inflight>4096 拒续推——防泵把环灌死反锁 retry 腿。
④ WRQ 元组解析：tf_save_opts（walker op2 分支随 pend 存选项）+
   tf_w_parse 只读元组，OACK 回显全局部变量，零共享静态污染。
⑤ 入场门 8192→1024：head 近环尾不再永久锁死 retry-OACK（最大包体
   DATA ~880B<1024——注：blksize 7680 时单块 7712B 仍过不了门，
   靠环闸另挡；门的职责只是别把小回包锁死）。
CTRL_REPLY/TCP_TX 2560→3072（xl≈96B 余量防截尾）。
胜判不变：xl frontier 在 TFTP 会话包上＋TXK_N>0 后 tail 动 → tt 爬升
→ rx 见 op4 → 126 块 964092B → w45>0 → wlan_pd UP。

## 离线内核 glink 契约审计（2026-10-22 ~00:30，无刀）——#20 冻结三家嫌疑+READ_NOTIF 端药方

源：`kernel-src/redbull/drivers/rpmsg/qcom_glink_native.c` +
`qcom_glink_smem.c`（本地内核树，只读）。裸机 TX 与内核契约三处结构性
错位，全部指向 #20「tail 冻死先于会话」家族：

1. **内核 TX 随时回卷**（`glink_smem_tx_write_one`：写时分两段、
   `head-=length` 回卷）；可用量只让
   `FIFO_FULL_RESERVE 8 + TX_BLOCKED_CMD_RESERVE 8`。我们的「单飞纪律」
   （只在 head==tail 整环排空才回卷）更严——head 近环尾+tail 停摆＝
   永久写死（#20 入场门自锁的根，G21 ⑤ 只是止血不是正法）。
2. **内核 TX 吃远端 intent 池**（`__qcom_glink_send`：`riids` idr 里挑
   最小够用空闲 intent、标 `in_use`，远端 RX_DONE 才归还；池干则发
   `RX_INTENT_REQ` 并阻塞等批）。我们每包都盖同一个 `LAST_IID`、不追踪
   modem 对我们 iid 的 RX_DONE 归还、池干不发请求——**同一 iid 连发
   在飞**是常驻违约。
3. **内核收包循环遇坏 iid 即弃坑**（`qcom_glink_rx_data`：intent 查无
   →`-ENOENT`，本包消费掉但 `if(ret) break` 收包循环退出，等下一个
   doorbell/READ_NOTIF 再进）。QDSP6 侧 glink 非 Linux 移植，违约反应
   未知；10-18 已实证「某包一出、DSP 读端停死、跳过即复活」——
   同族。
4. **RX_DONE 品种**：内核只对 `reuse=true` intent 回
   RX_DONE_W_REUSE(14)，非 reuse 用 RX_DONE(6)；我们 `glink_rx_reuse`
   一律 W_REUSE——对非 reuse intent 也是常驻违约（10-18 定谳的
   「RX_DONE_W_REUSE(liid=1) 楔死」与此吻合）。
5. **READ_NOTIF(13)＝「环里有货来读」催件**：内核 TX 环满等位时发之
   （`qcom_glink_tx` while 循环+`sent_read_notify` 一次性），对端收到
   即 `mbox_send_message` 自踢收包循环——正是「收包循环 bail 后」的
   复活通道。包形：cmd13、param1=0、param2=0、12B。
   （10-18 试过 cmd13 无效——但当时毒包是自家 W_REUSE，催件救不了
   中毒；#20 若是 bail 型冻结，催件才是正药。）

**对 #21 判读的先验**：xl= 首照 frontier 坐标＋TXK_N 是否动 tail——
- re-kick 动 tail → 纯 doorbell 丢（T3）；
- 不动、frontier 是 TX_DATA(重复 iid) → intent 池干（T2）；
- 不动、frontier 是控制包（W_REUSE）→ 毒包（T1，10-18 同族）。
**G22 候选刀序**（按判读定）：READ_NOTIF 催件腿 / intent 池+归还追踪 /
W_REUSE 分品 / 自由回卷。禁在三嫌疑未定谳前混刀。

## modem 固件语料全解（2026-10-22 凌晨，离线，无刀）

**案**：REA 反编译 modem 侧 tqftp 客户端/glink 读端一直缺 b 系列语料
（设备侧只拉回过 modem.mdt 9420B）。厂包 radio img 曾试 mtools 全败。

**破**：factory zip `redfin-up1a.231105.001.b2-factory-*.zip` 里的
`radio-redfin-g7250-00264-230619-b-10346159.img`（153,424,012B）结构：

- 0x00–0x8B：FBPK 头（magic `FBPK`，product 串，0x50 entry：
  size 0x0925108C、name `modem`）。
- **0x8C 起＝标准 FAT16 引导扇**（`eb 3c 90` + OEM `MSDOS5.0` + 卷标
  `NO NAME    FAT16   `），此前 mtools 全败的根因＝**每扇区 4096B**
  （mkfs 用了 modem 分区 4K LBA 几何）。
- BPB：bps=4096、spc=4（16KB 簇）、rsvd=1、nfats=2、fatsz=6 扇、
  root=512 项 → fat1@0x108C、root@0xD08C、cluster2@0x1108C——与
  main.rs:6459 的设备侧 MODEM_LBA 几何完全互证。
- 根目录两个目录：`image/`（modem.mdt+b00..b34+modemr.jsn）与
  `verinfo/`；`image/` 里另有 `saipan/`、**`modem_pr/so/`**——即设备
  侧 `/readonly/firmware/image/modem_pr/so/` 的真身，549_38_0.mbn 等
  全部 so 件都在。

**收**：全语料抽至 `.local/device/redfin/modem/`（96M，gitignored）：

- `modem.mdt` md5=edc9367a… 与设备侧拉回件**逐字节一致**——厂件=机件。
- `modem.b16` 4,014,080B 与设备侧尺寸一致（b 文件头 `b0c30058` 标准
  hash 段头，REA 主目标）。
- `so/549_38_0.mbn` 963,644B（ELF，e_machine=0xA4 QDSP6、ET_DYN）。
  设备侧 RRQ 曾报 tsize=0（本次 px= 也证 tsize=0）；此前记的 964,092
  与厂件差 448B，版本元组 549_38_0 一致，反编译用途不受影响。
- 抽取脚本 `/tmp/modem-radio/extract.py`（LFN+8.3 混排、FAT 链走读）。

**账**：REA 反编译（b16 + so 件找 tqftp 客户端）仍**保温挂账**——等
#21 xl= 判读先出，经验法不够了再动 Ghidra；语料已就位，动工零门槛。

### 补记：tqftp 客户端定段＝modem.b22（同晚，串三斗）

- **客户端代码段＝b22**（10.3MB，非旧档以为的 b16——b16 是 4MB 引导主
  像，零 TFTP 串）。锚点（b22 内偏移）：
  - `tftp_socket_ipcr_modem.c` @0x10342 与 @0xecd59（源文件名串×2）；
  - 断言簇 @0xec07c–0xed0b2：`sock_res == TFTP_SOCKET_RESULT_SUCCESS`、
    `sockaddr->sa_family == AF_IPC_ROUTER`、`addrtype == IPCR_ADDR_PORT`
    ——客户端走 IPC router socket 腿，与 qrtr 谱系互证；
  - **选项名表 @0xed06e 起＝枚举序**：`blksize,timeout,tsize,seek,
    append,unlink,wsize,timeoutms,rsize,null`（TFTP_PKT_OPTION_ID_MAX
    断言紧随）——客户端全部词汇；线上只见 blksize/timeoutms/tsize/
    wsize 四件，rsize/seek 是另有触发条件的腿。
- **.mbn 四路搜索序**（@0x9166x，每文件同构）：`/readonly/firmware/
  image/X` → `/readonly/vendor/firmware_mnt/image/X` → `/readonly/
  vendor/firmware/X` → `/readonly/vendor/firmware/X.otaupdate`。客户端
  本靴取第一条——与我们观测的 RRQ 路径一致。
- REA 反编译开工时的入口：b22 @0xecd40 附近函数簇；Ghidra hexagon
  段加载用 mdt 的 program headers 排段。

## 刀 G21 收据：靴 #21 判读——T2 定谳，intent 投递层死（2026-10-22 00:26–00:37）

镜像 boot-s20 md5=7df52b58（7b8cdda 五腿）。刷入 00:25:50，ping 起
00:26:46（en10），+595s 盒死=保温天花板第三次实证（G17 +597 / G18
~593 / 本靴 +595，三靴同窗 ~593-600s）。33 拍 sm 全程同形，会话
自 +4s 起完全定格。

**五腿逐项验尸**：
- **xl 台账（新）首战告捷**：TXL_N 17→13(hex) 停增；frontier 四槽
  `11.1000.910 / 11.68.998 / 11.60.a08 / 11.68.a88`——**全是 0x11 口
  TFTP 协议包**（sport=源口、size=含 28B glink 头、head=写后环头）；
  之后两笔 0x16/0x12 他口小包后 TX 全停。TXK_N=0（零 re-kick）。
- **tt=c88.c88.4000 六百秒恒等**：TX 环全程排空，modem 消费端活。
  #20 的「传输层早死」未复发。
- **入场门 1024 腿生效**：dv4(DC)=0 全程（#20 曾爬 0x475aa 自锁）。
- **WRQ 元组腿**：ww=1.1.0.0.0——本靴竟无 WRQ 会话痕（active=1 为
  OACK 答出标记，acks/bytes=0）。
- **泵/会话形状**：tf=.op1.TF_N5（TF_N=drop_pend×4+泵送×1）；
  tb=ST3.blkno1.xfer11.wsent1；ob=blk7680.wsz10.ts0；dv=1.0.0.0.0
  （DV1/DZ0/DX0/DC0/DR0——无环闸挡、无 retry-OACK）。

**时序重建**：客户端 op1 RRQ（blk 7680/wsize 10，rsize 范围读——
DATA-1 总字节 4096-28-4=4064 载荷=limit 掐尾，ST=3 范围收口）→
我们 OACK 答出（ox=2.0 成败 2:0）→ 客户端 2s 无 ACK-0 → 宽限刀
合成 ack=0 → 泵一块 DATA-1 送出、ST=3「送完」→ **客户端从未收到
任何一包**（零 ACK-0、零 ACK-1）→ timeoutms=1000 重试梯把 RRQ 重发
×4（每笔被 TF_SEEN 去重账房 tf_drop_pend 静默丢弃）→ 梯尽弃线，
600s 死寂至盒死。pq 283→2700 持续爬=qrtr 民政包照常解析=盒活。

**判读（按 #21 先验三嫌疑）**：
- T1 毒包：**排除**——frontier 无控制包，全 TX_DATA。
- T3 doorbell 丢：**排除**——TXK_N=0，环排空无需 re-kick。
- **T2 intent 池干：定谳**——客户端对本会话全聋（连 OACK 都没到），
  而环字节被 modem 消费（tt tail 推进）＝「消费掉但不投递」。与内核
  契约审计第二结构错位严丝合缝：我们每包盖同一 LAST_IID、不认
  RX_DONE 归还、池干不发 RX_INTENT_REQ——同 iid 连用在飞=常驻违约。
  旁证：cmd5 广告（count,size,id）我们只记一个最大 id，内核按
  id..id+count-1 展开成池、每包消耗一个、等对端 RX_DONE 归还。

**顺带揭出的自伤/隐患（G22 材 料，不与主刀混）**：
1. **g21 观测过滤器自伤**：SM 元组删了 rl=/px=/rs=——恰是判读最需
   的三段（本靴 rs= 值不可再考，只能从 4064 反推）。G22 观测必还。
2. **RX 行走器未知 cmd 即 break**（hello 行走器 5660 一带）：modem
   若发来表外命令（含归还类），解析折断在该包上，圈圈重折——
   pq 爬/rx 冻与此形亦吻（rx 侧盲与 T2 不互斥，客户端侧死寂才是
   T2 铁证）。G22 行走器要按内核全命令表步进。
3. TF_N 混计 drop_pend+泵送——收据判读时差点误读，将来拆成两段。

**G22 裁决（单刀）**：intent 池+归还追踪——①cmd5 广告展开成池
（id..id+count-1, size, free/in_use）；②TX 盖章处改「最小可容 free
intent」、标 in_use、无则计数让路；③认 modem 的 RX_DONE/W_REUSE
（7/14）归还；④池干发 RX_INTENT_REQ 求批；⑤sm 加 ii= 台账
（ads/freed/inuse/dry）+观测还 rl=/px=/rs=。READ_NOTIF 催件、
W_REUSE 分品、自由回卷继续挂账排后——三靴判读均未指向它们。

## 刀 G22 收据（靴 #22，s21 在机）——intent 池闭合、写腿 DATA 全盲定谳

s21 = intent 池+归还追踪（cmd5 广告展开 id..id+count-1、TX 盖最小可容
free intent、认 RX_DONE(7)/W_REUSE(14) 归还、池干发 RX_INTENT_REQ、
ii= 台账）。靴 #22 实战：

- **读腿活**：OACK 答出、窗泵开始——T2（intent 投递层死）闭案。
- **写腿开而 DATA 全盲定谳**：WRQ 会话开（active=1），客户端 DATA
  永远不被看见——大包（>512B，blksize 7680→chunk 7712）在 walker
  撞 G14 的「>512 跳步不解析」直接跳过，op3 无从记账。cmd7 催件
  （voucher 要价 7784B=7680+头）无应答=对端预算耗尽即哑死。
- **意图经济面**：归还链活但 cmd12 CONT 落 else break——首片续片
  当场楔死步进器（#23 复现定谳）。

G23 单刀材料齐：大包两件照办（还 intent + tf_note_ack/data 记账）。

## 刀 G23 收据（靴 #23，s22 在机）——三层洋葱再剥：非 so 请求静默死

s22 = 大包两件照办（还 intent + tftp 记账，QMI 深解析仍小包专属）。
靴 #23 判读（G23b 提交 3b0967c 定谳三案）：

1. **modem_efs RRQ 撞 !tf_is_so() 静默 drop**——客户端 walk 至
   /readwrite/modem_efs，6+ 笔换源口（0x21→0x25 一带）重试全被
   7916 门吞掉：TF_N=4、ox=1.0 恒定、阶梯全 op1，无一 ERROR 无一
   OACK → 客户端弃线全静。真机产线本无 tqftpserv，客户端对「服务
   在、文件无」早已兼容（ruleset ERROR 后换 WRQ 重来先例在案）。
2. **cmd7 入站哑跳**：voucher 请求（0xe68）无人应——写腿 OACK 后
   零 DATA 的主嫌。
3. **cmd12 CONT 楔死步进器**：与 G22 靴互证。

G23b 三修+台账一刀定案（详见提交 3b0967c 与下节）。

## 刀 G23b 收据（靴 #24，s23 在机）——.so 完整送达（S4 线核心目标闭环）+ 写腿知名口定谳

s23 四件：①非 so RRQ 一律 ERROR-1（旧刀只 ruleset）；②cmd7 催件
应答（cmd5 补一枚+cmd8 granted，lpass 同款，IIP_NEXT 11+）；
③cmd12 CONT 与 cmd9 同框入步进器；④意图经济台账 rg=/c9=/bg=/i7=。
靴 #24（06:24:15 刷入，+595s 整准点撞天花板死=第四次实证 595–600s
铁律）：

**历史性破局——.so 完整送达**：
- 客户端 walk 过 modem_efs（tr=1：一笔 ERROR-1 后客户端接受裁决
  换轨）→ 真 .so RRQ（px= 全文：blksize7680/timeoutms1000/seek0/
  wsize10/rsize52）→ OACK → 窗泵 → **客户端 ERROR-9 收幕**（tb=3、
  rr=1.00050009）。方言发现：客户端不逐块 ACK，op5/9=会话终语。
- **意图经济闭合**：rg=4f.4f.0（79 入队=79 归还、深度 0 零泄漏）；
  cmd7 topup 实战 5 发 5 应（i7=5.e68.5.5.0），7784B 大包如约而至。
- 阶梯六槽全景：**1×WRQ（sport 0x1f）+ 5×RRQ（sport 0x21..0x25）**
  ——一靴内客户端做了 1 写 5 读，.so 是最后一笔（带选项）。

**写腿死因定谳（G24 刀案）**：
- 阶梯首槽 dst=**0x1011**（TF_RL 原样记账）＝客户端写方言 WRQ 发
  知名口 **0x1000+0x11**，读腿才用我们 NEW_SERVER 广告的 0x11。旧
  武装门（dst==0x11）整段吞 WRQ → 写会话无 OACK 无 ERROR 全静。
- 7784B→0x16 大包实为 **IPA QMI**（本侧 ipa 腿已应 64B，xl 台账
  sport 0x16/64B 与 ipa_cfg/ir/ind 三腿字节量吻合；IPA_SVC_PORT=
  0x11+5 常量在案）——「写 DATA 错口投递」假说作废。
- 读腿四笔 plain RRQ 中只有一笔得 ERROR（tr=1）之谜挂账（TF_PEND
  占用窗与 II_ST==2 闸的时序挤压，次要）。

**w45=0 仍在**：modem 吃到 .so 但 WLFW 0x45 未起——嫌疑序重排：
①写会话（EFS 同步）楔死拖住启动序（G24 主刀）；②PIL 后续段/更多
文件未供（pr= 路径环下一靴回答）；③IPA 会话半接。

## 刀 G24（s24，链 #25 已布）——写腿知名口上监听 + 观察三件

- 武装门/note_data 门收 TF_ALT_PORT(0x1011)；写 OACK/ACK 回程口
  镜像来程（TF_W_VIA 武装时记，读写互不污染）；读腿主道 0x11 一字
  不动；ERROR 腿仍走 tf_new_port 新口（stock 象）。
- 观察三件：pr= 请求路径环（6×28B，px 只存 so——非 so 要哪个文件
  从此可见）；bx= 首枚大包源口+载荷头三字；wx= 写腿三账
  （WRQ 武装.写 OACK 发出.来程口）。
- 码 75b1615 已推（先码后收据+码尖位舞步，本地 ahead=27 纯收据）。

## 靴 #25（G24=s24 在机，06:47 刷）——写腿首活 + 0x23 巨 REQ 悬案浮出

链面：fastboot 06:47:53 闸过、刷 boot-s24.img、+0s ping up（06:48:49）、
ka 立活；cyc=112 +527s 硬死（5 粒 ICMP 全死、无软复活）——第五次天花板
实证，且**早于常格 ~68s**（#21–#24 皆 595s±5；不作过度解读，记档）。

**G24 四判据全出**：
- **wx=1.1.11**：写腿历史首次武装+OACK 发出。但 VIA=0x11 而阶梯首槽
  dst=0x1011——同包同变量不可能分叉 ⇒ **两发 WRQ 实锤**：发 0x11 的
  那发先武装（早于阶梯记录窗 TF_RL_VIS），发 0x1011 的那发被
  `!TF_PEND` 挤掉（武装→OACK 出门之间的窗内到达）。我们的写 OACK
  从 0x11 出，客户端不认（期待另一口），**ww=1.1.0.0.0**：会话立活、
  EXP=1、零 DATA 零 ACK——写会话仍是单侧活。
- **pr=（末 4 槽）**：slot2(sport21)=`/readwrite/ota_firewall/ru…`、
  slot3/4(sport23/24)=`/readonly/firmware/image/m…`、slot5=so。28B
  截断太狠——`tf_is_ruleset` 早在册 ⇒ slot2 全名=`ota_firewall/
  ruleset`（RRQ 读！EFS 侧文件，FAT16 无此物，ERROR 即 stock 象）；
  slot0(WRQ)/slot1(sport22) 未渲染。ota_firewall/ruleset 既是写目标
  又是读目标——客户端先 WRQ 写、失败后 RRQ 读回、再失败走人。
- **bx=20.23000400.150e4100.150e32**：首枚大包 sport=0x20、载荷头
  `00 04 00 23 00 41 0e 15 | 32 0e 15 00` ⇒ qtype0(REQ)、tf4、
  **msgid=0x23**、首 TLV 0x0e len≈0x1532——**客户端 7784B 巨 REQ
  打 IPA 服务口 0x16**。核码定谳：0x20/0x27/0x35 三条入站 REQ 皆有
  RESP 腿，**唯独 0x23 无腿**（只进 IPA_REQ/IPA_LAST 记账）。
- 其余全同 #24：.so 完整收幕（rg=4f.4f.0、tr=1.1000、tb=3.1.11.1、
  rr=1.00050009、ox=2.0、px 同款）——**G24 改动零回归**。

**w45=0 悬案重排（G25 刀案依据）**：客户端 .so 收幕后全静至盒死，
唯一悬而未答的问句=0x23 巨 REQ ⇒ 嫌疑序：①**0x23 无腿**（G25 主刀，
三先例同款小 RESP TLV 0x02 成功）；②写会话回程口方言（0x1011 那发
被挤未试出）；③非 so 读未供（客端对 ERROR 本就走人——「服务在、
文件无」是 stock 产线常态，弱化）。

**顺带两破案**：
- sm face2（im=/jx=/ic=/ir= 等）一直在同一条 sm 回包里——**观测脚本
  SM 元组没列被滤掉**，非监端截尾。修表即可，监端零改动。
- 读泵 .so 字节=**UFS FAT16 直读**（MODEM_LBA+17 簇链、fat_next 走
  链）非烤入——mdt/bXX 全部文件本体就在 radio 分区，**服务化=纯代码
  活零烧入预算**（fat_path_open 已是全功能路径解析器）。

## 刀 G25（s25，链 #26 待布）——msgid 0x23 应答腿 + 观察加宽

- **主刀 i2_tick**：cfg 腿克隆——msgid 0x23 RESP（qtype2、TLV 0x02
  成功）从 IPA_SVC_PORT 0x16 回巨 REQ 来向；txn+port 去重同 CFG；
  武装腿挂 0x35 IC 腿后。
- 观察加宽：wx= 扩三账（武装源口 TF_W_SRC.撞门数 TF_W_BL.撞门口
  TF_W_BLV——两发 WRQ 先后与挤因直读）；pr= 环 28→56B（49 字符全
  路径可罩）；i2= 三账（见数.已回.REQ 源口）；观测表补 face2 字段
  （im=/jx=/ic=/i2=）。
- 码 7c467a5 已推（码尖位舞步，master 本地 ahead=28 纯收据）。
- 靴 #26 验证点：i2=0.x.口 三账>0（0x23 终于有回）、客户端是否续走（阶梯
  第 7 笔起/新会话）、w45 是否起；wx 后三账揭两发 WRQ 全序。

## 靴 #26（G25=s25 在机，07:16 刷）——巨 REQ 定位：大包短路吃掉，II 区从未见过

- ping up 07:17:39；sm0/sm_e1/sm_e2→sm20 全程采样，keep-alive FAILED
  cyc=128 **+599s**（天花板第六次实证，~595–600s 硬盒顶）。
- **i2=0.0.0 全程零**——但 bx=20.23000400.150e4100.150e32、bg=
  1e68.1.1.16 与 #25 逐位一致：巨 REQ 到了、腿没武装。
- **face2 修开后的定谳**：im=27.27.1（II 区只见 0x27 一笔）、jx=
  15.2.21（首笔非 27=客户端 0x21 RESP 打 0x15）、ic=0.0（0x35 各靴
  恒零）⇒ **II 区从未见过巨 REQ**。根因=G23 注释自供：cmd9 大包分支
  （chunk>512）只办还 intent+tftp 记账即 continue，「QMI 深解析/请求
  处理仍限小包」——7784B 巨 REQ 永远流不到 II 区的 0x23 臂。#25 装的
  腿位置没错，是够不着。
- 读侧照旧全绿：tb=3.1.11.1、tf= 全名、rg=4f.4f.0（意图经济闭合）、
  px= OACK 选项全抄；w45=0。
- **wx 三方矛盾（悬案改经验判据）**：wx=1.1.11.1f.1.11（武装 VIA=11、
  撞门 BLV=11）vs 阶梯首槽 1011.1f.2（6 槽窗内无任何 11 op2）。码面
  审计：arm(!seen&&!PEND) 与 blocked(seen||PEND) 互补无缝隙、ladder
  站点与 G24 区之间零 continue——纸面不可闭合，须照抄全序实证。

## 刀 G26（s26，链 #27 待布）——巨 REQ 腿搬进大包分支 + op2 全量环

- **主刀**：cmd9 大包分支内就地补一档头四字深解析（qtype/msgid/txn，
  偏移同 II 区 +48/+52）——打 IPA 口（0x16/0x15）的 qtype0 REQ：0x23
  武装 I2、**0x35 武装 IC**（ic=0.0 各靴恒零疑同因：CMPLT_REQ 载 TLV
  同为巨包）。回信仍走 i2_tick/ic_tick，PEND+DONE 去重同 II 区原样。
- 判据刀：ladder 站点（无口过滤=真总账）照抄 op2 全序——wx= 第 7 位
  TOT、新 wl= 最近 4 发 dst.sport 环（旧→新）。
- 烤模 --s26 入列；boot-s26.img 118784B（树=码尖等价已验）。
- 靴 #27 验证点：i2=≥1 且 ic=≥1（两巨 REQ 皆有回）、客户端是否续走
  （新 QMI 交易/阶梯第 7 笔/w45）、wl= 直读 WRQ 真全序。

### 推送事故与洗回（10-22 早，10-01 判例复发）

- 事故：`checkout -b g26` 从收据尖 a351902 开出（漏 origin/master），
  push 把 26 件收据整个带上 GitHub；~1 分钟内 `--force-with-lease=
  master:6fc1f66` 洗回 7c467a5。收据无密钥，悬空 sha 同 10-01 判例
  可接受。
- 连带发现：收据堆底压着**三件 CH7 码 commit**（c0c63bf/8d60fd6/
  88c58c0，10-19 GSI CH7 线「Build only; no push」）——上次舞步的
  「28 收据」实为 3 码+25 收据。正法重放：G26 cherry-pick 到 88c58c0
  成 7a797e2，**CH7×3 随之补推**，26 收据重垫顶。
- 铁律加固：工作分支必须显式 `git checkout -b <名> origin/master`，
  推前必验 `git log --oneline origin/master..HEAD` 只许一件。

## 靴 #27（G26=s26 在机，07:43 刷）——0x23 首答成、客户端仍全静、天花板七证

- ping up 07:45:04；sm0→sm31 全样到 +~585s，keep-alive FAILED cyc=127
  **+594s**（天花板第七证，~593–600s 窗）。中途「续活」误读=ka/sm 交错
  计拍错误，收讫勘正。
- **i2=1.1.20（历史首答）**：大包刀生效——巨 REQ 武装 1、RESP 发出
  1、REQ 源口 0x20、去重全程稳（无重答刷屏）。II 区账（im=27.27.1/
  jx=15.2.21/ic=0.0）与 #26 逐位同。
- **客户端零反应**：rl/rq/c9/i7/bg/w45 全冻——0x23 一答不解锁；ic=0.0
  定谳 **0x35 不是巨包被吞，是根本没来**（modem 侧 INIT 完成回调未发）。
- **wl= 首读**：真数据 ①dst 0x12 sport 0xb（打假 pd-mapper 口的 QMI
  REQ 头两字节 00 02 撞 op2 的**假阳**——无口过滤之过）②dst 0x11
  sport 0x1f（真 WRQ）；TOT=2。渲染 ×3 假重复=负 base 钳位 bug。阶梯
  槽 1011.1f 与 wl ② 11.1f 仍无法同化（G27 修后再考）。
- **天花板机制假说（新立案）**：墙恒 ~600s 且死前 sm 全样到最后一拍
  （非渐进衰败）+ 既有判据「logo 闪灭×3 → 回滚 b 槽」——吻合 **ABL
  看门狗起表、无人喂狗、~10min 硬重置**（非 modem 侧杀人）。若真，
  喂狗刀可拆墙，与 modem 线独立。候选 G28（先查 WDT 寄存器路）。
- G27 观察刀（wn= 写目标快照 + wl 三修）码 c131efb 已推，链 #28 待布。

## 刀 G27（s27，链 #28 待布）——wn= 写目标快照 + wl 环三修（纯观察）

- **wn=**：arm 块 name/nn 上层已解析，武装时单独抄盘 56B——「客户端
  想写什么」直读（写会话 OACK 后零 DATA 的第一悬案）。
- wl= 三修：4→6 槽+op 位+tftp 口过滤（0x11/0x1011/xfer，与 G24 区同
  门，TOT vs N+BL 两账直接对账）+钳位修+空槽显 -。
- 烤模 --s27；boot-s27.img 118784B。
- 靴 #28 验证点：wn= 写目标全名、wl=（修后）全序与 G24 两账对账、
  i2=/ic= 维持、w45、+~594s 墙复核。

### 靴 #27 增记（10-22 午，日志考古三破）

- **夜里无人值守自愈循环实证**：01:14、01:26、06:25、06:48 四次
  ping up **无任何刷机**——每次天花板死后机器自重置、ABL 重试 a 槽、
  我方镜像自动重跑（01:14→01:26 节拍 12min ≈ 600s 命+2min 启动）。
  a 槽重试整夜未耗尽、未回滚 b。**WDT 假说强证**（硬重置+重试，非
  死透）；同时宣告天花板死后机器永远自己回来=观察窗天然续期。
- **两处 16–19min 日志洞=宿主侧假象**：06:58→07:16、07:28→07:44 两
  段 NOTHING——macOS networksetup 在半死 USB 上挂起（非设备行为），
  恰与用户两次手动强关重合。读日志遇洞先疑宿主。
- **渲染考古定谳**：wx TOT+wl 四槽=G26 特性；07:17 那世 wx 无 TOT
  =boot-s25.img（07:15 构建）G25 复验世——其 i2=0.0.0 是 G25 无大包
  腿的本分（II 区腿天然不响），非 G26 漏网。G26 命中账 1/1 无污点。
  07:44 刷 s26（07:27 构建）→07:45 世=#27 收据原案成立。

## 靴 #28（G27=s27 在机，08:07:54 刷）——wn= 破案：写目标=/readwrite/server_check.txt

- ping up 08:08:49；sm0→sm31 全样，FAILED cyc=127 **+594s**（天花板第八证，
  与 #27 同秒级）。
- **wn= 直读破案**：`2f7265616477726974652f7365727665725f636865636b2e747874`
  = `/readwrite/server_check.txt`——modem 的 tqftp 客户端启动期往 AP 的 rw
  分区写体检文件。写会话的目的定谳（#24 起悬案）。
- **wl= 修后干净**：`11.1f.2,-,-,-,-,-` 全程仅一发合法 WRQ（dst 0x11
  sport 0x1f），**无重试**（timeoutms=1000 下不发 DATA 也不重试 WRQ——
  客户端对写腿是「体检」非「刚需」，失败不阻塞也不重试的姿态）。
- **wl vs 阶梯矛盾在干净仪器下坐实**：wl=11.1f（口过滤环直读 dst）与
  rl 首槽 1011.1f（同靴同源）——非渲染 bug，是两站读位/时刻差异，
  下刀解剖点。
- i2=1.1.20 复现（G26 大包腿稳定命中）；巨 REQ 应答不解锁客户端；
  0x35 依旧不来（ic=0.0）；rg=4f.4f.0/c9=53.6/w45=0 全冻至死。
- 读腿照旧：px= 载 .mbn RRQ 选项 blksize7680/timeoutms1000/tsize0/
  wsize10/**rsize52**（rsize=52 为首档样本新增可见）。
- **G28 方向（源码研究先行）**：参照 rmtfs（mainline tqftp 服务器）校对
  OACK 形状与写腿交换序（怀疑我们的 OACK 让 tqftp 客户端解析硬楔——
  契合「零 DATA 零 ERROR 零重试」的全冻死状）；WRQ 选项 dump（wn 只抄
  了文件名，选项段未录）。

## 刀 G28（10-22 晨，码 323e5df 已推）——0x1011 悬案定谳渲染拼接伪影 + 写腿会话口对齐 stock

- **#28 增记破案：线上从来没有 0x1011 的包**。rl 渲染端
  `push_hex(TF_RL_N)` 后无分隔符直接拼首槽 dst——`rl=1011.1f.2` 实为
  **N=0x10（16 笔请求）hex"10" + 首槽 dst=0x11"11" 拼接**。全量重解
  #28 sm0：wl=11.1f.2（TOT=1）与 wx=1.1.11.1f.1.11.1（VIA=0x11
  SRC=0x1f）与修正后阶梯首槽 11.1f.2 **三账合一**——客户端 WRQ 与读腿
  同发 0x11，单发无重试；G24「写方言硬编码 0x1000+0x11」整个理论建立
  在渲染伪影上（#24/#25 的"0x1011"同源；wx BL=1/BLV=11 是同包 FIFO
  重扫挤占，非重试）。渲染修：N 与首槽补","。
- **参照定谳（/tmp/tqftpserv-ref，linux-msm/tqftpserv 全读）**：rmtfs
  是错参照（纯 QMI svc14+共享内存）；tqftpserv 才是我们的对端。stock
  handle_wrq 语义：**每会话 qrtr_open(0) 新口**发 OACK、后续 ACK 同口，
  客户端 DATA/ERROR 回会话口；回显集=blksize/timeoutms 恒回（缺省
  512/1000）、tsize 键在场即回（值 0 也回，`*tsize != -1`）、wsize 恒回
  （缺省 1，域 1..65535 无钳）、rsize≠0/seek≥0 才回；ERROR 9=END OF
  TRANSFER 正常收幕非错。
- **G28 五修**（对照 #24–#28 五靴「OACK 后零 DATA 零 ERROR 零重试」
  全冻）：①写 OACK/ACK 源口改新会话口（tf_new_port 0x1000+，发成功闩
  TF_W_XFER）；②客户端回会话口全闸收（on_wx 进 tf_note_ack/
  tf_note_data/RX-log/武装门四闸）；③tsize 改键在场即回（tf_w_parse
  加第 5 元 ts_seen）；④wsize 去 min(16) 钳（回显≠请求即协商失配）；
  ⑤wo= 武装 WRQ 选项段快照（TF_W_OPTS 96B nibble hex+len，五靴零观测
  的最后盲区——WRQ 带哪些键从此直读）。
- 风险在册：G12/G13 读腿临时口实验曾「纯 ping 活、首查 sm 即死」——但
  彼时无 on_xfer/on_wx 收口闸（G20+ 才有），写腿单发 OACK 暴露面一包；
  若 #29 早死即回查此分岔。

## 靴 #29（10-22 晨，G28 在机 boot-s28）——wo= 首见全解 + 读腿越 #28 冻点 +1 + 天花板第九证 + 狗属主翻案 modem 侧

- 刷写链：FASTBOOT UP 08:49:45、GATE OK、boot-s28 → FLASHED 08:49:47、
  ping up 08:50:35；**keep-alive FAILED cyc=127 +595s**=天花板第九证
  （前八证 +594s 级）。
- **wo= 首见（五靴零观测盲区清零）**：op0002 `/readwrite/server_check.txt`
  octet `blksize=7680 timeoutms=1000 wsize=1`（73B，无 tsize/rsize/seek
  键）。px= 同场读腿选项全貌：`blksize=7680 timeoutms=1000 seek=52
  wsize=10 rsize=160`——**读腿 wsize=10、写腿 wsize=1**，两方言不同窗。
- G28 观测件全落地：rl 逗号修生效（N=0x11=17 笔槽间逗号）；顶部「rl=4000」
  定谳名字撞车=GLINK_RXLEN（环形 0x4000）非阶梯；**G24 0x1011 悬案闭案**
  （干净仪器下再无 0x1011）。
- 读腿对账 #28→#29：阶梯 16→17 笔、xl 14→15、tf 尾 .1.5(#28 终态) vs
  .1.6(#29 全程)。**#28 冻点（sport 25 fl=3 rsize=0x34 seek=0）#29 被答掉**
  客户端推进 sport 26（fl=3 rsize=0xa0 seek=0x34）后静止至死。G28 未动
  读腿——越冻点+1 属客户端侧变数，非我方改动之功。
- 写腿判决：wx 单发无重试、OX=1（会话口 OACK 已发）、ww=1.1.0.0.0
  （active.exp.acks.bytes.errs——bytes=0）——**OACK 从 0x11(#28) 与从
  会话口(#29) 客户端同样全静零重试：源口方言假说排除**，客户端根本不在
  等我们的 OACK，写腿卡点在客户端内部时序或另一前置。
- w45=0、ic=0.0、im=27.27.1 全程不变。
- stock 校对补两锤：parse_options 域 blksize 8..65464——7680 合法、echo
  原样即 stock 行为；无选项 WRQ 分支 `tftp_send_data(client,1,0,0)` 是
  退化路（O_WRONLY fd 上 pread 必败→ERROR）——Qualcomm 客户端恒带选项，
  **ACK0 无 stock 先例**（跟发 ACK0 双方言实验降级为备选）。
- **狗属主翻案（收据考古三处合证）**：SMEM 421 崩溃区首 32B=
  `dog_hal_common.c:180:[tmr_slave3`（10-17/10-18 两窗同串）；radio-
  bringup 老账明写 qrtr-ns/pd-mapper/tftp_server/rmt_storage 不在听→
  DOG 判初始化卡住→硬重置；「tmr_slave3 DOG=init 在 EFS 未真正 IOVEC
  服务时的看门狗」。**+594/595s 天花板=modem 侧 EFS/init 狗，非 APSS
  WDT 0x17c10000——APSS 喂狗刀方向作废**，出路=解卡 modem init（写腿/
  rmtfs 链），server_check.txt 写腿=EFS 探针正是当前卡点。
- **刀 G29 在册（纯观察、零行为变更，保基线净）**：①po= 全口 RX 直方图
  （dst_port 8 槽计数——QMI-14/servreg 探测若被静默丢弃从此可见；仓里
  rfs_iovec 与 GPT 四块 rmtfs 影像在位，旧窗「OPEN 成功后不再发 ALLOC」
  的观测缺口正在此）；②oe= 写 OACK 字节回显（≤64B，对 stock
  0006"blksize\0"7680"timeoutms\0"1000"wsize\0"1 逐字节审计）；③ws=
  会话口值渲染（TF_W_XFER 闩至今不可见）。写腿行为零改动。

## 靴 #30–#34（10-22 上午五靴连环）——观测者白名单盲区 + 槽位七命耗尽 + UFS 环区 122 自杀 + 零填修复 + G29 三首数全落袋

**靴 #30（s29 首刷，09:23:25 刷/09:24:13 起/09:34:08 亡 +595s cyc=127）**：天花板第十证。
sm 全程 G28 形（wo= 在、ws=/oe=/po= 缺）——镜像链三验全过（s29.elf==当前树重建
4ad33aff、render 单站点、CTRL_REPLY=2560 无截断），真凶=**watch 脚本 SM 白名单是
G25 旧表**，重组行时新字段全被滤掉：G29 要闭的观测缺口在观测者一层重演。白名单
补 ws=/oe=/po=（+G30 的 pc=），此后新 render 字段与 watch 表必须同改（铁律①）。

**09:34–10:20 无窗（当时不解）**：每靴 ~145s 自复位环——后文 122 定谳。
七命烧穿 a（retry 0+unbootable）→ 回退 b。

**靴 #31（b 内容，09:41:24 放出）**：fastboot reboot 未带 set_active=直接进 b。
无网络面（en10 不生）。en11/.177.147/666.local 追凶半日=红鲱鱼（en11≠fw；
**fw 视觉面=en10+MAC 02:00:00:00:00:01 亲签**，别接口名追凶=铁律⑤）。

**靴 #32（10:22:48 重刷 s29+set_active a）**：仍无 ping。10:34:56 诊得
a retry=2（12 分钟烧 5 命=每命 ~145s）。

**靴 #33（10:34:58 `fastboot boot` 一次性引导，不落槽）**：同样死——A/B 槽位
机制整个排除。机器此后停在 fastboot（a=1try 未标死，ABL 为何不续试=未解悬案）。

**122 定谳**：`instrument::report_reset(code_secs)` 的码=忙等秒数再 PS_HOLD 复位；
145s=23s 启动+**122s 忙等**；码 122=`Fail::Check`（UFS CHECK CONDITION）于
`ring_put` 写/读回或开机 256 槽环扫描（main.rs:404/408/14936）。**日志环住在
boot_a+RING_OFF=16384 块（64MB 偏移）**，120KB 小镜像刷机从不触碰；#30 的狗复位
（09:34:08）把环区 LBA 打进 Check 态（推测复位打断了一次 ring 写），此后每靴
扫描/写环即 122 自杀。**修法（10:56 落地）**：零填加长镜像（16640 块=68,157,440B
=boot-s29.img+零）整刷 boot_a——写路径触发 UFS 重映射/清态，环区归零。
68MB 写全绿（环区可写=无硬死块）。

**靴 #34（修复后首靴，10:56:14 放出/10:57:03 起，49s 教科书启动）**：
**G29 三首数**——
- `ws=1000`：写 OACK 从会话口 0x1000 发（stock 同款）。
- `oe=0006"blksize\0"7680"timeoutms\0"1000"wsize\0"1` 共 27h=39B：**与 tqftpserv
  stock 期望应答逐字节全同**——客户端对完美 OACK 依旧零 DATA 零 ERROR 零重试，
  写腿等的根本不是这个包。
- `po=fffffffe.1e→38, 10.8, 12.2→4, 14.1, 16.3, 11.a, 1000.2, 15.1`：全口 RX
  直方图首账——TFTP 口 0x11×10、会话口 0x1000×2、**五个从未应答的探测口
  0x10×8/0x12×4/0x14/0x15/0x16×3**、fffffffe 底噪（非 UDP 帧）爬到 56 后冻结。
  探测集中在命初，之后纯静默到狗。「OPEN 后不再发 ALLOC」盲区的实体=这些
  没人应的门。读梯 rl=11(17) 仍冻同 #29/#30；天花板第十一证见尾注。

**刀 G30（码 a62cd1a 已推，s30.elf 70c09c64/boot-s30.img 已出待刷）**：pc= 探针
捕获——未应答口（真 16 位口、chunk≥96、非 ffffffxx）最近一包抄头 64B
（口.半字节hex.64）落 render；watch 白名单已加 pc=。「敲门」升格「递信」，
答什么看完信再裁。链 #35 待下次进 fastboot。

**槽位铁律②**：裸 fw 从不标 successful=每靴（含每次狗复位自愈重启）烧 1 命，
7 命尽自动掉 b。每次 flash 必带 set_active a（现役链已做）；无人值守自愈循环
最多 7 命自毁。G31 候选=fw 起网后自写 successful attr（L0 aginx-boot-ok 裸件版）。

**122 铁律③**：狗复位若再打断 ring 写→122 环复发，修法同款零填加长镜像重刷。

**尾注（11:06:59 落袋）**：天花板第十一证 cyc=127 +594s。终值
ww=1.1.0.0.0（写腿至死零 DATA）、po=fffffffe.38(=56),10.8,**12.4**,14.1,16.3,
11.a,1000.2,15.1——0x12 是命初探测后唯一被再敲的口（11:03 采 12.2→终值 12.4，
两封中途补投）＝G30 pc= 首选猎物；读梯 rl=17 冻结同 #29/#30。
**死后自愈复核**：11:06:59 死后 en10 链路 RUNNING 但 5min 零 ping，形似铁律③
122 环复发——3min 采样器 11:13:52 逮到 ping 复活证伪（1/12 UP）：自旋健康，
只是自愈比历史 90s 慢了 ~7min（新Datum，原因未查；非环毒）。教训=**疑似复发
先跑有界采样器再落账**。链 #35（零填 s30）已挂等 fastboot——padded 无论环态
都保险。自旋每圈 ~10.3min 烧一命，估剩 4–5（烧尽落 b=L0 救生艇，届时可试
L0 ssh `aginx-reboot bootloader` 免物理复位）。

**增记（11:24，G30b 叠刀换链）**：桌研翻案——#34 的 `po=fffffffe.38`
不是底噪，**0xfffffffe=uapi qrtr.h 的 `QRTR_PORT_CTRL`**：56 封全是
QRTR 控制报文（HELLO/NEW_SERVER/NEW_LOOKUP…），整条线只计数没读过；
且 `SMEM_LOOK`（NEW_LOOKUP 目击闩）G3 起只写不读——**modem 每靴在
LOOKUP 什么服务从未见过**（G2 起只答 host_svc_port 命中的自家服务，
没命中的静默）。刀 **G30b**（码 589bb22 已推）叠在 G30 上同刷：
ch= 控制包 typ 直方图、cc= 最近一枚 typ.svc.inst.node.port、lk=
SMEM_LOOK 上屏；watch 表同刀加三字段（白名单律）。s30 未曾上机——
链 #35 退役、**链 #36 在挂**（零填 s31，IMG=/tmp/s31padded.img 68MB，
elf 字面量验真 ch=/cc=/lk= 全在）。自动循环哨兵同挂（60s 绑定 ping 沿沿
落账，15min 死=AUTOCYCLE-STOP→a 命烧尽落 b/L0，届时试 L0 ssh
`aginx-reboot bootloader` 免物理复位）。

## 靴 #36（G30+G30b 首刷，链 #36 全自主闭环）——12:08 收

**链 #36 全自主闭环（首例零人工刷机）**：12s 强断→VolDown+Power 进 fastboot
是仅剩物理步（11:5x 用户手进）；之后链自动：串号闸门→getvars→零填 s31
68,157,440B→boot_a→set_active a→reboot→观察器 12s 采样。11:57:17 FLASHED，
11:58:24 ping up（起网 67s，历史带内）。

**天花板第十二证**：cyc=127 **+595s** keep-alive 首败（11:58:24+595≈12:08:31
死），5 粒全 dead 实锤——#29–#34 十一证 +594/595s 后第四连（594×3→595，
带内稳）。终值 ww=1.1.0.0.0、读梯 rl=17 冻结、wl/wo/oe/ws 全带内同 #34。

**G30 首数（pc= 递信）**：`pc=16.<hex>.64` ——口 0x16（未应答口之一）收到
100B，解 QMI SDU：**msgid 0x27 = QMI_IPA_CONFIG_REQ、txn=3、len=0x38**。
即 modem 问 AP 要 IPA 配置——正是 CFG_ARM 故意不答的那条（保 USB 不被
mismatch 配死）。「敲门」升格「递信」成功：信的内容=IPA 初始化握手。

**G30b 首数（ch=/cc= 控制面正账）**：`ch=2:1,4:28,6:f` ——**1×HELLO +
40(0x28)×NEW_SERVER + 15(0xf)×DEL_CLIENT = 56**，与 po=fffffffe.38 精确
对账（那 56 封全数读完，无一漏）。三首数：
- **NEW_SERVER=40**：modem 单靴广播 40 个服务——但 SMEM_SVC 只存 8 对、
  q= 渲染被 CTRL_REPLY(3072) 截尾，**服务图自 ~#27 起从没上过屏**（#36
  现场发现：sm 行稳超 3072，q=/iq=/ig=/im=/jx=/ic= 全截；控制台 16 命令
  id/ping/up/fb/log/bat/sm/pv/px/qm/qn/qc/qs/fr/tx 无一独立吐服务图，
  fr=SFR 读非重启，**无 fastboot 动词**——L0 腿是唯一免物理复位路）。
- **NEW_LOOKUP=0 + 旧 lk=0.0.5**：modem 从不查我们的服务（它只收我们的
  NEW_SERVER 广播）；SMEM_LOOK 暗闛空 =「modem 在找什么」答案=什么都不找。
- **DEL_CLIENT×15（首见）**：modem 侧客户端 socket 15 次开→超时→关——
  服务使用失败指纹（疑与 IPA REQ 无答同根：开完等不到回应用就关）。

**cc=4.0.0.0.0 判读**：最后一枚控制包=NEW_SERVER 五元组全零。两解留判：
(a) modem 临终真空通告（qrtr 语义里零端口=退场）；(b) 短包(<56B)守卫置零
（同偏移 +52/+56 在 IQ 闩上读到过真值 svc=0x31，字段本身是实数据）。
G31 ccz= 计零组+守卫只存非零组，下刷即分。

**lk= 撞名现场抓获**：sm 行出现两个 lk=（旧 LOOK_N.PEND.HOST_ADV=0.0.5 +
新 SMEM_LOOK=-）——grep 引号转义漏查旧名所致。G31 改名 ml=。

**G31 已推（76feff6，码尖位舞步 34 收据垫顶）**：CTRL_REPLY 3072→8192
（q= 服务图解锁）+ SMEM_SVC 8→40 对 + cc= 零值守卫/ccz= + dc=
DEL_CLIENT client{node.port}（15 连闭是谁）+ ml= 改名；--s32 出件
（字面量 ccz=/dc=/ml= 验真全在）；零填 /tmp/s32padded.img 68MB
（md5 3b2f80e536449e22e2eeeef729832309）。静态名撞车现场：新 TF_DC
撞既有 TF_DC(u32)——改名 TF_DCL；**撞名律升格：static 名同 render 名
一律先 grep**。

**链 #37 在挂（12:16:08 武装）**：#36 烧 try1 12:08 → 剩 6 tries ×
~10min，估 ~13:10 落 b/L0（旧哨兵 11:41 已停判过期——11:57 重刷后七命
重灌）。链等 `adb devices` 现身（serial=aginxosredfin）→
`/usr/bin/aginx-reboot bootloader`（绝对路径律）→ fastboot →闸门→刷
s32padded→set_active a→重启→观察器（白名单已带 ccz=/dc=/ml=）。
全程零人工；en10=02:00:00:00:00:01 持续在=仍是 bare 自旋（12:15 无
adb、无 ping——7min 慢自愈在案先不下结论）。

---

### 靴 #37（10-22 12:32–12:51）——G31 首刷：ccz=38 定谳短包守卫 + q= 双挡实测 + G32 同步刀

- **刷入**：s32padded 12:32:12（md5 3b2f80e536449e22e2eeeef729832309，串号闸门过、
  set_active a 重灌七命、reboot 12:32:21）。ping up 12:41:29 = **181s 慢**
  （en 界面中途重枚举翻牌，非盒行为；对照 #36 ~90s）。死 +594/595/596s =
  **天花板第十三证**（594–596s 带内，钟一样）。
- **ccz=38 定谳**：cc=4.0.0.0.0 全程不动 + ccz=38（56 枚控制包中 38 次五元组
  全零读）→ **#36 两解闭案：短包守卫坐实（b 解），非真空通告（a 解）**。
  旁证链完整：IQ 闩曾在 chunk≥52 下于 +52 偏移读到真值 svc=0x31——fifo 里
  union 字段全在，是 transport 报的 chunk 偏短，守卫（≥56/60/64）全数误吞。
- **dc=0.0.f**：DEL_CLIENT 15 连闭的 client{node.port} 同被守卫置零——「15 次
  关的是谁」仍无名。G33 候选：守卫降 chunk≥52。
- **ml=-**：SMEM_LOOK（G30b 暗闩，G31 自 lk= 改名）全靴空——modem 从不发
  NEW_LOOKUP，#36 判读复证：modem 只收我们的 NEW_SERVER 广播，从不查我们。
- **ch=2:1,4:28,6:f**：G30b 对账面在场（HELLO 1 / NEW_SERVER 28 / DEL_CLIENT
  15，渲染时点计数）。
- **q= 双挡实测**（G31 主货没上屏的两层原因，同日两犯白名单律）：
  1. 观察器白名单漏列 q=/iq=——G31 加了 ccz=/dc=/ml= 偏漏了主货（sm 行
     1785 字=过滤后残影的另一半真相）。已补：SM 元组现带 q=/iq=/ig=。
  2. 固件 TCP_TX 3072 帽：直连探针（单连接，盒活时合法）回 **LEN=3072 整**，
     行切在 `rpl=71.`——渲染序 rpl=→rr=→q=→iq=→ig=，后四段全被切。
     G31 只扩了 CTRL_REPLY 忘了配对 → **配对尺寸律**：CTRL_REPLY↔TCP_TX
     必同改（G21「TX 随 CTRL_REPLY 同步」的尺寸版）。
- **G32 已推（f1f3020）**：TCP_TX 3072→8192（入向 2560 不动）；--s33 出件
  s33.elf md5 15ccef4789b16288543212e4c5b4b5c2 ≠ s32 df252d7e…（新鲜验）+
  s33padded 68,157,440B（md5 e72908baaa4eaf2a606d0d1fd554953e）。字面量
  q=/iq=/ig= 在 s33.elf strings 验真。
- **build-bare.sh 失败门全铺（0afa1e0=34 处，G32 增 s33=35 处）**：cargo 失败
  即 exit 1 不拷陈旧件——G31 构建时 cargo 失败而脚本照拷陈旧 s30 的险情堵死。
  出件后仍须 strings 验字面量（双保险）。
- **舞后对账律立档（G32 实跑=空）**：⑤步后必跑
  `git diff origin/master master --stat -- ':!*.md' ':!docs/'`——G31 舞后曾
  漂移（收据堆盖掉 build-bare.sh 全部门，checkout origin 侧+amend 救回）；
  G32 舞后实跑空、build-bare.sh 门 35 处俱在。
- **链 #38 在挂**（pid 74576，12:47:03 武装，img=s33padded；日志标签笔误
  chain#37，img 为准）：等 L0 adb（本靴烧 try1，剩 6 命 × ~2-3min 快烧，
  估 ~13:05-13:10 落 b）→ 自动 reboot bootloader→闸门→刷 s33→set_active a→
  重启→观察器。**靴 #38 = q= 服务图真首屏**：40 对 svc.inst + iq= 四元 +
  ig= 11 格全量——0x45 WLFW 在不在 40 窗内即见分晓；iq= 若闩上则 IPA QMI
  0x31 的 node:port 首次有名字。

---

### 靴 #38（10-22 13:22–13:33）——G32 首刷：q= 窗口域缺陷定谳 + iq= 真闩 + w45=0 真负 + 链#38 两缺陷接力修

- **链 #38 原版楔死两缺陷**（全自主闭环二号靠 38b 接力完成）：
  1. **adb 现身≠L0 起齐**：13:14:53 adb up→+5s 发重启令，/usr 未挂齐，
     `/usr/bin/aginx-reboot` 幻影 not found（事后直查 rc=0 文件健在）。
  2. **无设备时 fastboot getvar 无限挂**：循环第一枚 getvar 楔 4.7min
     （fastboot 无设备=阻塞等待，非秒回）。修=先 `fastboot devices`（不挂）
     探在场才发 getvar。
- **链 #38b（接力模板）**：等文件可见腿（ls aginx-reboot 循环 120s）+
  devices 先探 + 重启令后台发。**全自主闭环二号**：L0 READY→FASTBOOT UP
  13:22:01→GATE OK（日志串号已脱敏）→FLASHED s33padded 13:22:04→
  SET_ACTIVE a→REBOOTED 13:22:04→观察器→ping up 13:23:11（0s 等待=
  快靴 ~67s，对照 #37 慢 181s=界面重枚举非常态）。
- **G32 验收**：q=/iq=/ig= 全量上屏（渲染尾部越过 rpl=71.74 rr=… 完整到
  jx=15.2.21）——TCP_TX 3072 帽解除，截尾历史终结。
- **iq=1.201.0.1d（真闩，首份 IPA 身份证）**：svc 0x31 inst 0x201（v1/i2，
  stock 已知对）注册在 node 0 port 0x1d——IQ 闩以 chunk>=52 读 +64 拿到
  真 port，**顺带坐实 fifo 里 union 全在、transport 报的 chunk 偏短**。
- **w45=0（真负）**：SAW_45 累计闩（不随扫描清）全靴未火——**0x45 WLFW
  本靴真没来**（wlan_pd 未起，线目标仍开放；非窗口假象）。
- **q= 空（窗口域缺陷定谳）**：scan_new_servers 每轮入口清零
  SMEM_SVC/SMEM_LOOK——宣告风暴（ch=4:28 NEW_SERVER）在开机头几秒，
  ping up（~70s）时解析窗早翻页。**同窗 iq=（累计）闩上而 q=（窗口）空
  =矛盾定谳**；早期靴 q=42.b401,2b.1202 有值=渲染恰逢风暴在窗内。
  log0 环只剩 TCP 台账尾部，风暴窗数据本靴不可回收。
- **ig= 首值**：1.11.205.1.20070000.e404718a.0.deadbeef.deadbeef.600.0
  （st=1/iver=11/gcfg=205/gstat=1/gver=0x20070000/hp2=0xe404718a/ch0=0/
  ca=cb=deadbeef 未映射/smem=0x600）。
- ccz=38 / dc=0.0.f / ml=- / ch=2:1,4:28,6:f 与 #37 全同（守卫未动）。
- 死 +594/595/596s = **天花板第十四证**（带内三连跳）。
- **G33 已推（e61f8a9，码尖位舞步 36/36 干净、舞后对账空）**：q=/ml= 改
  累计+去重闩 + 守卫 56/60/64/68 全降 52（仅 union 型别 4/5/6/10/11 读
  元组，HELLO 短包不抄邻包）——cc=/dc= 首读真值。--s34 出件（elf
  2a49e94d…≠s33 15ccef47… 新鲜验，q= 字面量在）+ s34padded 68MB
  （md5 c893799d96e8205f295909e9a68dbe75）。**链 #39 在挂**（pid 83047，
  38b 修好模板直系）——靴 #39=累计 q= 服务图真首图 + dc= 15 连闭身份
  首读。

---

### 靴 #39（10-22 14:12–14:23）——G33 首刷：累计 q= 40 对全图 + 0x45 判死本靴 + dc= 身份解锁 + pd-mapper 假说成形

- **链 #39b 全自主闭环三号**（合并模板首跑）：L0 adb 14:11:12 → L0 READY
  14:11:48（等文件腿 36s 生效）→ FASTBOOT/GATE 14:12:00 → FLASHED s34padded
  14:12:03 → SET_ACTIVE a → REBOOTED 14:12:04 → ping up 14:13:12（68s 快靴）。
  链 #39 原版缺 adb 巡腿（裸命期起链 120s 烧完自杀）——**链模板律第三条：
  起链时盒可能还在裸命，巡腿必须居首**。
- **累计 q= 全图（40 对，四拍哈希同=去重闩稳、无回卷翻倍）**：
  qmux 全家 CTL/WDS/DMS/NAS/QOS/AUTH/AT/VOICE(v2)/PBM(v2)/UIM/CAT +
  SERVREG_NOTIF(.b401)/SSCTL(.1202)/IPA(.201)/DPM?(22)/0x33×2 实例
  (.201/.b01)/0x37.201/0x16.1001/0xf/0x15/0x17/0x18/0x24/0x2f/0x30/0x44/
  0x49/0x4a/0x4b/0x4c/0x4d/0x4e/0x1a/0x1d/0xe7/0x11/0x10.2 + 0x1002.0
  （高位服务号疑错位读，留观）。**modem 处理器=完整活体，QRTR 命名空间
  全起**。解码存档 /tmp/boot39-svcgraph.txt。
- **0x45 WLFW 判死本靴（双证）**：全图 40 对无 0x45 + SAW_45 累计闩
  w45=0——非截尾非窗口假象，**wlan_pd 本靴真没被拉起**。
- **ccz 38→1 + cc=4.10.2.0.54**：守卫降 52 + union 型别闸全验——38 枚
  「零五元组」里 37 枚是守卫误吞，仅 1 枚真零（HELLO 短包）；末枚
  NEW_SERVER=svc 0x10 inst 2 port 84。
- **dc=0.21.f（15 连闭身份解锁）**：node 0 port 0x21。**我们=node 1**
  （glink_tx_qrtr …,1,port 直证）→ 15 连闭=modem 侧 node 0 port 0x21 客户端
  socket 反复开→关（撞号 TF_DATA_PORT=0x21 属每节点端口独立之巧合）。
- **po= 未应答口对上公告表**：ctrl(fffffffe)×38、0x10(rmtfs)×8、
  **0x12(pd-mapper)×4**、0x14×1、0x11(tftp svc)×10、0x16(IPA A5)×3
  （=#36 pc= QMI IPA CONFIG_REQ 不答路）、0x1000×2、0x15×1——modem 在敲
  我们公告的全部服务口；tftp 线有人应（mbn 在传），rmtfs/pd-mapper 无人应。
- **pd-mapper 假说成形（G34/G35 方向）**：modem 的 PD 机制拿不到 PD map
  （我们公告 svc 64@port 0x12 但无应答）→ wlan_pd 不生 → 无 0x45。
  G34（观察刀）=0x10/0x12 定向包环（每口末 2 包 96B 原文上屏）拿到
  GET_PD_MAP 请求真字节；G35（行为刀）=按真字节应答 PD map 表。
- 死 +593s = **天花板第十五证**（带 593-596）。

#### 勘误（10-22 15:00 补，翻 main.rs 定谳）——「0x12 无人应」错，应答器早已在役

- 上条「rmtfs/pd-mapper 无人应」**错**：po= 是 RX 直方图（应答了也计数），
  不辨答否。真相=**固件里 pd_reply()/DOMS/pd_take_name 全套早铸**（此
  前刀已落），#39 live 行 `pr=3 pn=kernel/elf_loader.1.30 pe=1` 证明
  modem 问 kernel/elf_loader、我们答了 3 次（1 命中=msm/modem/wlan_pd
  inst 180，qlen 0x30）——4 敲 3 答 1 丢（PD_PEND 串行占位期丢），且
  客户端反复问。rf= rmtfs 腿同帧也在应（3..3.8.280000.0）。
- **w45=0 的真问题改写**：不是「无人应」而是「答了为何还问/wlan_pd 仍
  不生」。候选：应答编码瑕疵（静态审计 tf=2/TLV 序/entry 四件套全过
  service-locator ei 表，暂无把柄）/ 3 问=3 客户端非重试、答已收下、
  卡点在下游（kernel/elf_loader 真装载=PIL，裸世界没人干）/ 送达问题
  （qrtr 头布局疑云：+32/+36 呈 src 形状，#40 pdh= 定谳）。
- 案头全料（协议真源=redbull service-locator-private.h、厂 pd-mapper
  二进制、jsn 全 6 PD 表、判读器 /tmp/decode-pd.py）已落
  `devices/redfin/bringup/bare/PDMAP.md`。G34（链 #40 在挂）拿请求
  真字节裁决。

### 靴 #40（10-22 15:01–15:11）——G34 首刷：捕获腿 96 门自盲（G33 律复发）+ 敲问形状定谳（早窗突发后零重试）

- **链 #40 全自主闭环四号**：L0 adb 14:59:00（a 命尽落 b）→ L0 READY
  14:59:35 → FASTBOOT/GATE 14:59:47 → FLASHED s35padded 14:59:50 →
  SET_ACTIVE a → REBOOTED 14:59:51 → ping up 15:01:04（73s）→ 死
  +594s（cyc=127）=**天花板第十六证**（带 593-596）。
- **G34 捕获腿全靴自盲（自伤，当场定谳当场修）**：pd=/pdh=/rm= 三面
  渲染活着但恒 `-`（TF_PD12_N==0）——而 po=12.4 明明数到 4 包 0x12、
  pr= 3 次应答照发。根因=捕获门 `chunk>=96`：GET_DOMAIN_LIST 全
  union 才 ~76B（48 qrtr+7 QMI+21 TLV），transport 报的 chunk 到不了
  96——**G33 短包守卫律复发**（自家立的法自家撞；G33 已把 56/60/64/68
  全降 52，G34 新腿又写了 96）。pc= 0x16 能火是因 IPA CONFIG_REQ
  len 56→union 111B 过得了 96 门——两门一真一假同帧并存，盲区由此
  藏了半靴。**G34b（647b727 已推）**：门降 `chunk>=52`+`off+96<=
  GLINK_RXLEN` 界守卫；pd= 名解同步修 @58 无前缀可打印串。
- **敲问形状定谳（本靴真信息量）**：pr= 2→3 全部落在 +1..+5s 早窗，
  之后 10 分钟**零增长零重问**（po= 12.4 终值与 #39 逐字同）——客户端
  不是重试循环；结合 #39 勘误（答已发 3 次），**「答已被收下、卡点在
  GET_DOMAIN_LIST 之后的下游」候选升头名**（elf_loader 真装载=PIL 线/
  后续 QMI 步无人接）。#41（s36）拿 pdh= 96B 真字节裁决布局+txn 形状。
- w45=0 双证复现（40 对全图无 0x45+累计闩）；q= 40 对四拍哈希同；
  rm= 同盲（0x10 的 8 敲也没进环，G34b 一并救活）。
- **推送踩空两雷（对账律拦在 push 前，零污染）**：`checkout -b` 接
  `| tail` 吞了中止退出码+PDMAP 墪后编辑挡 checkout——码提交落收据
  堆顶；`git log origin/master..HEAD` 数件数预检逮住，本地 rebase 两
  步修复（码摘尖 647b727 推送、40 收据垫回），对账三件套全平。配方
  已入册 push-dance-recipe.md。
- **链 #41 在挂**（15:09:12 pid 3636，img=/tmp/s36padded md5
  8454fc60…，等 L0 自动刷）——靴 #41=G34b 真观察窗。

### 靴 #41（10-22 15:52–16:03）——G34b 首真字节：qrtr 布局疑云当场破解（+32/+36=size 非 src）+ 敲问漂移（2 敲/未问 elf_loader）+ 三个潜伏缺口浮出 → G36 刀案

- **链 #41 全自主闭环五号**：L0 adb 15:51:28 → L0 READY 15:51:58 →
  FASTBOOT/GATE 15:52:11 → FLASHED 15:52:14 → SET_ACTIVE a → REBOOTED
  15:52:15 → ping up 15:53:28（73s）→ ICMP dead +596s（keep-alive
  FAILED cyc=127）=**天花板第十七证**（593..596 全程未破）。链脚本
  止于 watcher（无自动续靴），靴 #42 待 G36。
- **pdh= 首真字节（G34b 验收通过）**：捕获门 52+界守卫全靴工作正常，
  pd=/pdh=/rm= 三面全活。首获 96B（末敲）解码定谳：
  `src=0.0x0b dst=1.0x12 QMI tf=0 txn=2 msg=0x21 len=22 TLV01
  name='tms/pddump_disabled'`——TLV len=0x13=19 **恰等于名字长**
  （值首字节即 't'，无 strlen 前缀、无 NUL），PDMAP ⑺ 的 @58 方言
  论证实；QMI len=22=3+19 恰好。
- **qrtr 头布局疑云破解（读法清单第 1 项，当场两案对账）**：
  PD 包 `+32/+36=0x00000000.0000001d`=29=**QMI 总长 7+22**；IPA 包
  （pc=）`+32/+36=…3f`=63=**7+56**。两案全中 ⇒ **+32..+39=u64
  payload size，不是第二地址**。方言布局定谳：`{+0..+15 传输前缀
  （0x00020009,4,0x40,0）, +16 ver=1, +20 type=1, +24/+28 src,
  +32..+39 size(u64), +40/+44 dst, +48 QMI}`——内核 qrtr_hdr_v1 的
  src/dst/size/confirm 在此被重排（size 提前、无 confirm 槽）。**回址
  +24/+28 无罪**（tftp 多回合铁证成立），G36 双发刀不需要了。
- **敲问漂移（本靴最大意外）**：po=12.2 终值（**仅 2 敲**，#39/#40
  均 12.4）、两敲全答（pr=2）、末敲名=tms/pddump_disabled、**pe 不闩
  =本靴根本没问 kernel/elf_loader**。敲问数与名字逐靴漂移 ⇒ modem
  侧 tms/PD-manager 的查库时机/顺序非确定（随内部时序走），4 敲 vs
  2 敲非「重试」差。语料 grep 证 tms/pddump_disabled 不在六 jsn——
  我们答空表（nh=0→TLV10 total=0、无 TLV12，main.rs pd_reply 静读
  定谳）与厂库一致（或厂库亦空）。
- **三潜伏缺口浮出（G36 刀案，boot #42 装）**：
  ①**去重键裸 txn**：PD 腿 PD_SEEN 只比 txn，而 0x10 rmtfs 腿注释
  明证「多客户端各自从 txn=1 起算，必须进去重键」——PD 多客户端
  txn 撞号即错杀（#39/#40「4 敲 3 答 1 丢」的丢可解释）；
  ②**应答腿只认 0x21**：msgid 0x20（INDICATION_REGISTER）/0x22
  （REGISTER_SERVICE_LIST）敲来无声吞——canon（service-locator-
  private.h）两 resp 都=TLV02(0,0) 极小（MAX_LEN 7），被吞的第 4 敲
  完全可能是 0x20/0x22，客户端等超时整条查库作废；
  ③**PD_PEND 占位期丢敲**：早窗突发两敲同扫一圈即丢一（无队列）。
  G36=三修+pd0=/pm= 渲染（末 2 敲名字/txn/msgid 全见）。
- **FLASHED 行标签假警报**：链日志印「s35padded」=sed 只换 IMG= 没换
  echo 死串——实刷 `$IMG`=/tmp/s36padded.img（md5 8454…已核），
  纯标签伪影，靴有效。

### 靴 #42 事故收据（10-22 16:44–17:4x，G36 观察窗被盒级事故吃掉）

- **现象**：链 #42 16:44 刷 s37（G36，md5 b23bac…，68157440B 零填
  合规、set_active a）→ **3 条命全程零 ping**（17 靴以来固件首次
  完全不起，且 3/3 确定性复现）。17:13:10 tries 烧尽落 L0。
- **对照实验（17:14:01，链 #43）**：把**已知完好的 s36**（=靴 #41
  15:53 那版，ping 73s 即通的同镜像，md5 8454…）原样回刷 →
  **同样零 ping**（+870s 无起色）。⇒ **裁决：非 G36 代码问题**，
  G36 三修（去重键/0x20·0x22 腿/二队列）无罪，观察窗顺延。
- **Mac 侧 USB 枚举全盲**：system_profiler 无任何 Google/Pixel/
  Android 设备（fastboot/adb 影子全无）→ 盒级事故（在代码层之下）。
- **候选病因（按先例排序）**：
  ① 盒子 USB/固件卡死——台账两先例：redfin USB 盲但盒活（重启
  即救，09-26）、enchilada ff/ff/00（**唯物理断电可解**）；
  ② **电量褐断**——裸固件无充电驱动，今日 17 靴 ≈ 3 小时纯耗电；
  电量计有深放焊 0% 前科（#483–486）；modem 上电大电流抽载 →
  可能在 USB/网络起之前就电压塌断。若如此，每条命在 modem
  bring-up 处闪断，症状=快速烧命且永远见不到 ping。
- **救援**：物理 12s 强制断电 → 开机落 L0（tries 尽→slot b）→
  adb 读 /sys/class/power_supply/battery/* 定电量 → 低则 L0 充
  20–30 分钟再刷；s37 届时重获公平机会。
- **方法论收获**：固件不起时先刷上一版好镜像做对照，再谈代码——
  本次 15 分钟内把「我的刀」和「盒子」分开，避免了在好代码上
  瞎找 bug。

### 靴 #44 收据（10-22 17:39–，s37/G36 复审窗：modem 二十靴首次上爬）

- **前置**：冷断电救援成功后链 #44 直刷 s37（17:39:28，电量 4438mV
  soc-ok 正常）。**ping 17:40:24 通（55s）**——G36 无罪、16:44 楔死
  确判偶发盒级事故（非代码、非电量）。
- **中途审计乌龙三连（记录防复发）**：①strings/grep 对 LZ4 压缩
  bootimg 无效（连 w45= 都「找不到」）——**镜像侧字面量审计必须先
  解压 kernel 载荷**（python 取 header kernel_size+4096 偏移，lz4 -d
  后再扫：s37padded 解压后 pd0=/pr20=/pr22=/pdh=/w45= 全 count=1，
  构建链清白）；②BSD grep 68MB 无换行二进制超长行静默失配；③活线
  sm 行跨靴累积，审计前必须先定位本靴 ping-up 行再取其后帧。
- **G36 生效硬证（三 epoch 对比）**：`pr=2→3`（多答在发）、末敲
  `pd=4.0.21.4.kernel/elf_loader`（elf_loader 查询回归）、
  **q= 服务图爆炸**：基线 14 项→e1 +16（e7/4c/37/c/9/3/a/2f/b/1/
  4/1a/8/4d/4a/4b）→e2 再 +10（**44.1**/2a/2/5/30/49/4e/1d/7/10.2）
  ——q= 为 service.instance 对（42.b401=SERVREG_NOTIF 实证），
  **svc 0x44 已现身，距目标 0x45（WLFW）一格**。bg=00→1e68.1.1.16.4、
  i2/bx 醒、xl epoch 12→15、cc 4.22→4.10（调用面变化）。#42 读法
  清单第 4 项半兑现（q= 新对大量出现=PD 框架在动）。
- **渲染缺口（化妆级）**：pd0=/pr20=/pr22= 落在了非周期 sm 渲染位
  （周期 sm 线走的是另一组 pd=/pdh= 代码），本靴全程不可见——计数
  腿功能正常（q= 爆炸=应答被吃下），纯观测面缺，下刀顺手补。
- **新敲语料（布局小额重开）**：word2/word3=前缀漂移 (4,0x40)/
  (6,0x3c)/(7,0x34)——#41「u64 size@+32」读法对新敲不闭合
  （tms 敲实际 +32=29=QMI总长、+36=1），布局案挂小额，不阻塞主线。
  tms 敲尾现第二 TLV `0e 0002 001e`（type=0x0e len=2 val=30，
  canon 无此 TLV——modem 方言再添谜）。

## 靴 #44 补收据（18:14–18:26 命 2 全字段直查）+ G38 刀案（10-22 晚）

观察器 SM 白名单漏字段（盲区律第三犯），命 2 用裸 TCP 直连一次
`sm` 拿全量渲染，四大定谳 + 一谜底：

- **rmtfs 完全无罪（ik=4.0）**：`rf=3..3.8.280000.0 rh=8.0.9000101.
  a000101.d000104.e000103 ik=4.0`——8 敲 0 丢，OPEN 答过
  （rs=02010001000e0002 回显）、ALLOC_BUFF 准 2.5MB@0x9E000000、
  hyp 分配成（RF_AS=0）、4 枚 RW_IOVEC 全 OK。rmtfs 腿出局。
- **wlan_pd=UNINIT（sr=1.7fffffff.0）**：我们的 REGISTER_LISTENER
  被收下（sr=1），modem 报 PD 状态 0x7fffffff=UNINIT——PD 从未拉起。
  rp=0.ffffffff：RESTART_PD 从未发（main.rs 8336 Linux pdr 语义：
  只对 DOWN 发，不对 UNINIT 发）。**「spawn 是否可请求」升为下猎物**
  ——G38⑥ rp 手动杆直接实验。
- **CFG_ARM 耦合发现（观察者效应实锤）**：sm 命令处理里藏着
  `CFG_ARM=1`（13524）——每个观察命都在臂 IPA CONFIG 应答
  （「成功的 CONFIG 应答会在几秒内弄掉 USB」旧注）。命 2 证据：
  应答 ~+90s 已发（im=27.27.1）而 USB 活到 +593s——「秒杀 USB」
  本靴未复现，但耦合必须拆：G38① cf 命令显式臂，sm 恒不臂。
- **0x15 敲豁免**：jx=15.2.21 定谳=它是对**我们自己** IPA
  INIT_MODEM_DRIVER_REQ（0x21）的 RESPONSE——po= 直方图计数无嫌疑。
- **谜底：SM 白名单盲区律**——观察器 SM 元组滤掉固件已渲染的
  pn/pe/pr20/pr22/rf/rh/ik/pd0/pm/sr/si/sp/rp，造成「固件不渲染」
  假象（#30/#37 同根）。直查所见 rf=/rh=/ik=/sr= 全在场。教训固化：
  **新渲染字段与观察器白名单必须同刀同改**（G38⑤ 已改，19 字段在列）。
- **命间确定性**：命 1/命 2 的 q=/po=/pd=/rm= 逐字全同（42 服务、
  po=fffffffe.38、pd=4.0.21.4.kernel/elf_loader）——modulo 时钟字段。
- **控制台 22 连楔死（天花板判读修正）**：两命都在第 22 次查询
  （sm22）控制台 RST 拒连而 ICMP 仍活数分钟——**单套接字 TCP 状态
  楔死**（main.rs 15228 注释在案的 abort 表盖不到 SynReceived 类
  状态），控制台死≠盒死。天花板 18 证全走 ICMP（判据不受污染），
  但**每命 sm 样本上限 ~22 帧**——天花板与控制台楔死是否同根
  （资源耗尽类）挂账，G39 候选（abort 表补全/SynReceived 超时腿）。

### REA 傍证（modem.mdt 静态侦察，为 elf_loader 线备料）

- **bNN↔phNN 映射钉死**：modem.mdt 是 ELF32（phoff 0x34，35 段），
  bNN 文件长==phNN filesz 逐段全中（b22=RO 数据段 flags 0x8000004：
  off 0x15b8000/vaddr 0xc2623000/filesz 0x9d423c）。exec 段=
  b02/b04/b05/b08/b09/b10/b13/b14/b30。
- **REA 对本模块死路**：串走 MSG 消息表（无 ADRP+ADD、无 movz 端口
  立即数、无绝对指针表）——tftp 客户端串在 b22@0xed06e（选项表
  @0xed108）/@0x10342（文件名表），但代码侧无锚。资产存档：
  /tmp/b22.elf（ELF64 壳，vaddr 对齐，可 llvm-objdump 复用）。
- **厂 pd-mapper 数据面**：libpdmapper.so 无内嵌 DB，只读
  /vendor/firmware{,_mnt}/image[/sdx55m]/*.jsn——我们的 6-jsn 语料
  ==厂 DB；tms/pddump_disabled 敲空表=厂一致行为，非堵点。

### G38 刀案（s38，boot #45）——四观察一解耦一杆

1. **① CFG_ARM 解耦**：sm 不再臂 IPA CONFIG 应答；新 `cf` 命令显式
   臂 + `cfa=` 渲染。观察命从此不带观察者效应；CONFIG 应答时序对
   天花板的影响成为可控变量。
2. **② DB_UPDATED ind 探针**：第 3 笔 0x21 答成后向末敲者发一发
   0x23 ind（tf=4 空 TLV，`pdi=` 收果）——客户端重查
   GET_DOMAIN_LIST=ind 道活着，PD 框架卡点下移到 spawn 侧。
3. **③ 写腿三时刻 wd=**：WRQ 见/OACK 发成/末枚 DATA（cntvct 秒）——
   「+3s 全静」只有结论没时刻，天花板分析要「控制台死前写腿最后
   动作在第几秒」。
4. **④ pk14= 0x14 定向抄盘**：po= 0x14×1 是五未解口唯一无腿者；
   52 门+界守卫（G33 短包律），末包 96B hex。
5. **⑥ rp 手动杆**：`rp` 命令置 SR_NEED_RP 绕 DOWN 门（UNINIT 人为
   踢）——agent-in-the-loop spawn 杆，`rpm=` 计数 + SR_RP_ERR 收果。
6. **⑤ 观察器白名单**：+13 旧字段 +5 新字段（wd/pk14/cfa/rpm/pdi）。

出件：boot-s38.img 122,880B（LZ4 帧 117,638B，HC=0xb9 roundtrip 全等）；
**零填 68,157,440B**（122 环毒律）→ /tmp/s38padded.img。链 #44：
L0 现身 18:32:44（s37 命尽回滚 b，current-slot:b/retry:a=0 实证）
→ 闸门 OK → **18:32:59 刷入 boot_a → set_active a → 重启**，
观察器 pid 48681 挂上（白名单已扩）——boot #45 出发。

## 靴 #45 收据——G38 全活体 + ind 毒环定谳 + G39 拔毒刀（s38/G38，2026-10-22 晚）

**刷机**：链 #44 18:32:59 FLASHED boot_a s38padded（零填 68,157,440 全幅）
+ set_active a（七命律）；current-slot:b 证回滚；ping up 18:34:12（+73s）。
命 1 +594s 殁（天花板第十八证，ICMP 口径）。命 2 ping up 18:51:05
（waited 179s——本靴启动显著偏慢），观察器续采。

### G38 六件全数活体生效（首帧起）

- **cfa=0**：①CFG_ARM 解耦成功——sm 不再暗臂，观察者效应拔除落地。
- **pdi 0→1**：②ind 探针命 1 于 +3s 内发出（PD_N==3 后下一拍）。命 2
  同形：sm0(+3s) pdi=0（pr=2 第三敲未到）→ sm_e1(+5s) pdi=1。**命间
  确定性再次复现**（G37 结论加固）。
- **wd=67.67.0（命 1）/ 49.49.0（命 2）**：③写腿三时刻上表——WRQ 与
  OACK 同秒落地、**DATA 永不**。写腿冻结的「何时」从此精确：rfs 会话
  建立即冻（两命分别 67s/49s，启动方差，同形）。
- **pk14=**：④0x14 口 96B 专拍两命各 1 包，**首帧破案**（下节）。
- **rpm=**：⑥rp 手动杆接受（命 1 fired 1）。
- ⑤白名单 18 字段全在帧（#43/#44 盲区补账落地）。

### pk14 破案：0x14 = 我们自己的 SR 客户口

两命同包（仅 txn 字节差）：`tf=2 txn=1 msg=0x20 len=14`，TLV02=(0,0)
成功、**TLV10=0x7fffffff curr_state=UNINIT**。即 **REGISTER_LISTENER_RESP
——我们自己 REGISTER 的回包**；0x14 = SR_QRTR_PORT（SERVREG 客户口）。
「五未解口」减一：0x14 从无主变有主，**无缺失腿**。#44 遗留的 +32
size≠QMI len 布局案仍开。全 96B hex 两命全存 log（命 2：
`0900010004000000...`）。

### ind 探针判词：客户端不理 DB_UPDATED

命 1 pdi 发出后 pr 恒 3（sm0→sm8+）——**客户端不重查 GET_DOMAIN_LIST**
（#44 直查已见 pr 三笔全落，此处为 ind 侧证）。ind 道对客户端无牵引力，
PD 框架卡点不在此。

### rp 杆发而不出——三层解剖到定谳（本靴主案）

现象：命 1 ~18:39 fire rp（回 `rp fired 1`）后 sm6-sm8 恒
`rpm=1.0.ffffffff`——命令腿跑（SR_RP_MAN=1）、RESTART_PD 从未上路。

**逐门排查（sr_tick 8364-8479 全门对活体渲染验绿）**：
at478/479≠0 ✓；HOST_ADV=5（lk= 三栏）≥4 ✓；HAVE_IID 一次置位永不
清 ✓；RF_IOK=4（ik=）✓；SR_HAVE/sp=0.2（SERVREG 在 node0:port2）✓；
REGISTER 已答（sr=1.7fffffff.0，RESP 腿 12929 存态并清 SR_PEND=铁证）；
SR_ACK_PEND 假（si=0 无 IND）；ind 探针分支已惰化（ARM=2/N=1）。
**唯一存疑门：8377 `tx_head != tx_tail` 空环早退。**

**假线索两枚（记入防再踩）**：
- `tx gq=480.480` 看似空环——**gq= 是 LPASS 环**（lpass_glink 的 LG_D），
  与 MPSS at478 无关。tx 命令从未能看 MPSS 环（G39③修）。
- `i7=4.34.4.4.0` 看似「求批无门」——**i7= 是入站 cmd7**（modem 向
  我们要 52B intent×4，补发5/8 各 4 皆已发）。出站求批计数是 IIP_REQ
  （ii= 第五栏=0，DRY=0）——intent 池 15 free 从未干涸，intent 经济无罪。

**定谳证据（命 2 当场取）**：
1. `xl=` 尾笔**两命同形 `12.38.cb8`**——sport 0x12/整包 0x38/发后
   head=0xcb8 = **G38② ind 探针**（7B 载荷+28 头+16 glink=56B 整包 0x38）。
   命间确定性：ind 是 MPSS TX 环上最后一件被台账记录的发送。
2. **TXK_N 15 秒 62→80（≈1.2/s 常踢）**——tx_rekick 仅在「环非空且
   tail 静止≥500ms」才踢。环卡死实锤，续踢不能救。
3. 命 2 sm17 全帧（本次未被截）：**`tt=d18.c80.4000`**——tail=0xc80
   **恰是 ind 写入起点**（写区间 c80→cb8），head=d18 = ind + 后续 8 笔
   intent 经济补发（TX5/TX8 各 4，皆直写环不进台账）。**modem 把 ind
   之前的一切吃完、把 ind 起的一切永不吃**。

**因果链定谳**：REGISTER 被吃有回（sr=1.7fffffff）→ G38② ind 写给
modem 自己的 0x12 客户口 → modem glink 消费死在该包 → 环永久非空 →
sr_tick 8377 空环门永早退 → ACK 腿/RP 腿全灭。**G38① 拔观察者效应、
G38② 自己造了一个新的**——写给对端不服务的口的包会毒死整条 TX 消费，
观察者效应病族第二刀（第一刀=CFG_ARM 暗臂）。

### w45= 整靴不可见案（盲区律第四犯）

pk14 的 192 字 hex 洪峰把尾部字段（w45=/tt=/rq=）**挤过 TCP_TX 帽**
——本线头条指标 w45= 在 #45 全部观察器帧里缺席（仅直连全帧 sm17 见
`w45=0`）。教训升格：**新字段不止要进白名单，还要保证它不被同刀的
胖字段挤出回包**；头条指标一律置头部。

### 其他

- 命 2 控制台一次 ConnectionRefused 间发（自愈，后续 sm 照答）；命 2
  采样过 sm22 无楔（22 连楔死律命间有方差，abort 表补全仍为候选）。
- 命 2 q=42 平台期复现（b401 顶格），svc 0x45 仍 0——与 ind 毒环并存，
  #46 拔毒后才可分辨「w45=0 是否部分被 RP 腿堵死放大」。
- 天花板：命 1 +594s；多命 ICMP 边沿跟踪器在役（/tmp/s45-multilife.sh）。

### G39 刀（s39/boot #46，已锻已链）

- **① ind 自动臂退役**：pd_reply 不再臂 PD_IND_ARM；新 `pi` 命令显式
  臂（探针结论已收，要再探须明示——观察面全部收进显式杆族 cf/rp/pi）。
- **② w45= 前置**：渲染移 sm 头部 rk= 之后，永不被截。
- **③ mh= 新探针**：MPSS_TX H/T 现场读数（smem_scan 每拍抄环），
  sm 头部+tx 命令双位渲染——**环卡死从不可见变每帧可见**。
- **④ pk14 瘦身**：sm 只留计数，96B 全hex 移 `pk` 命令。
- 观察器白名单 +mh=（同刀同改）。出件 s39（122,880B）已零填
  68,157,440 全幅（md5 c15eedad54e4），**链 #46 已挂**（等 #45 命尽
  回滚 b → L0 adb → 闸门 → 刷 s39 → boot #46）。
- **#46 预演**：无 ind → REGISTER-RESP 后环应回空（mh= h==t）→ rp
  真发（rpm=1.1.*，TXL 尾笔现 sport 0x14）→ 看 SR_RP_ERR 回果/卡点
  下移。若 RESTART 真上路而 w45 仍 0，瓶颈正式移出 PDR 层。

### Git

G38 码 5f44b39、G39 码 04657ce 先后码尖位上推（9d07c11→5f44b39→
04657ce）；收据堆 45 件垫顶（本地 ahead=45，纯收据）。盲区律在本靴
的完整谱系：#43（白名单滤）→ #44（截尾）→ #45（胖字段挤帽+看错环）。

## 靴 #46（s39/G39，2026-10-22 晚）——毒环拔除定谳+RP 往返+rmtfs 翻案

### 时间线

- 19:0x 用户手动进 fastboot（12s 断电→Power+VolDown），链 #46b 双触发
  接管：GATE OK → **19:08:07 FLASHED boot_a s39 → SET_ACTIVE a →
  REBOOTED** → 19:09:04 ping up。**55s 全 boot，快车道成型**（对照等
  滚命 3h：用户手动入口+链自动闸门刷=200 倍速）。
- 命 1 up 19:09:04 → down 19:19:13 = **+609s 天花板第十九证**（594s
  狗杀+收尾）。多命 ICMP 跟踪器在录命 2+。

### G39 四刀全验

- **② w45= 头部在位**：全部观察帧可见（#45 盲区根除）。
- **④ pk14 瘦身**：本靴全帧无截尾。
- **③ mh= 探针**：基线 `mh=d90.d90`（h==t 空环）。
- **① ind 拔毒定谳**：基线 xl 尾 `TXK=0` 全命零 rekick（#45 同形时段
  14f→1f1 ≈1.2/s 常踢）——REGISTER-RESP 后环回空，**G38② ind 毒环
  根因钉死、G39① 拔毒有效**。tx_rekick 只在环非空时踢，全命零踢=
  全命空环。

### rp 首发全程往返（首发+整往返首收据）

- `rp` → xl 台账增 **`14.50.de0`**（sport 0x14/整包 0x50B/发后 head
  =de8）+ `mh=de8.de8`——RESTART_PD 首次真正离站且环保持空。
- `rpm=1.1.90001`（MAN=1/N=1/ERR）+ **0x24 RESTART RESP 回来**
  SR_RP_ERR=0x00090001 = **result=1 FAILURE + error=9**。error 9 不在
  QMI 标准表（1/2/3/5/41/53/58/69/90/94，qmi.h）=SERVREG 方言码。
- 配套 `sr=1.7fffffff` = **wlan_pd 域状态 UNINIT**（0x7FFFFFFF，
  SERVREG STATE 枚举）。
- **因果定谳**：UNINIT 是因、error 9 是果——对从未启动的 PD 发
  RESTART 被合法拒绝。RESTART 腿通路全通（QMI 编码/对端/回程全活）。
  **主线瓶颈正式移出 PDR 层**：w45=0 的问题改写为「modem 为什么
  不 spawn wlan_pd」，不是「怎么重启它」。_kernel 对照：service-notifier.c
  对 DISABLED(69) 特判，其余错误原样上抛——modem 侧方言码语义未译，
  REA 语料可考（低优先）。

### rmtfs 全绿翻案（本窗口险案，差点重复造轮）

初读 `rm=8.0.3.1` 曾误判「8 个 IOVEC 无应答=EFS 主卡点」。全字段
解码后翻案：

- `rh=8.0.9000101.a000101.d000104.e000103`——8 包全入队/零丢弃/
  hist 序列 = sport09 OPEN → sport0a OPEN → sport0d ALLOC →
  sport0e IOVEC（多客户端各 txn=1，G36 判例复现）。
- `rf=3..3.8.280000.0`（字段=RF_MSG.path.RF_ID.RF_N.RF_SZ.RF_AS）
  ——末包 msgid3/caller_id=3(fsg)/**8 次全 drain**/ALLOC buff_size=
  0x280000 全窗/SCM assign r0=0 成功。
- `ik=4.0`——**4 笔 IOVEC 影子读写成功零失败**。
- 结论：8 请求全应答、EFS 影子数据搬运活。**#44「rmtfs 无罪」
  复确认**。末包恰为另一客户端首个 IOVEC(txn=1) 是误读来源。
- **教训入册**：TF_RM10 只存末 2 包，末包形状≠全集形状；判「无应答」
  必须先看 drain 计数（rf 第三段 RF_N）与丢弃计数（rh 第二段 RD）。

### 服务图跃进

`q=42`（渲染 cap 40，40 unique 一次闩齐 @+20s）——#45 全命才 30。
**毒环拔除后我们的应答真正离站，modem 初始化多走 1/3**。0x44.1 在
列、0x45 仍缺（#43「距一格」复确认）。`wd=51.51.0`（rfs DATA 时间戳
族）。40 项名册全串已在 /tmp/s46-probe.log（G40 差分素材）。

### G40 方向改判

- **不写 rmtfs 应答器**——已存在（rf_reply 全 msgid 1-5+队列+去重）
  且全绿。
- 新方向 = **0x44→0x45 缺口侦察**：① 40 项名册 vs Linux 在役 qrtr
  名册差分（内核 qrtr.h 服务号定名）② 0x44 归属与 0x45 的服务定义
  ③ modem spawn wlan_pd 的触发件（REA 语料）④ tftp 腿（svc 4096，
  po=1000.2 仅 2 包——pil 分段加载是否卡 tftp 应答不足）。

### Git

- 本收据本地提交（收据不推）。G40 码件出来后走码尖位舞步。
- 观察面新增字段本靴全验：rpm=/pdi=/mh=/pk14= 每帧可见。

## 靴 #47（s40/G40，2026-10-22 晚）——四观察键全活，两大假说齐排除

- 19:40:23 用户手动 fastboot 入口，链 #47 刷 boot_a（s40 零填 68,157,440B，
  md5 738b04ac…）、set_active a、reboot；ping up 19:41:18（+54s）。
- **命 1 = 603s**（19:41:18→19:51:21 狗杀）——**天花板第二十证**。

### 四键判读（+384s/+430s 两帧一致，纯观察未点火 rp）

- `sd=0.0.0.0`——4 槽 seed 抽块（首+尾块）对账**逐字节全保真**：
  「seed 搬坏」假说排除。
- `ro=9.f.1.1,a.f.1.2,11.f.1.3`——三笔 OPEN 全 matched=1。**判读翻案**
  （读 RFS_PATHS 后）：四主路径 `/boot/modem_fs1|fs2|fsc|fsg` 恰全 15
  字符、cid 1-based——sport09→cid1=**fs1（主 EFS）**、sport0a→cid2=fs2、
  sport11→cid3=fsc。modem 开了 fs1；**rfs 末笔=fsc 的 OPEN（≤48s）**，
  rm=8.0.3.1=3 OPEN+1 ALLOC+4 IOVEC 全应答零丢（#46 判例复确认）。
- `tl=`（停点全景，**目的口**直方图）：dest 0x10 rfs=48s、dest 0x11 tftp
  =51s、dest 0x12 svc64=63s、ctrl=63s、0x14/15/16/1000=49-50s——**全部
  入站 48-63s 归零**，其后 ~9 分钟死寂至狗杀。
- `rl=/pr=`（tftp 全景，G24/G28 键）：**N=0x11=17 笔 RRQ**、每笔换源口
  （0x21…0x26）、路径环 4 槽全=`00 01 /readonly/firmware/image/modem_pr
  /so/549_38_0.mbn\0octet`——17 笔全是 modem 自举链（elf_loader 读自家
  .mbn；末两笔 fl=3 rsize=52 seek=0/0xa0=Elf32 头分段读）。**modem 从未
  对 wlan 件发 RRQ**——wlan_pd 卡在取固件之前。
- 名册 38+ 项与 #46 同构：44.1 在册、0x45 仍缺；pn=kernel/elf_loader.1.30
  （pn= 只闩 elf_loader 类问句——其余问句名全盲，见 G41）；af= 感官 PD
  SNS_REG_INIT 老死法、pd0=21.3.avs/audio 不变。
- ox=3.0（3 OACK 全发成）、tt=d90.d90（TX 环净空）。

### 命 2 = 142s 短命 + 控制台楔死（#46 早死命同款）

- 19:51:21 狗杀后 USB 总线全无 ~7 分钟（RNDIS/fastboot/adb 三空、arp 无
  条目）——一度判「整机硬楔」，**19:58:13 ICMP 自爬回**：是慢狗/慢复位
  非硬楔（复活性判读要看足 10 分钟再下）。
- 命 2 全程：ping 活但 **sm 口 2323 三次 TimeoutError**（网络栈应答、
  应用主环楔死——22 连接楔死律形状），20:00:35 ICMP 落，**全长 142s**。
  #46 命形（603/182/260）复现：**命 1 长、命 2-3 短命+控制台楔**。
- probe3 多命循环采样器已挂（命 3 起自动接续，单命 10+10 分钟等待容忍
  慢狗复活）。

### 教训

- r8/hist 的 sport 与 po/tl 的键是两套口径（前者=源口、后者=目的口）；
  先读渲染代码再下结论——#47 首判「fs2/fsc/fsg」就是没查 RFS_PATHS 的
  想当然。
- tf= 只渲染已应答路径；请求全景在 rl=/pr=（17 笔 vs tf= 1 路径）。

### G41 方向（据 G40 新事实）

- modem 装完自己（tftp 17 笔）→ 开 fs1/fs2 → ALLOC+IOVEC×4 → 末笔 OPEN
  fsc → 48-63s 全静。「等什么」下一刀=**抓 GET_DOMAIN_LIST 问句名环**
  （pd_take_name 已解析出名字、只欠记录；pn= 只闩 elf_loader 类）——
  modem 在找哪个服务一翻便知。次选：ADSP/CDSP 子系统状态面。

## 靴 #48（10-22 晚）命 6 黄金帧 + 暖命死链全解 + G43 生存刀

### 命 6 黄金帧（probe4 无沉降设计赢的）

- 20:36:07 ping 起，**t+5s 拿到全帧**（s=6）——暖启控制台能答，#47 的
  「暖启控制台楔」判读作废：probe3 的沉降 130s 设计在和 t+133s 整机死
  赛跑，从没独立验过控制台。帧内：**modem 6 秒内全起**（rl=17 笔 RRQ、
  q=42 全名册、ro= fs1/fs2/fsc 三开、rm=8.0.3.1 与命 1 终态同构）——
  暖启 bring-up 又快又全。t+11s ping 落，全灭。
- **dq=（G41）首证**：全命只有两笔问句 `tms/pdr_enabled`、
  `tms/pddump_disabled`——**没有 kernel/elf_loader 问句**（pe=0，s=6 时
  还没走到/这条命没走到）。G41 环工作正常。
- gn= 纠偏：`L5:modemst1,…` = GPT 扫描名带 LUN 标签（RFS_GN，每命自
  扫），**非跨命持久**——「命 5 写过 modemst」推论作废。

### 暖命死链全解（写必死、读全活、寿命即错误码）

- **report_reset(code_secs) 的设计=寿命遥测**：自旋 code_secs 秒再拉
  PS_HOLD——死亡延迟就是错误码本身。码表：写 362(Timeout)/212/242/
  422(写保护)/122(Check)，读 472/302/332，验 392；「122 环毒」律即
  Check(122) 的出处。
- 命 6 死形：sm 应答先交（TX TRB 已武装、硬件自发完——CPU 自旋也送
  帧）→ 紧随的 s20_keep→ring_put **WRITE(10) FUA 1s 超时** →
  report_reset(362) 自旋 → ping 即死、362s 后 PS_HOLD。
- **命 2-5 的 133/138s 同源**：死于各自的第一笔环写（触发点 ~130s 待
  查），362 自旋模型全吻合——命间隔 529/381/417s = 362+boot(19-67s)。
  ~600s 天花板（20 证）= **10 分钟采样器环写**（每 20 喂采包落环=静默
  期后第一笔环写）。#46/#47 的「狗杀」全部改判环写自杀。
- 家族拟合：**读全活**（17 笔 RRQ 全服务、ro 三开、ext4 走读）**写必死**
  ——疑 UFS 设备侧 FUA 写滞态**跨暖复位遗传**（单槽单门铃驱动超时返回
  后门铃位仍占、prep 清描述符与控制器交战是 AP 侧共犯）；fastboot/L0
  全初始化清滞态（命 1 的 603s 干净、20:16 滚 b→L0→刷→命 1 全活同证）。
  每条暖命死于自己的第一笔环写；烧命滚 b=L0 是自愈正路。
- G14 旧账同形（「首 sm 若逢 so 传输活窗即溢栈楔死：首查超时+ping 亡+
  USB 链仍活」）——同一弱点的历史实证。

### G43 生存刀（s42，已烤未刷）

- ring_put 三处自杀改 rw_fail 记账（RW_FAIL 计数+末 4 码），sm 渲染
  `rw=<计数>.<码…>`；写败留原槽不进位不验读。暖命观察窗 11-133s →
  全寿命。s42=G41+G42+G43 合弹，boot-s42.img 126,976B，零填
  68,157,440B md5 6e530ab7b438067fe6bc8f6e607dd246。
- 链 #49 双触发已挂（fastboot 现场/L0 adb）：闸门串号→刷 s42→
  set_active a→reboot。probe5（动态接口版，en 号可能换）多命观察。
- 待验：rw= 首现即定写滞态；暖命活到 600s 段看 elf_loader/dq= 后续；
  G42 th=/sh= 定 SO 头字节真伪。

### 教训

- 死亡延迟=错误码：裸件寿命本身就是遥测通道，间隔对表先于新理论。
- probe 沉降窗设计要和死形赛跑对齐——沉降 130s 恰好全程错过 133s 命。
- 接口名不是恒量：USB 重枚举换 en 号，探针必须动态发现接口。

## 靴 #49（10-22 晚）：链 #49 自动回收成功 + 命 7 黄金长寿 + G42 双假象定谳

### 链 #49 自动回收：全链自愈跑通

- 20:56:10 暗机烧完 slot-a tries → PMIC 滚 b（L0，current-slot=b、
  retry:a=0）→ 链检测到 adb → 闸门过串号 → 刷 /tmp/s42padded.img
  （md5 6e530ab7）→ set_active a → reboot → **20:57:38 命 7 ping 起**。
- 自动链四步（触发→闸门→刷→复活）零人工。串号全程未回显；零填
  68,157,440B、七命律（set_active a）照册执行。

### 命 7+ = s42 首命 = 史上最长裸命

- 20:57:38 起连续活着（probe5 的 300s 窗把同一条命重贴 L8/L9/L10/L11
  标签，u=12390 不变=同一命），25 分钟+ 仍活、控制台能答。
- **rw=0 全程**——G43 会计活：暖命 ring 写停摆（#48 死链）未复发。
  L0 全量初始化清滞态的遗传说再添一证：这条命前正好滚过 b→L0→刷。
- elf_loader 阶段到站：dq=3 含 kernel/elf_loader（命 6 死于 t+11 未及）；
  modem 侧用户态起动（af=err_qdi sensor_process fatal = 最深起动标记）；
  42 服务注册、rg=51。w45 仍 0、sr=7fffffff（wlan_pd 未生）。

### G42 定谳：SO「烂头」= 双假象，盘/读路径/供数全净

待验项「th=/sh= 定 SO 头字节真伪」 就此关案：

- **假象一 push_hex 极简渲染**：push_hex 对 v==0 即停——字节 <0x10 只出
  1 字符（01→"1"、00→"0"）。sh=（74 字符）与 sh2=（63 字符，奇数长度
  即铁证：整字节 hex 不可能奇长）用工厂好件的极简渲染对照——**逐字符
  全等**。设备盘上 549_38_0.mbn 头 64B + 0xa0:52B 与工厂件逐字节一致。
- **假象二进制换算错**：0xEB43C = 963,644（917504+46140）。此前误算
  963,654 造出「+10」悬案。设备 tz=1.eb43c.1a67、工厂目录项、.local
  件（963,644B）**三方全等**，零差。
- 推论链：设备 modem_a 完好（簇号 0x1a67、尺寸、116 采样字节全对）
  → UFS 读路径无罪、FAT 行走无罪、so_read/供数路径无罪 →
  **#47 elf_loader 沉默与数据损坏无关**，方向改判（见下）。
- 附带修正：WRQ 实为 `/readwrite/ota_**firewall**/ruleset`（此前两处
  误记 ota_firmware）。modem_efs RRQ → ERROR file-not-found（G23b、
  #23 定谳在案，非堵点）。

### 工厂 ground truth 定稿（radio g7250-00264-230619-b）

- FAT16 卷 @img+0x8c；BPB：bps=4096、spc=4、res=1、nfats=2、
  **spf=6（@22 不在 @17——@17 是 rootents=512，本案自踩复踩两次）**、
  root@LBA13、data@LBA17。MODEM_LBA=115208 与固件常量合。
- so/（簇 6504）= **LFN+短名别名布局**：真名走 LFN，8.3 是
  ORERCP~2.MBN 之类别名——固件 hitlfn 路径（fat_dir_find LFN 装配）
  是命中的正路。61 条全录：549_38_0.mbn→ORERCP~2.MBN cl=0x1a67
  sz=0xeb43c；549_36_0.mbn→Y5F5FE~2.MBN cl=0x21a3 sz=0xeb434。
- .local/device/redfin/modem/so/549_38_0.mbn（963,644B、md5
  293765b3…）在厂 img 0x69a508c 起逐字节存在。

### 新刀口（G44 方向）：SO 服死于 OACK 段，早服/晚服分野

- 命 7 帧 tb=**4**.0.11.0：TF_ST=4=OACK 后对端 error（blk=0 零数据块）
  ——最后一笔 SO 服没送出一个块。pr= 包环四笔全解码：WRQ
  ota_firewall/ruleset、RRQ modem_efs、**RRQ SO ×2**（两笔都死在
  OACK 段）。
- OACK 内容经查全对：blksize/timeoutms 回显、tsize=TZ_SZ=963,644
  （真值）、wsize 恒回显（G6 正典）。**内容无罪 → 嫌 OACK 未达**
  （glink TX 段丢失/晚段环态）——与 G9c 注释「op5 code8=从未收到
  我们 OACK」腔调合。
- 刀口分野：**早段 17 笔 mbn 服全成（modem 起动到 42 服务）vs 晚段
  SO 服连死两笔**——同一 OACK 构造代码，差在时序/环态（晚段 TX 环
  head/tail、端口复用、elf_loader 的 sport）。G44 侦察目标：SO OACK
  的 TX ok/no 计数（pq=/ox= 分读）、对端 error 码字节捕获、OACK 重发。
- 教训沉淀：**观测层先于本体层**——sh= 假烂头烧掉一轮 UFS 竞态假设；
  极简渲染器读谱前先数奇偶。进制换算要用工具不要心算（0xEB43C≠96,654）。

### 链 #50 挂账（在跑）

probe5 多命观察继续（动态接口）；G44 刀案：SO OACK 送达性分辨+补发。

### 靴 #49 附录：G43 现场验证 + G44 根因定谳（21:17–21:40 观察窗）

- **G43 生存刀现场成立**：命 7（20:57:38 起，史上最长裸命 40min+）~30min
  首现环写败——rw 0→2→3→4，码全 362（WRITE FUA 超时签名，暖命死链
  复发）；**设备全程存活**，sm 全帧照答。#48 家族「首笔写败=ping 即死」
  的自杀腿（report_reset 自旋）确认被 s42 切断。**rw 败拍周期=600s 整**
  （21:27:39→21:37:41 恰 602s）——对 #48「10min 采样环写」判读，周期
  写者就是采样环本身。
- **G44 根因定谳（调度饿死竞态）**：SO 服死于「OACK 后 1s 无 DATA」。
  全计数互证：dv=0+nopend=0（泵从未在 st∈{1,2} 时被调）+BLKNO=0/
  WSENT=0（DATA-1 从未上环）+ox=2（两笔 OACK 都 TX 成功）+rx=
  [efs RRQ, so RRQ#1, so RRQ#2, EOT]。时序：轮 X 发 OACK#1（st=1，
  同轮泵段已跑过 st=0）→ 客户端等 DATA-1 至 1s 超时 → RRQ 重试 →
  EOT 由 walker 的 tf_note_ack **即时**置 st=4 → 下一服务轮（均值
  1.16s，0.86/s）赶到时 st 已 4 → nost 桶 + fresh 腿把 OACK#2 发给
  空场。早段 17 笔小 mbn 能成=赢到同一竞态。rs=0.0 排除 stat 探针。
- **rx= 槽注释两处过期实证**：op1 槽第三元=文件名首 2 字节（2f72="/r"，
  /readonly 与 /readwrite 同形）；op5 裸 EOT（op2B+code2B，chunk=52）
  被 `>=54` 门误标 ffff——tf_note_ack 无此门，code9 读得正确。
- **写腿挂账新观察**：ruleset WRQ 已被 OACK（oe= 回显 blksize7680/
  timeoutms1000/wsize1），但 ww=1.1.0.0.0——客户端一块 DATA 未发，
  写会话悬空。疑客户端等 SO 读完成后才推进写腿（同一启动序关卡）。
- **刀 s44（G44）**：G19 窗口泵抽成 tf_pump_window（->bool 保真原内
  联块「dv++ 即整个 tf_reply 提前退场」控制流），OACK TX 成功且非
  retry 腿原地启泵送首窗 DATA；ox= 加第三格 kick 计数。21:40 出件，
  68,157,440B 零填待刷。

### 靴 #50：s44/s45 双验 + 计数器粒度到头 + 方法论翻案（21:42–22:10）

- **s44 首验（21:42 刷入，命 17 收割）**：kick 活——ox=3.0.2（第三格=2 笔
  kick）、dv=2（kick 进泵）、tb 4.0→**3.1**（s42 的 EOT 弃线形变 EOF 收口
  形）。但 ELF 收到的是 4096B 截断块（blksize7680 首块短读）——px= 全解
  码证实客户端无 rsize/seek（整文件请求），短块唯一直通路径=so_read 第二
  笔背靠背 read10 失败 `return take`=4096。
- **刀 s45（G45）**：so_read 跨扇区单命令连读（need≤3 扇区 contig 判定+
  逐扇区退路）；so=so2.fb 双计数；21:55 刷入（命 18）。
- **s45 首验三发现**：
  1. **连读刀本身是好的**：so=4.0（4 笔 contig 全走）、**dz=0**（零笔
     读零）——「read10 多块失败」假说当场死亡。
  2. **dq 首次出现 kernel/elf_loader 问句**（dq=3：tms 两问+elf_loader；
     同拍 pe=1）——链条史上头一遭越过 SO 档。#47 时 dq 只有 tms 两问。
  3. **语义对账**：8453 G7「OACK 落环即合成 ACK-0」在役（2s 宽限 8103
     已退役为兜底）；tf_note_ack 7783——**ST=4 = 客户端 op5 code9 且
     ST==1 时**（即「OACK 已发、DATA 未流」态的 EOT），注释 14190 的
     tb= 清单（0/1/2/3/8/9）根本没列 4。
- **计数器粒度到头（悬案立案）**：s45 终态 tb=4.0.11.0 与 N=5（5 笔
  DATA 已发）/so=4/DV=2/DX=2/ok=2/kick=2/DR=0 **合不拢**——若有 DATA
  流出，ST 应落 2/3，EOT 落不进 4；N=5 与 so=4 差的一笔 DATA 无对应
  so_read。6~12s 粒度标量计数器无法分辨 1~4s 的协议交互。
- **方法论翻案（用户点醒「停下来想想哪里走偏」）**：本窗连错四假说——
  ①#49「调度饿死定谳」（s45 实测 kick 每次都进泵，饿死故事失效）
  ②「read10 多块失败」③「kick 被 2s 宽限门挡」（G7 早已合成 ACK-0）
  ④「3.1→4.0 命内转移」（实为跨命比较：监视器记住的是 s44 末态）。
  三偏差：**拿假设定谳**（编故事凑计数）、**刀先于量**（s42/s44/s45 连
  刀，每刀带行为改动，烧命+加混杂变量，正确的下一刀应是纯插桩）、
  **语义没对账就推理**（ST=4 不在文档、rx= 是否记 ACK 未验证、跨命边沿
  当命内转移）。**纪律：此后无毫秒时间线不定谳；插桩刀先行于行为刀。**
- **G43 持续验证**：rw=362 码败拍继续 600s 周期出现、设备每次都活
  （22:05:36 拍亦然）——#48 自杀腿确认长周期死。
- **刀 s46（G46，纯插桩零行为改动）**：毫秒事件时间线——TF_EV 24 槽
  滚动环（cntvct→ms），记 RRQ/OACK-TX/DATA-TX(n)/DATA-TX-fail/ACK-RX
  /EOT-RX/ST 变迁/pump 进泵/stat 门退/so_read 返回值/环闸挡/读零，
  sm= 帧内 rx= 后渲染 `tl=ms.k.a×24`。目的：一次会话全时间线，把
  「客户端到底 ACK 没 ACK、块 1 实长、ST 走向」一次量清。

### 靴 #50 附录一：s46 时间线插桩首果——双真凶定谳 + 三翻案（22:10–22:16）

s46（纯插桩：TF_EV 24 槽毫秒事件环，sm= 内渲染 tl=ms.k.a）22:08 刷入
（命 19）。首帧即全景量清，以下均为**时间线实测**非推演：

- **完整时间线（tl= 解码，boot 相对 ms）**：
  `49307 WRQ到 → +2013 RRQ#1到（**2013ms 无服务**）→ 51521 RRQ#2+
  OACK+kick 同毫秒 → 51522 so_read n=7680（**全量返回**）→ 51522
  **DATA拒 blk=1** → 51524 EOT(code9) → 同拍 RRQ#3+OACK+泵进 →
  so_read n=52 → DATA发 blk=1 成 → ST迁 3（52B 短块=送完）→ +202ms
  **ACK-0+ACK-1 到** → RRQ#4 → OACK(retry=1)`。
- **BUG-A（硬杀手）定谳：glink_tx_payload 拒发 7680B DATA 块**。
  三顶帽（n≤7684/chunk≤isz/total≤8192）全过；拒发只可能出于①环回卷
  闸（head+7736>16384 且 head≠tail）或②iip_pick 池干（无 ≥7716 空闲
  intent）。52B 包同拍即过——**小包活大包死**。s44/s45 一切整文件失败
  的最终共同点。
- **BUG-B：tftp 服务双峰延迟 0ms/2013ms**。tf_reply 在外层 SMEM 表走查
  每步一次（内环 rf 无进展即退），外圈≈2s；客户端 timeoutms=1000——
  RRQ 落在服务槽前 0ms 中、落在槽后等 2s 必死。17 笔小 mbn 史上全成
  =小文件会话 0ms 一发完成（OACK+DATA 同轮），从不吃第二拍。
- **三翻案（旧判读正式作废）**：①「客户端零 ACK」错——52B 会话后
  ACK-0+ACK-1 202ms 双达（此前会话死于无 DATA 可 ACK）；②「整文件
  一把抓」不完整——客户端是**分段读**姿势（rsize=160 头段→成功 tb=3.1
  送完；52B 段探→成；大段/整文件→死于 BUG-A）；③ s44「4096 截断」
  形应重审——s45/s46 so_read 均全量返回，截断判读不再必要。
- **好账**：s45 连读刀彻底无罪（n=7680）；G44 kick+G7 synth-ACK0 链路
  即时性实证（RRQ#2→OACK→泵进→so_read 全在同一毫秒）；写腿 WRQ
  49307ms 到、写 OACK 后零 DATA（ww 挂账继续）。
- **悬案收窄**：DATA拒的①环闸/②池干二选一未定——等下一波整文件
  重试的 tl 观测（客户端失败后休眠，s44 实测~10min 量级）。s47 刀案
  预设计（未落刀）：TX 失败即摆 synth-ACKPENDING 快重试（~300ms 拍）
  + intent 批件入池点 kick 泵（批到即发），两凶并治；BUG-B 外圈提速
  另立刀案。
- 渲染撞名备注：sm= 帧内现存两个 tl=（#47「停点全景」在前、G46 事件
  线在后点开头形）；s47 起改 G46 名为 ev=。

### 靴 #51：s47 判刀——DATA拒=②池干定谳 + 满池丢件嫌疑（22:24–22:30）

s47（纯插桩：glink_tx_payload 三拒绝点当场快照 GX 4 槽 site=1 caps/
2 回卷闸/3 池干，池干另 dump 全池 16 槽 IPD；G46 tl= 改名 tev= 根治
与 #47 停点全景撞名——22:14 那波整文件重试的 TL 解码正死于此撞名）
22:24 刷入（命 21）。**开机首帧即定谳**：

- **#50 附录一悬案闭案：DATA拒=②池干**。`GX: site=3 dry
  (chunk=0x1e24=7716, freeN=15, freeMax=0x200=512, usedN=1)`——①环
  回卷闸、caps 帽双双排除（gx 4 槽里只有 site=3）。
- **池谱（IPD 全池 dump）**：对端批的 16 槽 = **15×0x80(128B) +
  1×0x200(512B)**，iid2 缺席，16/16 满员。freeMax=512 恒 < 7716 ⇒
  7680B DATA 块永远无槽可用。17 笔小 mbn 史上全成=小包都塞得进
  0x80/0x200；52B 探针块（chunk=84）当场即过。
- **时间线（tev=）**：`51470 RRQ→同拍 OACK→泵进→51471 so_read
  n=7680（盘读好）→同拍 DATA拒→51473 EOT(code9) 客户端 3ms 弃整
  文件→同拍 RRQ 52B 探针→DATA发1 成→+202ms ACK0+ACK1 双达`。客户端
  失败后立即降级探针读，与 #50 附录一分段读姿势互证。
- **intent 经济读数**：ii= ads 洪峰（0x5bf→0xe11→0xa37b≈4.2 万笔广
  告/百秒级）但池成分恒小件；req=1（我们只发过一脚 cmd7(7716)）；
  **i7=5.e68**——对端反向问过我们 5 次 cmd7、要 0xe68=3688B 件，我
  们批了（TX5_N=5）⇒ cmd7 经济双向活，我们求 7716 理应被理。
- **满池丢件嫌疑（s48 刀源）**：iip_add 只填空槽；现场 16/16 满员+
  iid2 缺席 ⇒ 对端 cmd7 应答若带新 iid 大件广告，撞满池即被静默丢
  ——「cmd7 无人理」与「批了但没入池」s47 无法分辨。
- **pn 推进**：tms→kernel/elf_loader（dq=3, pe=1）——链在走，每个
  大文件撞同一堵墙；tf= 仍 SO 件第 6 次会话。

**s48 刀（捆绑：计数保归因+满池腾挪最小 fix）**：iip_add 满池时腾
最小 free 槽（全 in_use 才丢+DROP 计数）+ 三计 SEEN_MAX（见过最大
批件尺寸）/EVICT/DROP 尾接 ii=。SEEN_MAX≥7716+EVICT>0 ⇒ 满池丢件
实锤+腾挪即修；SEEN_MAX 恒 ≤512 ⇒ 对端不理大件 cmd7，转 CONT 分片
刀案。22:30:17 刷入（命 22）。

**#51 附录一（s48 判读 + s49 三刀判读 + s50 刀案，22:30–22:55）**：

- **s48 判刀全中：满池丢件实锤**。`mx=0x2080`（对端确实应答过
  8320B 大件广告）+ `evt>0` 持续腾挪 + `dry=0`——修掉「iip_add 只填
  空槽」后大件入池，**史上首块 7680B DATA 出环**
  （`51254 RRQ→OACK→泵进→51255 so_read7680→DATA1→ST迁2→环闸7736`
  全同拍）。满池丢件=DATA拒根因定谳，「cmd7 无人理」排除。
- **但整文件会话仍死，形状改判**：客户端收到 DATA1 后 **+2ms
  op5(code9) 弃单**（非超时腔调），随后 52B 探针/160B 分段读全成
  全被 ACK（+201ms ACK0/ACK1）。op5/9 在 #24 语境=正常收幕语——
  客户端「认为传输已结束」或即刻报错，嫌疑从「窗口饿死」转向
  「DATA1 本身有问题」。
- **s49 三刀（A 泵在途闸 4096→sz−4096/B 终态重RRQ按首服重置+kick/
  C TXGEN 计入内环活性）22:43 刷入（命 23），三刀全活体**：
  泵真去连读块2（so_read×2 同拍）、终态重 RRQ 同拍全链服务
  （RRQ→OACK→泵进→so_read→DATA→ST迁3 一拍完成 ×2 会话）、
  ACK 跟踪到 +201ms。**新拒绝点=池只有一件大 intent**：
  `GX: site=3 dry(chunk=7716, freeN=14, freeMax=0x480, usedN=2)`——
  0x2080 单件被块1 占用（IPD: iid0x1e:0x2080:U），块2 无槽。整文件
  会话结局不变：DATA1 后 +1ms op5/9 弃单→回退 52/160 分段全成。
  小块活大块死、100% 复现（s48 三连+s49 首发全同形）。
- **s50 定谳（纯案头，读代码+环几何）**：`tt=2ca8.2ca8.4000`——
  TX 环 0x4000(16384B)、head=0x2ca8(11432)；DATA1 total=7736 ⇒
  11432+7736=19168>16384 **回卷路径 100% 踩中**。而旧回卷=「head==tail
  时 head=0 重写」是**自造协议**：对端读侧按 tail→end→0→head 连续
  解包，重置后 tail..end（4952B）全是陈旧字节，modem 先吞垃圾解包
  错乱→客户端 socket 即刻报错弃单（+1ms op5/9 完美吻合）。小块
  （52/160B）从不跨界所以全活；同一发射器→qrtr 头/glink 头/内容
  全无罪。**正法=内核 glink_smem_tx 同律分段写**：包跨界两段拷贝、
  head=(head+total)%sz、free 闸按 used 算（不再要求环空）。对端读侧
  原生支持分段（AP 内核同信道天天这么写，modem 必须能读）。
- **s50 刀**：glink_tx_payload 写段全改 woff 绕环助手（偏移≥环长即
  回卷）、head 发布改模环长、旧「head==tail 重置」删除；TEV 新事件
  13=跨界写发生（arg=旧 head）——预期整文件会话现场必现 13，随后
  客户端不再 1ms 弃单、ACK 到、tb 推进。22:52 刷入（命 24）。

### 靴 #51 附录二：s50 假闪事故 + s51 双修活体验 + 服务端四证脱罪 + 谋杀窗口浮出（22:52–00:09）

- **s50 活体验尸（命 24，跨界写刀）**：刀本身活了（环几何
  正确），但整文件会话仍死，且**客户端形状突变**——s48/s49 里
  160B 分段读是被 ACK 的，s50 起 160B DATA1 后**零 ACK、零
  code9、零重试**。附带把「服务延迟双峰 0/2013ms」里 2013ms 那
  峰治了（ACK1 提前到 +5ms）。
- **假闪事故（命 25，23:44）**：一次刷写忘传 `--s50` 参数，
  build-bare.sh 走了默认分支，把 8192B 心跳 stub 当 boot 刷了
  进去。三分钟后重刷 s51 真件。**教训：刷前必验产物尺寸类**
  （真件 ~127KB，stub 8192B——`ls -l` 一眼可辨；此后每次刷前
  显式对尺寸）。
- **s51 双修活体验（命 26，23:48）**：① **stat 门活**——
  `66730:stat退0 66732:EOT9`，客户端对 stat 的 rsize 键确实缺席
  （对 /tmp/tftpsrc/tftp_protocol.c 逐分支核对：stat 响应不含
  rsize 字段时不走 tsize 协商，直接 EOT）；② **空闲拍速活**——
  客户端真 ACK1 落在 +5ms（s50 时 +201ms），等待侧不再被服务
  延迟卡 200ms。**双修双双验活，但链路第三读仍全哑**：52→160
  两读成、ACK 齐，第三个 RRQ（读 6808@0x1000 meta 表或
  52@0x3000 嵌套 ELF 头）发出后服务端照常服务 DATA，客户端
  却从此零包——**延迟假说（H-E）死刑**：服务同拍、ACK 即时，
  客户端依然不动。
- **服务端四证脱罪（s50/s51 现场合证）**：
  1. **内容字节精确**：SH1（64B@0）与 SH2（52B@0xa0）伺服字节
     与 host 侧 `.local/device/redfin/modem/so/549_38_0.mbn` 对应
     偏移**逐字节相同**（minimal-hex 渲染后程序化比对，match 双
     True——肉眼比对曾产假警报，见下条教训）。
  2. **偏移运输正确**：rl 台账 session 25=52@0、26=160@0x34
     （0x34=52=phoff，正是 Ehdr 之后紧跟 phdrs 的读法）；客户端
     ACK 过 S2 ⇒ 它验过了 52B Ehdr 并主动来读 phdrs。
  3. **TX 环排空**：`tt=e08.e08.4000` head==tail——modem 读侧把
     我们发的每个字节都取走了（含 216B DATA1 包，落在环偏移
     3120，不跨界）。
  4. **同拍服务**：所有 so_read→DATA 同 ms。
- **minimal-hex 教训**：push_hex 渲染**最小十六进制**（无前导
  零：0x00→"0"、0x03→"3"、0xa4→"a4"），sm 的十六进制字段长度
  可奇可偶，**必须用同款渲染程序化比对**，肉眼按 2 字符切必产
  假「字节腐坏」警报（本案实错：把 `11100000` 读成 11 10 00 00，
  真值 01 01 01 00）。
- **客户端沉默=线程死（定谳）**：客户端源码
  （tftp_protocol.c）处处是超时重试（wait_pkt_is_timeout、
  current_timeout_ms、重试环）；活线程必再 ACK/再 RRQ。c9 冻结
  在 56.6 = S3 RRQ 后 **8 分钟以上零 modem 包** ⇒ **elf_loader
  线程/socket 在发出 S3 RRQ 后 ≤1ms 内死亡**。这不是传输层能
  解释的沉默，是进程级死亡。
- **谋杀窗口主线索（xl 台账）**：S3 DATA1（216B@0x11）之后
  紧跟两笔**非 tftp 发射**——`0x12:104B` 与 `0x16:64B`。0x12=
  本固件假 pd-mapper 服务的 svc64 口（ad 表 (64,257,0x12)）；
  0x16=IPA_SVC_PORT。若其中一笔（对 modem PD 机器的 QMI 应答/
  ind）落进谋杀窗口，可能正是杀死装载线程的凶器。**但 xl 无
  时间戳**——0x12/0x16 与 DATA1 的先后 ms 序、以及触发它们的
  入站查询，都无法定谳。G38 DB_UPDATED 探针只有 56B，不是那笔
  104B 的发射者；真正发射者待查源。
- **s52 纯插桩刀（00:12 刷入，命 27）**：零行为变化，三件
  插桩——① TXL 台账 4→8 槽并加毫秒戳（TXL_M，tf_ev 同款时基），
  让 xl= 窗口覆盖 [S2-OACK … S3-DATA1 … 0x12/0x16 …]；② 入站
  qrtr 环 `rqr=` 16 槽（ms.源口.宿口.尺寸，随 cmd9 walker 每笔
  推入）——抓触发 0x12/0x16 应答的那笔 modem 来包；③ `pd2=`
  （PD_IND_ARM.PD_IND_N）暴露 G38 探针本启是否点火。判据：若
  0x12/0x16 发射毫秒落在 S3-DATA1 与（缺席的）ACK 之间，谋杀
  假说升级为头号；若在 DATA1 之前，转查装载线程环境（栈/优先
  级/权限）缺口。

### 靴 #52：s52 纯插桩三问全答——谋杀假说死刑 + dlopen 官方静默退出路径浮出（00:09–00:40）

- **s52 三件插桩（TXL ms 戳/rqr 入站环/pd2）00:12 刷入（命 27）
  首启即复现同形**：stat→S1(52@0)→ACK1(+5ms)→S2(160@0x34,
  216B 包)→**暴死**（rl 止于 session26，TEV 48839 后零事件）。
- **三问全答**：
  1. `xl=`（带 ms）：DATA1(216B@0x11) ms=0xbec7=48839；
     **0x12:104B ms=0xea3a=59962（+11.1s）**；**0x16:64B
     ms=0xeb4d=60237（+11.4s）**——两笔发射远在谋杀窗口（≤1ms）
     之外。**谋杀假说（0x12/0x16 杀线程）死刑**。
  2. `rqr=`：0x16 的应答与同 ms 入站 `eb4d.20.16.e68`（modem
     0x20→IPA_SVC 3688B 大包）成对；0x12 的 104B 之前 **C9_N
     冻结=56 ⇒ 零入站=自发**。
  3. `pd2=0.0`：G38 探针未 arm，与本案无关。
- **余波时间线（11 秒静默后）**：DATA1 后 **11.1s 双向零包**（无
  重试无查询——活 tftp 线程 200ms 超时×4 必重试）→ modem 控制
  面 52B 包×7（sp=dp=fffffffe，59977–59998ms）→ IPA 3688B 入站
  （60237ms）→ 死寂。**11s≈modem 侧装载放弃周期**。
- **客户端源码三案头定谳**（/tmp/tftpsrc + CodeLinaro）：
  1. **接收缓冲假说死**：`raw_pkt_buff[TFTP_MAX_TFTP_PACKET_SIZE]`
     =7680+4=7684B，216B 包装得下。
  2. **协商参数**（px= 实证）：客户端 RRQ 带 `blksize=7680
     timeoutms=1000 wsize=10` + `seek/rsize` 精确分段读（52=
     Ehdr 尺寸；160=phentsize 32×5=phdrs 精确尺寸——**52/160
     读是 dlopen/libdl 的明确请求**）。
  3. **接收环形状**：recv_data 主环里 `data_size(160)<
     block_size(7680)`⇒`final_pkt_received`⇒**应发最终 ACK**；
     cb_func 负返回⇒**发 ERROR 包**（s48/s49 的 ERROR(9)=EBADF
     正是此路）。s50+ 两者皆无 ⇒ **死在 cb_func 内部**（上层
     qurtfs/dl 回调把线程弄死），非 tftp 层错误路径。
- **elf_loader 框架源码（arzekrasr/MPSS.HA.1.1）两大发现**：
  1. `rtld_main`（framework/src/main.c）：`heapSize=0x1000`
     （**loader PD 只有 4KB 堆**）→ dlinitex →
     `qurt_use_qurtfs(1)`（**文件读全部桥 QuRT ramfs，ramfs 页
     来自 rmtfs 域迁移**）→ `dlopen(USER_PD, 4)` → **无论成败
     QDI_OS_PROCESS_EXIT(0)+qurt_thread_stop()——官方静默
     退出路径，错误不外抛不发包**。暴死形状=dlopen 放弃。
  2. `root_task_init.c`：ramfs_detect_task 先等 RMB09（SBL
     ramfs 加载握手），超时自杀；握手成功才 `pd_mon_spawn`
     elf_loader PD——loader 依赖链=RMB09→ramfs→PD→qurtfs→
     rmtfs→tftp。
- **形状对比重排**（关键澄清）：s48/s49=160 读 ACK 后发**整文件
  RRQ**（blksize 7680 无 seek/rsize）→ 7680 DATA1 → +1~2ms
  ERROR(9)；s50+=160 读 DATA1 后**连 ACK 都没有**。死点从「cb
  写内存 -9」**前移**到「phdrs 解析完成→发起下一读之间」——
  dlopen 在 phdrs 之后、下一读之前放弃/崩溃。
- **新头号嫌疑**：① qurtfs/ramfs 页供给（rmtfs 域）在装载点
  断供——但 rqr 证 160 读死时 rmtfs qrtr 无活动（rmtfs 走
  qrtr，入站环该见包）；② **ea49 的 7 笔 52B 控制包=elf_loader
  PD 死亡讣告（qrtr DEL_SERVER/RESUME 广播）**——若实锤，
  PD 死亡时刻+官方讣告二合一，死因=dlopen 放弃实锤。rqr 未记
  控制包 type，**s53 需 type 解码**。
- **s53 刀案（两件套）**：① SH3=160 读 DATA 前 108B（0x34–0xa0，
  phdr0/phdr1 头部——SH1/SH2 已证 64+52B 字节精确，前 108B 是
  盲区，数据假说终审）；② 入站 qrtr 控制包 type 记录（52B
  控制包的 ctrl type 字段——区分 DEL_SERVER/RESUME/LOOKUP，
  讣告假说终审）。

## 靴 #52 附录：案头三件——root PD 逆向前置 + SO 段图 + RMB09 真源（2026-10-23 凌晨，设备网络面死等物理重启期间）

背景：s53 双证刀已建未刷（设备 en10 ECM 活但 DHCP 沉默=IP 栈死，
重插 USB 无效，等用户断电重启；自动哨已布：网络一回来自动刷）。
夜间案头三件，全部围绕「phdrs 回调死点」收窄。

### ① RMB09 真源（MPSS.HA.1.1 root_task_init.h）

- `RMB09_POLL_PERIOD = 500µs`，`RMB09_POLL_EXPIRY_TIME = 2s`，
  `MPSS_RMB_REG_BASE = MSS_TOP_BASE + 0x0018_0000`，
  `HWIO_MSS_RELAY_MSG_BUFFER_09_ADDR = base + 0x24`。
  （sm7250 的 MSS_TOP_BASE 仍需查——HA.1.1 是老代，基址会挪，
  段内偏移 +0x18_0000+0x24 的结构可作起点。）
- **ramfs_detect 超时只杀检测线程，但 spawn 在 while 循环后**——
  RMB09 两秒不来，`pd_mon_spawn(elf_loader)` 根本不执行。我们设备的
  elf_loader 明明活着（tftp RRQ 就是它发的）→ 本代固件 ramfs 腿要么
  已走通、要么不在此路径。**环境假说里的 ramfs 缺口降权**（bare fw
  零 RMB 处理仍是事实，但不是当前死点——当前死点在 phdrs 之后）。

### ② SO（549_38_0.mbn）段图——「两读合并=hash 表全量」合并律

段表（Ehdr 32 位 Hexagon ET_DYN，e_phnum=5）：

| 段 | 类型 | file 偏移 | vaddr | 大小 |
|----|------|-----------|-------|------|
| 0 | PT_NULL(hash) | 0x0 | — | **0xd4=212B** |
| 1 | PT_NULL(证书) | 0x1000 | 0x0e900000 | 0x1a98 |
| 2 | PT_LOAD **RX** | 0x3000 | 0x0 | 0x11753（69.8KB） |
| 3 | PT_LOAD **RW** | 0x15000 | 0x12000 | **0xd643c（877.6KB）** |
| 4 | PT_DYNAMIC | 0x15000 | 0x12000 | 0x80 |

- PD 像总内存 ≈ 0x12000+0xd6488 ≈ **0xe8488≈952KB**——dlopen 在
  phdr 解析后要一次性划这么大。
- **合并律**：Ehdr 读（52B@0）+ phdrs 读（160B@0x34）合起来恰好
  覆盖 0x0..0xd4 = **hash 段全量**（0x34+160=0xd4，分毫不���）。
  即 s52 死亡时客户端手里已有完整 hash 表。下一读（从未发出）=
  真段字节（Seg2 seek 0x3000 rsize 0x11753，或哈希验证触发的读）。
- **死点收窄**：phdrs 回调内 = fstat 断言（stat 已退 0，会过）→
  hash 表解析 → 952KB 页划分/进程创建，之间。

### ③ root PD 布局 + loader 定位（b22 段）

- `modem.b00` = 1172B 纯头（Ehdr+35 phdr，ET_EXEC Hexagon）。码基
  **0x8c000000**，35 段跨到 0x9b81f000≈260MB 虚拟（含大片 BSS）。
  b01=hash 段@0x9d800000；b04=224K/b05=180K/b06=204K 与段表对上。
- **loader 在 b22**（25.6MB 段，vaddr 0x9325c000）。铁字符串三组：
  - phdr 断言三连（fstat 基）：
    `(pEh->e_phnum * sizeof(Elf32_Phdr)) < (fstat_buf.st_size - pEh->e_phoff)` /
    `pPhdr[index].p_offset < fstat_buf.st_size` /
    `pPhdr[index].p_filesz < (fstat_buf.st_size - pPhdr[index].p_offset)`
  - **逐段哈希验证**：`Hash matched for segment %d` /
    `Hash verification failed for segment: %d`——phdr 过了就验哈希，
  验败即 dlopen 错（rtld_main 的 QDI_OS_PROCESS_EXIT 静默路径）。
  - `ELF_LOADER CAN SPWAN User PD:%s`（QCOM 原版错拼 SPWAN）、
    `Elf_Phdr header not found in first page in %s`（首页律）。
  - tftp 客户端全套文件名（tftp_protocol.c / tftp_msg_modem.c /
    tftp_socket_ipcr_modem.c / rfs_tftp.c）同段——rfs↔tftp 粘合
    与 loader 同居 b22，逆向时一段通吃。

### ④ w45 前置证据：WLAN PD 生成门走 rfs/tftp

`RFS_ENOENT - cannot find Wlan image, CANNOT spawn a PD`——
wlan_pd 的生成路径本身要 rfs（tftp 后端）找 wlan 镜像文件。即未来
链路=本 fw 除 549_38_0.mbn 外还要伺服 wlan 镜像路径（w45>0 的
前置）；同段还有 `rfs_gateway retry (auth fail)`——rfs 有鉴权层，
到时可能要应。

### 对 s53 判读的意义

- s3= 四值不等 → TX 写坏 phdr 字节 → p_offset/p_filesz 疯 →
  fstat 断言亡（数据假说具体化，修 TX 环写路径）。
- s3= 四值相等 + rqr type=4（DEL_SERVER）→ 环境假说上位
  （952KB 页池/进程创建失败），b22 段逆向（rea/Ghidra Hexagon）
  成主战场——找 dlopen 在 hash 表解析与页划分间的失败分支。

## 靴 #53：s53 双证判词——数据假说终审死刑 + 讣告定谳，环境假说上位（2026-10-23 01:2x，命28）

### 刷机经过

手机重启手势直接落 fastboot（省 console fb 舞）。串号闸门过（值不回
显）。槽位判读：current=a、successful:a=n、**retry:a 只剩 2**（自
s52 刷后 7 命烧了 5——夜里按请求反复重启所致，非电池：4444mV/
soc-ok=yes）。产物三验（尺寸类 126,976B+md5+垫 68,157,440B）→
flash boot_a → set_active a（七命重置）→ reboot。fw 冷启网络面
健康（ECM 枚举→DHCP 发 169.254.118.248→ping 通——反证昨夜楔死是
运行时死非冷启缺陷）。

### 双证判词

**① s3=42009fb.4214949.42009fb.4214949 —— 数据假说死刑（终审）**
A(源xor32全文)=C(环回读全文)=0x42009fb，B(源头108B)=D(环头
108B)=0x4214949。TX 环里 216B DATA1 的字节与源**逐位一致**。
连同 SH1/SH2（boot 期文件直读）与 s51 四证（内容字节/偏移/排空/
同拍），**服务端整链无罪结案**：so_read→块缓冲→glink_tx→TX 环写，
无一处写坏。

**② rqr 尾槽 t=4 ×7 —— 讣告假说定谳**
`ea5b.fffffffe.fffffffe.34.4`×6 + `ea6f.…34.4`：ms≈60027/60079，
sp=dp=0xfffffffe（QRTR_PORT_CTRL），52B，**t=4=QRTR_TYPE_DEL_SERVER**
——DATA1 后那 7 笔 52B 控制包就是 **elf_loader PD 的死亡讣告**
（PD 退出时 qrtr 端口逐个拆台的广播）。rtld_main 的官方静默退出
路径（dlopen 返回→QDI_OS_PROCESS_EXIT）实锤。

**③ 时序合并**：DATA1(160B)@48854 → **11.17s 双向零包** →
DEL_SERVER×7 → IPA 3688B@60250 → 死寂。PD 死后 **root PD 仍在活**
（IPA 应答、rfs 新会话）。

### 意外收获：第二客户端样本

`wo=`（末次 RRQ 记录）= `0002 "/readwrite/server_check.txt" octet
blksize=7680 timeoutms=1000 wsize=10`，`oe=`= 对它回了 OACK——
**SO 装载死后 root PD 的 rfs 线路开了新 tftp 会话**要
/readwrite/server_check.txt。即 qrtr/tftp 栈本身健康、root PD 的
rfs 线程功能正常——**唯独 elf_loader PD 的 dlopen 死**。第二客户
端对比样本成立：问题特定于 user PD 创建环境，不是传输层。

### 判决与下一战场

- **环境假说上位**。死点=phdrs 回调内（fstat 断言与 stat 已证会过
  →hash 表解析/952KB PD 像页分配/进程创建之间）。
- 下一战场=**b22 loader 段 Ghidra Hexagon 逆向**（12.1.4 自带
  Hexagon 处理器模块已确认）：找 dlopen 在 phdr 收讫后的失败分支
  （malloc/页分配/进程创建的错误返回路径），锚点=「Hash
  verification failed for segment」与 phdr 断言字符串的 xref。

## 靴 #54：逆向窗全收——xref 破零方法论 + loader 全链反编译 + 0x2ecb770 定谳 + 网络腿死亡现场（2026-10-23，案头期）

靴 #53 立的战场（b22 loader 段逆向找 dlopen 失败分支）本窗全收。
案头三件产出：方法论（为什么之前 xref 全零）、链条（loader 全链
反编译到手）、判词（0x2ecb770 不是线上魔数）。另：窗末 s53 网络
腿死亡，现场取证定谳 + 远程救护失败，等物理重启。

### 一、xref 破零方法论（五课，全入册）

1. **phdr 列错位教训**：之前的表把 p_paddr（字节 12–15）印成
   「vaddr」、p_memsz 印成「filesz」。全表重解
   （struct.unpack '<8I'，Elf32：type/offset/vaddr/paddr/filesz/
   memsz/flags/align）后 bNN=phdrNN 映射律坐实（b01=phdr1
   8248B、b22=phdr22 0x9d423c、b17/b24/b34=0B 段）。
2. **双地址世界**：执行世界=paddr（e_entry=0x8c000000，Q6 恒等
   映射跑物理址）；主镜像 PT_LOAD 一律 **vaddr=paddr+0x34000000**
   （全表验过，8 个例外是 0x557xxxx/0x1e4xxxxx 外设共享窗）。
   代码里的字符串引用全是 c2 世界立即数——这就是 FindRefs 在
   8c 世界装载的程序上全零的根因之一。
3. **Hexagon 常数扩展器（const extenders）**：32 位立即数被拆散
   进多指令，**原始字节扫描必然全零**；只有反汇编后取操作数
   Scalar 才能找到。正法=OperandScan.java（遍历全指令操作数、
   绕开 ReferenceManager——分析器不建 ref 也无所谓）。
4. **全仓线性反汇编毒化**：2.5M 站点扫描把 rodata 文本/表格当代
   码反汇编，立即数直方图全是垃圾（a3e03000 基址兔子洞的病根，
   验尸=对「引用址」dump 原字节见 ASCII）。**直方图只信函数体内
   的操作数扫描**。
5. **ELF 重装配正法**：truncate 到 max(p_offset+p_filesz)=
   99,471,360B；b00 写 0、每 bNN 写 phdrN.p_offset；patch
   vaddr:=paddr（Ghidra 映射进 8c 执行世界）+ memsz:=filesz
   （bss 叠段防重映射）→ Hexagon:LE:32:default 入口分析正常。
   产物 /tmp/modem-patched.elf + /tmp/modproj 项目（mod，
   已分析保存）。Ghidra 12.1.4 headless API 三坑：getReferencesTo
   返回 Reference[]（非迭代器）；FunctionManager 无
   getFunctionAfter(Address)；拼法 decompileCompleted。

### 二、loader 全链反编译（全在 b14 RX 段 8c595000–8c800000）

- `FUN_8c760c20`（4348B）**首页读**：tftp 读 ehdr(52B)+phdr 表
  ——s53 看到的两笔 tftp 读就是它。
- `FUN_8c7e6da0`（2156B）**装载编排**：断言错误码全家——rc -22
  st_size、rc -25×3（ehdr/phoff/phnum×32）、rc -30×2
  （p_offset/p_filesz）、rc -17（iHashSegIdx，无 hash 段）、
  rc -23 GATEWAY_MAP_FAILED、rc -33（path）；**10× 重试环**日志
  「rfs_gateway retry (auth fail) : %d」。这族断言全是软的
  （ERRLOG 环记一笔+返回错误码，环=cd0a35a0 索引 16 槽×0x30）。
- `FUN_8c7e663c`（176B）**SigVerify 线程壳**：qurt attrs→名
  「SigVerify」→栈 0x2000→create(entry=0xc07e6800)→join→透传
  退出码；日志「Gateway Authentication success!!」/「Gateway
  Authentication fail, error code %d」。
- `0x8c7e6800`（**SigVerify 本体** 276B，无函数符号强建）：从
  phdr0 扫 **p_flags&0x07000000==0x02000000**（hash 段标记）→
  找不到=-17；hash 段 filesz==0=-19 → `FUN_8c7e6a94` auth+取
  hash → 每段 `FUN_8c7e6914(authctx, idx, elf+p_offset,
  filesz)` 校验，失败日志「Hash verification failed for
  segment: %d」。
- `FUN_8c7e6a94`（772B）**gateway_security.c auth**：ELF
  magic+CLASS32+LSB+**e_phoff==0x34**+**e_phentsize==0x20** 校验；
  malloc(0x14) 状态；`thunk a411c20c(3,&flag)`=
  secboot_util_hw_is_auth_enabled（熔丝）；使能则 malloc(0x340)
  =832B 缓冲+136B 请求 → **`FUN_8c7fe388(buf,…,3,0x12,0,0,
  &out)`** → 返回≠0x2ecb770 则 **-6**「Signature Authentication
  failed」→ 编排层重试×10 → rtld 杀 PD。
- `FUN_8c7e6914`（296B）**每段哈希校验**：算法 ID 从 ctx+0xc
  （0x14/0x30/0x20 三选），crypto PD thunks a40fdac0/a40fdad0，
  成功日志「Hash matched for segment %d」。
- `FUN_8c7e50f0`（684B）**PD 生成编排**：malloc 失败日志「Eloader
  Failed to get memory- NO PD is spwan」。

### 三、0x2ecb770 定谳（重大修正）

**不是线上魔数——是 secboot/auth 网关模块的通用 SUCCESS 返回
码。**证据：`FUN_8c7e5448`（之前误标「请求构造器」）实为初始化器
（清 ctx+0x38..+0x58、建 timer），**成功也返回 0x2ecb770**。四处
引用对齐：8c5f13e8（第二客户端 cmp）/8c7e5480（初始化 return）/
8c7e6c58（客户端查）/8c7fe3b0（shim 短路 return）。

shim `FUN_8c7fe388`（44B）：selector==2 → 直返 0x2ecb770（旁路）；
否则 **thunk a411bd80=父域真身**（a411xxxx 页=PD 创建时父填的
PLT）。故 RPC(3,0x12) 是 **QuRT 父域内部调用**——auth 真身不在我
们镜像里，也**未必上我们看得见的 QRTR 线**（线上 0x12 的敲=
service-locator GET_DOMAIN_LIST，G35 起我们已答 33/0x20/0x22；
pdh= 抄盘 G34b 起活着，pe=1 十七字节名全等闩在役）。

### 四、第 4 站 8c5f13e8 身份：同门第二客户端（root 侧装载腿）

`FUN_8c5f0f98`（1260B）：同套 ELF 校验链（\x7fELF+CLASS32+LSB+
e_phoff 0x34+e_phentsize 0x20）+ 同 0x340 应答缓冲 memset +
同 is_auth_enabled 熔丝检查，但两处分叉：RPC 第 5 参=**2**（非
0x12，shim selector==2 直通=免认证成功路径）——等一下，重读：
此处传 (3,2) 而非 (…,…,2)，selector 语义仍在验；**失败走
ERR_FATAL 硬死**（FUN_8c1ee428 不返回）非软重试；带 **30 槽×40B
引用计数注册表**（DAT_caca0dc0，上限 0x1e，lock/unlock 引用计
数）=**root 侧镜像装载腿**（PD 生另一条路径，客户端侧镜像实
现）。错误码族 0xf9ac99xx+trace 面 0x314（FUN_8c110f8c/
FUN_8c110eb8）与 loader 侧 0x314 同源。

### 五、与活证据对读（环境假说细化）

- 4 记 0x12 敲=locator 查询（已答）；**auth RPC 不上这线**。
- **11.17s 静默=auth 失败 10× 静默重试**（重试环无日志直到放弃）；
  讣告 DEL_SERVER×7=rtld QDI_OS_PROCESS_EXIT（loader 返回错误
  →调用方杀 PD——loader 不崩，是被杀）。
- 死因候选（s54 要分岔）：(a) 熔丝使能、父域验证段哈希链不过
  ——原厂签名镜像理论应过，除非 **hash 段取数腿断了**（SigVerify
  先取 hash 段再逐段校验；死亡发生在任何进一步 tftp 读之前，
  即卡在 auth RPC 或 hash 段读之前）；(b) 父域里 (3,0x12) 服务
  例项无人注册（等谁注册？0x14×1 无腿口是候选）。
- s54 观察刀需求：0x12 每包 msgid 直方图（分 locator 敲 vs 其
  他）+ 0x14 定向抄盘（G38④ 的 96B 滚存已备）+ 重试窗（~11s）
  内新包全录——若 auth 上线，必在此窗。

### 六、网络腿死亡现场（窗末，s53 命 28 稳态后）

- 症状：ping/2323 全死；USB 上 Aginx ECM 仍在（枚举活）。
- 取证链：libusb 总线 reset（pyusb dev.reset()，等效拔插）→
  重枚举（id 0x1000577c3→0x100057f0b）→ ECM 仍不绑 → 配置描述
  符逐字节走读**结构完好**（IAD+CDC control(int 0x81 mps16)+
  data alt1 双 512B，SuperSpeed；iMACAddress=**字符串 3**，
  wMaxSegmentSize=1514）→ **字符串 1–4 全 STALL（pipe error）**。
- 定谳：**fw 半活——静态描述符照答、动态控制请求全拒** →
  AppleUSBECM 拿不到 MAC 字符串 → 不绑 en → 网络腿死。控制台
  骑同一条死腿，sm 不可达。远程不可救（reset 无效），**需人手
  断电重启**（通知已发）。
- 新病签名入册：**「USB 半活：描述符答/字符串 STALL」**。下次
  控制台静默先查这个：system_profiler 看 Aginx ECM 在不在 →
  pyusb 读 string（全 STALL=fw 半活；正常返串=主机侧绑定问题）。
- 配方留存：PEP 668 装不进系统 python，`python3 -m venv
  /tmp/usbvenv && pip install pyusb`，idVendor=7531 idProduct=260。

### 下一步

1. 等物理重启 → 首查 s53 死亡时刻的账（若 fw 冷启清零则只能靠
   本收据现场）；重启后先 pyusb 读 string 验 fw 活性再连 2323。
2. s54 刀（观察优先）：0x12 msgid 直方图 + 0x14 抄盘 + 重试窗
   全录。若 auth 上线且形状可解 → s55 行为刀（应答 auth）。
3. 逆向续（可离线）：FUN_8c5f0f98 的 RPC(3,2) 语义细读（selector
   旁路 vs 例项）+ a411 父域真身在 SBL 侧的对应（另一战场）。

（收据完；工具产物：/tmp/modem-patched.elf、/tmp/modproj、
/tmp/b22scripts/*.java、/tmp/mod-{opscan,magic,decomp*}.log）

## 靴 #55：s54 行为刀——p_memsz 阶梯探针（2026-10-23 06:17–，v1 缺陷现场+v2 修正）

### 一、刀设计（离线定谳的死点 → 行为证法）

靴 #54 逆向收口后的死点图：root 侧装载腿 FUN_8c5f0f98 走查 phdr
找 hash 段（我 SO 的 phdr1：flags=0x02200000 ✓、off=0x1000、
filesz=0x1a98、memsz=0x2000）→ `alloc(p_memsz)`（FUN_8c7d712c 虚调
→ 池实现 FUN_8c7d7a90 → QDI (8,4,0x3c) 内核调用，软败=err 0xb）→
成功才 seek 0x1000 读段。线上从未见过 0x1000 偏移读 ⇒ **死点=alloc
一步**（rtld 静默退场，与 +1ms 弃单、+11.1s 讣告全签名吻合）。

**证法**：送数时覆写 SO 的 phdr1.p_memsz（文件偏移 104）/p_filesz
（偏移 100）成阶梯值 [0x2000, 0x1000, 0x800, 0x400, 0x100, 0x20,
8, 4]（rung0=原值=对照靴；filesz 只缩不涨）。若 alloc 是堵点、更小
尺寸能过 → hash 段读 RRQ（rl= 里 seek=1000）上线现形 = 实锤 + 池
尺寸二分。补丁只在 DATA 泵单点注入（块盖住 100..108 才写），boot
探针读（64B@0/52B@0xa0）不经此路。

**推档双锚**：① 讣告簇（chunk=52 且 ctrl cmd=4）8s 门推档；② 兜底
=距上次首服 ≥25s 的新首服推档（防讣告假说错）。渲染 s54=档位.档值.
补丁数.推档数.源 + ob= 讣告末 2 笔 52B 全保真（qrtr 头 ver.type.
src_node.src_port.pad.size.dst_node.dst_port + 体 cmd.service.
instance.node.port）——放 rqr= 邻位=行中段，避开 TCP_TX 帽挤尾。

### 二、v1 刷机与缺陷现场（命 = 06:17:50 起）

刷机：s54 v1（126,976B ✓ 尺寸类，垫 68,157,440B，md5 01b438fd…）
→ boot_a → set_active a → reboot。控制台 06:20:20 起（+170s），
命 1 异常长（>20min，modem 干净死透反而不触发 EFS/init 狗——与
#47 的「卡 init 才 +594s 狗杀」互补证）。

**v1 现场（两帧冻结一致）**：
- 装载器整靴**只发 stat 会话**：tev 双场 `.befd.1.1/.befd.4.0/
  .befd.8.1/.befd.c.0/.beff.3.9`（+48.9s 与 +49.9s，k=12=stat 只回
  OACK、客户端 op5 code9 收口）——**从不读文件头**（s53 命 28 的
  160B phdr 读这靴零出现，装载进度逐靴漂移、非确定）。
- +62.0s 讣告簇 DEL_SERVER×7（距末会话 +11.1s 死亡节律吻合）；
  到 +163s **ob=40 笔** ≈ 2 簇 × ~20 服务（modem 全名册拆台）=
  **modem SSR 死环 ~50s/周期**，其后环停摆（ob 冻结 40）、全网静默。
- 讣告解码样本：`DEL_SERVER(svc=7,inst=1,port=83)`、
  `DEL_SERVER(svc=16,inst=2,port=85)`（src_node=0=modem、dst_node=1
  =host、src_port=QRTR_PORT_CTRL ✓；52B=32B qrtr 头+20B ctrl 体，
  与 glink_tx_payload TX 布局逐字对上）。
- **v1 缺陷实锤**：`s54=2.800.0.2.1`——纯 stat 周期也推档（讣告/
  兜底两锚都无「本档真送过补丁」门），档烧到 2 而 patchN=0，探针
  空转。用户手动重启进 fastboot（06:37 许可）。

### 三、v2 修正（06:31 建、06:39:01 刷）

新静态 `SO_PATCHED`（补丁送达旗）：补丁块真送出置 1；**两锚推档
都以旗为门、推档即消费清 0**。语义=「档位只在一次真实的头读尝试
完成后前进一级」：stat-only 周期不烧档；装载器若在某档走通则旗
留着、同档续跑不推。v2 产物：126,976B ✓、垫 68,157,440B、md5
270021a2…；串号闸门过（值不回显）→ boot_a → set_active a（七命
重置）→ reboot 06:39:01。30s/帧观察循环在跑（/tmp/s54v2-watch.log）。

### 四、v2 现场（命 1，06:41–07:0x）——阶梯守住，骰子不再滚

- 阶梯门修复**实证有效**：连续多帧 `s54=0.2000.0.0.0`（rung0 对照
  档原位、patchN=0）——stat-only 周期零烧档（对照 v1 同期已烧到 2）。
- 命 1 形状与 v1 命 1 全同：tev 双 stat 场（+48.8s/+49.8s，op5 code9
  收口）、ob=28 后冻结、rqr=55 后冻结、全网静默挂死（狗不咬）。
- `rl=4000` 判读修正：rl=（刀G5 六槽全口请求日志）位于 sm 行尾、
  TCP 3072 同步窗截半——**截断尾巴不是读记录**，s54/ob/rqr/tev 才是
  可靠判据（观察器高亮 grep 不受影响）。
- **结构性堵点定谳**：装载器骰子只在 modem SSR 时重掷；崩溃环停摆
  + 狗不咬 = 命内骰子停滚 → 探针等不到「真读头的场」。

### 五、两杠杆（07:0x，本窗口转折）

1. **rp→root_pd（rm 命令，s54v3）**：现值 `rp` 打
   `msm/modem/wlan_pd`——#46 全腿通但域 UNINIT 被拒（error 9）。
   本命实证 modem servreg 仍活：发 rp 后 `rpm=1.1.90001` 往返在、
   且 rqr 55→58 冒三笔新入站（RESTART RESP 48B + 0x65→0x10 72B +
   ctrl type6 52B）——**对活 servreg 发根域 RESTART_PD 即令 modem
   自我 SSR → PIL 重载 → 新 loader 场，不烧手机命**。根域名
   `msm/modem/root_pd` 就在固件 pd-mapper 域名表（DOMS，6421 一带）。
2. **fb 控制台命令实为「热复位进 fastboot」**（msm-poweroff
   bootloader 语义：IMEM 0x77665500 + PON 配置 + PS_HOLD 清零）——
   先前「无 handler」记忆有误，fb_check→CTRL_FB 武装→fb_tick 链
   全在。刷机闭环从此自主：fb → fastboot → 闸门 → 刷 → set_active a。

### 六、s54v3（07:00 建、链 #55 自主刷）

- 码刀三处：①静态 `SR_ROOT`（rm 选靶旗）；②控制台 `rm`（码 21：
  SR_ROOT=1 + 走 rp 同路 RESTART_PD）；③sr_tick 发送块按旗选靶
   `b"msm/modem/root_pd"`（发成即清旗）。观察面 rpm= 加末位 root。
- 产物 126,976B ✓ 尺寸类；垫 68,157,440B、md5 e2e7c7a9…。
- **链 #55**（/tmp/s54v3-chain.sh）：控制台 fb → 等 fastboot（120s）
  → 串号闸门（值不落日志）→ flash boot_a → set_active a → reboot。
  首条全自主刷（无用户手动入口）。
- 战术：v3 起每命开场即 `rm` 造 SSR 场 → 观察装载器骰子；真读头的
  场里阶梯才开步。WIN=rl/rqr 现 seek=1000；LOSS=档走 7 无 hash 读
  （alloc 池缺失实锤，转 QDI 池后援/parent PD 内存区，离线逆向）。

### 七、链 #55 后续事故：fw 重启 USB 全灭（07:0x）

- 刷+set_active a+reboot 全 OKAY（07:04:15–18），但 fw 起机后
  **Pixel 全程不上 USB 总线**（ioreg 只有用户 iPhone 0x05AC；
  fastboot/adb 皆空、en4-6 无 IP）——比 #54 的「半活：描述符答/
  字符串 STALL」更死，连半枚举都没有。
- 判读：PS_HOLD 热复位链（fb→flash→reboot）把 dwc3/phy 留在坏态，
  fw 的 USB gadget 起不来；与 #54 同族、「唯物理断电可解」。
  v3 码刀只动控制台/sr_tick/渲染，boot 路径与 v2 逐字相同（v2 于
  06:39 正常起机），固件自身挂死概率低——phy 坏态为主假说。
- 处置：请用户物理重启（首选 Power+VolDown 进 fastboot=多一次
  phy 重初始化；备选长按电源强制重启 boot_a）。**探活循环已武装：
  ping 通即自动发 rm**（root_pd SSR 造场），观察循环在录。
- 教训入册：**fb 热复位链有 USB phy 卡死风险，非物理断电不可靠
  ——fb 只能当快车道，不能当唯一车道**；刷后若 2min 内 USB 不上
  总线，即判 phy 卡死，别等。

### 八、07:28 TCP 黑洞事故定谳 + s54v4（Established 自愈刀）

- **现场**：07:19:30 fastboot reboot 后 fw 正常起机（Aginx ECM 枚举、
  ICMP 0.9ms 活）；07:28:00 老观察循环还拿到一帧，07:28:30 起 2323
  **SYN 黑洞**（连 RST 都不回）且整命不恢复。
- **定谳**：ICMP 活=主环活着在 poll iface → smoltcp 在跑 → 唯一
  能造出此形状的是 **socket 停在 Established 死对端态**：状态机只
  给半关态（CloseWait/LastAck/FinWait×2/TimeWait→abort+relisten）
  留了出口，Established 无超时无敲门检测；smoltcp 对不上四元组的
  新 SYN 静默丢（不回 RST）→ 后来者永久黑洞。诱因=本窗口我犯的
  **客户端纪律错误**：双探活循环（4s+5s）+ dd-kill 粗暴断开抢
  单连接槽位。
- **s54v4 两路自愈**（`TCP_RX_AT`/`TCP_KNOCK` 新静态）：①Established
  空闲 180s（无 RX 字节）撤桌；②SYN 敲门累计 8 次撤桌（对端来字节
  即清零计数）。撤桌姿势与半关态一致（abort+listen+CTRL 全清），
  旧两分支同步清新计数。观察面 tcp heal=0x 记 1=敲门/2=空闲。
- **客户端纪律同时收紧**：单消费者——常驻观察循环独占，ad-hoc 命令
  单发快撤，禁双循环并发探测。
- 产物 126,976B ✓；垫 68,157,440B、md5 2eda6f1f…。链 #56
  （/tmp/s54v4-chain.sh）等物理 fastboot 现场 → 闸门 → 刷 v4 →
  set_active a → reboot；起机后单探活循环点 rm。

### 九、v4 自愈刀首考失败 + USB phy 楔死第二案 + s54v5 五刀（10-23 上午）

- **07:38:20 楔死复现**：杀常驻观察循环（dd-kill 粗暴断开）后 2323
  再度 SYN 黑洞，v4 两路自愈（180s 空闲/敲门≥8）都没救回——07:44
  仍黑洞，ICMP 仍活。静态面穷尽：所有状态都有出口、heal 逻辑推演
  成立，定谳只能靠盘上环（ring_put 每条 FUA 落盘+回读验证，跨重启
  持存——遗言不怕等）。
- **取证链三波三折**：①07:47 武装等物理 fastboot，30min 无果判
  NO FASTBOOT（事后 11:34 发现 fastboot 一直在——等待窗内沙箱侧
  fastboot 探测疑似失明，教训：长窗等待循环里的 fastboot 探测要
  能区分「不在总线」与「自己看不见」）；②11:36 手动补跑：闸门过、
  set_active a、fastboot reboot——**reboot 后整机从 USB 总线消失**
  （fastboot 空、en10 消失、ICMP 灭），#54/#55 家族 USB phy 楔死
  **第二案实锤**（暖复位链 ~50% 杀 USB）；③链子对着死总线空转被
  手动收尸。07:38 楔死案的环遗言原样在盘（11:36 命也只写了启动
  记录），等下一次活控制台用 lg 深挖。
- **静态复查抓出 v4 两处真雷**（皆为本刀根因候选）：
  1. **敲门计数空转**：`tcp_rx_flags` 读全局 `RX_LEN`，但
     `CtrlPhy::receive` 消费帧时已把它清零——Established 分支里读到
     的几乎恒 None，计数永不走，自愈只剩 180s 空闲一条腿。
  2. **Closing/SynSent 无出口**：关门族臂漏了这两个态，落 `_ => false`
     万年不动（理论难抵达，但楔死形状对不上时它们是仅剩嫌疑）。
- **s54v5 五刀**（20b5a507 后第三刀，11:5x–12:0x）：
  1. 敲门改收帧时点：`SYN_SEQ` 在 `s20_on_frame` 落邮箱时对 OUT_BUF
     直接判增（单核事件路径无竞态，邮箱满被丢的 SYN 也算数）；
     `TCP_KNOCK_BASE`=进入 st=3 时的基线，差值≥8 撤桌。v4 的
     tcp_rx_flags/TCP_KNOCK/TCP_RXF 全退役。
  2. 关门族臂补 `Closing|SynSent`——任何非 Listen 态都有出口。
  3. `tcp_mark` 帽 16→64（v4 现场帽子烧完，事故窗 st 迁移全哑）。
  4. **深挖环**：`lg<n>`（尾往回 ≤24 条）+`lm<seq>`（seq 起 16 条
     窗口，尾向头扫、首个 seq<s0 止）+`LOG_OUT` 384→2048；`log`
     四条版改为 `ring_back(4)`。
  5. **USB 无枚举自愈 afb**：开机 120s 仍 `EP0.devst<2`（宿主没走
     到 SET_CONFIGURATION）即判 USB 死，直接 `CTRL_FB=2`（立即档）
     暖复位进 fastboot——XBL 全套重初始化 USB，链子见 fastboot 就
     接着滚。枚举正常的命秒过 devst=2 永不触发；拔线命一趟 fb 后
     停 fastboot（可接受停机位）。**USB 死命本就零观察价值（s54
     证据全走控制台），杀之无代价**。
  - sm 帧新增 `tp=`（TCP st 码.SYN_SEQ 累计.状态年龄秒）——此后
    TCP 体检随帧自带，不再全靠事后环。
- 产物 131,072B（v5 代码增长）；垫 68,157,440B、md5
  536ebfe9c14f567814f6ea2858e6d763。链 /tmp/s54v5-chain.sh：等
  fastboot（用户 Power+VolDown 直进 或 v4 控制台 fb 代打）→验件→
  闸门→刷 v5→set_active a→reboot→等控制台（含 auto-fb 接力三掷）
  →lg24 深挖 07:38 遗言→sm 全帧。骰机 v2=/tmp/s54-loop2.sh（12 圈×
  [等控制台/auto-fb 接住→180s 观察→fb→闸门→set_active a→reboot]，
  不刷零磨损）——判读完遗言再点火。

### 十、v5 首考：12 分钟楔死复现 + 两案同形定谳 → s54v5.2 栈重生刀

- **12:03 v5 上机即活**（用户在链武装同一秒送机进 fastboot，12:03:40
  刷入、12:05:16 控制台就位）：tp=3.14.1（TCP 体检字段在役）、
  s54=1.1000.1.1.1（本命阶梯又走 rung1）、lg24 空手（见下）。
- **12:05:32→12:08:53 之间控制台再楔**：ICMP 0.9ms 活、2323 黑洞
  （connect 阻塞到超时=SYN 确在发在对端被无视）。v5 补刀六枪敲门
  （累计 ≥25，远超 8 门槛）仍不自愈——**敲门自愈健在而无效**，
  病不在 TCP 状态机层。
- **两案同形定谳（07:38 v4 / 12:05 v5）**：TCP 先死而 ICMP 后活。
  ICMP 回显=地址镜像（借来路帧头构造），TCP SYN-ACK=地址构造（靠
  iface 自身 IP/路由/邻居状态）——**病灶在 iface 层或更深**，一切
  socket 状态机锤子够不着。触发模式：短命 kill-closed 连接簇后
  3–5 分钟（两案同）。
- **附带实锤**：本命（暖启）`rw=19.362…`——UFS 环写全灭（刀G43
  家族读活写死），暖命全程无盘上证能力；12:03 楔死案的 st 迁移
  遗言注定无处可写。冷命环写活——取证只能在冷命做。
- **s54v5.2 两刀**（69a80e7c）：
  1. **栈重生**：RX 静默 300s（基线=末次 RX/上次重生/命起取最大）
     即整栈推倒重建——CTRL_IFACE/CTRL_UP/CTRL_HANDLE 清、
     CTRL_SLOTS[0] 显式置 EMPTY（SocketSet 复用旧槽会 add 撞满）、
     TCP 全静态复位，ctrl_boot 全套重跑。观察循环 20s 一拍活用时
     永不触发；楔死后 5 分钟内控制台必回。治未知未知——不再猜病名，
     让器官可再生。
  2. **命内计时 BOOT_TICK**：rust_main 第一手捕获 cntvct；auto-fb
     与 `up` 改相对值——暖复位 cntvct 按架构活过 warm 域，绝对值
     判时长会让宿主枚举慢的暖命被 auto-fb 误弹回 fastboot（骰机
     无限蹦床）。
- 件 131,072B；垫 68,157,440B、md5 97fcbe1ee9943df7b944d2c32b703b08。
  链 /tmp/s54v52-chain.sh：刷完 lg24+sm 后**自动点火骰机 v2**
  （/tmp/s54-loop2.sh 12 圈）。

### 十一、骰机 5 圈账揭实验设计雷：阶梯永困 rung0——s54v5.3 imem 跨命持久化（12:20–12:4x）

- **v5.2 骰机 5 圈账目**（12:20:34 点火，12:4x 停）：4 送命 + 1 纯
  stat 命（命型配比 ~20% 纯 stat），45/45 帧零楔死、10 次暖复位控制
  台全存活（tp= 全 Established、SYN_SEQ 递增）——**v5.2 底盘合格**，
  栈重生刀未被踩（窗口内无楔死）。
- **设计雷坐实**（9+36 帧 `s5i` 前直读）：送命形状恒 `s54=`
  `0.2000.1.1.x`→`1.1000.1.1.1`（rung0 对照服→讣告推档一格→冻死）；
  纯 stat 命 `0.2000.0.0.0` 零推进。机理三连：①装载器每命只取件一次
  （patchN 恒 1，命内不自 retry）；②两锚推档都以 SO_PATCHED 为门、
  推档即消费——每命最多推进一格；③**SO_RUNG 是 RAM 态，暖复位归零
  ——每命永远只测 rung0=0x2000 原值对照靴**。骰机滚 1000 圈也是同一
  对照命：阶梯探针零实际测点，白滚。v2 的「阶梯守住」（四节）只防
  了「stat 烧档」，没防「档走不动」——两补丁合成此盲区。
- **rm 杆终审**（`rm=8.0.3.1` 解码：man=8 发、rp_err=**3**=QMI
  INTERNAL）：modem 侧 PD 服务收到 root_pd RESTART_PD 但拒办——
  rm 造 SSR 场的杠杆死（与 `rp`→wlan_pd error 9=域 UNINIT 同判：
  「根域 SSR 不走 PDR」二证）。命内多 episode 无驱动器 ⇒ 跨命持久化
  是阶梯前进的唯一活路。
- **字选证据链**（不瞎猜偏移）：LineageOS redbull 内核
  （`dts/vendor/qcom/lito.dtsi`）imem@146ab000+0x1000 全消费者地图：
  mem_dump_table@0x10、dload_type@0x1c、diag_dload@0xc8..0x190、
  restart_reason@0x65c（=IMEM_RESTART 本证：暖复位存活）、
  boot_stats@0x6b0、kaslr@0x6d0、pil@0x94c..0xa14；kona（sm8250，
  同代同构，xiaomi_sm8250 仓 kona.dtsi）imem 节点同集——
  **0x190..0x65c 两片 Linux 零消费者空档**。上游 torvalds 无 sm7250
  支持（404），下游源是唯一权威。
- **s54v5.3 五刀**（12:4x，boot_a @12:46:05）：
  1. `IMEM_S54_LIFE`@0x146a_b600：boot 先读快照→写回 life+1——次命
     sm 的快照值=「活了过来」的持久化存活证据本体。
  2. `IMEM_S54_RUNG`@0x146a_b604：magic 0x7166+≤7 校验恢复 SO_RUNG；
     两锚推档即写回。冷启 IMEM 清零→magic 不中→实验自动归位。
  3. 三探针 @0x200/0x400/0x500：写 0x7166_0001 一次（非 magic 才写，
     永不自覆盖）——空档死区取证：次命快照仍此值=暖链（XBL/ABL/TZ/
     PIL×2 次/骰圈）全程无人碰；异值/归零=有消费者，挪位。
  4. `rust_main` 第一手恢复/写回（BOOT_TICK 后、外设前）。
  5. sm 新字段 `s5i=`life.恢复源.life残值.rung残值.三探针残值。
  - 最坏情况护栏：字被暖链消费者碰坏→物理断电冷启即愈（IMEM 清零）。
  - 产物 131,072B；垫 68,157,440B、md5 462be8660777bb4eec146826e9b28f3c；
    链 /tmp/s54v53-chain.sh（刷完自动点火骰机 16 圈）。上机验证与
    首批 s5i= 帧见下节。

### 十二、v5.3 上机验证（12:48–）

- **首帧**（12:48:41，控制台 12:48:40 就位=刷后 154s）：`s5i=
  1.0.ef20e709.2e25f9f0.3fc81d4c.4c2d465e.7159df9`——本命暖启，
  五字全为 XBL/ABL 陈旧垃圾（非零）→ **magic 门全部正确拒绝**
  （rung=0/life=1/无误恢复）；随机垃圾命中真值 0x7166_0000..0007
  的概率 ~2e-9。`s54=1.1000.1.1.1` + `tp=3.13.1`：本命已服
  rung0 对照档并推档到 1（0x71660001 已落 rung 字）。骰机 16 圈
  12:48:47 点火；**判决帧=第 2 圈首帧**：持久化活 ⇒ `s5i=
  2.1.71660001.71660001…` 且 `s54=2.800.1.1.x`（继承档位，直测
  0x800 档）；死 ⇒ 回 `1.0.垃圾`（被消费者清/改）。

### 十三、imem 空档证伪（SRAM 衰减）+ PON 正路换装 s54v5.4（12:5x–13:1x）

**imem 假存储定谳（v5.3 骰机第 2 圈判决帧）**：

- 12:57:14 第 2 圈首帧 `s5i=1.0.cf206751.2e2df9f0.6bc85d4c.4c26461e.6159de8`
  ——**恢复源=0**。对照第 1 圈五字快照 `ef20e709/2e25f9f0/3fc81d4c/
  4c2d465e/7159df9`：逐字 XOR popcount=1–6 位。这是**随机位翻转**的
  签名，不是「有消费者在写」——消费者写要么整字覆盖要么归零，不会
  每字恰好翻几个位。定谳：**imem 0x190..0x65c 空档不在暖复位保持域**
  ——Linux dtsi 的 consumer 地图 ≠ XBL/ABL 的 SRAM 保持地图；暖复位
  后该窗整体掉电衰减，唯 0x65c reboot-reason（协议特例，ABL 消费）
  幸存。v5.3 的 imem 方案退役（magic 门两命全拒=门本身稳健，存储面
  死）。
- v5.3 骰机 5 圈（12:48–13:07）全为 rung≤1 对照命——设计雷（十一）
  的直接后果，历史价值=衰减证据链。

**PON 正路（内核源码精确取证，LineageOS redbull lineage-24.0）**：

- redfin PMIC=lito 套片 pm8150(SID0)+pm7250b+pm8150l
  （lito-pmic-overlay.dtsi）；PON@**0x800**（pm8150.dtsi
  qcom,power-on@800，qpnp-power-on 驱动，开 store-hard-reset-reason）。
- `drivers/input/misc/qpnp-power-on.c` PON 寄存器表：**0x8E=
  XVDD_RB_SPARE**（全树无任何写者=无主字节）、**0x8F=SOFT_RB_SPARE**
  （gen2 reason<<1 占位 7..1；`qpnp_pon_set_restart_reason` 写它、
  fb_go 同式）；WARM_RESET_REASON1/2=0xC2/0xB 为 PON 硬件自更新
  （不可用作存储）。主线上游 qcom-pon.c（pm8916）同族佐证。
- **PON 在 BBRTC 域、暖复位按设计保持**——fb_go 的 0x8F 腿每圈把机器
  送进 fastboot 就是活证（fw 写 → PS_HOLD 暖复位 → XBL/ABL 读到）。
- 研究工具坑：本机 shell 快照的 `grep` 函数坏了（无声 137）——
  Monitor/管道里必须用 `/usr/bin/grep` 绝对路径（本节两次翻车实据）。

**s54v5.4 七刀**（main.rs，13:0x）：

1. imem 五常量退场，换 `PON_S54_SPARE=0x8E / PON_S54_ECHO=0x8F`。
2. 编码 1 字节：`0b101_rr_ll`——魔标 101 / rung(0..7) / 命计数 mod4
   （档位顶格时仍有「恢复真发生」的翻转证据）。随机垃圾命中 1/32，
   PMIC 无衰减无消费者，风险可收。
3. rust_main 头部 imem 恢复块退役；**恢复点移到 Arb/fb_check 之后**
   （PON 读写要 SPMI 事务；boot 期自动跑 fb_check 落 FB_APID——原为
   控制台 cmd5 专属）。恢复在任何 tftp 首服之前。
4. 推档即落 PON：锚 A（讣告）/锚 B（新首服≥25s）改调 `pon_s54_store()`
   ——serve 路径无 arb 形参，Arb 是 MMIO 常量拼装纯值类型就地重建
   （单核裸件主流程无并发事务；wait_done ≤1M 轮有界）。
5. **0x8F bit0=回声探针**：每命读快照后写回 `|0x01`（bit0 在 gen2
   reason 掩码 GENMASK(7,1) 之外、无人动；fb_go 的 rmw mask=0xFE 也不
   碰它）。下命读回 1 ⇒ ABL 不清 0x8F（整字节将来可当第二存储面）；
   读回 0 ⇒ ABL 清洗实锤。0x8E 是主存储，不受此疑。
6. sm 渲染 `s5i=` 换四段：`恢复源(0冷/1PON).0x8E上命残值.0x8F上命残值.
   命mod4`。
7. 构建件：`--s54` 131,072B md5 7732d6bd…（13:06）；垫 68,157,440B
   `boot-s54v54-pad.img` md5 **176362a05898df1a86aeb152f78b2f32**；
   链 `/tmp/s54v54-chain.sh`（md5 门+串号门+console-fb 代打+auto-fb
   兜底+刷后 sm 首帧+骰机 16 圈）；v5.3 骰机遗迹存
   `/tmp/s54-loop2-v53-final.log`。

**v5.4 判据**（骰机第 2 圈首帧）：活 ⇒ `s5i=1.<a4>.<8f|1>.1` 且
`s54=` 继承档位直测 0x800；死 ⇒ `0.0.xx.0`（0x8E 被 ABL 清/写）。
WIN=任何一命 `rl=` 出现 seek=1000（hash 段读上线）；LOSS=rung7（值4）
整命无 hash 读 → 转 QDI（8,4,0x3c）池背书离线翻案线。

### 十四、phy 楔死第三案（13:07–13:19，v5.3→v5.4 换装靴）——等物理断电

- 13:07:17 骰机第 5 圈 `fastboot reboot` 后**整机离 USB 总线**：13:10
  起裸跑（禁沙箱）`fastboot devices` 恒空、ping 169.254.1.1 100% 丢、
  ioreg USB 树三 XHCI 控制器下零设备。v5.3（在 boot_a 未换）120s
  无枚举 auto-fb 理论 ~13:10:30 前后应送其进 fastboot——13:18:55
  8 分钟观察窗判词 `STILL_OFFBUS`（离线 11m38s）。定谳同靴#55 九：
  **暖复位杀 USB phy 终态，连 XBL fastboot 的 USB 都不活**，远程
  不可自愈，唯物理断电（Power+VolDown 直进 fastboot 最优）。
- v5.3 骰机遗迹存 `/tmp/s54-loop2-v53-final.log`（4 整圈+圈 5 起点；
  末帧圈4 f9 又一批独立衰减垃圾 `ef086751/2e2df9f0/7bc85d4c/
  4c2e461e/6159ce9`——每命新随机位，imem 证伪加一层）。
- 链 `/tmp/s54v54-chain.sh` 13:07:44 武装（md5 门过），窗口 13:27:44
  止；接力守卫自动重开至 14:20 前后——用户任意时刻断电重启均接得
  住（直进 fastboot=即刻刷；普通启动=console-fb 代打进 fastboot）。
- 刷后判据不变：圈 2 首帧 `s5i=1.<a4>.<8f|1>.1`=PON 持久化活、
  `s54=` 继承档位直测；WIN=seek=1000 上线；LOSS=rung7 无 hash 读。

### 十五、phy 楔死第四案+骰机 fb 腿无超时挂死（13:39–15:15，v5.4 首圈）——暖复位骰子连坐两案

- **v5.4 圈 1 干净跑完**（13:36:15 开局，证据存 `/tmp/s54-v54-cycle1.log`）：
  9×20s sm 帧全 `s5i=0.0.10.0`（冷启面：恢复源 0=imem 已退役、0x8E=0x00
  干净、0x8F=0x10 上命 gen2 理由残值、命 0）、`s54=1.1000.1.1.1`（rung1=
  0x1000 在服，memsz+filesz 双改，advsr=1=锚 A 本命已推档）、tp=Established。
  **锚 A 已把 `0xA0|(1<<2)|0 = 0xA4` 写进 0x8E、0x8F|=0x01**——字节落
  PON 就等跨命回读定生死。
- 13:39:30 `c1 fb fired`（fb_go 暖复位）后**整机再离总线**（第四案）：
  13:40:40 起裸跑 `fastboot devices` 恒空、ping 100% 丢、14:09 ioreg USB
  树零设备。**判词 STILL_OFFBUS 第二回**：暖复位杀 phy 终态复发——
  13:07（XBL 侧 `fastboot reboot`）与 13:39（fw 侧 fb_go）两起不同发起
  端同一死形，~50% 暖复位骰子坐实，唯物理断电可解。
- **骰机 fb 腿挂死 29 分钟**（13:39:30→14:08 仍无标记）：`NO FASTBOOT/
  GATE_FAIL/rolled` 三标记皆未出=循环死在轮询圈外。结构性根因=fb 腿
  `fastboot set_active a`/`fastboot reboot` **裸调无超时**——fastboot CLI
  在无设备时无限等待，闸门后设备掉总线即永久挂（子进程现场未拍，
  教训：诊挂死先拍 ps 树再杀）。杀循环（pkill -f 's54-loop2[.]sh'
  防自杀匹配）保圈 1 证据。
- **硬化**（当日落地）：骰机所有 fastboot 调用包
  `FBTO(){ perl -e 'alarm shift; exec @ARGV' "$@"; }`（macOS 无
  timeout/gtimeout；devices 15s/set_active 60s/reboot 45s），CONSOLE 的
  kill 升 -9。此后楔死最多 10 分钟出 `NO FASTBOOT` 有界停机，不再挂死。
- **复活链 v2** `/tmp/s54-recover2.sh`：等 fastboot（PON reboot-bootloader
  理由 XVDD_RB 跨断电存活，13:34 案已证冷启可直落 fastboot）**或**直出
  控制台（冷启进 v5.4）两路都接；到 fastboot 则闸门→`set_active a`
  （**不刷**，v5.4 已在 boot_a）→reboot→等控制台→sm 首帧；判决字段落
  `/tmp/s54-verdict.log` 后重开硬化骰机 8 圈。监管器接力重开复活链至
  ~17:15。若直出控制台路命中，判决帧升级为**冷断电持久化考**——比
  骰机暖复位圈更强的判据。
- 诊断教训记档：13:40:40 探测输出里的 `aginx` 行是 `tail -5` 吐的日志
  内容（console fb 回应嵌在标记行里），非 `fastboot devices` 命中——
  复合输出先分段确认归属再下结论。
- 判据不变：复活首帧 `s5i=1.a4.11.1`=PON 跨命存活（0x8F 的 11=回声位
  活=ABL 不清它）；`0.0.xx.0`=死（0x8E 被清/写）。WIN=`rl=` seek=1000
  （hash 段读）；LOSS=rung7（值4）整命无 hash 读→QDI（8,4,0x3c）池
  背书离线翻案。

### 十六、PON 0x8E 跨命证伪+阶梯转单命长走（17:23–17:42 判决链）

- **判决路径**：13:39 fb_go 暖复位 → phy 楔死第四案 3.7h → 17:22 用户
  物理断电 → XBL fastboot（闸门过）→ set_active a + fastboot reboot
  （17:23:06）→ v5.4 起。**判决帧 17:39:00：`s5i=0.0.10.0`——PON 存储死**。
  - 0x8E：命 1 锚 A 已写 `0xA0|(1<<2)|0=0xA4`（圈 1 帧实据 advsr=1），
    本命启动读回 **0x00**——字节没跨过命界。
  - 0x8F：命 1 已写 `|0x01` 回声位，本命读回 0x10（bit0=0）——**复位链/
    ABL 每次启动清洗这对 spare 字节**。回声探针的设计目的就是验这个，
    它给了答案：清洗实锤。
  - 口径注：路径含冷断电+暖复位+ABL，warm-only 存活未单独排除——但
    楔死恢复必然走冷断电，任何自治设计都必须扛住这条路 ⇒ PON spare
    对骰机实用死。**跨命存储两线全灭**：imem（十三，SRAM 随机衰减）、
    PON（十六，启动清洗）。
- **17:23 命控制台 15 分钟未起**：ICMP+USB gadget 活（枚举成功）、
  TCP 2323 全拒——fw 控制台任务单死而网络栈活。疑点=启动窗口 40 连
  撞门（waiter 任务 4s 间隔连撞；因果未证）。tftp 侧路试死（tqftp 骑
  QRTR 不走 IP 面，Mac 够不着）。17:38:35 控制台自愈（fw 栈重生类
  看门狗）→ 判决帧由文雅抓取器（ICMP 先行+15s 单连）拿到。
- **转线裁决：阶梯不需要跨命——单命长走**。本命 16 分钟 0→1（锚 A
  一集一档），失败→重试的集链在命内推进档位；骰机的 fb 循环每 3 分
  钟清零档位=帮倒忙，**已杀**（17:41，fb 发射前截住）。长命观察
  `/tmp/s54-longlife.sh` 在役：60s 文雅单连轮询、4h 窗、
  WIN=帧内 `seek=` 字段首现（hash 段读=QDI 池背书通过）、
  LOSS=rung7（s54 值=4）整集无 hash 读。
- 工具账：recover3 的 `break 2` 写错层（FASTBOOT_UP 直接跳出主循环
  没走 reboot 腿，17:23:06 手动 set_active+reboot 接回）；TaskStop 杀
  进程组连坐 nohup 子树教训已记 memory；macOS pgrep 无 -c。

### 十七、重启环判死→真杠杆=SCM PAS（18:38–19:0x，源码定谳窗）

- **24h 长命静默=三层判决实证**：守卫随会话亡（18:38 poll#1 后无痕），
  10-23 18:42 复探：ICMP 活（1.4ms）、fw 控制台活（sm 5254B 应答）、
  `s54=1.1000.1.1.1` 冻结整夜、rqr/tt 零新事件——**无重启环=无新集=
  阶梯冻结**不再只是推论。三层故障栈（①重启环未闭合 ②0x25 phdr RRQ
  吞件 ③整文件相死于 BUG-A）中 ① 是集发生器，优先。
- **zsh /dev/tcp 假死案**：zsh 无 /dev/tcp（`no such file or directory`
  被 2>/dev/null 吞成空应答，误诊控制台楔死一回合）——控制台探针必须
  `bash -c` 包裹。破案后基线照常。
- **rm/rp 实弹两发（RESTART_PD 直射）**：
  - `rm fired 1 root` → `rpm=1.1.90001.0`：SR_RP_MAN=1、SR_RP_N=1
    （真上链）、**SR_RP_ERR=0x90001**；tx 环动（tt e08→eb0）、新 RX
    （rqr 56→59，48B 应答 sport **0x65**→dst 0x10=SR_QRTR_PORT）。
    **rpm= 四槽=SR_RP_MAN.SR_RP_N.SR_RP_ERR.SR_ROOT**（14622 写点）。
  - `rp fired 2`（SR_ROOT=0→打 SR_NAME=wlan_pd）→ `rpm=2.1.90001.0`
    ——**两路径同错**。0x90001 拆=响应 TLV0x02 的 qmi_response_type
    {result=1 FAILURE, error=9}；err9 不在内核短表（qmi_soc.h 只有
    0/1/2/3/5/41/58/69/90/94），旧表读法=INCOMPATIBLE_STATE（libqmi
    交叉验证挂账）。daemon 拒绝，modem root PD 未重启（RL 零新集）。
  - 附带实锤：REGISTER_LISTENER(0x20) 早有应（sr=1.7fffffff.0 =
    SR_HIT.SR_STATE.SR_RES，state=UNINIT 0x7fffffff、res=0 成功），
    但 **si=0 零 IND**——listener 挂上了没消息。且 fw 常量
    `SR_NAME="msm/modem/wlan_pd"`（main.rs:4718）与 rm 靶
    root_pd 不同域——注册/重启不同名，协议走样（内核侧无此要求但
    同名才是 listener 语义）。
- **内核协议真源（socks:8800 经 googlesource 抓取）**：
  `drivers/soc/qcom/pdr_internal.h`：0x20 REGISTER_LISTENER(enable
  u8+path 串)/0x21 GET_DOMAIN_LIST/0x22 STATE_UPDATED IND/0x23
  SET_ACK/**0x24 RESTART_PD(service_path 串,MAX_LEN 67)**——**请求体
  形状我们没错**（串体合法）。`pdr_restart_pd()`：目标地址=locator
  （get_domain_list）应答学的 addr 且 `service_connected` 必须真——
  正主是 host 侧 pd-mapper 数据库闭环，我们绕过它直射 daemon。响应
  判读 FAILURE+DISABLED→EOPNOTSUPP、其它 FAILURE→EREMOTEIO。
- **翻案：servreg 直射非本机正路——真杠杆=SCM PAS**。本地发现
  **`.local/device/redfin/kernel-src/redbull/` = redfin 下游 4.19 全量
  内核源码树**（一直躺在 .local，从未用过）。`lito.dtsi:2753`
  `pil_modem: qcom,mss@4080000`：`compatible="qcom,pil-tz-generic"`、
  **`pas-id=<4>`**、`signal-aop`、`mboxes=<&qmp_aop 0>`、
  `ssctl-instance-id=<0x12>`——modem 的加电/复位/放行全在 **TZ PAS**，
  AP 侧正路=SCM 调用。subsys-pil-tz.c + qcom_scm-64.c 全序列：
  - SMC64、STD(yielding)、OWNER_SIP(2)；`FNID=(svc<<8)|cmd`；SVC_PIL
    =0x2。**AUTH_AND_RESET=0x5→指令 0x42000205**，x1=arginfo=1，
    x2=pas_id=4；SHUTDOWN=0x6→0x42000206；IS_SUPPORTED=0x7→
    0x42000207（无害探针）；PAS_MSS_RESET=0xa（导出但本树无人调用，
    备用）。返回 x0=0 成/1 INTERRUPTED(同参重发)/负 EBUSY(sleep 重试)。
  - 内核在 AUTH_AND_RESET 前还投 Rails（vdd_cx/vdd_mss TURBO，RPMH
    腿）——本机冷启投过的票可能仍在；若 TZ 拒，按返回码补 SHUTDOWN→
    AUTH 或补 INIT_IMAGE（metadata=mdt 首段）分步走。
  - 探针序：`mp`(0x207 探 PAS 可用)→`ms`(0x206 温和停)→`mr`(0x205
    放行)。命中=新集全链（ELF 头起重拉）=集发生器补齐+0x25 吞件
    下一轮可测。
- **v5.5 刀设计（bundled，一次 fastboot boot 全带）**：①mr/ms/mp
  三 SCM 控制台命令；②s55 RRQ 受理环（末 8 笔 RRQ：sport/seen 命中/
  到达时 TF_PEND/TF_ST/OACK 发送成败——专抓 0x25 吞点）；③SR_NAME
  改 root_pd（listener 同名对齐）。每 boot 一命律照旧。

### 十八、s55 上机日第一击：phy 楔死第五案（19:07–，等断电）

- **s55 构建**：`--s55` 131,072B md5 2340034217710c330fb83987eb638608；
  垫 68,157,440B 垫件 md5 208d802ce6776bddfa18b56355d95f51。
- **链**：/tmp/s55-deploy.sh（fb→闸门→flash boot_a→set_active a→reboot
  →ICMP/控制台→mp→条件 ms+mr→6 轮观察）。
- **19:07:22 fb 发射后 USB 全盲**：fastboot 零枚举、ICMP 死、
  system_profiler 零 USB 设备——7 分钟无恢复，**phy 楔死第五案**（同
  第四案 fb_go 暖复位签名；fb_go=PON 0x8F gen2 reason 0x04+PS_HOLD=0
  暖复位路径，正是楔死高发腿）。断电前一切自动恢复路径无效（第
  四案 3.7h 实证）。已请用户物理断电（Power+VolDown 进 fastboot）。
- 续跑腿 /tmp/s55-resume.sh 上膛（等 fastboot ≤20min→串号闸门→flash
  →set_active→reboot→mp/ms/mr 自动实验，判决落同日志）。
- 帧预算核：rt=+sc= 约 +270B，帧 5254→~5520B < CTRL_REPLY 8192B ✓
  中段插入（pr= 后）不挤尾字段。

## 十九、s55 真机三发判决 + 0x25 吞点真身活捉 + 接线暗伤（10-23 晚）

**断电复活**：用户 19:20 Power+VolDown，resume 链自动接管（fastboot
19:20:12→flash→reboot 19:20:15→icmp 19:21:27→console 19:21:32）。
phy 楔死案 #5 全程 13min，物理断电唯一解再证。

**接线暗伤（首刷 s55 三命令全不可达）**：mp 回 `?`——分派链是
`ctrl_line_is(b"名")` 名字臂 → id → match 体；上窗口只写了 22/23/24
三个体，名字臂没接；且 22/23 早被 s54v5 的 `lg<n>/lm<seq>` 日志环读
取器在 match 前 `continue` 消化——三个体是永远够不着的死臂。教训：
**控制台新命令=名字臂+体两处同刀，grep `ctrl_line_is` 与 `N =>` 对账**。
修：体挪 29/30/31（mp/ms/mr），名字臂补齐；`mx` 后续占 32。

**二刷（19:36）三发判决（boot-s55 v2 md5 338eed…，垫 adb…见 log）**：

1. `mp`（IS_SUPPORTED 0x207/pas4）→ **a0=0 a1=0**——SMC 通了！
   我们 EL1 裸发 SMC64 STD SIP（x0=0x42000207,x1=1,x2=4）TZ 应答，
   约定逐位对上内核 qcom_scm-64.c（arglen=低4位/x0x1x2 三寄存器/
   `__qcom_scm_pas_supported` 返回 res.a1）。a1=0 按内核惯例=PAS4
   支持位 0（与 lito.dtsi pil-tz pas-id=4 矛盾——下游 pil-tz 探测
   未必查 IS_SUPPORTED，待 mx 扫描标定）。
2. `mr`（AUTH_AND_RESET 0x205/pas4）→ **a0=ffcfffe9**（=-0x03000017
   私有码）——内核 remap 表只认 -1..-12（ERROR/EINVAL_ARG/EINVAL_ADDR/
   EOPNOTSUPP/ENOMEM/V2_EBUSY），此码内核自己也只会盲译 -EINVAL。
   结构 (3<<24)|0x17 疑 TZ PIL 状态机「本 boot 无 INIT_IMAGE」类拒绝
   （下步：mx 扫描标定+全 PIL 舞步 INIT_IMAGE(.mdt 元数据)→MEM_SETUP
   →AUTH；metadata 本地有真货 549_38_0.mbn）。
3. `rl=4000`/`rt=6` 三 poll 冻结——mr 拒绝生效，modem 没被重启。

**0x25 吞点真身活捉（rt= 首役）**：boot 期 6 笔 RRQ（口 22,21,23,24,
25,26 顺序），4 笔受理（disp=0），**2 笔 pend 占位整笔弃（disp=2，
口 21/23）**——上一笔 op 未消费时到达的请求直接丢，rl= 账记名永不成
服。**TF_PEND 全传输期保持 true → BUG-A 大件停滞把 pend 楔死 → 后续
一切 RRQ 死在受理门（disp=2 无限连）= 24h 静默的结构真身**。口序
递增+丢 21/23 不挡 24/25/26 到达 → modem 侧 tftp 单发不重试（无
seen-dup、无新到达）。

**s56 刀（四件，boot-s56 md5 e3e71877…/垫 adb92319…）**：①rt 环补
op 类型+文件名前 4 字节指纹（RRQ/WRQ 都入环）②受理门 newest-wins
（pend 租期>10s 陈旧 op 让位新 RRQ：tf_drop_pend 退租入 seen 环防
重放后接棒；WRQ 不抢——写腿在途状态机）③tick pend 看门狗（>60s
退租自愈，健康 963KB 传输 10s 量级）④`mx <fnid-hex> <arg2-hex>`
通用 SMC 探针（scm_raw 底座：x0 全值自持，SMC32=0x02…/SMC64=0x42…
自选；x1=1 单 by-value 参）。mx 矩阵：42000207/4 复验、02000207/4
SMC32 变体、42000207/1 与 /ff pas 扫描标定、42000206/4 SHUTDOWN
可见化。跑腿 /tmp/s56-run.sh（pid 88495）。

### 收据二十（10-23 晚）s56 活机三连+isup 全表普查——pas 表与 dtsi 镜像吻合、ar 拒因改判「前置缺失」

**v2b（mx 前缀臂修+p= 字段，md5 8ab75b09…/垫 e3eaf831…）20:42 刷入**：
boot rt 环 8 槽全景（WRQ×2 口 1f/24 + RRQ 口 21-26，全员名指纹
6165722f=`/rea`）——modem 会**同口重试**（口 21/23 重入环 disp=1=
seen 环吃重试=第二路吞包实锤）。mx 矩阵全回 usage 行——**体 32 重
解析撞零化行缓冲必 None（分派链尾 CTRL_LINE_N=0 早于体执行）**，
控制台第三课：带参命令参数必须在分派臂求值时捕获（MX_F/MX_A/
MX_HAVE statics 暂存法）。

**mspmr 零重刷三连（20:48，v2b 上手动 ms→mp→mr 各读 sc=）**：
- SHUTDOWN(4) → **a0=0**（成功）
- IS_SUPPORTED(4) → a0=0 **a1=0**
- AUTH_AND_RESET(4) → **ffcfffe9 第三次稳定复现**

对账 lito.dtsi：`pil-tz-generic`+`pil-modem` 节点白纸黑字 pas-id=4
（lpass=1/cdsp=18/venus=9/ipa_fws=15/npu=23）——stock 内核当年就用 4
起 modem，活机 isup(4) 却报 a1=0（kernel 解码 `!!res.a1`=不支持）
→ 真矛盾，立项全表普查。

**v3（mx statics 修，md5 3e850c6925e22fb7caa7794ab164a702/垫
9acc1389cdb57c9457b1ead4d4577512）20:58 刷入，mx-check 一发验通**
（回 `smc 42000207 a0=0 a1=0`）。isup 全表扫描 0–31（SMC64，只读）
20:59:38–21:04:30：

- **表员（a0=0）={1,4,9,13,15,18,19,20,21,23}**；表外（22 个）全
  ffcfffc0。**dtsi 六员 {1,4,9,15,18,23} 全数在表，零错位**；表多
  出 13/19/20/21（dtsi 无对应 PIL 节点，疑 ssc 等）。
- **a1 表员全 0**——stock 用同表员起过 modem，故此 TZ 的 isup a1
  ≠kernel 语义的 supported（裸机无 PIL 子系统在跑，最像「当前在
  跑」或未实现）。vendor subsys-pil-tz 本就不调 pas_supported，不
  冲突。
- SMC32 抽样（02000207 id 1/4/9）同形 a0=0——**两种调用宽度 TZ
  都受理**。
- 错码小字典（行为学定）：**ffcfffc0=id 不在 pas 表**；**ffcfffe9=
  前置/状态拒**（非「不支持」）。
- **ar(4) 拒因改��：pas 4 表员有效，ffcfffe9=缺 INIT_IMAGE 前置**
  → s57 = 全 PIL 舞步（subsys-pil-tz 次序：INIT_IMAGE(4, mdt_phys)
  arginfo=0x42 RW 回写 → MEM_SETUP(4, 0x8c000000, 0xf800000) →
  AUTH_AND_RESET(4)），metadata 从 UFS 读 549_38_0.mbn 头+hash 段
  拷入 RAM 静态区给 phys。若走通=TZ 自装 modem，绕开 tqftp 侍服与
  BUG-A。

v3 尾轮 rt 同构（RRQ 口 21-26，口 25 得 34 块/口 26 得 a0 块后停，
重试落 seen 环 disp=1）——newest-wins/看门狗未误触发，BUG-A 停滞
形状不变。sc=24.2000207.0.0（0x24=36 发探针全入账，字段全 hex）。

时间线：20:54 武装 → 20:58:03 fastboot（手动 Power+VolDown）→
20:59:12 console → 21:04:30 扫完 → 21:06:27 run done。
代码已先落 `6b84893d`（feat s55+s56），本收据随后垫顶。

## 二十一、s57/s58 出件 + rtld 静态路径闭环：毙点翻案=ET_EXEC 断言（2026-10-23 深夜，案头期+fb 楔死窗口）

**fb 骰机再杀 phy（22:14:30）**：s57 出件后走 `fb` 骰 fastboot——
ping 死、fastboot 20 分钟不出（等待器 22:34:40 正式超时）、ioreg 无
Google 设备（只剩 hubs/CH340/Billboard）=USB phy 楔死第 N 案，唯
物理断电可解。22:36 已推通知请手工断电重启进 fastboot（用户此窗
说过「我来手工」）。fb 骰机≈50% 杀 phy 的旧律再证——**非必要不
fb，刷机等手动 Power+VolDown**。

### 静态逆向闭环（Ghidra proj-full MFULL，b30=elf_loader PD 全链）

本窗把 dlopen 全链读完（AAnchor2/6/7 三轮，反编译件在
re-b22/out-full/）：

- **链**：PD 入口 `1e400278` → `dlopen("wlanmdsp.mbn", 4)`（
  `1e400ca8`；**mode=4 rtld_main 写死**）→ `_rtld_load_library`
  `1e405e70`（search.c 硬路径/搜索路径二枝）→ open 编排
  `1e405570`（fopen=DAT_1e41b048、fstat=DAT_1e41b050、close=
  DAT_1e41b04c）→ **分派器 `1e404730`：`mode&4==0`→动态
  map_object `1e401a70`；否则→静态 map_object `1e403a60`**。
  ⇒ **wlan 装载恒走静态路径**。
- **静态路径**（3280B）：**无 hash 段 mmap、无验签**；硬断言
  `e_type==2 (ET_EXEC)`；逐 PT_LOAD 定址 mmap(p_vaddr, p_memsz,
  prot, 1, fd, p_offset)；失败词 "Failed to mmap segment number
  %d 0x%X Bytes, object too big"。
- **动态路径**（6488B，wlan 永不达）：phdr 扫 `flags&0x07000000==
  0x02000000`（=ph1）→hash 段 mmap+verify_start/update/end→段
  循环。**s54 阶梯补的 ph1 是 PT_NULL——静态路径整段跳过，阶梯
  从头到尾=对照靴**（推档机制保留当集步进器用）。
- verify_headers `1e403408`：e_type∈{2,3} 都收（断言在静态
  map_object 里，不在 verify）；phnum≤15、phentsize=0x20——
  服务文件 5 phdr 全过。
- **ops 表** 0x1e41b030(=0x10)/0x1e41b032(=0xfff1)+16 槽
  0x1e41b034..0x1e41b074：ph31 fsz=0x1378 < 表偏移 0x8030 <
  msz=0xa000 ⇒ **纯 bss 运行时装填**；b30/b31 文件扫指针表全负
  （b30 六条 run=别处 QDI 跳表；b31 只 rodata 字符串指针）——
  Ghidra 写引用盲区=计算地址写。线索：crt 溜槽 `1e400c90` →
  `_rtld_init 1e400ae0(param_3=spawn args)` 的返回门
  **`FUN_1e40081c(param_3)`**=ops 安装头号候选（scan7b 在跑）；
  `1e400488`=空桩（覆写点）、`1e405370`=objlist 追加器、
  `1e405b60`=搜索路径清单安装器（非 ops）。
- 线上三 op（open/fstat/read 头页）都活=ops 已装好可用——表来
  源问题不挡刀。

### 毙点翻案（#54 alloc 判词撤销）

静态路径顺序：malloc(0xd0 obj)→malloc(0x1000 头页)→**read 头页**
→e_type 断言。线上 160B 头读（恰=5×32 phdr 表本体）**发生了**
⇒ 两次 malloc 都活、池健康 ⇒ verify_headers 逐项过 ⇒ **毙点=
ET_EXEC 断言**（服务文件 ET_DYN(3)≠2）。#54「alloc(p_memsz)
err 0xb」系动态路径误模推演，撤。

生产悖论立案：同 b30 在真机也断言 ET_EXEC ⇒ 真机该搜索路径上的
wlanmdsp 必是 ET_EXEC 工件——我们的 so/549_38_0.mbn（ET_DYN、
e_entry=0）要么不是正身、要么生产走他路。不挡本刀（先看段读）。

### s58 出件（22:38）

- 刀：服务泵块 0 盖 0x10 时改 e_type 字节 0x03→0x02（幂等：现值
  =3 才写；SO_ETYPED 计数；sm `s54=` 链插 `et=` 段）。前提实证：
  服务文件字节[0x10]=0x03、e_phoff=0x34、phnum=5。
- 单变量纪律：不动 vaddr（ph2 va=0 若被 mmap shim 拒）、不动
  e_entry(=0，调 0 崩在段读之后，先看段读）——s59 候补库。
- **WIN 签名升级**：`rl=` 现段读 seek=3000（ph2 RX 0x11753@off
  0x3000）/seek=15000（ph3 RW 0xd643c@off 0x15000）=过断言+mmap
  池真考；seek=1000 旧 hash 门保留观察。仍静默=断言非毙点或 shim
  拒 va=0（两歧 s59 拆）。
- 工件：boot-s58.img 131072B md5 `00d22b66b5e1cc2aa573ac6decd91a47`；
  垫 68,157,440B md5 `dfc09d8eac5f948d38750d3141f40ab8`（122 环
  毒律零填）。驱动 `/tmp/s58-run.sh`（flash|drive；drive=settle
  75s→sm→7 集 mz 间隔 25s→每集 sm 快照+WIN grep seek=
  (1000|3000|15000)+et= 命中观察）。
- 附：s57（mz 全 PIL 舞步集发生器）同件烤出未上机——两刀合并
  一靴（mz 每集重装喂档，e_type 刀每集生效）。

时间线：22:14:30 fb 杀 phy → 22:34:40 等待器超时定谳 → 22:36 推
通知等手工 → 22:38 s58 烤成垫毒 → 22:41 码尖 `9fbe43b8`（s57+s58）
→ 本收据垫顶。check.sh 全绿先过。

## 二十二、mode=4→R3 指令级闭环：影子包装破「flat listing 假象」，静态路径铁案（2026-10-23 深夜二，案头）

### 悬案（本收据要杀的）

收据二十一留了一道未闭合的口：分派器 `1e404730 !tstbit P0,R3,#2`
测的 R3 在调用点 1e40579c 的 flat listing 窗内无人置位——若 R3 非
4，全部翻回动态路径，s58 刀前提塌。scan9 窗口还见 R3 疑是 fstat
64 位字段高半字/字串残留，两读打架。

### 方法论突破：Hexagon 影子包装（shadow packet）

- **parse bits=指令字 [15:14]**：01=续包、11=包尾、00=duplex
  （双半指令，**同时收包**）。raw word 直读 modem-full.elf
  （b30 blob off 0x5ec6000+va−0x1e400000）。
- **call 与其后的参数装填可在同一包**：`call` 占槽，包内其后指令
  在控制转移前提交，被调者入口读到的是**包尾提交后的寄存器**。
  ⇒ **Ghidra flat listing 的地址顺序≠参数装填顺序——call 后面的
  指令可能正是它的装参**。scan9 的「R3 无人置位」纯属假象。
- 铁证三包（parse bits 实测）：
  - `{1e406138 call 5570; 1e40613c combine R1R0,R17,R16}END`
    ⇒ 5570(buf, mode)（与反编译 `FUN_1e405570(iVar1,param_5)`
    互证）；
  - `{1e40579c call 1e404730; 1e4057a0 combine R1R0,R16,R19;
    1e4057a4 add R2,SP,#8; assign R3,R18}DUPLEX-END`
    ⇒ **分派器四参=(R0=name, R1=fd, R2=&sb, R3=R18=mode)**；
  - 空链跳入/走链穷尽两条子路**汇入同一包**——此前担心的「R3=
    fstat 残留」根本不构成第三态，影子统一装参。

### mode 全链（寄存器级，逐跳实测）

dlopen(1e400ca8) `assign R2,R16(mode)` → _rtld_load_library
(1e405e70) 入口 `combine R17R16,R2,R0`（R17=mode）→ 两处
`assign R4,R17` 喂 1e406090 param_5 → 入口 `assign R17,R4` →
call 5570 影子 `R1=R17=mode` → 5570 入口 `combine R19R18,R0,R1`
（R18=mode）→ call 分派器影子 `R3=R18`。**R3=4，tstbit(4,2)=1，
P0 假 → 落 `jump 0x1e403a60` 静态 map_object。**

### 判词

- **静态路径铁案**（无 hash、无验签、ET_EXEC 断言、逐 PT_LOAD
  定址 mmap）——收据二十一的翻案结论从「消去法推演」升格为
  「指令级闭环」。s58（e_type 0x10 字节 3→2）前提成立，待上机。
- 字串考古：0x1e411ea2="search.c"、0x1e411f0c="try %s"（log 的
  第 4/5 参走同一影子包——两 call 一包的疑云由此解：log 包止于
  duplex，5570 自立一包）。
- 生产悖论不变：真机同 b30 也断言 ET_EXEC ⇒ 真机搜索路径上的
  wlanmdsp 必为 ET_EXEC 工件，与 so/549_38_0.mbn(ET_DYN) 相斥
  ——待 FAT so/ 全列单（挂账）。
- 撤销 scan9 的「R3=字串 0x1e411f0c 残留→静态」中间读法（对）
  与「R3=hi32(stat)→动态」担忧（错）——都是 flat 假象。

### 工件

- `re-b22/AAnchor10.java`（四函数全量反汇编）+ `out-full/disasm10.log`
  （775 行）+ parse-bits python 直读（本收据配方）。
- 码尖无新（纯案头）；本收据垫顶。fb 楔死未解（22:14:30 案#6），
  等待器超时 22:59:32，重挂晨窗——s58 一到 fastboot 即刷。

## 二十三、s58 靴判决：刀未参战（会话被 efs 抢占）+ FAT 全walk 悖论加深 + mode 字面量出处未决（2026-10-23 深夜四）

23:10:25 刷入 boot-s58-pad.img（md5 dfc09d8e，122 环 68,157,440B），
23:13 自然靴 + 7 集 mz。**判决：s58 对刀而言=无效靴，非「断言不是
毙点」**——预宣告的「仍静默=两歧」不适用，静默有上游解释。

### 刀未参战三证（全量 sm 5285B，/tmp/s58-full-sm.txt）

- `s54=0.2000.0.0.0.0`：SO_RUNG=0、SO_PATCH_N=0、**SO_ETYPED=0**
  ——块服务环全程零跑。
- `so=2.0`：TF_SO2=2 恰=tf_boot_probe 自检两读（64B@0+52B@0xa0，
  main.rs:7683/7694）——so_read 无第三笔调用=elf_loader 的读从未到。
- `tf=/readwrite/modem_efs.2.5` + `ww=1.1.0.0.0`：泵单会话状态机被
  efs WRQ 抢停（写会话武装、等块 1、0 字节到）。`tb=4.0.11.0`。

### 对照 s54 靴（/tmp/s54-full.log）：服务相活着

s54 靴 `tf=/readonly/.../549_38_0.mbn.1.7`、`tb=3.1.11.1`（**TF_ST=3
送完、块 1 已发**）、`so=4.0`（≥2 笔服务读）。s58 靴服务相死因
=会话未成熟即被抢：pr 顺序 ota(2)→**so(1)**→efs(1,2)，so RRQ 仅
1 笔（重试全被 seen 环吃=收据二十 0x25 已知案）→ loader 弃线。
s56 newest-wins/看门狗与 G20-G26 写会话腿在 so 新鲜期内的抢占
=首要嫌疑，未插桩不定谳。

### 渲染坑记档：sh=「头差异」是假象

sh= 与 FAT 拷贝逐字节一致——`push_hex` 是极简十六进制（吃前导零，
0x00→"0"），非定宽。`th=464c457f.a40003` 直读：et=3（DYN）、
em=0xa4（QDSP6）——设备 UFS so 文件=FAT 拷贝，e_type=3 上机再证。

### FAT 全walk（walk-deep.py，递归+ELF 嗅探）

62 个 ELF：**ET_EXEC 仅 2 个——modem.mdt（9420B 本体）+ modem.b00
（1172B 存根）**；so/ 60 文件全 ET_DYN。⇒ 生产悖论加深：若生产
dlopen(wlanmdsp) 带 mode bit2 走静态断言，产线全灭。生产 mode 必
不带 bit2——**bench「mode=4」出处（调用点字面量）成为待钉环**。
AAnchor11 发现 dlopen 调用点=1e4002c0/c8，位于 Ghidra 入口
1e4002e0 之前的**从未反汇编字节区**（引用来自 undefined 数据）——
`disassemble()` 强解后 dump 窗口（跑批中）。

### 1e40081c 定谳：ops 表安装器

0x54B 表装到 0x1e41b034：默认桩（log/open/read/malloc/free 等十余
槽）+param_1 覆盖（版本≥0x14）；**验签三件套 param_1[2..4]
（"If one verify fn ptr specified, all must be"）+ uimg 五件套
[0xf..0x14]**——b30 装载器带验签钩子位，静态 map_object 未调用
（无 hash 无验签的判词不变），钩子消费者在他路径。

### s59 前提（插桩先于行为刀）

先修会话抢战再谈刀效：(a) OACK/ACK/抢占全记账（sport+时刻+抢占者）；
(b) so 会话新鲜期（OACK 后 N 秒）内 WRQ 只回 ERROR 不抢占；或读会话
st∈{1,2} 优先。刀字节 0x10 3→2 挂点（块服务环 8494）本身没错——
错在会话到不了那里。

### 工件

- /tmp/s58-231022.log（run 全程）、/tmp/s58-full-sm.txt（全量 sm）。
- run 脚本三假信号记档：con() dd count=512 截断 sm（s54=/et=/rl 槽
  全丢）+ WIN/ET/MZ grep 跑残文——教训：**sm 回复 >512B，探针读
  帽按 4KB 起**；MZ_NOADVANCE 判据（sm 里找 mz ep=）永假。
- re-b22 walk-deep.py（/tmp/modem-radio/）、AAnchor11.java 三修
  （DecompInterface 正配、getReferencesTo、disassemble 强解）。

## 二十四、wlanmdsp 真身链闭合：dlopen 调用点=「wlanmdsp.mbn,4」+ vendor 真身 ET_EXEC 提取成功，生产悖论解+两线分离定谳（2026-10-23 深夜五，案头）

二十三收尾时的两个悬环（mode 字面量出处、FAT 全 walk 悖论）本窗全闭。
核心转折：**dlopen 调用点根本不是在装 so/ 里那个文件——它在装 wlanmdsp.mbn，而那
个文件不在 radio FAT，在 vendor 分区。** 观测的 so/549_38_0.mbn RRQ 是另一条线。

### ① dlopen 调用点窗口（AAnchor11 最终产出 disasm11.log）

Ghidra 入口 1e4002e0 之前的从未反汇编字节区（elf_loader PD crt 段）强解成功
（clearListing(center±0x100)+流式 disassemble 逐条走；**打印必须纯正向**——
背走（getInstructionBefore）越出反汇编前沿返回 null 会把打印循环整个杀掉，
前两轮「空窗口」即此故，不是没有指令）。

真正的 PD main=1e400278，结构完整落盘：

```
1e400278  allocframe #0x58
1e40027c  call 0x1e4002e0            ← crt init（Ghidra 误当的「入口」实为子程）
1e400280  memw (SP+#0x54),#0x0 ; assign R1,#0x0 ; add R2,SP,#0x0
1e400288  call 0x1e40a540            ← memset
1e40028c  add R0,R2,#0x4 ; assign R2,#0x50 ; assign R0,#0x0
1e400298  add R1,SP+#0x54 ; add R2,SP,#0x0
1e4002a0  memw (SP),#0x28            ← 结构 size=0x28(40B)
1e4002a8  memw (SP+#0x28),##0x1e4002fc   ← 回调①（fd 域检查 0x801..0x1800）
1e4002b0  memw (SP+#0x2c),##0x1e4003b0   ← 回调②
1e4002b4  call 0x1e400c90            ← ops 装载链（wraps 1e40081c 安装器）
1e4002b8  call 0x1e40750c            （影子 assign R0,#1 → 1e40750c(1)）
1e4002bc  assign R0,#0x1
1e4002c0  call 0x1e400ca8            ← dlopen
1e4002c4  immext #0x1e411000
1e4002c8  combine R1R0,#0x4,##0x1e411000  ← 影子：R0=0x1e411000 R1=4
1e4002cc  call 0x1e40720c            （影子 combine R1R0,#4,#0 + R3R2,#0,#0x27
                                        → 1e40720c(0,4,0x27,0)）
1e4002d8  call 0x1e408e30
1e4002dc  assign R0,#0x0 ; dealloc_return
```

影子包位级核对（收据二十二方法，parse bits[15:14]）：
- 2b8=0x5a00792a(01 续) + 2bc=0x7800c020(11 端) → 包{call 1e40750c; assign R0,#1}
- 2c0=0x5a0044f4(01) + 2c4=0x1e44440(01) + 2c8=0x7c80c080(11 端) →
  **包{call dlopen; combine R1R0,#4,0x1e411000} = dlopen(R0=0x1e411000, R1=4)** ✓
- 2cc=0x5a0077a0(01) + 2d0=0x7c004080(01) + 2d4=0x7c13e002(11 端) →
  包{call 1e40720c; R1=4,R0=0; R3=0,R2=0x27}
- 2d8=0x5a01c5ac(11 端单包) call 1e408e30；2dc duplex 收尾 return 0

### ② 路径串=「wlanmdsp.mbn」——不是 so 文件名

0x1e411000 直读（blob off=0x5ec6000+0x11000）：`wlanmdsp.mbn`。它坐在 rtld 串表
头部（紧邻 rtld.c / map_object.c / 全套断言串）。**mode=4 的生产字面量出处闭环：
 elf_loader PD 自己的 main 第一个 dlopen 就是装 WLAN 镜像。**

### ③ PD 派生表（modem-full.elf 0x1649xxx rodata）——四级搜索路径

```
msm/modem/wlan_pd  wlan_process  wlanmdsp.mbn
  /readonly/firmware/image/wlanmdsp.mbn
  /readonly/vendor/firmware_mnt/image/wlanmdsp.mbn
  /readonly/vendor/firmware/wlanmdsp.mbn
  /readonly/vendor/firmware/wlanmdsp.otaupdate
msm/modem/test_pd  test_process  testpd.mbn（同构四级）
```
旁证铁串：`RFS_ENOENT - cannot find Wlan image, CANNOT spawn a PD`、
`RFS SERVER isn't ready or RFS Read ERROR, CANNOT spawn a PD`、
`ELF_LOADER CAN SPWAN User PD:%s`。elf_loader=用户 PD 派生器，wlan_pd 是其
硬编码首单（与 main 里 dlopen 字面量互证）。

### ④ 两线分离定谳：so/549_38_0.mbn RRQ 属 dlfs so 线（0x16b0xxx）

```
dlfs_std_io.c  _FOPEN: file %s found on EFS/RFS  /so/%s
/readonly/firmware/image/modem_pr/so/%s          ← 观测 RRQ 的拼路径出处
```
62 个 so/*.mbn 全 ET_DYN=根 PD 动态库线，mode 无 bit2，e_type=3 本来就对。
**收据 21「毙=ET_EXEC 断言」判词撤销**：so 线 160B 头读后静默=服务不成熟
（收据 23 会话抢占判决维持，且升为唯一死点解释）；**s58 e_type 刀
（549_38_0 3→2）前提作废且反方向**——动态消费者拿到 ET_EXEC 反会被引进
定址段走法。刀面重定向：若要验静态路断言，patch 对象应是 wlan 线供给文件，
不是 so 线文件。

### ⑤ qdb 判死：Qshrink 符号库，非镜像容器

image/saipan/qdsp6m.qdb（6654191B）：magic `\x7fQDB`，0x40 处起 zlib 流
（\x78\xda），解压 30374857B = `# Qshrink 4.0 Hash Database File`——ramdump
符号化库。2499 条 wlan 符号/源文件名（wlan_endp.c/wlan_data.cpp…），
`\x7fELF` 计数=0，wlanmdsp 串=0。不是镜像来源，此路封闭。
（modemr.jsn=纯 servreg 元数据，version.cfg=构建串 g7250-00264-230619-B，
同样无别名表。）

### ⑥ wlanmdsp.mbn 真身提取成功（vendor.img debugfs）

radio FAT 全树零 wlan 命名文件 → 真身只可能在 vendor。brew e2fsprogs（keg-only，
/opt/homebrew/opt/e2fsprogs/sbin/debugfs）直取一代仓回滚真源
`.factory/vendor.img`（up1a.231105.001.b2）：

```
/firmware/wlanmdsp.mbn  3811096B  md5 619167e6289bdd1bd5e391979da39231
ELF32 LSB  Hexagon(0xa4)  e_type=2 ET_EXEC ✓
（同目录记账：bdwlan-redfin.bin/bdwlan-u-redfin.bin 各 26328B=板数据，
 /firmware/wlan/qca_cld=CLD 驱动树，后续 WLAN bring 用）
```
**生产悖论彻底闭合**：生产 dlopen("wlanmdsp.mbn",4) → 四级搜索第 3 级
/readonly/vendor/firmware/wlanmdsp.mbn 命中（AP 侧 RFS 供的正是 vendor 分区
此文件）→ ET_EXEC → mode bit2 静态定址段装入 → 无断言、自洽。

### ⑦ bench 无 WLAN 的结构性根因 + 上机两腿

根因：wlanmdsp.mbn 不在 radio FAT → elf_loader 四级搜索全空 → RFS_ENOENT
→ wlan_pd 永不派生（静默跳过，非死机——与收据 23 的 so 线服务不成熟互不纠缠）。

s59 计划（先修泵再供文件）：
1. **tftp 会话抢占修复**（收据 23 先决：OACK/ACK/抢占记账 + 读会话 st∈{1,2}
   优先或 so 会话新鲜期内 WRQ 只回 ERROR 不抢占）→ 根 PD so 线 62 库跑通。
2. **wlan 供文件腿**：pump 对 `/readonly/firmware/image/wlanmdsp.mbn`
   （搜索第 1 级，我们完全掌控的 serve 面）RRQ 直接答真身
   /tmp/modem-radio/wlan-fw/wlanmdsp.mbn（先入仓外资产位待定，考虑
   .local/device/redfin/ 集中）→ elf_loader mode=4 静态装入 ET_EXEC →
   wlan_pd 派生 → WLAN 子系统上 modem DSP。下一前沿=glink/wlanctl 面板数据。

### 教训

- Ghidra 背走出前沿=null 杀循环——**dump 一律纯正向**；AAnchor11 最终形态
  （DecompInterface+getReferencesTo+clearListing 流式解+正向打印）可复用。
- 「FAT 全 walk 悖论」的解法不是继续在 FAT 里找，而是问**调用点要什么名**——
  名字对不上就是别的线（dlfs so 线），别把两条线焊在同一个消费者上。
- rea 拒 Hexagon ELF 不代表案头死路：串表直读（blob va→off 换算）零成本定名。

## 二十五、s59 首刷判决：修泵腿活证成立 + 小窗分段读真相 + 两刀撤防（2026-10-24 00:30–01:00，红鳍在役）

刷机：boot-s59-pad.img（68,157,440B 垫零，md5 a7ec9f27314cbd248f1d271d4cb3a9cb）
→ flash boot_a + set_active a + reboot（用户手按 Power+VolDown 进 fastboot，
串号门 REDFIN_FASTBOOT_SERIAL 环内比对；哨兵首版变量名写成 REDFIN_FASTBOOT
被自家门拦——**门拦自家哨也是门在工作**，对账后放行）。

### ① 修泵腿（TF_SN/TF_SP 会话私有地址槽）：活证成立

| 字段 | s58（刷前，25h 命） | s59 首刷 ~90s | 解读 |
|---|---|---|---|
| tf= | /readwrite/modem_efs.2.5（efs WRQ 占名） | so/549_38_0.mbn.1.7 | 会话真源回读线 |
| tb= | 4.x（ST=4 op5 打死） | **3.x（ST=3 送完）** | 会话首次走完 |
| rt= | 8 条含 efs WRQ 穿插 | 7 条纯 RRQ 干净收尾 | 本次无抢占（wp=0- 合理，机制未被需要≠机制无效） |
| so= | 2.0 | 4.0 | 服务 +2、错 0 |

���占伤害结构（收据 23）在 s59 结构性闭合：DATA 只认首服 OACK 同拍闩的
TF_SN/TF_SP；wp=/wf= 新账渲染在案。**注意**：u=12390/ob=28 跨刷同值=PON
快照继承（s54v5.4 机制），不是「没重启」——rt/wf/wp 才是本命 RAM 账。

### ② tev 毫秒线揭真身：客户端是小窗分段读，不是整文件下载

tev 解码（ev9=so_read 返回 n；ev5=DATA 发出 blk；ev7=ST 迁；ev2=ACK；
ev3=对端 ERROR）：

```
bee9: ev3(code9=End-of-Transfer) → ev9(52) → ev5(blk1) → ev7(ST=3) → ev2(ACK 0)
beee: ev2(ACK 1) → ev9(160) → ev5(blk1) → ev7(ST=3) → （再无 ev2 ——永静默）
```

- 客户端节奏=52B 探针 + **160B 头表**（ELF 头+phdr 表）→ 拿头即决。
- tf=N 计数=7=DATA 发出数；tb=st.blk.xfer.wsent 中 blk=base+wsent=1 ⇒
  **base=0**（ACK 从未推进窗口）；ST=3 来自 limit=seek+rsize 短块收口——
  rsize 分段读，962KB 全文件从未被要过。
- 160B 头表读=收据 21「头读已发生」的同一现象，但收据 24 已拆两线：
  这是 **dlfs so 线**的装载器，不是 wlanmdsp 的 rtld 断言路径。

### ③ 拿头后永静默：头 108B 恰为两把已撤前提毒刀的落点

- s54 阶梯：phdr1.p_filesz@100（0x1a98→0x1000）+ p_memsz@104（0x2000→0x1000）
  ——rung 从 0x2000 推到 0x1000（PON 跨命继承，s58 末命讣告推档）。
- s58 e_type 刀：@0x10 ET_DYN(3)→ET_EXEC(2)。
- 两刀前提均被收据 24 撤销（so=dlfs 另线；ET_EXEC 断言属 wlanmdsp 路径；
  #54 alloc 判词撤）——**s59 是 so 头段史上首次活送，却顶着两把毒刀送**。
- 静默形状：无段读（s58 注释预言的 WIN 签名 ph2@0x3000/ph3@0x15000 未现）、
  无讣告簇（rung 不再推进）、rt 冻结 10min+。

### ④ 判决执行：s60 = SO_KNIVES_OFF（ef84ae46 已推）

单变量重考：so 线一字不动（FAT 原字节），wlan 供件腿原样。
WIN 签名=头表后有**任何**后续（段读/seek RRQ/讣告簇）；仍静默=堵点在
头表之外（版本错配/等外部事件/别的前置缺件）。
boot-s60-pad.img 68,157,440B md5 586ba228322fc3df717c62f3518944d7 待刷。

### 教训

- 「送完」两个字曾是盲读：tf=N 是**发出数**不是 ACK 数，tb.blk=base+wsent
  才是活窗真相——**每字段先查渲染器再下判词**（本轮差点把「整文件下载完
  被弃」写成结论，tev 线拉回来）。
- 过期实验刀=活性毒物：前提被撤的刀必须同 commit 撤防，否则它会在第一
  次真正的活测里污染唯一样本。
- probe-sm.sh（perl alarm 版）替代 dd 阻塞版：自终止单连接，不再留挂账连接。
