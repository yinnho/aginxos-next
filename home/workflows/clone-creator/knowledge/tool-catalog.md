---
name: tool-catalog
description: 所有可用工具与 D17 依赖（api 腿/cli/频道）的完整目录，用于生成分身时选择依赖与工具
tags: [tools, api-legs, architecture]
---

# 工具目录

生成分身时，根据需求从以下工具中选择合适的组合。

## 选择原则

**只配两个地方，系统自动推导其余：**

1. **template.json** — 声明依赖（D17 四元组 `dependencies`：api_legs/clis/channels；旧 mcp_servers/plugins 已退役）
2. **flow 的 tools** — 列出该 flow 需要的所有工具（内置 + 腿工具），不列 core 工具

外部 REST 服务一律走 **api 腿**（声明式工件，见下），不再接 MCP 服务器。

---

## Core 工具（始终可用，无需声明）

| 工具 | 说明 |
|------|------|
| `file_read` | 读取文件内容 |
| `file_list` | 列出目录文件 |
| `tool_search` | 搜索工具目录 |
| `flow_load` | 加载 flow |
| `knowledge_read` | 读取 knowledge |
| `knowledge_list` | 列出 knowledge |
| `session_summarize` | 保存对话摘要 |
| `memory_tree` | 查询用户记忆树 |
| `cron_create` | 创建定时任务 |
| `cron_list` | 列出定时任务 |
| `cron_cancel` | 取消定时任务 |
| `task_plan` | 拆分复杂任务 |

## filesystem

| 工具 | 说明 |
|------|------|
| `file_write` | 写入文件 |
| `file_convert` | Pandoc 格式转换（md, html, docx, pdf） |
| `apply_patch` | 多段 diff patch 精准编辑 |

## shell

| 工具 | 说明 |
|------|------|
| `shell_exec` | 执行 shell 命令 |

## knowledge

| 工具 | 说明 |
|------|------|
| `knowledge_add` | 添加 knowledge |
| `knowledge_remove` | 删除 knowledge |
| `knowledge_lint` | 检查 knowledge 健康 |
| `knowledge_heal` | 修复 knowledge |
| `knowledge_index` | 重建索引 |
| `knowledge_import` | 导入外部数据 |
| `knowledge_extract` | 从对话提取 knowledge |
| `flow_create` | 创建 flow |
| `flow_update` | 更新 flow |
| `clone_evaluate` | 评估 clone 质量 |

## media

| 工具 | 说明 |
|------|------|
| `image_analyze` | 分析图片 |
| `image_generate` | 文生图 |
| `media_describe` | 视觉模型描述图片 |
| `media_transcribe` | 音频转文字 |
| `text_to_speech` | 文字转语音 |
| `speech_to_text` | 语音转文字 |
| `canvas_present` | 展示交互式 HTML |

## process

| 工具 | 说明 |
|------|------|
| `process_start` | 启动长运行进程 |
| `process_poll` | 读取进程输出 |
| `process_write` | 写入进程 stdin |
| `process_kill` | 终止进程 |
| `process_list` | 列出运行中进程 |

## web

| 工具 | 说明 |
|------|------|
| `web_search` | 网页/图片搜索（AginxBrowser 后端；`categories=images` 搜图返回 `image_url` 直链） |
| `web_fetch` | 抓取 URL（GET/POST），HTML 转 Markdown |

## agent

| 工具 | 说明 |
|------|------|
| `agent_send` | 向其他 agent 发消息 |
| `agent_list` | 列出运行中 agent |
| `agent_find` | 搜索 agent |
| `agent_spawn` | 创建 agent |
| `agent_kill` | 终止 agent |
| `agent_restart` | 重启 agent |
| `train_write` | 写入目标 clone workspace |
| `train_read` | 读取目标 clone workspace |
| `train_list` | 列出目标 clone workspace |
| `train_evaluate` | 评估目标 clone |

## misc

| 工具 | 说明 |
|------|------|
| `location_get` | IP 获取位置 |
| `system_time` | 当前时间时区 |
| `user_profile` | 用户画像 |
| `event_publish` | 发布自定义事件 |
| `schedule_create` | 自然语言/cron 定时任务 |
| `schedule_list` | 列出定时任务 |
| `schedule_delete` | 删除定时任务 |
| `task_post` | 发布共享任务 |
| `task_claim` | 认领任务 |
| `task_complete` | 完成任务 |
| `task_list` | 列出任务队列 |

## sqlite

| 工具 | 说明 |
|------|------|
| `sqlite_query` | 只读 SQL 查询（SELECT/PRAGMA） |
| `sqlite_schema` | 列出数据库表和列 |

