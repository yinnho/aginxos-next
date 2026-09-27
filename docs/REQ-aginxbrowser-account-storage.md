# 需求：aginxbrowser 账户会话回放 localStorage/sessionStorage

提出：2026-09-27（账户管理系统只读审查收据）
对象仓：`~/Documents/aginx/aginxbrowser`（禁改仓——本件是需求文档，不是补丁）
状态：待裁决

## 发现

账户记录**落账时**采集了 `local_storage`/`session_storage`，但**建会话时**从不回放。
重启 aginxbrowser 后，同账户新会话只经私有 jar 恢复 cookie；登录态存
localStorage 的站（JWT 型 SPA——代码注释自己点名 xinzao 类）回到登出态。

## 证据链

落账侧（存了）：

- `src/session/manager.rs:460` — `read_login_state` 组记录时带
  `local_storage`/`session_storage`（Storage 读命令 ：1161-1165 采自当前页）。
- `src/session/manager.rs:398-415` — `capture_account` 落账，verify/persona 存活。

回放侧（漏了）——注入机器完整在位，只是没人喂：

- `src/session/manager.rs:819-826` — 落到 start_url **之后**按页 origin 注入，
  注释明写设计意图：「this is what cookies can't carry (the xinzao-class
  login token lives in localStorage, not the cookie jar)」。
- `src/session/record.rs:94-109` — `inject_storage_js` 生成注入脚本。
- 快照重放路径（`manager.rs:231-238`、`:506-513`）已正确接线传 storage。

但四个账户建会话入口 storage 一律 `None`/透传调用方（默认 None）：

- `src/account.rs:287` — verify 的 scratch 会话
- `src/account.rs:492` — login 向导
- `src/curl_import.rs:384` — curl 导入带 account
- `src/mcp/mod.rs:433` — MCP `session_create`（透传 `params.storage`，没人替
  调用方读记录）

## 症状

- **verify 假阴性**：重启后复检 localStorage-token 站，scratch 会话没回放
  storage，已登录账户报 logged_out。
- **登录向导误判**：向导重开时看到登录门，agent 以为要重新人工接力。
- **workflow 断档**：以账户身份跑的内置流（13 个）重启后集体回登出态。

cookie 型站点不受影响。

## 需求

账户会话创建时，若调用方未显式给 storage，从账户记录取
`local_storage`/`session_storage` 走既有注入链（落页后按 origin 注入）。
**显式参数优先于记录**（curl 导入/快照重放等显式路径不被动改）。

## 建议实现（单点接线）

`src/session/manager.rs:690-710` `session_thread` 的 account 分支——
`persona_for`/`jar_for` 已在此分支读记录，同处再读一次记录的 storage：

```
Some((owner, name)) => {
    let persona = ...;
    let record_storage = crate::account::record_storage(owner, name);  // 新 accessor
    ...传给 session_thread 已有的 storage 变量，仅当入参为 None 时采用...
}
```

需在 `src/account.rs` 加一个记录读取 accessor（store 侧 `load_accounts`
已有读路径，零新表零迁移）。快照重放两处（:231/:506）语义不变。

## 边界与注意

1. **origin 错配**：记录里的 storage 是单 origin 平铺 map（采自落账时活页）。
   若新会话 start_url 的 origin ≠ 采集 origin，条目会落错家（无害但无用，
   还可能污染目标站同名键）。**建议同刀**：落账时顺手记
   `storage_origin`（Storage 读命令同点返回 `location.origin` 即可），
   回放仅 origin 匹配才注；v1 可先接受此限制并在文档标注。
2. **凭据纪律不变**：storage 内容与 cookie 同级（服务端 SQLite 记录），
   `AccountSummary` 维持纯元数据——**不得**把 storage 加进任何回显面。
   回放导出（replay_bash）头已有「视同凭据」警告，storage 进导出属同类。
3. **失败非致命**：注入条目碰撞/写失败 = 会话起于登出态（现注释已如此），
   不回滚会话。

## 验收

- 单测（account.rs 既有测试区，参照
  `two_accounts_same_cookie_name_do_not_clobber`）：
  1. 记录带 local_storage → `session_create{account}` → 页内
     `localStorage.getItem` 取回值；
  2. 调用方显式 storage 时记录值**不**覆盖；
  3. 无记录/空记录的账户行为与现状一致（零回归）。
- 活体：任一 localStorage-token 站登录 → 重启 aginxbrowser →
  `session_create{account}` → 登录态存活；`account_verify` 复检 logged_in。
