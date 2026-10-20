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
