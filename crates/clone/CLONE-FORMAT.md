# 分身定义层格式规范（CLONE FORMAT）

> 本文是分身定义层格式的**唯一权威规范**，与 Rust 解析器同仓库同 commit 维护。
> 任何修改 `FlowDef` 解析、`scan_flows`、`clone_install` 文件布局的代码变更，
> **必须同批修改本文档**——文档中的示例是 CI 金样本测试的解析对象，文档与代码
> 打架时测试会失败。
> 本文档通过 `include_str!` 种入每个新装分身的 `knowledge/format-spec.md`
> （由 `clone::CLONE_FORMAT_SPEC` 提供）。

## 治理钩子（Governance Hook）

**这是强制流程，不是备忘。** 本仓库的格式正确性由以下钩子保证，违反其中
任一条 = 未完成的变更：

1. **格式变更必须同批改本文档**。改动以下任一位置，commit 必须同时更新本文档
   的对应章节（字段表 / 目录结构 / 示例）：
   - `crates/types/src/flow.rs` — `FlowDef` 结构、`parse_flow_def` 字段
   - `crates/clone/src/manifest_builder.rs` — `scan_flows`、flow 文件发现
   - `crates/clone/src/manifest.rs` — `write_files_to_workspace`、`validate_install_format`
   - `crates/kernel/src/handle.rs` — `clone_install_files` 文件布局/种入

2. **金样本测试是机器闸门**。`types::flow::tests::clone_format_doc_golden_sample`
   用 `parse_flow_def` 解析本文档内嵌的 flow 示例——文档改了示例没跟解析器对齐、
   或解析器改了字段没改文档，CI 都会失败。**不靠人记得改文档，靠测试挡。**

3. **clone-creator 定义层是复述，必须同步**。格式演进后，clone-creator 的
   clone-generate flow 格式段落与 skill-designer/quality-reviewer 子代理要在同一批
   工作中清扫到新格式——否则源头继续产出旧格式分身（"活病人"）。规范的真源只有
   本文档 + 解析器，clone-creator 是翻译层；分身 workspace 里的 knowledge/format-spec.md
   由系统按版本重种，是每个分身可读的活副本。历史遗物 knowledge/agx-format.md
   （.agx tar.gz 归档格式规范）已随文件级格式落地删除，勿再引入。

4. **安装期硬校验兜底**。`validate_install_format` 在 `clone_install_files` 落盘前
   拒收 `skills/` 根目录与缺非空 description 的 flow——即使某个 generator 产出了
   旧格式，也进不了系统（结构化报错供 agent 自修复）。

## 三端管线（分身定义层的流动方向）

```
本地 opencarrier-clones/<name>/   ← 唯一工作区（dup 库，最全）
   │ 修改定义层
   ├─ dup push → 服务器 workspace（carrier.yinnho.cn，运行真源，agent 在这跑）
   └─ dup push → DupHub（duphub.com，分发镜像，用户安装从这来）
```

- 改分身定义层 = 本地改 → push 服务器 → push hub（两推都做）
- **禁止 ssh 上服务器直接改分身文件**——会与 dup 管线 divergence
- 每个分身目录带 `.dup/state.json`（remote_url / remote_base hash，进 git）
- 推送工具用增量 sha256 对比（hub push 字段 `hash`，服务器 `base_hash`；
  重型知识库分身必须增量推，否则撞 nginx 413）

## 顶层目录结构

```
<clone-name>/
├── template.json       # 元数据（必需）
├── profile.md          # 名称、描述、标签（必需）
├── SOUL.md             # 人格定义（推荐）
├── system_prompt.md    # 系统指令（推荐）
├── MEMORY.md           # 知识索引（可选，系统维护）
├── EVOLUTION.md        # 进化策略（推荐）
├── knowledge/          # 知识文件目录
├── flows/              # 流程目录（canonical，见下）
│   └── <flow-name>/
│       ├── flow.md     # ← 流程定义（canonical 文件名）
│       ├── references/ # 详细参考，flow 激活时按需注入（可选）
│       ├── examples/   # 触发示例（可选）
│       └── scripts/    # 可执行脚本，flow 经 shell_exec 调用（可选）
├── agents/             # 子代理目录（可选）
└── style/              # 风格样本（可选）
```

**不包含**：`output/`、`sessions/`、`history/`、`logs/`、`agent.toml`、`AGENT.json`
（运行时数据）；`skills/`（**已废弃**——`scan_flows` 只扫 `flows/`，`skills/`
下的流程定义完全不可见）。

