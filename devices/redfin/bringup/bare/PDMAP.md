# PDMAP——SERVREG_LOC（svc 0x40）应答刀案头（G35 备料，10-22）

G34（s35）在机拿 GET_DOMAIN_LIST 请求真字节；本档=G35 应答刀的全部
案头备料。语料三源全本地，零网络。

## 真源

1. **协议**（客户端镜像）：redbull 内核树
   `drivers/soc/qcom/service-locator.c` + `service-locator-private.h`
   ——APSS 侧 svc 0x40 客户端的 wire 格式（编码表 ei 全文在 private.h）。
2. **服务端**（厂参照）：redfin 厂 vendor.img（一代仓 `.factory/vendor.img`，
   debugfs 掏出）`/bin/pd-mapper`+`/lib64/libpdmapper.so`——Qualcomm 变体
   读配置于 `/vendor/firmware{,_mnt}/image[/sdx55m]`（非上游
   /usr/lib/misc/pd-mapper）。
3. **PD 数据库**（redfin 全 6 PD）：**本仓语料**
   `.local/device/redfin/modem/jsn/*.jsn`（radio FAT 里的 jsn/ 目录，
   副档 /tmp/pdm/jsn）。

## wire 协议（svc 0x40 v1，端口=我们的 0x12）

QMI 头：`tf(1B=01 req/02 resp) txn(2LE) msg(2LE) len(2LE)`，载荷=TLV
`type(1B) len(2LE) value`。

| msg | 名 | req TLV | resp TLV |
|-----|----|---------|----------|
| 0x0020 | INDICATION_REGISTER | 0x10 enable_db_updated_ind(u8, opt) | 0x02 resp |
| 0x0021 | GET_DOMAIN_LIST | 0x01 service_name(串≤64: len(1B)+bytes)；0x10 domain_offset(u32, opt) | 0x02 resp；0x10 total_domains(u16 opt)；0x11 db_rev_count(u16 opt)；0x12 domain_list(len(1B)+N×entry) |
| 0x0022 | REGISTER_SERVICE_LIST | 0x01 domain_name；0x02 len+entries | 0x02 resp |
| 0x0023 | DATABASE_UPDATED_IND | —（空） | — |

- `resp` 结构（qmi_response_type_v01）= result(2LE)+error(2LE)，成功
  `0,0`。
- entry（servreg_loc_entry_v01）= name 串 + instance_id(u32LE) +
  service_data_valid(u8) + service_data(u32LE)。
- req 的 service_name 形如 `provider/service`（内核客户端实证：
  audio_pdr 问 `avs/audio`）。
- resp 最大 2389B；TCP_TX 8192 帽够用。
- 回程：从 0x12 发往请求者 (node,port)（环捕获 +32/+36 有 src）。

## redfin PD 数据库（jsn 全文提炼）

| PD 名 | inst | 服务（provider/service） |
|-------|------|--------------------------|
| msm/modem/root_pd | 180=0xb4 | tms/servreg；gps/gps_service |
| msm/modem/wlan_pd | 180=0xb4 | kernel/elf_loader；tms/servreg；wlan/fw |
| msm/adsp/root_pd | 74=0x4a | tms/servreg |
| msm/adsp/sensor_pd | 74 | tms/servreg |
| msm/adsp/audio_pd | 74 | tms/servreg；avs/audio |
| msm/cdsp/root_pd | 76=0x4c | tms/servreg |

活体交叉验证：#39 服务图 SERVREG_NOTIF=.b401（inst 0xb4=180）——
modem 侧 tms/servreg 服务器正是按此 instance 播的。

## G35 应答算法（草案，等 #40 的 pd= 定名后落刀）

1. 收 msg 0x0021 → 解 service_name。
2. 库里按名过滤（全 6 PD 的服务表含该名者全返，domain_offset 分页）。
3. 回 `0x0021`：TLV 0x02=`0,0`；0x10=total_domains；0x11=db_rev_count
   (0)；0x12=len+entries（entry 四件套，service_data_valid=0 时
   valid=0/data=0 照编码）。
4. 未知名 → result=SUCCESS+total_domains=0 空表（上游语义）还是
   MALFORMED_MSG，看 #40 请求真容再定（倾向空表）。

## 战略链（假说链，逐靴验证）

modem 拿不到 PD map（0x12 无应答）→ PD 框架卡 → wlan_pd 不拉起 →
无 WLFW 0x45。注意 modemuw 服务表含 **kernel/elf_loader**：wlan_pd 的
固件装载依赖 APSS 的 elf_loader 服务（Linux 世=cnss/PIL）——裸世界若
走到那步，我们还要供这个面（远期，先答 PD map）。

---

