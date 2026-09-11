# aginx

母体（D11：母体=aginx）——平台三件一树：路由器 face `/var/bin/aginx`
（symlink 进 pkgfiles）、心脏 server（单元直接指 pkgfiles 真身 spawn）、
fast-agi runtime（server 按需 spawn 的化身执行引擎）。装上它，裸机才有
会话/化身/派活；UDS /run/aginx.sock。

## 形态

- tree 包：files/bin/{aginx, aginx-server, aginx-runtime} 三件，
  exec=bin/aginx → /var/bin/aginx symlink 面
- [service] 单元随包走（/var/lib/aginx/units/aginx.toml，装完 svcd
  reload 立即拉起，无需重启）；单元名=包名 `aginx`
- 无依赖（它是依赖树的根）

## 验证

- `aginx-svc status aginx` → ready
- `aginx agent send me` 真往返（中文真答=brain 通路活，防绿灯无脑）

## 回滚

tree 包无 .prev——重装同版本即覆盖（sha 钉死，sync 自愈同律）。
