# git — 版本控制（Alpine v3.22 树包）

## 形态

整棵 usr 树住进 `/var/lib/aginx/pkgfiles/git/`（build-pkg.sh `git)`
分支从 dl-cdn.alpinelinux.org 拉 15 个 sha256 钉死的 apk 解包拼成）：
`usr/bin/git` 本体 + `usr/libexec/git-core/` multicall 命令族（相对
symlink 农场，`git-add -> ../../bin/git` 同款）+ `usr/lib/` 依赖闭包
（libcurl/openssl/pcre2/expat/zstd/brotli/nghttp2…）+ `lib/ld-musl-
aarch64.so.1`（musl loader 即 libc）。面 `/var/bin/git` 是一层 wrapper
（`bin/git`）：

- 首跑自链 `/lib/ld-musl-aarch64.so.1` → pkgfiles 树内的 loader
  （git exec 的子进程走内核 PT_INTERP，系统路径必须有 loader；python3
  包同工艺，互不冲突）；
- `LD_LIBRARY_PATH` 指树内 `usr/lib`（env 继承给全部子进程——musl
  ld.so 无 `--library-path` 旗标，env 是唯一通道）；
- `GIT_EXEC_PATH`/`GIT_TEMPLATE_DIR` 指树内 git-core/templates。

https 远端开箱即用（CA = 镜像烤入的 `/etc/ssl/certs/ca-certificates.crt`
——libcurl 编译期默认路径，与树无关）。ssh 远端不可用（设备无 ssh
客户端）；file/git:// 本地协议可用。裁掉：man/doc/locale、openssl
engine 模块、libcrypto 自带的 etc/ssl 配置副本（libcurl 走上述绝对
路径，树内副本无人读）。

## 验证（上机）

```sh
git --version                     # git version 2.49.1
git --exec-path                   # /var/lib/aginx/pkgfiles/git/usr/libexec/git-core
cd /tmp && git clone https://github.com/git/git --depth 1   # https 真收据
ls -l /lib/ld-musl-aarch64.so.1   # wrapper 首跑自链（python3 装过则在）
```

## 回滚

`aginx-pkg rollback git`（或重装旧 tar）。`/lib/ld-musl-aarch64.so.1`
的链接留给 python3（同 loader 家族，无人用则无害残留，可手动 rm）。
