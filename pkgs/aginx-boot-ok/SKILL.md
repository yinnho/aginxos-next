# aginx-boot-ok

A/B 槽 successful 标记器（可选包，债 #346 刀，2026-10-26）。ABL 每靴
排水一条命，L0 无标记器则 7 靴烧尽回退另一槽（enchilada #548 实录）。
本包装完由 svcd 拉起 oneshot_async：单元 args 内嵌等 `/run/boot.state`
出 `done ok`（≤300s）的循环，再把**当前槽**标 successful（succ+满
tries+ACTIVE，清 unbootable）。

- `aginx-boot-ok` — 标记当前槽（单元/wrapper 调）
- `aginx-boot-ok status` — 只读倒 GPT slot 表
- `aginx-boot-ok set-active <a|b> [tries]` — 切槽（等价 fastboot set_active，下靴生效）

位法=Qualcomm r3-0.6（pri=48-49 / ACTIVE=50 / tries=51-53 / succ=54 /
unbootable=55），enchilada sdf 活体定谳（#549）。盘发现：`/dev/block/
by-name` 优先，缺失则直扫 `/dev/sd[a-z]`（mainline raw-boot）。
**无 A/B 的板（X2）不装；redfin 走烤内 rcS 路径不装**（标记器幂等，
装了也不冲突）。

## 验证

- 装后 `aginx-boot-ok status` 倒出 boot_a/boot_b 位图
- 单元跑过后（`aginx-svc status aginx-boot-ok`）`status` 里当前槽
  succ=1/tries=7；重启后再看 tries 不减——排水停
- 回滚：`aginx-pkg rollback aginx-boot-ok`
