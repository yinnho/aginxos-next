# aginx-qr

QR 解码器（M42f）：jpeg 解码 + quirc 定位 + Bradley 窗 + AGINXPAIR1
配对码解析。voice/term 的取景扫码都 spawn 本面（`/var/bin/aginx-qr
<jpg>`，stdout 一行一个 payload）。依赖身份进 opt-in 闭包——随
aginx-voice / aginx-term 的 depends 自动带装，不单独 opt-in。

## 验证

- `/var/bin/aginx-qr <配对码 jpg>` → rc=0，stdout 出 AGINXPAIR1 五段
- host 侧 fixture 真源：rootfs 配方 n5-qr.jpg（qr 测试读它）

## 回滚

`aginx-pkg rollback aginx-qr`（首装无 .prev 时报 no_prev，重 sync 即回）
