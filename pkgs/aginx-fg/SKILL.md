# aginx-fg

redfin qpnp-qg 用户态电量计（可选包）。L0 缺 stock FG daemon 时
msoc 冻结在 PON；本包提供：

- `aginx-fg unlock` — 写 144B udata（拒 SOC=0 / bms<1；可选 md5 门）
- `aginx-fg check-canonical` — 规范 9900 blob md5 自检
- `aginx-fg pack` — 出 blob 文件
- `aginx-fg run` — 常驻：FIFO 库仑积分 + 周期写回 SYS/CC（`--once` 单拍；`--interval-secs N`）
- `aginx-fg read-kdata` — 非阻塞读一帧 kdata

`[service]` 装完由 svcd 拉起 `run --interval-secs 30`。无 `/dev/qg` 的板不要装。

## 验证

- `aginx-fg check-canonical` → canonical md5 OK
- 健康 bms≥1：`aginx-fg unlock` 后 SYS/CC 与 bms 一致
- `aginx-svc status aginx-fg` 见 running（装包后）
- `aginx-fg run --once` 后 `read-kdata` 有 FIFO 样本

## 回滚

`aginx-pkg rollback aginx-fg`（首装无 .prev 时报 no_prev，重 sync 即回）

## 库仑约定（redfin 实证 2026-10-12）

- FIFO `i` 与 `battery/current_now` **符号相反**（放电 FIFO 为正）；`integrate` 用 `fifo_i_ua_coulomb = -fifo_i_ua`。
- `fifo_length==0` 空帧跳过；Full nail 仅 `status==Full`，拔电后容量=100 不钉死。
- OCV 每拍优先 `voltage_now`（避免 SDAM `voltage_ocv` 冻结）。

## 内核 CC 单调（v0.1.1）

未插电时 qpnp-qg 拒收高于 live `cc_soc` 的 CC 写入（`is not monotonic`）。
`run` 种子优先 `sys_soc`/`cc_soc`；放电时 stamp 用 `hold_udata_monotonic`
把 CC 钳到 `min(soco, live_cc)`。勿与常驻 daemon 并行 `run --once`。

## 重启耐久（2026-10-12 夜真考 PASS）

路径：`/usr/bin/aginx-reboot reboot`（勿用 `adb reboot`）。

| | 重启前 (~23:30) | 重启后 (uptime~5min) |
|--|--|--|
| bms/capacity | 92 | **91** |
| sys_soc / cc_soc | 9171 / 9171 | **9075 / 9075**（齐、库仑续跌） |
| battery | 94% Discharging | 93% Discharging |
| dmesg PON | （缓冲已滚） | `using SHUTDOWN_SOC @ PON ocv_uv=4159198uV soc=91` |

**结案**：SDAM 重放正确，**未复零**；`aginx-fg` v0.1.1 周期 stamp 足以喂 PON。
全 unit ready（含 channels/research/browser/net-watch）；channels `active:true`。

## 从 0 恢复（v0.1.2，2026-10-15）

深放后 qg 会焊在 0（`qg_vbat_empty_handler` 盖 SDAM SOC=0；L0 无 stock FG，
msoc 冻在 PON 值）。而 udata 写入要求 bms≥1、bms 又只靠 udata 写入抬升
——**死锁**（10-15 实发：aginx-fg 拒启 `soco_raw=0`，`fg_reset` 单做无人回写）。

- 侧门条件（`zero_recovery_allowed`，单测覆盖）：bms<1 **且** VBAT≥4.35V
  **且**（status Full 或 charge_type Taper）——恒压段=近满真证据；Fast/放电一律拒。
- 动作（`recover_from_zero`）：先 `fg_reset`（S7 OCV 重采+force_soc）→ 等 5s →
  写 md5 门规范 9900 blob（「永不写 0」门不变）。
- 入口：`aginx-fg unlock --recover-from-zero`（手动）；`run` 启动遇 bms=0 自动
  尝试，条件不足照旧拒启。
- 实证（redfin 10-15）：VBAT 4.456V/Charging/Taper → 自动恢复，日志
  `zero-recovery: fg_reset + wrote canonical 9900`；post BMS=99 SYS=CC=9900、
  voltage_ocv 2.766V→4.456V；battery rl 1 分钟 0→4% 爬升（预期）。
