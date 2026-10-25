# D18 重构计划（2026-10-25 立 · 设计真源见 DUPHUB.md）

三仓联动：`opencarrier/aginx-hub`（市场服务端）、`aginx/aginxbrowser`→
diting（执行手）、`aginxos-next/crates/dup`（VCS CLI，本批不动）。
每批独立可回退、全部绿才合；wire 兼容优先（老 dup CLI / 老分身消费
不能断）。

## 现状盘点（2026-10-25 下午实测）

| 组件 | 已有 | 缺 |
|---|---|---|
| hub kind 枚举 | `valid_kinds() = [assistant, api-leg, channel-leg]`（export.rs:36） | `flow` / `panel-template` |
| hub 认证 | `get_user_any_auth` 已双模（`oc_` api_key / JWT），api_keys 表齐 | purchase 端点走 `get_current_user`（**只认 JWT**，agent 的 api_key 会被 401） |
| hub 计费 | templates.price + purchases + resolve_version_record 402 门 | 无分账、无订单/支付、无价格查询端点、402 体非结构化 |
| hub releases | 单产品（version+platform），无 product/sha256/sig/owner | pkg 泛化整体 |
| diting | flow_install 走 dup 协议 ✓（kind 过滤用旧名 `agx-flow`）；/open + qr 面板模板 ✓；tmpl.rs 两层模板 ✓ | kind 对齐、付费 flow 的 purchase 门接入、panel-template 进货、`diting update` |
| diting activation.rs | 2026-10-25 上午按自造 7 端点写的（未进 main） | 按真实 hub 端点重写（§批次 3） |

## 批次 1 —— hub：kind 五分 + agent 可购买（最小可上线）

**改动面：export.rs 一行 + handlers.rs purchase/错误体 + 一个 migration。**

1. `valid_kinds()` 加 `"flow"`、`"panel-template"`（枚举放开即上架，
   审核面靠批次 4）。web.rs 的 kind 徽章补两个映射。
2. `purchase_template` 认证从 `get_current_user` 改
   `get_user_any_auth`——**agent 的 api_key 能买**（一行）。
3. `resolve_version_record` 的 402 响应结构化：
   `{"error":"payment_required","price_cents":N,"purchase_url":"/api/templates/{name}/purchase"}`
   （dup 协议消费方读 dup/file 时同门——错误形状对 agent 可读）。
4. 新端点 `GET /api/price?name=`（匿名）→ `{"price_cents":N}` /
   404——价格单一真源（DUPHUB.md §三）。
5. Migration 016：无表变更（price 已有），只补
   `purchases.payout_cents BIGINT DEFAULT 0`（分账记账位，批次 2 用）。

**验收**：kind=flow 的工件 push→pull 全通；api_key 调 purchase 200；
未购拉 dup/file 收结构化 402；老三类工件行为零变化（回归跑 dup push/
pull/purchase 既有用例）。

## 批次 2 —— hub：分账 + 订单支付面

1. **分账**：purchase 成交时给 owner 打 70%（owner 收入进
   `users.balance`？——否，新表 `wallets(user_id, balance_cents)` 更
   干净：api_key 即钱包视图）。`purchases` 行记 payout_cents。
   提现挂账（先只进不出，运营手工结算）。
2. **订单面**（DUPHUB.md §五c）：
   - `POST /api/orders/topup {"amount_cents"}`（api_key 认证）→
     `{order_id, qr_svg, expires_at}`——订单落表，二维码先接
     **微信 Native**（支付宝后补）；支付通道未接通前 qr_svg 返回
     占位（沙箱态），端点形状先立。
   - `GET /api/orders/{id}`（同 key）→ `{status: pending|paid|expired}`。
   - 回调端点 `/api/payments/wechat/notify`（签名验证）→ 订单
     paid → 钱包入账。
   - 订单 5 分钟 TTL，过期态由轮询触发惰性翻转。
3. `GET /api/quota`（api_key）→ `{balance_cents, purchases:[…],
   payouts_total}`——agent 查余额/收支的单一端点。

**验收**：沙箱态全链路 agent 走通（topup→轮询 paid→purchase→pull）；
分账数字正确（owner 70/平台 30，两行账可对）；微信回调签名错拒绝。

## 批次 3 —— diting：消费侧对齐（含 activation 重写）

1. **删自造端点草案**：activation.rs 的 7 端点客户端按 D18 作废
   重写——`DITING_DUPHUB_KEY`（env 或 `~/.dupconfig` 同形状）+
   hub 真端点（price/purchase/quota/orders）。
