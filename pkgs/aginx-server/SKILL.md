# aginx-server

平台心脏（D11 母体的宿主引擎）：化身登记、会话光标、请求路由、会话账。
UDS /run/aginx.sock，由 svcd 守护。蛋上本包是必装清单项——裸蛋装齐
八件后才等价于整机。

## 形态

- 裸 musl 二进制 → /var/bin/aginx-server（place_binary 面 + sidecar）
- 单元烤在 svc.d（蛋机组 sed cmd 到 /var/bin；整机档仍指 /usr/libexec）
- 依赖：aginx-runtime（server 按需 spawn 的化身执行引擎）

## 验证

- `aginx-svc status aginx-server` → ready
- `aginx agent send me` 真往返

## 回滚

`aginx-pkg rollback aginx-server`（首装无 .prev 时报 no_prev，重 sync 即回）
