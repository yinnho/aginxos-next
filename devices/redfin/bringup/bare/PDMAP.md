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
