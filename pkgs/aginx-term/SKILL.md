# aginx-term — 终端面板（L0 包形态）

## 形态

树包两成员：

- `bin/aginx-term` — zigbuild musl（crates/term）
- `share/fonts/agterm-cjk.otf` — CJK 子集字体（M38a 冻结资产，1.5MB）

`exec = bin/aginx-term` → 安装器种 `/var/bin/aginx-term` face（相对符号链接）。
无 `[service]` 单元：面板不是 svcd 单元，rcS 的 `aginx-term-handoff`
缺席静默轮询 `/var/bin/aginx-term`（5s 节拍，无日志刷屏），装包即亮屏、
退出 2s 重拉。

## 验证（上机）

```sh
aginx-svc 无关——确认 handoff 拾取：
  ls -l /var/bin/aginx-term          # face 在
  tail /var/aginx-term.log           # start 行出现（≤5s）
  ls /var/lib/aginx/pkgfiles/aginx-term/share/fonts/agterm-cjk.otf
```

CJK 渲染走 cjk.rs `font_path()`：env 覆盖 > 烤入路径（老全量镜像）>
pkgfiles 兜底（本包）。字体缺失退化 ASCII `?`，不炸。

## 回滚

`aginx-pkg` 无卸载；树包回滚 = 重装旧版 tar（`.prev` 备份在 pkgfiles 侧）。
面板黑屏回到 bootcard 字标即 L0 出厂态，无破坏。
