# DupHub 设计真源（2026-10-25 立 · 同日 D18 大修）

DupHub=「改变**系统=这个人**的件」的市场。本页是它的设计真源：定位、
协议、工件分类、分工与挂账。世界观上层见 `docs/DESIGN.md`（§三之二
能力积木）；机上树见 `docs/FS.md`。

## 〇、D18 五条立法（2026-10-25 下午，owner 口述定案）

1. **消费者是 agent，不是人。** duphub 里的一切商品都是给 agent
   用的；搜索、评估、下单、购买、拉取、集成、执行、回报——全链路
   agent 的 API 动作。人只有两个动作：**扫码付钱**、**收 agent 的
   汇报**。一切接口按机器友好设计（结构化 manifest、结构化错误、
   错误里带下一步动作）。
2. **任何工件都可能是用户提供的。** 没有平台货/用户货之分：分身、
   api 腿、频道腿、flow、面板模板、pkg 发行——每一类任何用户都可以
   作者上架（dup push / pkg upload），先审后卖。
3. **任何工件都可能收费。** 定价/购买/分账是市场面的横切能力，对
   所有渠道（dup/pkg）统一：price=0 匿名自由拉；price>0 过 purchase
   门；成交即分账（作者 70 / 平台 30，见 §五b）。
4. **注册/登录/下单都在 agent 端完成。** api_key（=~/.dupconfig 里的
   key）就是用户的"码"——agent 视角是 Bearer 凭证，主人视角是一串
   字符。付真钱时 hub 返回**支付二维码**，agent 用 diting `/open`
   的 qr 面板模板展示给主人，主人扫码付款，agent 轮询订单状态后
   自主继续（见 §五c）。
5. **diting 的双重身份。** diting 既是 dup 市场的**消费者**（flow
   拉进内存执行、面板模板进货到盘上层），也是 pkg 渠道的**被分发
   对象**（本体更新走 `diting update` ← pkg 签名 manifest）。

## 一、定位（D17 分发分工 + D18 泛化）

| 渠道 | 卖什么 | 节奏 |
|---|---|---|
| **duphub.com（dup）** | 改变**人**的件：分身、api 腿、频道门腿、**flow、面板模板**（D18） | 市场节奏：agent 自己拉、热插拔、删目录即拔 |
| pkgs.aginx.net（pkg） | 改变**机器**的件：机身二进制、服务单元、**引擎真身（diting）**、**用户发行的软件**（D18） | 平台发版节奏：签名 manifest |

供应链不在四元组（api/cli/flow/channel）内——duphub 是四类积木的
进货渠道。设计意图（DESIGN.md §三之二）：第二台机器遇到同一个 API
直接拉腿，不重铸。

## 二、三方结构（D18 扩：agent 是买方，diting 是执行手）

| 方 | 件 | 位置 |
|---|---|---|
| VCS 客户端 | `dup` CLI | 本仓 `crates/dup`（与 aginx-carrier/crates/dup 同源双供养；上游=opencarrier-clones 移植） |
| 市场服务端 | `aginx-hub` | 生态仓 `opencarrier/aginx-hub`；线上 duphub.com（86quan `/data/www/aginx-hub`） |
| 消费方=**agent** | 大脑（aginxOS 分身 / 任何 LLM agent），持有主人的 api_key | 搜索/评估/下单/购买/回报全在 agent |
| 执行手 | **diting** | flow 拉进内存执行（`flow_run`）、面板模板渲染支付二维码（`/open` qr 模板）、本体更新（`diting update` ← pkg） |
| 落机形状 | 分身→`/home/workflows/`；api 腿→注册即用；频道腿→`channels/<名>/leg/`；flow→diting 内存或 `workflow/`；面板模板→`/var/lib/diting/templates` | |

## 三、协议：内容无关的文件树 VCS

**一个工件=一棵带 SHA-256 的文件树，hub 不解释树内语义**（除 kind
推导，见 §四）。内容无关是设计——这让新工件类零协议改动上架。

- **manifest**：`path → sha256` 映射 + `hash`（排序序列化的状态 id）。
- **pull**：`GET /api/templates/{name}/dup/manifest` + 逐文件
  `dup/file/{path}`。匿名拉取已放开（发现面开放；price>0 的过
  purchase 门）。
