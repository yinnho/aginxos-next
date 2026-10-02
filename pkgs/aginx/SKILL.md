# aginx

前台（刀④-4 引擎商品化裁形，2026-10-02）——两件一树：路由器 face
`/var/bin/aginx`（symlink 进 pkgfiles）、server（UDS 面：list/status/
install/remove 装卸链 + boot 期系统直通条目）。对话不走本包——
system 与助理都是网关名册里的 codex 条目，`agent://<机>.relay.aginx.net/
<名>` 直达（in-process 引擎与 send/create 面已随刀④-4 退役）。

## 形态

- tree 包：files/bin/{aginx, aginx-server} 两件，
  exec=bin/aginx → /var/bin/aginx symlink 面
- [service] 单元随包走（/var/lib/aginx/units/aginx.toml，装完 svcd
  reload 立即拉起，无需重启）；单元名=包名 `aginx`
- depends 锚 aginx-update（刀F 起律）

## 验证

- `aginx-svc status aginx` → ready
- `aginx agent list` 真往返（UDS 面+home 树双活，防绿灯无脑）
- `aginx agent install <名> <包.tar>` → workflows/ 落位 +
  网关条目（agents/<名>/aginx.toml，codex 形）+ workflows.md 行

## 回滚

tree 包无 .prev——重装同版本即覆盖（sha 钉死，sync 自愈同律）。
