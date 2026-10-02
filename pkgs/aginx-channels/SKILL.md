# aginx-channels

频道体系包（#69 改形，2026-10-02）。crates/channels 是全部频道的
唯一家：守护 + 运维 CLI 一件双面，**子命令带频道名**（`login weixin`）；
weixin 是第一腿不是包名，新频道=crate 新模块、面不变。

一频道 = 一目录 + 一包位（DESIGN.md §四）：数据与绑定全在
`/home/channels/<名>/`（channel.toml + `senders/<uid>/session.json`，
**绑定=记录字段，换绑=改字段不搬家**）。频道不感知引擎：入站消息按
「账号 bind_agent 字段 → channel.toml default_agent」选 agent，走 ACP
窄接口（本机口 127.0.0.1:8686，aginx-gateway-local 供）交给 gateway
名册里的引擎，回信原路发回。

## 面（weixin 第一腿）

- `aginx-channels daemon` — 频道守护（单元跑；前台手跑=排障）。
  扫 `/home/channels/*/` 起腿（channel.toml 的 type 指认；不认识的
  type 告警跳过）；weixin 腿首启自动迁移旧世界会话
  （workflows/*/senders/ 与家根 senders/）进频道家。
- `aginx-channels login weixin [--bind <agent>]` — 扫码起号。二维码
  直接在终端画（半块字符，无头机唯一人面是 ssh）；`--bind` 指名绑定，
  缺省走 channel.toml default_agent。
- `aginx-channels status [<名>]` — 频道/会话一览（缺省=全部频道）。
- `aginx-channels bind weixin <agent>`（单账号）或
  `bind weixin <uid> <agent>` — 换绑：改 session.json 的 bind_agent
  字段。守护 5s 拾取免重启；该发信人下一条消息自动开新引擎线程
  （gw.json 记 {agent, session_id}）。
- `aginx-channels send weixin <uid> <text>` — 出站对测（iLink 出站是
  关系账本 best-effort，HTTP 200 ≠ 送达）。

## 频道目录

```
/home/channels/<名>/
├── channel.toml        # type/default_agent/[policy] bound_only（缺省播种，只收绑定号本人）
└── senders/
    ├── <账号uid>/session.json   # 协议会话（bind_agent 字段=绑定真源）
    └── <发信人id>/gw.json       # 该人的引擎对话挂点 {agent, session_id}
```

## 排障

- 回信「频道桥开小差：ACP …」= 本机口 8686 不可达或 agent 条目
  超时——`aginx-svc status aginx-gateway-local`；名册在
  /var/lib/aginx/gateway/agents/（gateway-local 经符号链共享）。
- `status` 空 = 无会话，login 起号；`expired: true` = 24h 无轮询
  续命，重扫即活。
- env：`AGINX_CHANNELS_ACP_ADDR`（默认 127.0.0.1:8686）、
  `AGINX_CHANNELS_ACP_TIMEOUT_SECS`（默认 1200，须≥名册条目 timeout）。