- **push**：`POST dup/push`，`{base_hash, files(b64), deletes}`，
  Bearer 认证 + ownership 门。客户端 fast-forward-only；**服务端不
  强制 ff——每 push=新版本快照**（防抖是客户端的事，注释在
  handlers.rs `dup_push`）。
- **定义层边界双执法**：运行时目录（sessions/data/senders/history/
  logs/users/output/.dup…）客户端 manifest 就不收（manifest.rs
  SKIP），服务端同名单二次拒 + `..` traversal 拒（defense-in-depth）。
- **端点双形状**：`templates`（duphub，缺省）/ `clones`（runtime 形
  状，上游兼容）。配置 `~/.dupconfig` 或 `OPENCARRIER_URL` /
  `OPENCARRIER_API_KEY` env（diting 侧 `DITING_DUPHUB_URL` /
  api_key 同形状）。
- **本地史 content-backed**：`.dup/objects/` + commit/restore；远端
  无状态，**回滚永远本地操作**。

CLI 面：`clone/init/pull/push/status/diff/commit/log/restore/config`。

**D18 补充：机器友好的工件自描述是 agent 的"商品详情页"。** 每个工
件树里的 `template.json`（flow 类是 args 声明 + template.json 双件）
承载：`kind`、`display_name`、`description`、`requires`（agent 自查
前提，如登录态）、`input`/`output`（形状）。价格**不进**工件——市场
API 是价格单一真源（`GET /api/price?name=`），工件跨市场可搬。

## 四、工件分类（kind，migration 015 + D18 五分）

templates 表从「分身模板」泛化为「工件」，kind=五种货：

| kind | 是什么 | 谁消费 | 落机形状 |
|---|---|---|---|
| `assistant` | 分身（能力面整树） | aginxOS | `/home/workflows/<名>/` |
| `api-leg` | 出站 api 声明腿（`[[tool]]` TOML） | aginxOS | 注册即用；频道腿工件落 `channels/<名>/leg/` |
| `channel-leg` | 入站频道门腿 | aginxOS | 同上 |
| **`flow`**（D18） | diting 可执行的操作流（flow.json + 自描述） | **diting** | 免费装 `workflow/<名>/`；付费**内存执行不落盘** |
| **`panel-template`**（D18） | diting 面板模板（HTML 填槽树） | **diting** | `/var/lib/diting/templates`（盘上层覆盖） |

- kind 来源=`template.json.kind`；只认枚举，未知值拒写。
- **首推钉死**：kind 在首次发布时定型，后续 push 改 kind 只告警不改库。
- 缺省=`assistant`，存量行语义不变；列表/详情/web 面按 kind 过滤。
- 沿革：09-27 `REQ-aginx-hub-asset-kind.md` 的三分已被 D17 三分取代，
  D18 扩为五分；diting flow_install 里残留的旧名 `agx-flow` 需对齐
  改为 `flow`（挂账 §八.6）。
- **付费 flow 的内存执行**：price>0 的 flow 由 diting `flow_run` 判
  purchase 门（hub 402=未购）→ agent 购买 → 拉进**内存**执行，不落
  盘（"复制给别人用"物理不存在）；免费 flow 走 flow_install 落
  `workflow/`。diting 解释器带运行时自检（工件带 duphub_id 标记必须
  持有本次执行的内存票据，dump 出的副本在任何地方执行被拒）。

## 五、市场面（D18：横切全部渠道的核心）

### a. 定价与购买（已有，泛化）

- `templates.price`（分）+ `visibility`（public/private）已有；
  `purchases` 表记录买断（user×template 唯一）。
- **买断制起步**（hub 语义不动）；**按次扣费**挂账后续演进
  （§八.5），不阻塞 D18 上线。
- purchase 门在 `resolve_version_record`：price>0 且非 owner/admin
  → 402。**端点需支持 api_key 认证 + 结构化错误**（D18：错误体带
  `{"error","price_cents","purchase_url"}`——agent 读得到下一步）。

### b. 分账（D18 新增）

