---
name: tool-catalog
description: 所有可用工具和 MCP 服务器的完整目录，用于生成分身时选择工具
tags: [tools, mcp, architecture]
---

# 工具目录

生成分身时，根据需求从以下工具中选择合适的组合。

## 选择原则

**只配两个地方，系统自动推导其余：**

1. **template.json** — 声明 MCP 服务器和插件（基础设施依赖）
2. **flow 的 tools** — 列出该 flow 需要的所有工具（内置 + MCP），不列 core 工具

agent.toml 的 `mcp_servers`、`capabilities.tools` 等**不需要手动配**，系统从 template.json 和 flows 自动推导。

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

## MCP 服务器

MCP 工具在 flow 的 `tools:` 中用 `mcp_{server}_{tool}` 格式声明。对应 MCP 服务器需在 `template.json` 的 `mcp_servers` 中声明。

### wechat-oa — 微信公众号

`mcp_servers: ["wechat-oa"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_wechat_oa_get_access_token` | 获取 access token |
| `mcp_wechat_oa_upload_media` | 上传图片/素材 |
| `mcp_wechat_oa_upload_media_from_url` | 从 URL 下载并上传 |
| `mcp_wechat_oa_create_draft` | 创建草稿文章 |
| `mcp_wechat_oa_get_draft` | 获取草稿内容 |
| `mcp_wechat_oa_list_drafts` | 列出草稿箱 |
| `mcp_wechat_oa_delete_draft` | 删除草稿 |
| `mcp_wechat_oa_publish_draft` | 发布草稿 |
| `mcp_wechat_oa_get_publish_status` | 查询发布状态 |
| `mcp_wechat_oa_list_materials` | 列出永久素材 |
| `mcp_wechat_oa_delete_material` | 删除素材 |
| `mcp_wechat_oa_get_article_read` | 文章阅读数据 |
| `mcp_wechat_oa_get_article_share` | 文章分享数据 |
| `mcp_wechat_oa_get_biz_summary` | 业务概览数据 |

### wecom — 企业微信

`mcp_servers: ["wecom"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_wecom_send_message` | 发送消息 |
| `mcp_wecom_get_msg_chat_list` | 获取会话列表 |
| `mcp_wecom_get_message` | 获取消息记录 |
| `mcp_wecom_bot_generate` | 生成机器人创建链接 |
| `mcp_wecom_get_userlist` | 获取通讯录 |
| `mcp_wecom_get_doc_content` | 获取文档内容 |
| `mcp_wecom_create_doc` | 创建文档 |
| `mcp_wecom_edit_doc_content` | 编辑文档 |
| `mcp_wecom_get_todo_list` | 待办列表 |
| `mcp_wecom_create_todo` | 创建待办 |
| `mcp_wecom_create_meeting` | 创建会议 |
| `mcp_wecom_get_schedule_list_by_range` | 查询日程 |
| `mcp_wecom_create_schedule` | 创建日程 |
| ... 以及更多表格/待办/会议/日程工具 | |

### feishu — 飞书

`mcp_servers: ["feishu"]`

66 个工具，覆盖：消息、文档、表格、多维表格、日历、云盘、通讯录、任务、邮件、音视频、知识库、审批、OKR、幻灯片、妙记、白板、考勤。

常用工具：
| MCP 工具 | 说明 |
|----------|------|
| `mcp_feishu_send_message` | 发送消息 |
| `mcp_feishu_create_doc` | 创建文档 |
| `mcp_feishu_get_doc` | 获取文档 |
| `mcp_feishu_read_sheet` | 读取表格 |
| `mcp_feishu_create_event` | 创建日历事件 |
| `mcp_feishu_send_mail` | 发送邮件 |
| `mcp_feishu_create_task` | 创建任务 |

### browser — 浏览器自动化

`mcp_servers: ["browser"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_browser_navigate` | 导航到 URL |
| `mcp_browser_click` | 点击 |
| `mcp_browser_type` | 输入文字 |
| `mcp_browser_screenshot` | 截图 |
| `mcp_browser_read_page` | 读取页面内容 |
| `mcp_browser_scroll` | 滚动 |
| `mcp_browser_run_js` | 执行 JS |
| `mcp_browser_back` | 后退 |

### twitter — Twitter/X

`mcp_servers: ["twitter"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_twitter_search` | 搜索推文 |
| `mcp_twitter_timeline` | 时间线 |
| `mcp_twitter_post` | 发推文 |
| `mcp_twitter_profile` | 用户资料 |
| `mcp_twitter_like` | 点赞 |
| `mcp_twitter_follow` | 关注 |

### bilibili — B站

`mcp_servers: ["bilibili"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_bilibili_search` | 搜索视频 |
| `mcp_bilibili_hot` | 热门视频 |
| `mcp_bilibili_video` | 视频信息 |
| `mcp_bilibili_comments` | 视频评论 |
| `mcp_bilibili_subtitle` | 视频字幕 |

### zhihu — 知乎

`mcp_servers: ["zhihu"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_zhihu_hot` | 热榜 |
| `mcp_zhihu_question` | 问题和回答 |
| `mcp_zhihu_search` | 搜索内容 |

### xiaohongshu — 小红书

`mcp_servers: ["xiaohongshu"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_xhs_creator_notes` | 笔记列表 |
| `mcp_xhs_creator_note_detail` | 笔记详情 |
| `mcp_xhs_creator_profile` | 账号信息 |
| `mcp_xhs_creator_stats` | 数据概览 |

### reddit — Reddit

`mcp_servers: ["reddit"]`

| MCP 工具 | 说明 |
|----------|------|
| `mcp_reddit_search` | 搜索 |
| `mcp_reddit_subreddit` | 子版块 |
| `mcp_reddit_read` | 读取帖子 |
| `mcp_reddit_comment` | 评论 |

---

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

### 生成分身时使用

分身需要调用外部 API（天气/地图/股票/爬虫等）时，在 files 中包含 `api_tools.toml`，安装时自动注册。

---

## 常见分身类型的工具选择

| 分身类型 | 推荐工具 |
|----------|----------|
| 名人/作家 | `web_fetch`, `web_search` |
| 内容创作 | `web_fetch`, `web_search`, `image_generate` |
| 微信公众号运营 | `web_search`, `mcp_wechat_oa_*` |
| 企业协作 | `mcp_wecom_*` 或 `mcp_feishu_*` |
| 数据分析 | `sqlite_query`, `sqlite_schema`, `web_fetch` |
| 社交媒体管理 | `mcp_twitter_*`, `mcp_bilibili_*`, `mcp_xhs_*` |
| 自动化 | `shell_exec`, `process_*`, `web_fetch` |
| 浏览器操作 | `mcp_browser_*` |
| 数据爬取/量化研究 | `api_tools.toml` + `[tool.cron]` 定时拉取, `sqlite_query` 存储, `web_search` |
| 情报监控 | `api_tools.toml` cron 定时拉取, `cron_create` 定时触发 agent 分析 |

---

- 2026-05-21: 创建 — 基于 tools/TOOLS.md 和各 MCP README 汇总
