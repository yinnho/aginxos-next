# aginx-gateway

远端通道守护（N5⑥）：8443 长连、agc 往返。id/secret 走 env_file
（刷机日灌注），不进包不进库；单元烤在 svc.d。

## 验证

- `aginx-svc status aginx-gateway` → ready
- /proc/net/tcp 8443（:20FB）有 ESTABLISHED（netstat 禁用——busybox 必炸）

## 回滚

`aginx-pkg rollback aginx-gateway`（首装无 .prev 时报 no_prev，重 sync 即回）
