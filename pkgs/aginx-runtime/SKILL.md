# aginx-runtime

fast-agi 引擎：aginx-server 按需 spawn 的化身执行器（server 单元的
AGINX_RUNTIME_BIN 指向它）。一化身一进程、会话光标语义（D5 runtime
单引擎，冷热两态）。

## 验证

- `aginx agent send me` 真往返（server spawn 本件）
- svcd 日志里引擎起落无异常

## 回滚

`aginx-pkg rollback aginx-runtime`（首装无 .prev 时报 no_prev，重 sync 即回）
