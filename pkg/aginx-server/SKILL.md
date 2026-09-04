# aginx-server — 平台心脏并存包（N3）

AginxOS 二代的平台心脏，与老 carrier 线同镜像共存（N3 形态；N4 bake
接管后转正）。包内一树四工具：

- `aginx-server` — 前台守护：化身登记（进/住/切/退）、会话光标
  （开机=母体 me）、请求路由、D8 会话账。UDS `/run/aginx.sock`。
- `aginx` — 路由器（母体门面，唯一裸命令）。本包内因老 relay 未让
  `/var/bin/aginx` 裸名（N4 bake 改 aginx-gateway 让名），暂住包树，
  经 `AGINX_BIN` 显式路径起作用。
- `aginx-runtime` — fast-agi 引擎，server 按需 spawn。
- `aginx-{cam-shot,net-scan,net-join,sys-status}` — 对老镜像二进制
  （/bin/cam-shot、/bin/nlscan、/bin/wifi-join、/sys）的薄壳工具面，
  D13 命名 + `aginx:` 元数据，住包树 `tools/` 子目录；路由器经
  `AGINX_CMD_PATH`（=包树 tools/）发现。引擎/守护不进扫描路径
  （D13：常驻引擎名不进命令宇宙）。

## 形态

- 安装面：`/var/bin/aginx-server` → 符号链接进包树
  `/var/lib/agpkg/pkgfiles/aginx-server/`（files/ 树形态，M28 扩展）。
- 单元：`/var/lib/agpkg/units/aginx-server.toml`（agsvc 覆盖层，
  开机自起）。brain key 经 `env_file = /etc/aginx/env` 进环境。
- 化身树：`AGINX_HOME=/home/.aginx-n`（两线并行纪律——老线
  `~/.aginx` 不碰；N4 切换时迁 `/home/.aginx`）。

## 验证（日常）

    agctl status aginx-server
    /var/lib/agpkg/pkgfiles/aginx-server/aginx agent send 你好
    /var/lib/agpkg/pkgfiles/aginx-server/aginx commands

语音面：另写覆盖单元 `/var/lib/agpkg/units/voiced.toml`（cmd 指包树
voiced、envs 加 `VOICED_FRONT=<包树>/aginx`）即把日常 voiced 的自由
文本接到母体；封闭词表（WiFi/扫码/念读）仍是离线地板。

## 回滚

    agctl stop aginx-server
    rm /var/lib/agpkg/units/aginx-server.toml /var/lib/agpkg/units/voiced.toml
    rm -rf /var/lib/agpkg/pkgfiles/aginx-server /var/lib/agpkg/skills/aginx-server \
       /var/lib/agpkg/stamps/aginx-server /var/bin/aginx-server
    agctl reload

（删 voiced 覆盖单元后老 baked voiced 自动复活。）
