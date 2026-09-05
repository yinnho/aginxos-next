# rootfs/ — N4 镜像配方

新线整机镜像的**配方**：只有这个仓自己拥有的文件住这里。`scripts/build-rootfs.sh`
把本目录铺进烤机树，再把编译产物与**老仓资产**（见下）放进去，最后 mke2fs 出
`out/rootfs.img`。判据：改了本目录里的文件 → 重烤即变更；不在本目录的 → 脚本
从别处取，别复制进来。

## 目录内容

- `etc/` — 静态系统配置。init.d 全套（rcS/net-bringup/provision/aterm-handoff/
  app-registry/state-restore + 六个 bringup）、agsvc.d 五单元、aginx/
  （env 明文环境、groups.desc 命令分组、secret.policy sidecar 放行表）、
  apps.d 两 tile、crontabs（仅注释，文件在 crond 才有家）、agpkg.manifest
  （N4 切净：8 条，删 aginx/aginx-carrier 两行，sig 由烤机脚本重签）。
- `libexec/aginx/` — 守护的家（D13：libexec 不进路由器命令扫描）。net-watch/
  net-rejoin 两个 sh 在此；aginx-svcd/aginx-server/aginx-runtime/aginx-secretd
  由脚本落位。
- `usr/bin/` — **命令宇宙的元数据层**：15 个 `.aginxmd` sidecar（编译命令的
  门面说明，二进制由脚本落位改名后与 sidecar 同名相邻）+ 4 个 sh 面
  （aginx-web/file/mem = 桥到 provision 后的包二进制 agb/agf/agmem，
  aginx-sys-status）。桥壳**不声明 aginx:exec**——目标 sync 后才存在是合法暂缺。
- `var/bin/` — 3 个 voice 内部件 sidecar（aginx-asr/tts/ocr，hidden，被
  aginx-voice 直接 spawn，不是 brain 面）。

## 放置矩阵（谁烤进去、落哪）

| 来源 | 产物 | 落位 |
|---|---|---|
| 本仓 target/musl | aginx | /usr/bin（裸名，路由器） |
| 本仓 target/musl | aginx-server, aginx-runtime | /usr/libexec/aginx/ |
| 本仓 target/musl | aginx-voice, aginx-net-wizard, aginx-term, aginx-pkg, aginx-svc, aginx-boot-ok | /usr/bin |
| 本仓 target/musl | aginx-svcd | /usr/libexec/aginx/ |
| 老仓 target/musl | aginxos-init, aginxos-agent | /aginxos/（trampoline） |
| 老仓 target/musl | agdl→aginx-download, agupd→aginx-update, agqr→aginx-qr, agdone→aginx-done, agsecret→aginx-secret | /usr/bin（改名即落） |
| 老仓 target/musl | agsecretd→aginx-secretd | /usr/libexec/aginx/ |
| 老仓 rootfs/src/*.c（zig cc） | cam-shot→aginx-cam-shot, nlscan→aginx-net-scan, wifi-join→aginx-net-join, reboot2→aginx-reboot | /usr/bin |
| 老仓 out/voice, out/ocr | ag-asr→aginx-asr, ag-tts→aginx-tts, ag-ocr→aginx-ocr + 模型→/var/models | /var/bin |

不进镜像：老 `ag` 路由器、全部 `ag-*` 壳、carrier daemon、relay、ag-backup
（#86 备份线归 N5）。/bin 内部件（splash、binder-init、qrtr-lookup、qmi-req、
raw2jpg、snd-*、i2c-reg、bootcard、httpget、wdt、rtcal、fake-sm、dropbear、
rmt_storage、busybox）**保原名**，照抄老仓脚本落 /bin。

## 老仓资产引用（OLD=，单一权威源纪律——不复制进本仓）

busybox、rootfs/src/*.c、vendor-ramdisk-root（内核模块）、out/voice+models、
out/ocr+models、.local/dropbear、.local/radio、out/cacert.pem、字体、
usr/share/ocr 字典。密钥在**本仓** `.local/keys/`（N4② 落位）。

## lint

配方面集过路由器门：`aginx commands --check`（AGINX_CMD_PATH 指向烤机树），
22 面全绿。烤机脚本每次 bake 都重跑此门。
