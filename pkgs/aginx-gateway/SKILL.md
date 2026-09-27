# aginx-gateway

对外面守护（v0.2.0 换芯，2026-09-27 刀1）：真身=生态仓 aginx
（独立项目、专人开发，本仓只装配不仿制）aarch64-musl 静态原样上机。
8443 relay 长连 + ACP 对外面 + pair/auth 凭据面。v0.1.x 仿制品
（crates/gateway，单门 relay_secret、裸根可达）退役。

- **真身**：/var/lib/aginx/pkgfiles/aginx-gateway/bin/aginx-gateway
  （[service] cmd 直指）；面 /var/bin/aginx-gateway symlink（安装器
  自动带 .aginxmd）。
- **配置先写后装（铁律）**：`-c /etc/aginx/gateway.toml`（0600：
  [relay] id/relay_secret、[server] access=private、[auth] jwt_secret）
  ——缺 relay id 裸 exit(1)，5 连崩进断路器停摆（停摆=显式
  `aginx-svc start aginx-gateway` 才醒）。
- **状态**：AGINX_DATA_DIR=/var/lib/aginx/gateway（绑定台账、丢件
  柜台、agents 注册表——装备入网一份 toml 即直通，刀3 铺）。
- **运维面**：`aginx-gateway pair / devices / unbind / auth / auths /
  revoke`（同 -c 配置；auth 发 scoped token：-n 客名 -a 点名
  allowed_agents -m 方法白名单 --allow-system -e 天数）。

## 验证

- `aginx-svc status aginx-gateway` → ready
- /proc/net/tcp 8443（:20FB）有 ESTABLISHED（netstat 禁用——busybox 必炸）
- `AGINX_DATA_DIR=/var/lib/aginx/gateway /var/bin/aginx-gateway devices
  -c /etc/aginx/gateway.toml` 列绑定台账

## 换装（v0.1.x → v0.2.0，flat 裸包 → 树包跨形态）

relay id 唯一注册方——先 `aginx-svc stop aginx-gateway` 再装；装后
验 /var/bin/aginx-gateway 是 symlink（readlink 指 pkgfiles 真身）非
flat 旧件，旧 flat 件若残留即删（刀2 切流序全录 docs/HARDWARE.md）。

## 回滚

`aginx-pkg rollback aginx-gateway`（回 v0.1.x 仿制品=临时避难所——
仿制品无权限模型，正向路永远是修新包）
