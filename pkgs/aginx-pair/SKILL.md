# aginx-pair

配网 apply 面（C3/C4）：payload 一行进 stdin（AGINXPAIR1 五段），
join+IP 轮询+落 wifi.conf、env 三键合并、快速校时、internet 探测、
母体两单元 restart-ready、boot.state 定点刷新；汇总行回 stdout。
秘密只进 env 文件（0600）；argv 恒两词——psk/三键永不进
/proc/*/cmdline，两侧日志都永不记值。铸码 mint 是 host-only 面。
冻结扫码配网脸线（#422）。不再随 aginx-voice 自动带装；需要时单独 opt-in，或由仍声明 depends 的 aginx-term 带装。

## 验证

- voice/term 扫码配网全流程（m42c A/B 段收据）
- `ls /var/lib/aginx/stamps` 见 aginx-pair

## 回滚

`aginx-pkg rollback aginx-pair`（首装无 .prev 时报 no_prev，重 sync 即回）
