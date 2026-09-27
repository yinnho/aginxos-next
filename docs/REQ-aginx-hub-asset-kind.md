# 需求：DupHub 资产 kind 字段——一份协议，三种货

提出：2026-09-27（aginxbrowser 批222 flow_install 落地后的生态缺口）
对象仓：`opencarrier/aginx-hub`（服务端，86quan `/data/www/aginx-hub`）；
连带消费方：`aginx-carrier/crates/dup`、`aginx-carrier/crates/clone`（与
aginxos-next/crates/dup 同源双供养）
状态：待裁决

## 背景

dup 协议是内容无关的：一个 template 就是一棵带 sha256 的文件树。
这个设计让 aginxbrowser 的 flow_install（批222，20bc461，已上线）**零协议
改动**就能从 DupHub 拉 flow 包安装。但代价是 hub 不知道树里装的是什么——
`templates` 表里分身和 flow 包是同一种"模版"，市场层无法区分。

## 现状证据

- `migrations/001_init.sql:3` — `templates` 表：name/description/tags/
  category/…，无任何类型字段。category 是自由文本，不是机器枚举。
- `src/handlers.rs:155` — `GET /api/templates?q=&tag=&category=&page=&limit=`，
  SearchQuery 无 kind 过滤。
- `src/handlers.rs:1399` — `dup_push`：payload `{files, hash, changelog?,
  visibility?}`；模板行首次 push 时从树内 `template.json` 抄元数据
  （db.rs:139 INSERT），template.json 缺失走 unwrap_or_default——**树形
  本身携带着类型信号（根上有无 flow.json），服务端现在扔掉了它**。
- `src/handlers.rs:1330/1347` — dup manifest/file 匿名拉取端点，aginxbrowser
  flow_install 正在用（`AGINXBROWSER_DUPHUB_URL` 默认 duphub.com）。
- 消费侧：`aginx-carrier/crates/clone/src/hub.rs:155` search_templates 把
  结果格式化成"模版"表（名称/版本/下载/评分/描述）——语义上默认全是分身。

## 症状（第一个消费方已在线）

aginxbrowser agent 想装 flow，得先**知道名字**——flow_install 只吃 name，
没有发现面。正确姿势是 `GET /api/templates?kind=agx-flow` 列出可装的流，
但这个过滤今天不存在；不做的话 agent 只能拉全量列表自己猜哪些是 flow
（连猜的依据都没有——列表里没有 kind）。分身市场同理：flow 包一旦上架，
手机端"模版市场"会把 `xhs-post` 这种 flow 和分身混排。

## 需求

### 1. 字段（migration 015）

```sql
ALTER TABLE templates
    ADD COLUMN IF NOT EXISTS kind VARCHAR(20) NOT NULL DEFAULT 'clone';
```

- 枚举 v1：`clone`（分身，现状语义）| `agx-flow`（根带 flow.json 的包，
  aginxbrowser flow_install 消费）| `agx-template`（通用文件树模板，
  保留位——render 模板类，先注册枚举不做校验）。
- **默认 clone = 全量向后兼容**：存量行一次性语义不变；老客户端不送 kind
  推上来的也归 clone。
- 未来 kind 只增不改；未知值写入侧直接 400（枚举收紧在服务端，不靠
  serde 宽容）。

### 2. 写侧（dup_push）

kind 来源两级，冲突即拒：

1. payload 显式 `kind`（可选字段，老客户端不送）；
2. 缺省时**从树形推导**：根含 `flow.json` → `agx-flow`；否则 `clone`。

服务端校验（这是把 aginxbrowser install 侧的装前门镜像到上架门，垃圾
进不了市场）：

- `agx-flow`：树根必须有 `flow.json` 且解析后带 `steps` 数组，否则 400；
  显式 kind=agx-flow 但树里没 flow.json → 400；树里有 flow.json 但显式
  声明 clone → 400（自相矛盾）。
- `clone`：现状不动（template.json 仍可选）。
- `agx-template`：v1 无结构校验。

推导优先意味着 **flow 发布者零客户端改动**：dup push 一棵 flow 树，
自动归类。dup CLI 后续可加 `--kind` 覆盖（仅 agx-template 需要显式声明）。

### 3. 读侧

- `GET /api/templates` 增 `kind` 过滤参数（`?kind=agx-flow`）；
  TemplateSummary / TemplateDetail 响应项各加 `kind` 字段。
- **缺省 kind 过滤 = 不过滤（返回全部）**。理由：上线那一刻全表都是
  clone，缺省行为实际不变；等第二个 kind 有货时，分身侧 UI（手机模版
  市场、clone crate 的 search）在同窗口内显式加 `kind=clone`——比
  "缺省=clone、新 UI 漏过"更稳，旧调用永远不突变。
- dup manifest 响应可顺手带 `kind`（advisory；消费方以列表为准，不依赖）。

### 4. 消费方动作清单（aginxos 侧认领）

| 消费方 | 动作 | 优先级 |
|---|---|---|
| 手机模版市场 UI / clone crate search | 显式 `kind=clone` | 第二 kind 上架前 |
| aginxbrowser flow 发现 | （aginxbrowser 仓自理）`?kind=agx-flow` 列表面 | hub 上线后 |
| dup CLI | `--kind` 覆盖 + templates scope 推导提示 | 低 |

## 非目标

- 定价/可见性按 kind 分治——现机制统一适用，不碰。
- plugins / mcp_servers / releases 表——本来就是独立表独立面，不动。
- dup 协议 wire（manifest/file 端点）——内容无关是设计，kind 活在市场层。

## 开放问题（裁决点）

1. 缺省过滤语义：本 REQ 取"缺省=全部"（理由如上 3）；若手机端 UI 改造
   赶不上第二 kind 上架，翻转成"缺省=clone"是一行 SQL 级决策，两版都
   预留。
2. `agx-` 前缀 vs 裸名（`flow`/`template`）：本 REQ 取 `agx-` 前缀——
   留出非 agx 资产的命名空间，代价是字符串长一点。
3. flow 包的评分/下载计数是否与分身混榜：v1 建议混（同一个市场），
   榜单按 kind 分列是 UI 层后话。
