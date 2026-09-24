---
name: clone-generate
description: 创建新的 AI 分身--收集需求、staging 逐文件生成定义层、[CLONE_INSTALL] 标记安装上线、clone_publish 推送 DupHub
tools: ["clone_evaluate", "file_write", "file_read", "file_list", "shell_exec", "clone_publish", "clone_export"]
version: 2
---
# 分身生成流程

当用户表达了创建分身的意图时，执行以下流程：

## 流程

### 1. 需求收集

通过对话了解以下信息（不必一次问完，可以分步）：

- **分身名称**：英文短横线格式（如 customer-support）
- **用途描述**：一句话说清楚这个分身做什么
- **分身类型**：角色型（律师/心理咨询师/客服...）、公众型（对话马斯克/刘震云...）、人格型（克隆用户自己/好友/前任）
- **目标场景**：在什么场景下使用
- **人格特征**：什么性格、什么沟通风格
- **知识领域**：需要了解什么领域的知识
- **流程列表**：需要哪些能力（每个流程有触发条件）
- **插件依赖**：是否需要连接外部平台（如 wecom、feishu）
- **MCP 依赖**：是否需要 MCP 服务器（如 wechat-oa）
- **API 工具依赖**：是否需要调用外部 REST API（如地图、天气、股票数据）。如果有，生成 api_tools.toml 配置文件
- **进化策略**：保守/积极/关闭（默认保守，公众型分身默认积极，人格型分身强制保守）

### 2. 文件生成（staging 逐文件写入）

信息收集完毕后，**逐个文件**用 `file_write` 写入 `staging/<clone-name>/`（相对路径，落在你自己的 workspace 里）。一次 `file_write` 只写一个文件——写一个落一个盘，制作中断也不丢。

**写入顺序**：先 template.json（对齐 name/display_name/default_flow），再身份层（SOUL.md / system_prompt.md / profile.md），再知识层，再流程层，最后 MEMORY.md / EVOLUTION.md。

**续作规则（每次生成开工必做）**：

1. 先 `file_list("staging/<clone-name>/")` 看半成品清单
2. 如果已有半成品（上次中断/超时/被压缩），`file_read` 只读 template.json 和 SOUL.md 对齐名称与人格锚点，**不要全量重读**
3. 只写缺失或需要修改的文件，绝不从零重做
4. 全新生成时（staging 为空）不读直接写

需要准备的文件清单：

