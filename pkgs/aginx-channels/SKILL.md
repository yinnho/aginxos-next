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
- `aginx-channels send weixin <uid> --image|--video|--file <path>` —
  CDN 富媒体出站（AES-128-ECB → getuploadurl → novac2c → sendmessage
  image_item/video_item/file_item）。需该 uid 有过入站（context_token）
  或关系账本可达；小图 png 即可验。


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

## status · active（v0.1.4）

`active` 是 daemon 内存里的 poll 腿标志。CLI `status` 以前冷加载
session.json（种子 `active=false`）会把「在轮询」谎报成 false。

现：daemon 每 ~5s 写 `/run/aginx-channels/status.json`；`status` 优先
读活文件（`active_source=daemon`）。无活文件/过期（>90s）则
`active=null` + `active_source=disk-cold`。

## 富媒体出站（v0.1.5）

闭环：本地文件 → 随机 AES-128 key → PKCS7/ECB 加密 →
`POST /ilink/bot/getuploadurl` → `POST` novac2c（`upload_full_url` 或
`upload?encrypted_query_param=&filekey=`）→ 读响应头 `x-encrypted-param`
→ `sendmessage` 的 `image_item.media` / `video_item.media`（`aes_key`=
base64(hex ASCII)，`mid_size`/`video_size`=密文长）。

ACP 回信媒体（窄扩展）：引擎可用 content 块 `{type:image,path:/…}` 或
正文标记 `[[aginx-media:image|/abs/path]]`；桥剥离标记后 CDN 上传发送。
无媒体形状时与 v0.1.4 相同（纯文本）。

## 入站文件落盘（v0.1.8）

iLink 协议腿收到文件消息就拉本体（CDN 下载+AES 解密，走 channel.rs
`ITEM_TYPE_FILE`）。v0.1 的桥把字节扔了只留文件名；v0.1.8 起桥先落盘
`{AGINX_HOME}/files/<净化名>`（basename 化防路径逃逸；同名覆写
new-wins）再进分身回合，分身收到的文本是「收到文件 X（N B），已存
/home/files/X」，直接拿路径干活。CDN 下载失败或文件名不可用=如实
回「本体未落盘」。图片/语音本体仍弃（后续刀）。
