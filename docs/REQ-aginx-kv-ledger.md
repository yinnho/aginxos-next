# REQ：aginx 网关 kv 流水账与人格真源分家（刀④-5 差异记录）

- 日期：2026-10-05
- 需求方：aginxos-next（本仓刀④-5 人格真源一等件）
- 收件方：aginx 生态仓（gateway 守护，~/Documents/aginx/aginx）
- 性质：需求文档（本仓不碰生态仓源码；设备侧运维动作不受限）

## 背景

刀④-5 已把 carrier.db 的 kv 人格域导出成文件树真源：

- `/home/knowledge/*.md`（22 条）+ `/home/MEMORY.md` 真填——人格真源
  归属文件树（FS.md 世界观，用户裁决 C：db 封存为历史账）；
- 敏感条目（电话簿/账号）走 secretd，scope `persona.*`；
- 导出器=本仓 `aginx-mem persona` 子命令（crates/aginx-mem）。

## 现状差异

网关对话时仍向 carrier.db `kv_store` 写人格类键（`entity.misc`、
`preference.general`、`session_compaction.*`、`loop_state:*`）。这些
写入在文件树真源确立后成了**无主流水账**：导出后 db 新增行没有归宿，
文件树也不会自动跟上——两边必然漂移。

## 需求

1. **归属声明**：网关文档（生态仓 README/AGENTS 层面）明确
   `kv_store` 的人格域键是流水账、不是真源；真源在
   `{AGINX_HOME}/knowledge/` + `MEMORY.md`。
2. **可选落账钩**（低优先）：网关在写 `session_compaction.*` 等沉淀类
   键时，除 db 外同步 append/更新文件树（或提供 post-write hook，
   让 `aginx-mem` 侧接管落盘）。做不到也不阻塞——`aginx-mem persona`
   可随时重跑合并导出（幂等，多身份三元分节渲染）。
3. **不要**在网关里读 `/home/knowledge/` 做路由或权限判断——知识树
   是 codex 工位（folder=/home 原生拾取 AGENTS.md/MEMORY.md）的
   领地，网关只管名册与投递。

## 交叉引用

- 本仓收据：docs/HARDWARE.md #443（仅本地）
- 世界观：docs/DESIGN.md §二/§三、docs/FS.md
- 导出器：crates/aginx-mem/src/persona.rs