- **SOUL.md**（必需）：人格定义
- **system_prompt.md**（必需）：行为指令
- **profile.md**（可选）：基本信息
- **MEMORY.md**（可选）：初始知识索引
- **EVOLUTION.md**（推荐）：进化策略
- **knowledge/*.md**：简单知识文件（路径以 `knowledge/` 开头）
- **knowledge/{topic}/INDEX.md**：复杂知识目录格式（含 `references/`）
- **flows/{name}/flow.md**：流程文件（目录格式，canonical 文件名必须是 flow.md，不是 SKILL.md）
- **agents/{name}.md**：简单子代理（路径以 `agents/` 开头，可选）
- **agents/{name}/AGENT.md**：复杂子代理（目录格式，可选）
- **style/*.md**：风格文件（路径以 `style/` 开头，可选）

#### 知识文件格式（严格遵守）

**简单知识**：`knowledge/<topic>.md`

```markdown
---
name: <标题>
source: manual
type: knowledge
description: <一句话描述>
tags: [<tag1>]
confidence: EXTRACTED
status: active
---

<知识正文内容>

---

- YYYY-MM-DD: 从用户需求手动创建
```

**复杂知识**：`knowledge/<topic>/INDEX.md` + `references/`

INDEX.md 包含摘要（< 500 字），始终加载；references/ 包含详细参考，按需注入。

关键：
- `source: manual` — 因为是 clone-creator 手动创建的
- `confidence: EXTRACTED` — 手动编写的知识是直接提取的事实
- `description` — 一句话概括内容
- 第二个 `---` 分隔符 — 分隔编译层和时间线

#### 公众型分身专属流程（原名人型）

如果分身类型是"公众对话"（如马斯克、刘震云等公众人物），按以下流程生成：

**阶段一：研究（6维并行搜索）**

使用 `搜索` 工具对目标人物进行6维度研究（参考 `knowledge/celebrity-distillation.md`）：

1. 著作与文章 → 核心观点、思想体系
2. 长对话与演讲 → 即兴思考方式、真实表达风格
3. 表达DNA → 句式、词汇、节奏、幽默、确定性
4. 外部评价 → 关键争议、他人如何看待
5. 重大决策 → 决策模式、背后逻辑
6. 时间线 → 生平关键节点、思维演化

每个维度至少搜索2次，将研究结果整理到对应 references 文件中。

**阶段二：提取（三重验证）**

根据 `knowledge/extraction-framework.md` 的方法论：
- 提取3-7个心智模型（必须通过跨域复现+可生成性+独占性验证）
- 提取5-10条决策启发式（只通过1重验证的降级到此）
- 捕捉至少2个内在张力（价值观内在冲突）
- 量化表达DNA（句式指纹+风格标签+禁忌词/口癖）

**阶段三：构建（文件生成）**

```
<clone-name>/
  # 身份层（冻结）
  template.json           ← category: "公众对话", knowledge_version: 3
  profile.md              ← 核心身份 + 标签
  SOUL.md                 ← 视角摘要 + 说话风格 + 价值观 + 内在张力 + 禁忌（摘要+指针）
  MENTAL-MODELS.md        ← 心智模型详解（3-7个：一句话/证据/应用/局限）
  DECISION-HEURISTICS.md  ← 决策启发式（5-10条：规则/场景/案例）
  EXPRESSION-DNA.md       ← 表达DNA详解 + 经典句式速查 + 中文适配表
  TIMELINE.md             ← 人物时间线 + 智识谱系
  system_prompt.md        ← 身份锚定 + 回应规则 + 话题深度映射 + 诚实边界

  # 知识层（可进化）
  knowledge/
    iconic-quotes.md      ← 经典语录（每条标注来源）[必选]
    [topic-1].md          ← 专题1 (1000-3000字，标注来源)
    [topic-2].md          ← 专题2
    ...                   ← 5-8 个专题文件

  # 流程层
  flows/
    <name>-voice/
      flow.md             ← 主流程 + Agentic Protocol + 回应规则 + 质量自检

  # 运行时
  MEMORY.md
  EVOLUTION.md            ← aggressive 模式（身份层冻结，知识层可进化）
```

知识文件使用标准 frontmatter（`source: distillation`）。

**阶段四：验证（3项快速测试）**

1. **已知立场测试**：问3个此人有公开立场的问题，回答方向是否一致
2. **边缘问题测试**：问1个此人不熟悉的话题，是否诚实说"不懂"
3. **语气测试**：写100字回复，是否有辨识度、不像通用AI

通过验证后进入安装阶段。

#### 人格型分身专属流程（新增）

如果分身类型是"人格克隆"（基于用户提供的聊天记录、日记、社交媒体等私有数据，克隆用户自己、好友、前任等真实个体），按以下流程生成：

**触发关键词判断**：
- "把我做成一个分身"、"克隆我自己"、"复刻我前任"、"模拟我好友"
- 用户上传聊天记录、日记、社交媒体导出文件
- 用户提到"微信聊天记录""QQ记录""日记"等数据源

**阶段一：数据收集与预处理**

1. **确认授权与伦理声明**（必须先完成，不通过不能继续）：
   - 向用户确认："你拥有这些数据的使用权吗？"
   - 明确告知："这是 AI，不是真人。此分身仅你本人可用。"
   - 说明："你可以随时一键删除所有数据。"

2. **数据格式标准化**：
   - 微信/QQ 聊天记录导出 → 统一为 `时间 | 说话人 | 内容` 格式
   - 日记/笔记 → 保留时间戳，按主题分段
   - 社交媒体 → 提取纯文本，去除平台无关信息

3. **数据质量评估**：
   - < 1000 轮对话：提示"数据量较少，人格还原度可能有限"
   - 1000-10000 轮：理想范围
   - > 10000 轮：均匀采样，避免时间分布偏差

4. **清洗过滤**：
   - 过滤单字回复、纯表情、系统提示
   - 敏感信息标记（手机号、地址等标注为 `[SENSITIVE]`，提醒用户确认）

**阶段二：人格提取（调用 personality-extractor 子代理）**

根据 `knowledge/personality-extraction.md` 的方法论，从清洗后的数据中提取：

- **表达DNA**：平均句长、疑问句比例、高频词汇 Top 50、口头禅、标点特征、表情符号模式
- **认知特征**：决策风格、归因模式、价值观关键词
- **情感模式**：情绪表达频率、情绪调节方式、冲突应对风格
- **关系模式**：亲密称呼习惯、求助与提供支持比例、边界感特征
- **内在张力**：从矛盾表达中自动推断（如既说"想独处"又说"害怕孤独"）

提取原则：只保留**统计显著**的特征（出现频率 > 1% 或跨场景重复），标注置信度（HIGH / MEDIUM / LOW），不编造数据中没有的特征。

**阶段三：构建（文件生成）**

```
<personality-clone-name>/
  # 身份层（冻结，evolution_mode 强制 conservative）
  template.json           ← category: "人格对话", knowledge_version: 3
  profile.md              ← 核心身份 + 数据来源声明 + 授权确认
  SOUL.md                 ← 视角摘要 + 说话风格 + 价值观 + 内在张力 + 禁忌
  PERSONALITY-PROFILE.md  ← 人格提取报告（表达DNA + 认知 + 情感 + 关系模式）
  EXPRESSION-DNA.md       ← 表达DNA详解 + 高频句式速查 + 口癖列表
  TIMELINE.md             ← 可选（如有日记时间跨度）
  system_prompt.md        ← 身份锚定 + 回应规则 + 伦理边界（AI声明 + 健康边界 + 删除机制）

  # 知识层（可补充，但人格层冻结）
  knowledge/
    iconic-moments.md     ← 经典对话片段（标注来源时间）[必选]
    relationship-context.md ← 关系背景（此人与用户的关系历史）[必选]
    [topic-1].md          ← 高频话题专题
    ...

  # 流程层
  flows/
    <name>-voice/
      flow.md             ← 主流程 + 伦理边界协议（AI身份声明 + 情感边界 + 拒绝过度依赖）

  # 运行时
  MEMORY.md
  EVOLUTION.md            ← conservative（人格层冻结，只允许知识层补充事实）
```

知识文件使用标准 frontmatter（`source: user_data`，`confidence: EXTRACTED`）。

**阶段四：伦理审查（强制）**

由 quality-reviewer 执行，必须全部通过：
1. **授权声明**：profile.md 中是否明确"基于用户提供的私有数据创建，仅用户本人有权使用"
2. **AI 身份声明**：system_prompt.md 中是否有"我是 AI，不是真人"的强制声明
3. **情感边界**：flows/*/flow.md 中是否有"健康边界提示"（用户过度依赖时提醒）
4. **删除机制**：EVOLUTION.md 或 system_prompt.md 中是否说明"用户可以一键删除所有数据"
5. **第三方 consent**：如果数据源涉及非用户本人的第三方，是否有 consent 声明

