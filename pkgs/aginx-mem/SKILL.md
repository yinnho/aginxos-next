# aginx-mem

记忆面工具 CLI（D13 改姓自 `agmem`，2026-09-26）。双消费面：

- **母体桥**：runtime 的 agmem_bridge spawn `aginx-mem tool <name>`，
  stdin 喂入参 JSON（含 `_ctx` 的 agent/owner/user 三元组），stdout 收
  D1 信封。工具名 kv_get/kv_set/kv_list/memory_tree/knowledge_*/
  flow_*/clone_evaluate——flow 冻结，不随包改名。
- **人面 CLI**：`aginx-mem get/set/list/del`（kv）、`search/topic/source/
  global/drill/leaves`（tree）、`k`/`flow`（要 --workspace）（router
  `mem` 组）。裸跑要 `--db /home/data/carrier.db` 指真库（缺省
  $HOME/.aginx 是空壳）。

直开 substrate 库（与 daemon 同一 sqlite，WAL 并发安全）；身份与库路径
的单真源在 kernel 侧，CLI 只消费。

缺包时母体的记忆/知识工具全部报错——晨报类 cron 流程的第一步
（knowledge_list）就会死；本包是 agent 记忆的地板依赖。

## 验证

- `aginx-mem --db /home/data/carrier.db set demo hi && … get demo` 出 hi
- `ag agent send me '我上次说了什么'` 走通（桥 spawn 真身）
- `ls /var/lib/aginx/stamps` 见 aginx-mem

## 回滚

`aginx-pkg rollback aginx-mem`
