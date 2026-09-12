# aginx-update

A/B 自更新器（M14/M22）：capture（跑动系统上 stage state tar +
AGXSTATE 一次性 marker——重刷保 /root/.ssh + /etc/wifi.conf 的预武腿，
清头留体）、status（全 boot 表）、apply（签名 manifest→staged 原子换
userdata；跨机型拒刷）。母体包 depends 锚住——装 aginx 自动带。
裸箱（未装 aginx）升级=fastboot 重刷。

## 验证

- `/var/bin/aginx-update status` → 全 boot 表
- flash-redfin.sh 的 state pre-arm 腿走 capture（机会式，fail-open）

## 回滚

`aginx-pkg rollback aginx-update`（首装无 .prev 时报 no_prev，重 sync 即回）