有任意一项未通过，不能进入安装阶段。

**阶段五：验证（2项测试）**

1. **语气还原测试**：让分身回复3个日常话题，与用户提供的原对话风格对比，相似度 > 70%
2. **边界测试**：问1个超出数据范围的问题，分身是否诚实说"这我不了解"

通过验证后进入安装阶段。

#### 流程文件格式

`flows/<flow-name>/flow.md`（⚠️ 文件名必须是 flow.md；SKILL.md 是废弃旧名）：

```markdown
---
name: <流程名>
description: <一句话用途描述，非空必填——空 description 会导致 flow 不被注入，工具全失效>
version: 1
---

<流程 prompt 正文，核心流程 < 2000 字>
```

可选子目录：
- `flows/<flow-name>/references/` — 详细参考文档，按需注入
- `flows/<flow-name>/examples/` — 触发示例，帮助判断何时使用
- `flows/<flow-name>/scripts/` — 可执行脚本

#### EVOLUTION.md 格式

```markdown
---
evolution_mode: conservative
max_knowledge_files: 200
knowledge_capacity_mb: 50
auto_compile: true
compile_interval_hours: 24
bloat_stale_days: 30
bloat_delete_days: 60
feedback_to_hub: false
---

## 进化规则
- <根据分身类型定制的知识提取规则>
```

根据分身类型调整：
- 客服分身：`conservative`，只提取事实性知识
- 技术分身：`conservative`，提取技术方案和最佳实践
- 销售分身：`conservative`，提取话术和异议处理
- 研究/创意分身：`aggressive`，广泛提取相关知识

### 3. 安装（发 [CLONE_INSTALL] 标记）

**没有 clone_install 工具——安装由系统在轮末接管。** 全部文件写进 `staging/<clone-name>/` 后，在**最终回复的正文**里发这个标记（一行，独立成段，原样照抄）：

```
[CLONE_INSTALL:<clone-name>]
```

