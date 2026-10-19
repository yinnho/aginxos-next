# IPA v4.7 `ipa_4_7_mem_part`（lito / redfin）

来源：vendor kernel
`.local/device/redfin/kernel-src/redbull/drivers/platform/msm/ipa/ipa_v3/ipa_utils.c`
（`ipa3_init_mem_partition(IPA_HW_v4_7)` → `&ipa_4_7_mem_part`）。

DTS：`qcom,ipa@1e00000`，`qcom,ipa-hw-ver = <18>`（IPAv4.7），
`reg = <0x1e00000 0x84000>, <0x1e04000 0x23000>`（ipa-base / gsi-base），
`qcom,ee = <0>`。GSI 版本映射：`ipa3_get_gsi_ver(IPA_HW_v4_7) = GSI_VER_2_7`。

**用途**：stock `QMI_IPA_INIT_MODEM_DRIVER_REQ_V01` 的 mem TLV 取自本表
+ `smem_restricted_bytes`（`IPA_SHARED_MEM_SIZE` 的 baddr×8）。
**裸抄 offset 而不铺 SRAM canary / 表 = 假地址**；本文件仅离线收据，
供日后 GSI 写刀 + INIT，不构成 INIT 刀授权。

Canary 值：`IPA_MEM_CANARY_VAL = 0xdeadbeef`（`ipa3_sram_set_canary` 写在
section offset−4；部分 section 另写 offset−8）。

SRAM 直映（v4.5+）：
`phys = ipa_wrapper(0x1e00000) + reg_base(0x40000) + SW_AREA(0x10000) + smem_restr_bytes`。

## 表（offset / size，字节）

| 字段 | ofst | size | 备注 |
|---|---:|---:|---|
| uc_info | 0x80 | 0x200 | |
| ofst_start | 0x280 | — | SW 分区起点标记 |
| v4_flt_hash | 0x288 | 0x78 | ddr size 0x4000 |
| v4_flt_nhash | 0x308 | 0x78 | |
| v6_flt_hash | 0x388 | 0x78 | |
| v6_flt_nhash | 0x408 | 0x78 | |
| v4_rt_hash | 0x488 | 0x78 | v4_rt_num_index=0xf；modem lo..hi 0..7；apps 8..e |
| v4_rt_nhash | 0x508 | 0x78 | |
| v6_rt_hash | 0x588 | 0x78 | 同 v4 index 切分 |
| v6_rt_nhash | 0x608 | 0x78 | |
| modem_hdr | 0x688 | 0x240 | INIT `hdr_tbl_info` |
| apps_hdr | 0x8c8 | 0x200 | ddr 0x800 |
| modem_hdr_proc_ctx | 0xad0 | 0xAC0 | INIT `hdr_proc_ctx_tbl_info` |
| apps_hdr_proc_ctx | 0x1590 | 0x200 | |
| nat_tbl | 0x17A0 | 0xd00 | |
| pdn_config | 0x24A8 | 0x50 | |
| stats_quota_q6 | 0x2500 | 0x30 | INIT hw_stats_quota |
| stats_quota_ap | 0x2530 | 0x48 | |
| stats_tethering | 0x2578 | 0x238 | |
| stats_flt/rt v4/v6 | 0 | 0 | 本版空 |
| stats_fnr | 0x27B0 | 0 | |
| stats_drop | 0x27B0 | 0x20 | INIT hw_drop_stats |
| modem_comp_decomp | 0 | 0 | |
| **modem** | **0x27D8** | **0x800** | INIT `modem_mem_info` |
| apps_* flt/rt hash（本版） | 0x27B0 | 0 | 占位 |
| uc_descriptor_ram | 0x3000 | 0 | |
| **end_ofst** | **0x3000** | — | `smem_reqd_sz` |

## INIT 相关快速索引

| TLV | 表字段 |
|---|---|
| hdr_tbl_info | modem_hdr_ofst/size + smem_restr |
| v4/v6_route_tbl_info | v*_rt_nhash_ofst + v*_modem_rt_index_hi |
| v4/v6_filter_tbl_start_addr | v*_flt_nhash_ofst |
| modem_mem_info | modem_ofst / modem_size |
| hdr_proc_ctx_tbl_info | modem_hdr_proc_ctx_* |
| hw_stats_quota / hw_drop_stats | stats_quota_q6_* / stats_drop_* |
| ctrl_comm_dest_end_pt | **非本表** — `ipa_get_ep_mapping(APPS_WAN_CONS)`（需 GSI EP） |

