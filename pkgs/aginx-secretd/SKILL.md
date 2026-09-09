# aginx-secretd

秘密守护：policy 生证、加密存储、签验。人面是 /usr/bin/aginx-secret
（蛋基座自带）；密文在 /var/lib/aginx/secret，policy 在
/etc/aginx/secret.policy（蛋上 /var/bin 真身条目由 C9 增补）。

## 验证

- `aginx-secret list` 出清单
- server/gateway 的生证日志正常（值永不回显）

## 回滚

`aginx-pkg rollback aginx-secretd`（首装无 .prev 时报 no_prev，重 sync 即回）
