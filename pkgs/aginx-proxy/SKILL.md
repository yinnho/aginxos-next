# aginx-proxy — 共享代理隧道（stunnel 客户端树包）

## 形态

Alpine v3.22 community aarch64 的 stunnel 闭包 4 apk（musl loader +
libssl3 + libcrypto3 + stunnel，sha256 钉死 TOFU 于下载日）住进
`/var/lib/aginx/pkgfiles/aginx-proxy/`。面 `/var/bin/aginx-proxy` 是
wrapper（`bin/aginx-proxy`）：

- 首跑自链 `/lib/ld-musl-aarch64.so.1` → 树内 loader（git 包同工艺，
  同 musl 1.2.5 互不冲突）；
- `LD_LIBRARY_PATH` 指树内 `usr/lib`（musl ld.so 无
  `--library-path` 旗标，env 是唯一通道）；
- 无参调用时自填默认 conf `/etc/stunnel/aginx-proxy.conf`，带参原样
  透传给 stunnel。

## 设备侧配置（真源，刷机带走）

`/etc/stunnel/aginx-proxy.conf`（运维手写，模板见下）+
`/etc/stunnel/psk.txt`（0600，与隧道服务端同一 PSK，永不回显）：

```
client = yes
foreground = yes
pid =
[socks2ssl]
accept = 127.0.0.1:8800
connect = <隧道服务端>
ciphers = PSK
PSKidentity = <身份>
PSKsecrets = /etc/stunnel/psk.txt
```

`foreground = yes` 必须有——simple 单元要前台进程，daemon 化会让
svcd 误判退出。

## 用法

装：`aginx-pkg opt-in aginx-proxy`（先写 conf/PSK 再装，缺 conf 即退
是预期）。出口=本机 `socks5h://127.0.0.1:8800`：

- grok：`HTTPS_PROXY=socks5h://127.0.0.1:8800 grok -p "..."`
  （api/cli-chat-proxy.grok.com 被墙，裸连 Connection reset）；
- aginxbrowser：单元 env `HTTPS_PROXY`/`ALL_PROXY` 同值。

## 验活

`pidof stunnel` + `/proc/net/tcp` 里有 8800 LISTEN；穿透验=带 env 的
grok 一发真 turn。