## 只读刀 canary 位点（`ig=` ca/cb）

| sm | SRAM 相对 ofst | 期望（driver 铺过） |
|---|---:|---|
| `ca` | 0x684（modem_hdr−4） | `0xdeadbeef` |
| `cb` | 0x284（v4_flt_hash−4） | `0xdeadbeef` |

PAS 后、GSI 驱动前：通常 **不是** `deadbeef`（证未铺布局）。


## EP map（IPA_4_7，INIT `ctrl_comm`）

| client | ipa_ep_num | gsi_chan | ee | 备注 |
|---|---:|---:|---|---|
| **APPS_WAN_CONS** | **16** | **7** | AP | INIT `ctrl_comm_dest_end_pt` |
| APPS_WAN_COAL_CONS | 15 | 6 | AP | |
| APPS_WAN_PROD | 2 | 2 | AP | |
| APPS_LAN_CONS | 9 | 14 | AP | |
| APPS_CMD_PROD | 7 | 5 | AP | |
| Q6_WAN_PROD | 5 | 0 | Q6 | |
| Q6_WAN_CONS | 14 | 7 | Q6 | 与 AP WAN_CONS 同 gsi_chan 号、不同 EE |

`platform_type=3`。完整表：`.local/device/redfin/slot-b-ipa-init-20261019-154055/ipa47-ep-map.tsv`。

## 活体 R（`ig=` 尾字段）

`IPA_SHARED_MEM_SIZE` @ IPA cfg `+0x54`：`baddr[31:16] | size[15:0]`（单位 8B）。

| sm 字段 | 含义 |
|---|---|
| `sm` | 寄存器原字 |
| `r` | `smem_restricted_bytes = baddr × 8` |

INIT 绝对地址 = 上表 ofst + `r`。`0433396` 已读未导出；tip 起 `ig=` 尾带 `.sm.r`。

## GSI CH7（APPS_WAN_CONS）— 划区 `gr=` + tip config/start

已知：ee=0、ch=7、GPI FROM_GSI、re=16B、ring_len=2×(256×8)=**0x1000**（lito 默认 wan-rx-desc=256）。
EVT：WAN_CONS 专用 dedicated EVT = **同 0x1000**（非 `IPA_COMMON_EVENT_RING_SIZE=0x7C00`；common 仅 `use_comm_evt_ring`）。

### 裸机 DRAM 划区（`67d685c` 已钉）

| 区 | PA | size | 来源 |
|---|---:|---:|---|
| **基址** | **`0x9D800000`** | **`0x200000`** | modem_wlan PAS 尾 → RFS 前 8MiB 空洞 |
| CH ring | `0x9D800000` | `0x1000` | `ring_size = 2 * desc_fifo_sz` |
| EVT ring | `0x9D801000` | `0x1000` | dedicated EVT（EV0） |
| RX pool | `0x9D802000` | `0x1FE000` | tip 起铺 **4×8KiB** 最小 TRE（`IPA_GENERIC_RX_BUFF_BASE_SZ`） |

**SMMU**：I4 STE 恒等已覆盖。sm：`gr=`（划区）+ `gc=`（CHSTATE 后读）。

### tip config/start（无 QMI INIT / 无 EP16）

对照 `gsi_alloc_evt_ring` → `gsi_alloc_channel` → replenish → `gsi_start_channel`：

1. IRQ 面：TYPE_MSK CH_CTRL|EV_CTRL；SRC mask CH7+EV0；INTSET=IRQ
2. EV0 ALLOC → poll CHSTATE=ALLOCATED（超时 **5s**）→ program EVT CNTXT → prime DB
3. CH7 ALLOC → poll → program CNTXT（GPI FROM_GSI erindex=0 re=16 ring@carve）+ QOS v2.5 ESCAPE_BUF_ONLY
4. 写 4 颗 GPI TRE（`struct gsi_tre`：EOT|EOB|BEI、XFER）→ CH/EVT doorbell
5. CH7 START → poll STARTED（超时 **5s**）→ DB MSB

`gc=st`：1=STARTED；2=EV ALLOC 超时；3=CH ALLOC 超时；4=START 超时。`c0` 含 CHSTATE。

**仍禁**：QMI INIT、EP16 `ipa3_cfg_ep`、满池 replenish、WLAN/`0x45`。
