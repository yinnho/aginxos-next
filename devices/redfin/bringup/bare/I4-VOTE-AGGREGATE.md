# I4 offline BCM vote aggregate (SVS2)

**Stamp:** 2026-10-19 ~12:06 Asia/Shanghai  
**Scope:** offline only — **no flash**, no `rpmh_cmd` knife, no `pas_go`.  
**Inputs:**
- cmd-db capture `cmd-db-bcm-20261019-120213/` (knife `e4d9591`, raw `bc=…`, all `st=1`)
- slot-b topology `slot-b-ipa-bcm-20261019-114548/I4-TABLE.md` (+ of_node buswidth/agg-ports)
- DTS SVS2 `lito.dtsi` idx=1 + `lito-bus.dtsi` endpoint BCMs
- Linux: `msm_bus_arb_rpmh.c` / `msm_bus_fabric_rpmh.c` / `msm_bus_of.c` (`KBTOB`)

**Confidence (overall):** **MEDIUM** — addr/unit/width are live cmd-db; ab/ib are DTS SVS2 (not live `Act_IB/AB`, which were 0 post-PAS). Endpoint BCM set matches I4 topology; intermediate NOC hops not fully walked. Enough to **review** a narrow vote+SMMU knife; **not** enough to claim bit-identical to Linux `dump_bcm_clients`.

---

## 1. cmd-db fill (DONE)

Raw: `bc=IP0.1.50070.28.8,SH0.1.50004.3fa110.20,MC0.1.50000.1f47d0.10,ACV.1.5007c.1.0,SN1.1.5002c.81650.8,CN0.1.50074.7530.4,SH4.1.50014.3fa110.10`

| bcm | addr | unit_size | width | hw |
|---|---|---:|---:|---:|
| IP0 | `0x50070` | `0x28` (40) | `0x8` (8) | 5 |
| SH0 | `0x50004` | `0x3fa110` (4170000) | `0x20` (32) | 5 |
| MC0 | `0x50000` | `0x1f47d0` (2050000) | `0x10` (16) | 5 |
| ACV | `0x5007c` | `0x1` (1) | `0x0` (0) | 5 |
| SN1 | `0x5002c` | `0x81650` (530000) | `0x8` (8) | 5 |
| CN0 | `0x50074` | `0x7530` (30000) | `0x4` (4) | 5 |
| SH4 | `0x50014` | `0x3fa110` (4170000) | `0x10` (16) | 5 |

---

## 2. Aggregation formula (Linux letter)

From `msm_bus_of.c`: DTS `vectors-KBps` → `ab/ib = KBTOB(x) = x * 1000` (Bps).  
From `msm_bus_arb_rpmh.c` `bcm_update_bus_req` + `msm_bus_div64`:

```
ab' = div64(ab_Bps * width, buswidth * num_aggports)
ib' = div64(ib_Bps * width, buswidth)
vote_x = div64(ab', unit_size)   # vec_a
vote_y = div64(ib', unit_size)   # vec_b
# msm_bus_div64: if 0 < num < base → 1; else floor (pow2 via shift)
TCS  = (1<<30)|(1<<29)|((vote_x & 0x3FFF)<<14)|(vote_y & 0x3FFF)
     = BCM_TCS_CMD(commit=1, valid=1, vote_x, vote_y)
```

Node geometry (live of_node = DTS):

| endpoint node | buswidth | agg-ports | bcm(s) |
|---|---:|---:|---|
| slv-ipa-core-slave | 8 | 1 | IP0 |
| slv-qns-llcc | 16 | 2 | SH0 |
| slv-ebi | 4 | 2 | MC0, ACV |
| slv-qxs-imem | 8 | 1 | SN1 |
| slv-qhs-ipa | 4 | 1 | CN0 |
| mas-acm-apps0 | 16 | 1 | SH4 |

SVS2 ab/ib (KBps) from `lito.dtsi` case idx=1:

| path | src→dst | ab | ib |
|--:|---|--:|--:|
| 1 | IPA→LLCC | 150000 | 500000 |
| 2 | LLCC→EBI | 150000 | 700000 |
| 3 | IPA→OCIMEM | 75000 | 700000 |
| 4 | AMPSS→IPA_CFG | 0 | 76800 |
| 5 | IPA_CORE→IPA_CORE | 0 | 100 |

---

## 3. Filled I4 table (SVS2, ACTIVE, commit+valid)

| bcm | addr | unit | width | vote_x | vote_y | TCS data | conf | notes |
|---|---|---:|---:|---:|---:|---|---|---|
| **IP0** | `0x50070` | 40 | 8 | **0** | **2500** | **`0x600009c4`** | HIGH | path5; core “clock” ticket |
| **SH0** | `0x50004` | 4170000 | 32 | **35** | **239** | **`0x6008c0ef`** | MED | path1 endpoint |
| **MC0** | `0x50000` | 2050000 | 16 | **146** | **1365** | **`0x60248555`** | MED | path2 endpoint |
| **ACV** | `0x5007c` | 1 | 0 | **0** | **0x8** | **`0x60000008`** | MED-LOW | special — see §4 |
| **SN1** | `0x5002c` | 530000 | 8 | **141** | **1320** | **`0x60234528`** | MED | path3 endpoint |
| **CN0** | `0x50074` | 30000 | 4 | **0** | **2560** | **`0x60000a00`** | MED | path4 CFG slave |
| **SH4** | `0x50014` | 4170000 | 16 | **0** | **18** | **`0x60000012`** | MED | path4 AMPSS master |

