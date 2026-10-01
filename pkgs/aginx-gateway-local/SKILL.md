# aginx-gateway-local

本机 ACP 口（#68 ②b）：真 aginx 的第二个实例——direct 模式只听
`127.0.0.1:8686`，给频道桥（aginx-ilink）当「把文本交给某 agent，
拿回文本」窄接口的对端。主 gateway（relay 腿）在 relay 模式下没有
本地监听，这是它的本地脸。

## 形态

- 面是壳脚本 `aginx-gateway-local`（flat bin 成员）：确保数据世界
  `/var/lib/aginx/gateway-local` + `agents → ../gateway/agents`
  符号链在位、种子化 `/etc/aginx/gateway-local.toml`（包内
  `etc/gateway-local.toml` 是出厂缺省，/etc 已存在不覆写），然后
  exec aginx-gateway 包里的同一真身（`-c` 指本口配置）。
- **数据世界分家**：sessions.json/spool/auth.json 是整文件写，与
  relay 腿共世界会互覆——本口独立 `AGINX_DATA_DIR`；**名册不分家**
  （agents 符号链回主册）：换引擎=改名册条目，两腿同册生效。
- `access="public"` 仅回环可及（host 绑定即门）；ACP 客户端无
  authToken 腿。

## 排障

- 端口不在：`aginx-svc status aginx-gateway-local`；日志看壳的
  mkdir/ln 输出与真身 stderr。
- `Agent not found`：名册条目缺——在 `/var/lib/aginx/gateway/agents/
  <名>/aginx.toml` 建条目（主册），本口符号链即见。
- 改端口/访问面：改 `/etc/aginx/gateway-local.toml` 后
  `aginx-svc restart aginx-gateway-local`（频道桥侧同步改
  `AGINX_ILINK_ACP_ADDR`）。