标记必须是回复文本的一部分，不是任何工具的参数。系统在轮末自动完成：

1. 读取 `staging/<clone-name>/` 全部文件
2. 格式校验（缺 description、skills/ 根目录等硬闸门）
3. 建 workspace、写文件、启动分身 agent
4. 把标记替换成安装回执（成功 ✅ / 失败 ⚠️）

**失败自修复**：回执会列出全部校验错误。staging 原样保留——按错误修复对应文件（file_write 覆盖即可），然后在回复里重发 `[CLONE_INSTALL:<clone-name>]`。绝不动 staging 里没问题的文件。

**同名重装**：标记安装同名分身 = 重装（旧 agent 停掉、workspace 重写，`.dup/` 历史保留）。改版流程照常走 staging + 标记。

template.json 关键字段（生成时参考，文件在 staging 里）：

```json
{
  "version": "2",
  "name": "<clone-name>",
  "display_name": "<中文显示名>",
  "category": "<中文分类>",
  "description": "...",
  "author": "...",
  "tags": ["..."],
  "exported_at": "...",
  "knowledge_version": 3,
  "default_flow": "<首个flow名>",
  "mcp_servers": ["wechat-oa"],
  "plugins": ["wecom"]
}
```

`mcp_servers`/`plugins` 只在需要时加。

### 4. 安装后验证（下一轮）

回执确认安装成功后，**下一轮对话**（用户回复任意内容时）执行：

使用 `clone_evaluate` 工具评估分身质量得分。

### 5. 发布到 Hub（安装成功后）

安装成功后，使用 `clone_publish` 发布到 Hub（此时分身已真���落盘，publish 读得到）：

```json
{
  "name": "<clone-name>"
}
```

系统会自动：
1. 打包分身定义层文件（文件级，非归档）
2. 使用配置的 Hub API Key 上传到 Hub
3. 返回 Hub 上的模板 ID

前提：需要在 config.toml 配置 `[hub]` 的 `url` 和 `api_key_env`，并设置对应的环境变量。

## 导出已有分身

如果用户要求导出已安装的分身，使用 `clone_export` 工具：

```json
{
  "name": "<clone-name>"
}
```

返回定义层清单（文件路径、总大小、state hash）。

## 生成规则

- profile.md 必须有 YAML frontmatter
- SOUL.md 用自然语言描述人格，**不包含**工作规则
- system_prompt.md 是最关键的文件，要详细且可操作
- 流程的 description 写用途不写触发条件，50字以内
- 知识文件按主题拆分，每个 1000-3000 字为宜
- 所有知识文件使用双层格式（frontmatter + 正文 + `---` + 时间线）
- 手动创建的知识 confidence 设为 EXTRACTED
- 推荐生成 EVOLUTION.md，至少指定 evolution_mode 和进化规则
- template.json 必须包含 `version: "2"`、`display_name`（中文名）、`category`（中文分类）、`knowledge_version: 3`，有流程时加 `default_flow`
- **每个 flow 的 frontmatter `description` 必须非空**（写用途不写触发条件，50字以内）——空 description 的 flow 不注入，等于白做
- 如分身需要 MCP 服务器，template.json 中添加 `mcp_servers` 字段
- **如分身需要调用外部 API，生成 `api_tools.toml` 文件**（声明式 API 工具，无需写代码）
  - 支持自动鉴权、响应提取、定时拉取（cron 存 SQLite）、参数预解析（resolve 链式调用）
  - 格式参考 tool-catalog.md 的声明式 API 工具部分
- 如分身需要并行处理或多角色协作，生成 agents/ 目录
- Skills = "做什么"（操作手册），Agents = "谁来做"（执行实体），两者不要混淆

## 文件操作效率规则

- **绝不先读后写（全新生成时）**：staging 为空、从零生成时不要 file_read——你已经知道分身的定位和风格，直接写
- **续作半成品时只读锚点**：staging 已有文件时，只 file_read template.json 和 SOUL.md 对齐名称与人格，其余文件看 file_list 清单补缺，不全量重读
- **用户说"直接写"时**：立即调用 file_write，零次 file_read
- **避免冗余 file_read**：确认文件存在用 file_list，不用 file_read
- **一次只做一件事**：收到"写入2个参考文件"→ 只写入2个文件，不做其他操作
- **不要反复确认**：写入后不需要再 file_read 验证内容
- **绝不攒批**：不要把多个文件内容攒在一次回复/一个工具调用里——一次 file_write 一个文件，写完一个是一个