成交即分：作者 70% / 平台 30%。同一 api_key 既是钱包（余额可消费）
也是收款账户（作者收入进余额）。提现=平台侧结算。

### c. 支付流（D18 定案：agent 端下单，人只扫码）

```
agent 查价/查余额（API，api_key 认证）
→ 余额不足：POST /api/orders/topup {"amount_cents"} 
                → {"order_id","qr_svg","expires_at"}
→ agent 调 diting POST /open（qr 面板模板）
   {title:"需要充值 ¥50", subtitle:"扫码支付，付完我自动继续", svg_b64}
→ 主人手机面板看到二维码 → 扫码付款（微信/支付宝 Native）
→ agent 轮询 GET /api/orders/{id} → {"status":"paid"}
→ agent 自主继续：purchase → pull → flow_run → 回报主人
```

订单 5 分钟过期；hub 后端接微信/支付宝扫码支付。人全程只有一个动作
（扫码），其余都是 agent 的 API 动作。

### d. 经验反馈回路（已有，D18 注记：agent 原生评价体系）

`POST /api/feedback` → LLM 自动审核 → `merge-feedback` 合并进模板。
agent 执行后的回执（diting flow_run receipt）就是反馈数据源——**机
器回报成败，不是人写五星**。

## 六、pkg 渠道（D18 重设计：releases 面泛化）

releases 表/端点已有（upload/latest/version/install.sh），升级为多
产品+用户发行+签名：

```sql
ALTER TABLE releases
    ADD COLUMN product VARCHAR(50),           -- 'diting'/'aginx'/用户发行
    ADD COLUMN owner_id BIGINT REFERENCES users(id),  -- 谁发行（可收费）
    ADD COLUMN price BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN sha256 CHAR(64) NOT NULL DEFAULT '',
    ADD COLUMN sig TEXT NOT NULL DEFAULT '';  -- 签名 manifest（立法要求）
```

端点产品化：`GET /api/pkg/{product}/latest?platform=` 等。**diting 本
体更新走这里**（`diting update`：查 manifest → sha256 校验 → 下载 →
原子替换）；任何用户也可发行自己的软件（过审核，可标价）。

## 七、纪律

- API key 走 `~/.dupconfig` / env，**不进 repo 不进 docs**。
- 匿名=只读拉取（price=0 公开工件）；一切写（push/评分/购买/充值）
  走认证 + ownership。
- duphub 件**不进 state tar / backup**——它是市场缓存，删了重拉即回；
  （唯一例外：用户在本机的未推送修改，走 dup 本地 commit 史）。
- **先审后卖**（D18 owner 定案）：用户上架的任何类工件先审后上架，
  审核面按 kind 分流（flow 防恶意脚本、面板模板防 HTML 夹带、pkg
  发行验二进制来源）。
- 价格不进工件、单一真源在市场 API（§三 D18 补充）。

## 八、挂账与开放问题

1. **第一条 channel-leg 还没上架**（kind 枚举已备，消费面
   `channels/<名>/leg/` 条文已挂 FS.md）——公众号接手线（#525）是
   第一个真实候选。
2. dup CLI `--kind` 覆盖未做（低优；kind 由 template.json 携带已够）。
3. api-leg 的**发现面**：消费方按 `?kind=api-leg` 列表拉——装分身
   流程「读依赖声明→从 duphub 拉腿」的匹配规则（按名/按标签）未定形。
4. hub 与 pkg 的版本对账：同一工件腿更新后，已装机器如何感知
   （dup pull 定时？晨报提示？）未设计——对应「装分身」自动链的
   后半段。
5. **按次扣费**（D18 挂账）：买断制跑通后演进——purchases 之外加
   usage 计数/charge-settle 生命周期，付费 flow 按次而非买断。diting
   侧已预留运行时自检面。
6. **diting flow_install 的 kind 对齐**：`agx-flow` → `flow`。
7. **diting activation 客户端重写**：接 hub 真实端点（purchase 门 +
   api_key 认证），自造 7 端点草案作废（2026-10-25 讨论稿，未进
   任何 main）。
8. **微信/支付宝 Native 支付通道**接入（§五c 前提）。
