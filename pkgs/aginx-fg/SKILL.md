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