## 10-22 下午翻案节（重要——上节假说已部分证伪）

**「0x12 无人应」是误读，应答器早已在役。** 翻 main.rs 定谳：

1. **应答器全套已存在**（此前的刀已铸）：RX 武装腿（dst_port==0x12,
   chunk>=56, qtype==0 && msgid==33=0x21, txn 去重 PD_SEEN, 闩
   PD_TXN/PD_NODE/PD_PORT/PD_PEND）→ `pd_take_name`（TLV1 解名，
   **无 strlen 前缀、尾部多带 NUL——按真捕获写的**；名字==kernel/
   elf_loader 时置 PD_SAW_ELF）→ `pd_reply`（TLV 02 成功 + 10
   total_domains + 11 db_rev_count=1 + 12 域表 put_dom；qmi_finish
   tf=2 ✓；从 0x12 发往 PD_NODE.PD_PORT）。DOMS 表=jsn 十映射全量
   在编（6144–6155）。
2. **#39 实测**（live log 直读，watcher 面早有）：`pr=3
   pn=kernel/elf_loader.1.30 pe=1`——modem 问的就是 kernel/
   elf_loader，我们答了 3 次（1 命中=msm/modem/wlan_pd inst 180，
   qlen 0x30=48B），rf= rmtfs 腿也在动。**但 w45 仍 0**。
3. **#39 收据勘误**：po= 是 RX 直方图（应答了也计数），「0x12×4
   无人应」错；正确读法=「4 敲 3 答 1 丢（PD_PEND 串行占位期丢），
   且客户端反复问」。
4. **新问题（#40 主猎物）**：答了 3 次客户端为何还问/wlan_pd 仍不
   生？候选序：①应答编码有客户端不认的细节（静态审计全过——tf=2/
   TLV 序/entry 四件套/strlen 全对账过 service-locator ei 表，暂无
   把柄）②3 问=3 个不同客户端各问一次（非重试），答已收下，卡点在
   下游（elf_loader 真装载=PIL，裸世界没人干）③应答虽 TX 成功但
   送达问题（见下条布局疑云）。
5. **qrtr 头布局疑云（G34 pdh= 正好定谳）**：pc= 真实字节（#36 IPA
   CONFIG_REQ 捕获，+32 起）：`+32=0 +36=0x3f +40=1 +44=0x16
   +48=QMI(tf=0! txn3 msg27 len38)`。dst@+40/+44 与 src@+24/+28 双
   双被「活线」证明（tftp 用 +24/+28 回信且 .so 全量送达=#24；全体
   分派用 +40/+44 且 po= 桶全对）——但 +32/+36 也呈 (node0,port63)
   的 src 形状。qrtrhdr_v1 标准布局（ver@+16 type@+20 src@+24/28
   dst@+32/36 size@+40 confirm@+44）与观测矛盾（dst 在 +40/+44）。
   **唯一裁决=G34 的 pdh=（从 +0 抄 96B）**：看 +24/+28 与 +32/+36
   是否同值（src 双写）还是各异。
6. **G35 修订方向**：不再是「起应答器」而是——①看 #40 pd= 真名与
   重试形状 ②若答已被收下：下一堵墙大概率=elf_loader 装载本身
   （modemuw 依赖表的头一条）——那就是 PIL 线（wlan bXX 分段装载），
   或者先看 rmtfs 腿（rf=）是否齐活。
8. **#40 敲问形状定谳（15:11 死靴）**：pr= 3 次全落在 +1..+5s 早窗，
   之后整靴零重问（po= 12.4 终值与 #39 逐字同）——**客户端非重试
   循环**。结合翻案节（答已发 3 次），「答已被收下、卡点在
   GET_DOMAIN_LIST 之后的下游」升头名候选。#40 的 pdh= 因 96 门
   自盲全靴为空（G34b 已修，链 #41 在挂）——布局疑云与 txn 形状
   转 #41 裁决。
7. **TLV1 无 strlen 前缀=pe=1 定谳（10-22 午后二读）**：redbull 内核
   客户端的 string TLV=1B len+bytes（ei 表），但 modem 方言**名起
   TLV 值首字节、NUL 终止（或 ln 界定）**，无前缀。证明链：pd_take_name
   从 @58 直拷、遇 NUL 停 → PD_SAW_ELF 闩死要求 **n==17 且逐字节
   全等**；若 @58 是前缀（0x11）则拷成 18B（0x11+名）→ n≠17 → pe
   永不闩。#39 实测 pe=1 ⇒ 无前缀定谳。（注意 pn= 渲染器 0x21..0x7f
   是**跳过**非打印非断行——干净 pn= 单独不能分辨两格式，pe=1 才能。）
   **后果：G34 的 pd= 渲染（slen@58/名@59）按内核格式写的=错位一
   字节**——#40 里 pd= 若显名必缺首字符（形如「ernel/elf_loader」，
   slen 读到 'k'=0x6b 超窗截断），属渲染伪影**不是**不同请求；
   **pdh= 96B 裸 hex 是唯一真源**，名在手解时从 @58 起读。TLV1 是否
   居首（@55）也由 pdh= 定（pd= 的 .55==1 门同样未证）。

## 判读器

`/tmp/decode-pd.py`（自测过）：吃 sm 行抽 `pdh=` 96B hex → qrtr 源
/目的 + QMI 头 + TLV 全解。注意其假设 src@+32/+36（疑云未决，#40
数据落地后按真相修正）。

## #41 读法清单（s36/G34b，pdh= 首真字节窗）

1. **qrtr 头布局**：+24/+28 与 +32/+36 同值（src 双写）还是各异（谁是
   真 src）——判读器双解并印，一次定谳；dst 应=node1.port0x12。
2. **txn 形状**：末 2 包同 txn（=同一请求重传，答丢了）vs 异 txn
   （=不同客户端各问一次，答已收下）。
3. **msgid 形状**：末 2 包是否纯 0x21；若见 0x20(INDICATION_REGISTER)/
   0x22(REGISTER_SERVICE_LIST)=我们缺应答腿的实锤（4 敲 3 答 1 丢的
   那个「丢」可能是被无腿吞的 0x22）。
4. **名位复核**：TLV1 是否居首（@55==1）、名起 @58 无前缀（pe=1 已证
   的方言）——pd= 渲染应与 pn= 逐字一致（G34b 同位修）。
5. **service_data 假说已判死**（modemuw.jsn 全 valid=0/data=0，我们
   答 0=与厂逐字段同）——#41 不再追。

## #41 裁决（10-22 16:10，pdh= 已落地）——清单五项全裁

1. **布局破解：+32..+39=u64 payload size，非第二地址**。两案对账：
   PD 包 +36=29=QMI 总长 7+22；IPA 包（pc=）+36=63=7+56，全中。
   方言头（48B+前缀 16B）定谳：`前缀{0x00020009,4,0x40,0} +
   ver@+16=1 + type@+20=1 + src@+24/+28 + size@+32..39(u64) +
   dst@+40/+44 + QMI@+48`。内核 qrtr_hdr_v1 是 dst/size 重排前的
   另一形状——「src 双写」假说死。**回址 +24/+28 定谳正确**（tftp
   铁证成立）。判读器 src 取 A 案即真值，B 槽改标 size。
2. **txn 形状**：本靴 2 敲 2 答（txn 末=2），#39/#40 4 敲 3 答——
   敲数漂移（见 4），非重试循环维持 #40 定谳。
3. **msgid**：两敲均 0x21（末敲实见；首敲 pd0= 待 G36）——但 0x20/
   0x22 应答腿缺失是**静态实锤**（canon：两 resp=TLV02(0,0)
   MAX_LEN 7），「被吞的第 4 敲」假说升级为 G36 主修。
4. **名位**：TLV1 居首 @55==1 ✓、名起 @58 无前缀 ✓（TLV len=19 恰=
   名长，值首字节='t'）——⑺ 方言论全证实。**新名字
   tms/pddump_disabled 不在六 jsn**（语料 grep 证）→ 空表应答
   （nh=0→total=0 无 TLV12，main.rs 8766-8811 静读）=厂一致行为。
   本靴 pe 不闩=modem 没问 elf_loader（查库顺序逐靴漂移，非确定）。
5. **service_data**：死案维持。

## G36 刀案（s37，boot #42）——0x12 服务面补全

1. **去重键修**：PD_SEEN 比裸 txn → 改 `(sport<<16)|txn`（0x10 腿注释
   实证多客户端各自 txn=1 起算；#39/#40「1 丢」可解）。
2. **0x20/0x22 应答腿**：latch（node/port/txn/pend，每敲刷新）+泵内
   小应答（tf=2 镜像 txn、TLV02(0,0)、15B）；计数 pr20=/pr22=。
   canon=private.h：两 resp 都只有 TLV02。
3. **一深二队列**：PD_PEND 占位期新 0x21 存 PD2_* 不丢，pd_reply 成
   功后 PD2 晋升 PD（早窗突发同圈双敲不再吃掉一只）。
4. **pd0=/pm= 渲染**：TF_PD12[0]（倒数第二敲）名+txn+msgid 上屏，
   pm= 两槽 msgid——此后每靴全敲形状可见，不再只看末敲。
5. 不动：pdh=（末 2 敲 96B 滚存）、po=/pc=、其余全腿。