Worked example (IP0):  
`ib=100*1000=100000`; `ib'=div64(100000*8,8)=100000`; `vote_y=div64(100000,40)=2500`;  
`TCS=0x40000000|0x20000000|(0<<14)|2500 = 0x600009c4`.

**Do not use** the no-`KBTOB` toy numbers (those collapse most votes to 1–2).

---

## 4. ACV width=0 handling

cmd-db: `width=0`, `unit_size=1`. Not a byte-BW resource.

Linux (`bcm_update_acv_req` in `msm_bus_arb_rpmh.c`):

- Identified by **BCM id** `MSM_BUS_BCM_ACV` (7038), not by `width==0`.
- If post-scale `max_ab==max_ib==0`: clear APPS bit in RSC `acv[]`.
- Else: `acv |= BCM_TCS_CMD_ACV_APPS` (**`0x8`**); then **`vec_a=0`, `vec_b=*acv`**.
- So encoded vote is **enable bitmask in vote_y**, not a rate.

**Paradox / gap:** with `width=0`, the `* width` scale before that check yields 0, so the letter of the scale path never sees demand on ACV itself. Intent (and common practice) is: **enable ACV whenever the paired EBI/MC0 path has non-zero SVS2 demand**. Slot-b sysfs showed `Max_Act_IB 4878` on ACV historically but `Act_*=0` at capture.

**Knife encoding (recommended for review):** `rpmh_cmd(0x5007c, 0x60000008, …)` iff MC0 SVS2 is non-zero.  
**Confidence MED-LOW** until a live `bcm=ACV client=ipa` line confirms `act`/`vec_b`.

---

## 5. rpmh_cmd sequence (after SMMU STE plan; **do not flash**)

Linux probe order is **votes → SMMU attach (SIDs 0x520/521/522) → `subsystem_get(ipa_fws)`**.  
A combined narrow knife should mirror that (votes before or with STE; **never** `pas_go` in this knife):

### 5a. SMMU STE (prerequisite / sibling step — not implemented here)

| SID | context | group (slot-b) |
|---:|---|---:|
| `0x520` | `ipa_smmu_ap` | 13 |
| `0x521` | `ipa_smmu_wlan` | 14 |
| `0x522` | `ipa_smmu_uc` | 15 |

### 5b. ACTIVE BCM votes (apps RSC drv-2 TCS0 `@0x18220d00`, existing `rpmh_cmd`)

Suggested issue order (core clock first, then mem, then cfg; ACV with MC0):

| # | bcm | addr | data | sketch |
|--:|---|---|---|---|
| 1 | IP0 | `0x50070` | `0x600009c4` | `rpmh_cmd(0x50070, 0x600009c4, freq)` |
| 2 | SH0 | `0x50004` | `0x6008c0ef` | |
| 3 | MC0 | `0x50000` | `0x60248555` | |
| 4 | ACV | `0x5007c` | `0x60000008` | enable bit; pair with MC0 |
| 5 | SN1 | `0x5002c` | `0x60234528` | |
| 6 | CN0 | `0x50074` | `0x60000a00` | |
| 7 | SH4 | `0x50014` | `0x60000012` | |

Then sm-only check (e.g. completion / `ib=` style probe) — **no** `pas_go(0xf)`.

Optional: `mas_qxm_ipa` `node-qos-bcms = <7035 0 1>` is a fixed IP0 `(0,1)` QoS poke; SVS2 path5 `(0,2500)` dominates if both were sent.

---

## 6. Gap list

| # | gap | impact | mitigation |
|--:|---|---|---|
| G1 | Slot-b `Act_IB/AB=0` (no live ipa client rates) | Votes are DTS-SVS2 synthetic | Label MED; prefer live `dump_bcm_clients` if DEBUG_FS image ever available |
| G2 | Intermediate NOC BCMs not enumerated | May miss SN*/CE* on a2noc/snoc hops | Endpoint set is I4 scope; IP0 alone may be enough to “un-gate” IPA core clk — still review full 7 |
| G3 | ACV width=0 × scale paradox | `0x8` is reasoned, not live-proven | Keep ACV in review knife; verify with sm after send |
| G4 | Dual vs ACTIVE-only | IPA node has **no** `qcom,msm-bus,active-only` → Linux may also program SLEEP/dual | Narrow knife: ACTIVE only (matches bare `rpmh_cmd` path) |
| G5 | No SMMU STE bare helper for IPA SIDs yet | Vote knife alone ≠ PAS-ready | STE is sibling work; both green before any PAS discussion |
| G6 | Multi-client max() on shared BCMs | SH0/MC0/CN0 sleep votes from display/ufs etc. on slot-b | Our numbers are **ipa-only** floor; Linux would max with others |

---

## 7. Verdict: enough for a narrow vote+SMMU knife next?

**YES, for a review-scoped knife — with limits.**

- **Yes why:** cmd-db addr/unit/width filled; SVS2→TCS numeric table complete for the seven I4 BCMs; SMMU SIDs known; send path (`rpmh_cmd`) already proven for other resources; IP0 encoding confidence HIGH.
- **No (not yet) for:** claiming Linux-identical vectors; enabling `pas_go`; skipping ACV/STE review; flashing without human sign-off on G2/G3.

**Next coding (still no PAS):** (1) SMMU STE knife for 0x520/521/522, (2) BCM ACTIVE vote knife using the TCS column above, (3) sm health checks only. Restore/`c38a6cd` discipline unchanged.

