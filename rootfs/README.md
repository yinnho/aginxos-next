# rootfs/ — N4 镜像配方

新线整机镜像的**配方**：只有这个仓自己拥有的文件住这里。`scripts/build-rootfs.sh`
把本目录铺进烤机树，再把编译产物与**老仓资产**（见下）放进去，最后 mke2fs 出
`out/rootfs.img`。判据：改了本目录里的文件 → 重烤即变更；不在本目录的 → 脚本
从别处取，别复制进来。

## 目录内容

- `etc/` — 静态系统配置。init.d 通用五件（rcS/provision/
  aginx-term-handoff/state-restore/varlib-migrate）；net-bringup 与五个
  bringup 不在配方里——由 `devices/<codename>/bringup/` 烤机时注入
  （D14 机型是数据；E4b 起 net-bringup 同律：并网流程是机型数据）、
  aginx/svc.d 两单元（net-watch + aginxbrowser——L0 刀4 起
  server/secretd/gateway/voice 单元随包走）、
  aginx/（env 明文环境、gateway.toml 形状参数、groups.desc 命令分组、
  secret.policy sidecar 放行表）、
  crontabs（N5④：备份 now 定时行）、agpkg.manifest
  （N4 切净：8 条，删 aginx/aginx-carrier 两行，sig 由烤机脚本重签）。
- `libexec/aginx/` — 守护的家（D13：libexec 不进路由器命令扫描）。net-watch/
  net-rejoin 两个 sh 在此；aginx-svcd 由脚本落位（L0 刀4 起 server/secretd/
  gateway 走包不烤；结构刀① 起 aginx-runtime 物理删除——引擎在 server 进程内）。
- `usr/bin/` — **命令宇宙的元数据层**：16 个 `.aginxmd` sidecar（编译命令的
  门面说明，二进制由脚本落位改名后与 sidecar 同名相邻）+ 4 个 sh 面
  （aginx-web/file/mem = 桥到 provision 后的包二进制 aginx-web/agf/agmem，
  aginx-sys-status）。桥壳**不声明 aginx:exec**——目标 sync 后才存在是合法暂缺。
- `var/bin/` — 3 个 voice 内部件 sidecar（aginx-asr/tts/ocr，hidden，被
  aginx-voice 直接 spawn，不是 brain 面）+ aginx-web.aginxmd（provision 后
  /var/bin/aginx-web 是编译件，face 住 sidecar——post-provision 它遮住
  /usr/bin 桥壳，路由与摘要两处保持 lockstep）。

## 放置矩阵（谁烤进去、落哪）

| 来源 | 产物 | 落位 |
|---|---|---|
| 本仓 target/musl | aginx-pkg, aginx-svc, aginx-boot-ok（vendor-boot 机型）, aginx-done, aginx-secret, aginx-download | /usr/bin |
| 本仓 target/musl | aginx-svcd | /usr/libexec/aginx/ |
| 仓里 `home/` 整树 | SOUL/MEMORY、photos/files 空树、workflows/clone-creator 出厂助理 | /home（结构刀④：出厂整树烤进，真源=仓里 home/，docs/FS.md） |
| 冻结资产（.local，非老仓） | aginxos-init, aginxos-agent | /aginxos/（trampoline 对；device.toml 有 [update] 节才烤——redfin 烤、enchilada 不烤） |
| 本仓 rootfs/src/*.c（zig cc） | nlscan→aginx-net-scan, wifi-join→aginx-net-join, reboot2→aginx-reboot, paneloff→aginx-panel-off | /usr/bin |
| devices/${DEVICE}/cam + 本仓 rootfs/src/jpegenc_tj.c | cam-shot→aginx-cam-shot（build-cam.sh 带机型 cam 目录；传感器源=机型数据 D14） | /usr/bin |
| 老仓 out/voice, out/ocr | ag-asr→aginx-asr, ag-tts→aginx-tts, ag-ocr→aginx-ocr + 模型→/var/models | /var/bin |

L0 起不烤（一切皆包）：aginx/aginx-server（母体=`aginx` 树包，bin 两件 +
`[service]` 单元随包走）、aginx-voice/term/qr/update/secretd/gateway。
aginx-runtime 已随结构刀①（2026-09-24 workspace 合一）物理删除——引擎在
aginx-server 进程内直跑，再无独立二进制。

不进镜像：老 `ag` 路由器、全部 `ag-*` 壳、carrier daemon、老 relay/ag-backup（继任者
已由本仓烤入：aginx-gateway N5⑤⑥、aginx-backup N5④）。/bin 内部件（splash、
binder-init、qrtr-lookup、qmi-req、
raw2jpg、snd-*、i2c-reg、httpget、wdt、rtcal、fake-sm、dropbear、
rmt_storage、busybox）**保原名**，照抄老仓脚本落 /bin。
（bootcard 2026-09-13 退役出镜像——服务器版无屏；源码留 rootfs/src/，
回归走 aginx-bootcard opt-in 包，rcS 有 [ -x ] 门。）

## 设备资产（.local/device/redfin/，gitignored）

vendor-ramdisk-root（内核模块）、voice+models、ocr+models、dropbear、
radio、冻结 trampoline 对、stock 两镜像、qmi 头——2026-09-08 自老仓
sha256 验收迁入；重生成法见 `devices/redfin/boot/assets.md`。
密钥在**本仓** `.local/keys/`（N4② 落位）。老仓已封存
（`docs/ARCHIVED.md` there）。

## lint

配方面集过路由器门：`aginx commands --check`（AGINX_CMD_PATH 指向烤机树），
22 面全绿。烤机脚本每次 bake 都重跑此门。
