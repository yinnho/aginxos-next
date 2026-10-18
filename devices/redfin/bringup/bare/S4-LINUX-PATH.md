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
