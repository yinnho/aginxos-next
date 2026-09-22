//! Default definition-layer content seeded into every new clone.
//!
//! Currently: the `self-growth` flow — the clone's factory-baked autonomous
//! learning/creation capability. Seeded by `clone_install_files` (kernel) when
//! the clone doesn't ship its own `flows/self-growth/flow.md`.

/// The default `self-growth` flow, written to `flows/self-growth/flow.md` at
/// clone birth unless the clone ships its own.
///
/// Generic by design: the clone derives its own domain from its `knowledge/`
/// and identity files, so the same flow works for 86巴士, a calligraphy clone,
/// a writer clone, etc. The cron message (set by the reconciler) tells each
/// turn whether to run `mode=learn` (all enabled clones) or
/// （create 分支已随公众号渠道退役移除。）
///
/// The prompt is deliberately **restrictive**: self-growth may only ADD new
/// knowledge gained from external research, never edit existing files. This
/// prevents the "audit-and-rewrite" failure mode where the agent mistakes
/// "learn" for "fix my knowledge base".
///
/// `max_iterations: 12` is measured, not guessed: observed learn rounds need
/// 8+ iterations (search → read → knowledge_add → log append), and the
/// declared cap hard-stops at N+2 — a 6 was observed killing a healthy run
/// at wrap-up (2026-08-14).
pub const DEFAULT_SELF_GROWTH_FLOW: &str = r#"---
name: self-growth
description: 自主成长（空闲时自动学习）。由 self-growth cron 触发。学习=去网上搜本领域新信息→追加新知识；绝不整理/修复/改写既有文件。严格遵守下方铁红线。
version: 2
max_iterations: 12
tools: [system_time, web_search, web_fetch, knowledge_list, knowledge_read, knowledge_add, file_read, file_write]
---

# 自主成长

系统消息给了你 `mode=learn`：本轮**学习**——去网上搜你领域的新信息，追加成新知识。
（发布/创作模式已随公众号渠道退役移除；需要时随 OA 渠道回归再恢复。）

## 🚫 铁红线（绝对禁止，违反即失败）

1. **学习 ≠ 整理/修复/审计**。学习是**去网上**（`web_search`/`web_fetch`）搜你领域里你还不知道的**新**信息。**不是**读现有 knowledge 去改进它、合并它、修过时内容——那些一律禁止。
2. **只能新建 knowledge，绝不改既有**。往知识库加东西**只能用 `knowledge_add` 新建文件**。**绝不修改、重写、重命名、删除**任何既有 knowledge 文件（哪怕你觉得它过时/有错——那不是自主成长该干的，留给人工）。
3. **`file_write` 只许写一个地方**：
   - `flows/self-growth/log.md`（成长日志，追加一行）
   **严禁**写任何其他路径。**尤其严禁**：改 `flows/` 下任何文件、改任何既有 flow（如 daily-admin-brief）、`flow_update`、改 `knowledge/` 下任何文件、改 SOUL/system_prompt/EVOLUTION 等身份/配置文件。
4. **读 knowledge/ 只读不改**。读它只为两件事：(a) 搞清你的领域好搜对关键词；(b) 判断搜到的新信息是不是已经有了。**绝不为"改进"而读。**
5. 不编造、不灌脏、不重复（查 log + knowledge 去重）。

## 学习分支（mode=learn）

1. 用 `system_time` 取今天日期。
2. 读 `knowledge/`（`knowledge_list`+`knowledge_read`，**只读**）+ 身份文件搞清**你的领域**（如：86巴士出行客服 / 书法老师），只为搜对关键词。
3. 读 `flows/self-growth/log.md`（`file_read`，不存在当空）看之前学过啥，避免重复。
4. 据领域用 `web_search`/`web_fetch` 搜**最新外部信息**（行业动态、政策、新联系方式、时效内容）。
5. 每条搜到的过三闸：**相关**（你领域）+ **新颖**（knowledge 和 log 里没有）+ **可靠**（来源明确）。三闸全过才用 `knowledge_add` **新建**知识文件。
6. 搜不到有用的就如实记"无新知"，**不要硬凑、不要改既有文件**。
7. 追加一行到 `flows/self-growth/log.md`（`file_write` 追加，格式 `- YYYY-MM-DD 学: <一句话或"无新知">`）。

## 输出

- 回复简述本轮学了啥（1-3 行）。
- 不要调 message_push/send 类工具，不要给用户推消息。
"#;

/// The clone format spec, seeded into every new clone's `knowledge/format-spec.md`.
///
/// Source of truth is `docs/CLONE-FORMAT.md` in this repo — the runtime parser
/// (`carrier_types::flow::parse_flow_def`, `manifest_builder::scan_flows`) is the
/// legislator, this doc is its published translation. Keeping it `include_str!`
/// from the repo doc means the spec ships with the binary version: upgrading
/// opencarrier upgrades every clone's spec (via the reseeding reconciler),
/// without any hub round-trip.
///
/// A golden-sample test parses the flow example embedded in this doc with
/// `parse_flow_def`; if the doc and the parser disagree, CI fails.
pub const CLONE_FORMAT_SPEC: &str = include_str!("../../../docs/CLONE-FORMAT.md");

/// Marker stamped at the top of the seeded spec file so the reseeding
/// reconciler can tell "system-seeded spec" (version-tracked, may overwrite)
/// from "clone-authored file" (never touched). Bump when the spec changes
/// materially — the reconciler only reseeds workspaces whose stamped version
/// is older, so an edited doc WITHOUT a bump never reaches already-seeded
/// clones.
pub const CLONE_FORMAT_SPEC_VERSION: &str = "v3";
