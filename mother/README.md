# 母体运行时

从独立仓 `aginx-carrier` 迁入 AginxOS。**aginx-carrier 不再作为产品入口**：不发布 `aginx-carrier` 命令、不按 clone 名拉 ACP 桥。

本目录是母体引擎（kernel / runtime / clone / memory）。编译进 `aginx-server`。机上：

- `AGINX_HOME=/home` — 母体的家（总管 SOUL/MEMORY 在根上）
- `{AGINX_HOME}/workflows/<名>/` — 助理（分身格式翻本：性格、职责、flows）
- `{AGINX_HOME}/tools/`、`providers/`、`peers/` — 见 `docs/FS.md`

别人走 `agent://`（`crates/gateway`）。开发机必须设 `AGINX_HOME`，不要用隐藏的 `.aginx/carrier`。

```bash
cd mother
AGINX_HOME=/tmp/aginx-home cargo test -p carrier-types
```