2. **flow_install**：kind 过滤 `agx-flow` → `flow`（老 hub 兼容分支
   保留——listing 无 kind 时双试两值）。
3. **flow_run 付费分支**：本地无 flow → `GET /api/price` →
   price>0 → 查 purchase（402 即购买提示：结构化错误直达 agent，
   含 purchase_url）→ api_key 代购（余额足）→ 拉进内存执行（不落盘）
   → 失败回报。**运行时自检保留**（duphub_id ↔ 内存票据，dump 副本
   拒执行）。
4. **panel-template 进货**：`POST /panel-template/install {"name"}`
   ——dup 协议拉树 → 落 `/var/lib/diting/templates`（盘上层，同名
   覆盖内置；registry 合并规则：工件的 registry.json 条目并入本地）。
5. **支付展示**：agent 拿到 topup 的 qr_svg → 调 `/open`（qr 模板，
   现成）——diting 侧零改动，只在 SKILL.md 写明这个 agent 动作序列。

**验收**：agent 全链路无人工——搜 flow（flow/search 走新 kind）→
查价 → 余额不足 → topup → qr 经 /open 上屏 →（沙箱 paid）→
purchase → flow_run 内存执行 → quota 可见扣减；dump 副本执行被自检
拒绝；免费 flow 安装/本地流完全不受影响。

## 批次 4 —— hub：审核泛化 + web 面

1. review 队列按 kind 分流（kind=flow 审 steps 脚本面；
   panel-template 审 HTML 夹带；api-leg/channel-leg 原样）。
   **先审后卖**立法落地：price>0 的工件上架必须过审（015 后新增
   `templates.reviewed_at` 位，migration 017）。
2. duphub.com web：kind 筛选 tab 加 flow/panel-template；商品页展示
   自描述（requires/input/output）；作者收入面板（payouts）。

**验收**：未过审的 price>0 工件拉取被拒；web 五类可见可购。

## 批次 5 —— pkg 渠道泛化 + diting update

1. Migration 018：releases 加 product/owner_id/price/sha256/sig；
   端点 `/api/pkg/{product}/latest|{version}`（product 维度），
   老 `/api/releases*` 保持兼容（product 缺省 'aginx'）。
2. 发版签名：上传端 admin 签（minisign 形状，公钥钉在客户端）。
3. **`diting update [--check]`**：查 latest → sha256 + 签名验 →
   下载原子替换 → 重启提示；release CI 追加 pkg 上传步骤
   （v0.7.x 起 product='diting'）。

**验收**：`diting update --check` 报新版本；update 全程校验链；
坏签名拒替换。

## 批次 6 —— 按次扣费（DUPHUB.md §八.5 挂账，买断验证后启动）

charge/settle 生命周期 + usage 计数 + 失败退款扫描；diting 侧
flow_run 从 purchase 门切 charge 门（api 已预留）。**本批只出设计
文档，实施等买断制跑出真实数据后另立项。**

## 风险与对策

| 风险 | 对策 |
|---|---|
| purchase 改 api_key 认证后越权（别人 key 乱买） | 钱包余额即上限；充值前余额=0，损失面=已充金额 |
| 付费 flow 内存执行被 dump | 自检已备（§批次 3.3）；加 hub 侧版本轮换（flow 内容微扰）挂账 |
| 微信支付通道接通周期 | 批次 2 先沙箱态（qr 占位+手工置 paid），通道后补，端点形状不返工 |
| releases 泛化动到 aginx 既有发版 | 老 `/api/releases*` 兼容层保留；product 缺省填 'aginx' 回填 |
| diting activation 双写期混乱 | 批次 3 一次切净（草案未进 main，无兼容包袱） |

## 顺序与依赖

```
批次1 (hub kind+purchase) ──→ 批次3 (diting 对齐) ──→ 批次4 (审核+web)
       │                              ↑
       └──→ 批次2 (分账+订单) ─────────┘（3 的代购依赖 2 的钱包；沙箱态可先行）
批次5 (pkg) 独立，可与 3/4 并行
批次6 等数据
```

## 提交纪律

- hub 仓每批一提交（migration+代码+回执注释同批）；
- diting 仓批次 3 一提交（activation 重写含在内）；
- 每批验收段照抄进提交信息；全量测试绿才 push；
- DUPHUB.md §八挂账逐批销账（做完的条目改"已完成+批次号"）。