## template.json

```json
{
  "version": "2",
  "name": "clone-name",
  "display_name": "中文显示名",
  "category": "中文分类",
  "description": "一句话描述",
  "author": "作者名",
  "tags": ["tag1", "tag2"],
  "exported_at": "1712736000",
  "knowledge_version": 3,
  "default_flow": "booking",
  "mcp_servers": ["wechat-oa"],
  "plugins": ["wecom"]
}
```

- `version: "2"` — 当前格式版本
- `display_name` / `category` — 中文（分类举例：效率工具/社交媒体/名人对话/
  教育/陪伴/量化研究/营销/销售/内容创作/测试/官方/生活/视频创作）
- `mcp_servers` — **字符串数组**（对象数组会导致整棵 template.json 解析失败，
  所有字段静默丢失）
- `default_flow` — 兜底 flow 名（classify 未命中时使用；不绕过 classify）

## flows/<name>/flow.md — 流程定义（canonical）

文件名**必须是 `flow.md`**（`SKILL.md` 是废弃旧名，仅作兼容回退读取，不再生成）。

```markdown
---
name: flow-name
description: 一句话用途描述——非空必填，空 description 的 flow 不会被注入，声明的工具全部失效
version: 1
tools:
  - file_read
  - file_write
deny_tools:
  - task_plan
shell_allow:
  - "python3 flows/<name>/scripts/*"
  - pattern: "python3 flows/<name>/scripts/render.py"
    match: ["python3 flows/<name>/scripts/render.py --fast"]
    not_match: ["rm -rf /"]
max_iterations: 8
---

# Flow Name

流程正文（markdown）。
```

### frontmatter 字段

| 字段 | 必填 | 说明 |
|---|---|---|
| `name` | ✅ | 流程名（与目录名一致） |
| `description` | ✅ | **单行字符串**。非空必填。写用途与触发场景（≤50字），不写"xxx 技能"这种废描述。⚠️ YAML 块标量（`description: \|` 多行缩进）**不被解析器支持**，会读成字面 `\|`——必须写成单行 |
| `version` | ✅ | 整数，每次修改 +1 |
| `tools` | ➖ | 本流程额外需要的工具白名单（数组） |
| `deny_tools` | ➖ | 本轮禁止的工具（即使 core 工具） |
| `shell_allow` | ➖ | shell_exec 提权的 glob 模式（如 `python3 flows/<n>/scripts/*`）。条目两种形式：纯字符串，或带金样本的 map（`pattern` + 可选 `match`/`not_match`）——装机时硬校验：`match` 示例必须匹配（否则 pattern 过窄/typo）、`not_match` 示例必须不匹配（否则 pattern 过宽），不通过则拒收并给出修复指引 |
| `max_iterations` | ➖ | 声明的轮次上限（软提示 N / 硬掐 N+2，定值留 2~4 轮收束余量） |
| `output` | ➖ | 顶层输出模式；`report` = 最终回复必须是 Ralph report JSON（硬闸门校验） |

未知键（如旧格式的 `when_to_use`）被**静默忽略**——不要使用。

## knowledge/ — 知识文件

简单知识 `knowledge/<topic>.md`：

```markdown
---
name: 标题
source: manual
type: knowledge
description: 一句话描述
tags: [tag1]
confidence: EXTRACTED
status: active
---

正文（1000-3000 字为宜）。

---

- YYYY-MM-DD: 从来源手动创建
```

复杂知识 `knowledge/<topic>/INDEX.md`（<500字摘要）+ `references/`（按需注入）。

## agents/ — 子代理定义

简单 `agents/<name>.md` / 复杂 `agents/<name>/AGENT.md` + `scripts/`。
frontmatter：`name`、`description`（必填）、`tools`、`model`、`color`（可选）。
Flows = "做什么"（操作手册），Agents = "谁来做"（执行实体）。

## 安装期硬校验（clone_install_files）

以下情况安装被拒收（结构化报错回传，agent 修复后重新提交）：

1. 顶层 `skills/` 目录下的文件（废弃路径——迁到 `flows/<n>/flow.md`）
2. `flows/**/flow.md`（或 `SKILL.md`）缺非空 `description`

## 安装后自动种入

`clone_install_files` 会种入（`if !exists`，不覆盖分身自带文件）：

- `flows/self-growth/flow.md` — 自主成长出厂能力
- `knowledge/format-spec.md` — 本规范（跟随二进制版本，由 reconciler 重种刷新）