## a2a

| 工具 | 说明 |
|------|------|
| `a2a_discover` | 发现外部 A2A agent |
| `a2a_send` | 向 A2A agent 发任务 |

---

## MCP 服务器（已退役，D17 换代 2026-10-24）

MCP 面整体退役——外部服务一律走**声明式 api 腿**（`[[tool]]` TOML 工件，duphub 可上架可复用）。旧 MCP 服务器清单（wechat-oa / wecom / feishu / browser / twitter / bilibili / zhihu / xiaohongshu / reddit）归档备查：对应的 `mcp_{server}_{tool}` 工具不再生成、不再声明。

**现役腿先例**：`weixin-oa-api`（公众号发布链：token 链/draft_add/freepublish-submit，duphub kind=api-leg 匿名可拉；素材上传 multipart 走随腿包 cli 件）。铸腿方法见「声明式 API 工具」段与 DESIGN.md §三之二铸造链（读 API 文档 → [[tool]] TOML → 真调验活 → 防幻觉闸）。

## 声明式 API 工具（api_tools.toml）

不需要写 Rust 代码，在 `api_tools.toml` 里声明 HTTP API 工具，agent 直接可用。

### 核心工具

| 工具 | 说明 |
|------|------|
| `api_tool_register` | 运行时动态注册新 API 工具（传入 TOML 定义，立即生效） |

### 配置方式

在 `~/.opencarrier/api_tools.toml`（全局）或 workspace 的 `api_tools.toml`（单个分身）中定义：

```toml
[[tool]]
name = "amap_driving"
description = "驾驶路线规划"
url = "https://restapi.amap.com/v3/direction/driving"
method = "GET"
auth_env = "AMAP_API_KEY"
auth_param = "key"

[tool.params]
origin = { required = true, type = "string", description = "起点" }
destination = { required = true, type = "string", description = "终点" }

[tool.extract]
distance_km = { path = "route.paths[0].distance", transform = "divide_1000_round1" }

[tool.error_check]
field = "status"
expect = "1"
```

### 支持的功能

| 功能 | 字段 | 说明 |
|------|------|------|
| 自动鉴权 | `auth_env` + `auth_param` | 从环境变量读 API key，自动追加为 query 参数 |
| 响应提取 | `extract` + `path` | 从 JSON 响应提取字段，支持 dot-path 和数组索引 |
| 数值转换 | `transform` | 内置：divide_1000_round1, divide_60_round, to_int, round1, round0 |
| 派生字段 | `derived` + `tiers` | 按阈值自动分档（如距离<=50km=市内） |
| 错误检查 | `error_check` | 调 API 后先检查响应是否成功 |
| 参数预解析 | `resolve` | 链式调用：如 driving 先自动 geocode 地名再查路线 |
| 定时拉取 | `cron` | 定时执行不经过 LLM，结果存 SQLite（零 token） |

### 生成分身时使用（D17：腿是独立工件）

分身需要调用外部 API（天气/地图/股票/公众号等）时，**优先复用 duphub 现成 api 腿**（template.json `dependencies.api_legs` 声明，拉取即闭包）；无现成腿才铸新腿（本段格式=铸腿格式参考），铸完真调验活，可上架 duphub 复用。不再把 `api_tools.toml` 内嵌进分身包——腿与分身是两个工件。

---

## 常见分身类型的工具选择

| 分身类型 | 推荐工具 |
|----------|----------|
| 名人/作家 | `web_fetch`, `web_search` |
| 内容创作 | `web_fetch`, `web_search`, `image_generate` |
| 微信公众号运营 | `web_search` + api 腿 `weixin-oa-api`（dependencies.api_legs） |
| 企业协作 | 待铸腿（wecom/feishu 腿未铸；需求出现时走铸造链） |
| 数据分析 | `sqlite_query`, `sqlite_schema`, `web_fetch` |
| 社交媒体管理 | `mcp_twitter_*`, `mcp_bilibili_*`, `mcp_xhs_*` |
| 自动化 | `shell_exec`, `process_*`, `web_fetch` |
| 浏览器操作 | `mcp_browser_*` |
| 数据爬取/量化研究 | api 腿（`[tool.cron]` 定时拉取）, `sqlite_query` 存储, `web_search` |
| 情报监控 | api 腿 cron 定时拉取, `cron_create` 定时触发 agent 分析 |

---

- 2026-05-21: 创建 — 基于 tools/TOOLS.md 和各 MCP README 汇总
- 2026-10-24: D17 换代 — MCP 面退役、api 腿四元组依赖立法（dependencies），weixin-oa-api 首腿在役
