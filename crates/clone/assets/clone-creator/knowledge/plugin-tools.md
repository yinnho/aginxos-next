---
name: 插件工具（Plugin Tools）
source: manual
type: knowledge
description: OpenCarrier 插件系统提供的工具 — 如何在分身中使用插件工具
tags: [plugin, tools, architecture]
confidence: EXTRACTED
status: active
---

# 插件工具

## 概述

OpenCarrier 的插件系统为分身提供了额外的工具能力。插件（plugin）是独立的模块，通过 `plugin.toml` 声明它提供的工具（`[[tools]]`）。当插件被加载后，它声明的所有工具会被注册到全局 `PluginToolDispatcher`，然后根据分身的 `capabilities.tools` 配置决定哪些工具对该分身可用。

## 插件与分身的关系

- **插件** — 技术适配层，负责连接外部平台（如企业微信、飞书、钉钉），处理协议和消息格式
- **分身（Clone/Agent）** — 使用插件提供的工具来完成具体任务

一个分身可以同时使用多个插件的工具，也可以不使用任何插件工具。

## 在分身中使用插件工具

### 1. 声明依赖

在 `template.json` 中添加 `plugins` 字段：

```json
{
  "version": "1",
  "name": "my-clone",
  "description": "...",
  "author": "...",
  "tags": ["..."],
  "exported_at": "1712736000",
  "knowledge_version": 2,
  "plugins": ["wecom"]
}
```

`plugins` 是一个字符串数组，值对应 `~/.opencarrier/plugins/` 下的插件目录名（不含 `opencarrier-plugin-` 前缀）。系统会在安装时检查依赖的插件是否已加载。

### 2. 在流程中引用插件工具

在 flow 的 `allowed_tools` 中直接使用插件工具名：

```yaml
---
name: send-notification
description: 需要通过企业微信发送通知消息时使用
---
```

### 3. 在子代理中引用插件工具

在 agent 的 `tools` 白名单中使用：

```yaml
---
name: wecom-operator
description: 企业微信操作员
tools: ["send_wecom_message", "get_msg_chat_list", "send_message"]
model: sonnet
---
```

### 4. 工具解析逻辑

系统按以下顺序解析分身可用的工具：

1. **内置工具** — `file_read`、`file_write`、`knowledge_search` 等基础工具
2. **流程提供的工具** — flow 中定义的脚本工具
3. **MCP 工具** — MCP 服务器提供的工具
4. **插件工具** — `PluginToolDispatcher` 中注册的工具

如果 `capabilities.tools` 为空或包含 `"*"`，分身获得所有可用工具。如果列出了具体的工具名，只获得列出的工具。

## 已有的插件工具

### 企业微信插件（opencarrier-plugin-wecom）

**Channel**: `wecom_smartbot`

**工具列表**:

| 工具名 | 说明 |
|--------|------|
| `send_wecom_message` | 发送企业微信消息 |
| `wecom_bot_generate` | 生成企微 SmartBot 创建链接 |
| `wecom_bot_poll` | 轮询企微 SmartBot 创建结果 |
| `wecom_bot_qrcode` | 将链接生成二维码图片 |
| `wecom_bot_register` | 注册企微机器人到系统（写入 plugin.toml） |
| `wecom_bot_bind` | 将企微机器人绑定到指定分身 |
| `get_userlist` | 获取通讯录成员列表 |
| `get_doc_content` | 获取文档内容 |
| `create_doc` | 创建文档或智能表格 |
| `edit_doc_content` | 编辑文档内容 |
| `get_msg_chat_list` | 获取会话列表 |
| `get_message` | 拉取会话消息记录 |
| `send_message` | 发送文本消息（MCP） |
| `get_todo_list` | 查询待办列表 |
| `create_todo` | 创建待办事项 |
| `create_meeting` | 创建预约会议 |
| `list_user_meetings` | 查询会议列表 |
| `get_schedule_list_by_range` | 查询日程 |
| `create_schedule` | 创建日程 |
| 以及更多文档/表格/待办/会议/日程相关工具 | |

### 飞书插件（opencarrier-plugin-feishu）

类似的企业协作工具集，具体工具列表参考飞书插件的 `plugin.toml`。

## 典型的插件驱动分身类型

### 机器人创建分身

专门帮助用户在特定平台上创建机器人，并绑定到已有的分身。

**特征**：
- 需要了解目标平台的机器人创建流程
- 引导用户完成授权/扫码等步骤
- 创建完成后将机器人绑定到用户指定的分身
- 需要的工具：平台 API 调用工具 + 文件操作工具

**示例**：企微机器人创建分身
- `plugins: ["wecom"]`
- 流程：引导创建流程、扫码授权、注册机器人、绑定分身
- 知识：企微 SmartBot 创建流程、plugin.toml 格式

### 企业协作分身

使用企业微信/飞书的工具完成日常工作。

**特征**：
- 使用消息、文档、日程、会议等工具
- 可以代替用户操作企业协作平台
- 需要配置对应平台的 credentials

### 客服机器人分身

通过机器人 channel 接收客户消息，使用平台工具回复。

**特征**：
- 绑定到一个平台机器人
- 接收消息并自动回复
- 可以调用平台工具（查询订单、创建工单等）

## 设计注意事项

1. **capabilities.tools 必须完整** — 如果分身的 `capabilities.tools` 不为空且不包含 `"*"`，则每个需要使用的插件工具都必须逐个列出在 `capabilities.tools` 中。仅设置 `plugins = ["wecom"]` 不会自动授权插件工具，必须在 `capabilities.tools` 中显式添加每个工具名。
2. **工具最小化** — 只在 `allowed_tools` 中声明真正需要的插件工具
3. **依赖声明** — 在 `template.json` 的 `plugins` 字段中声明插件依赖
4. **错误处理** — 在系统指令中说明当插件工具不可用时如何处理
5. **知识支撑** — 如果分身需要引导用户完成某个流程（如创建机器人），需要在 knowledge 中提供详细的流程说明

---

- 2026-04-28: 手动创建 — 支持插件工具感知的分身设计
