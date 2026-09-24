# 交接：母体文件架构（2026-09-22）

给下一棒：产品树已经定了，代码还没接到 OS 前台。先读这份，再动刀。

> **2026-09-24 状态翻新（结构刀①–④）：** 下文「代码做到哪」是 09-22 快照。此后母体
> 刀1–4 与 workspace 合一已完成——`mother/` 并入 `crates/`（引擎跑在 `crates/server`
> 进程内，母体刀2）；出厂整树真源=仓里 `home/`（结构刀②③④：烤线整树拷，内嵌种子
> 退役）。「没做」三项均已做或改道；真机段与硬约束照旧有效。

## 以哪份为准

| 文件 | 管什么 |
|------|--------|
| **`docs/FS.md`** | **唯一**的机上文件树。母体 / 助理 / tools / providers / peers / run |
| 根 `README.md` + `AGENTS.md` crate 表 | 引擎怎么编译、家目录环境变量（`mother/` 已并入 `crates/`，2026-09-24） |
| `crates/clone/CLONE-FORMAT.md` | 助理文件夹里有哪些文件（分身翻本） |
| `docs/HARDWARE.md` | 真机实验日志。只追加目击，不写「应该会」 |
| `docs/DECISIONS.md` | 已锁决策（boot、许可、不进 git 的固件） |

**不要当现行产品读（已删，2026-09-24 整档）：** 原 `docs/ARCH.md` / `docs/ARCHITECTURE.md` 的「化身 = 用户切来切去的脸」「workspaces」「D16 派给字典序第一个化身」是旧环，已被 FS.md 替换。

仓：`/Users/sophiehe/Documents/aginxos-next`  
独立仓 `~/Documents/aginx/aginx-carrier` **停作产品**，不要再往那边加功能。

---

## 产品（三句话）

1. 人只对**母体**说话。Talk 圆就是 OS。没有桌面图标当家。
2. 一个人很多**助理** = `/home/workflows/<名>/`（分身那套文件夹：SOUL、profile、flows）。母体设置性格和干什么，然后派活。用户不切脸。
3. 别人 = `agent://`（`/home/peers/`）。Codex/Claude/Grok = **provider CLI**（`/home/providers/`）。普通命令 = `tools/`。没有 `skills/`。

机上 `AGINX_HOME=/home`。不要再搞 `~/.aginx`。

树的正文在 `docs/FS.md`，这里不抄第二份。

---

## 代码做到哪

已做：

- `mother/` = aginx-carrier 源码副本（无 uniffi / 无 `.git`）。自有 Cargo workspace。
- `mother/crates/types`：`home_dir()` 优先 `AGINX_HOME`，默认 `/home`；助理根 `{home}/workflows`。`cargo test -p carrier-types` 在 `mother/` 下能编。
- `.gitignore` 有 `/mother/target`。

**没做（下一刀软件）：**

- `mother/` **没有**链进 `crates/server`（aginx-server）。机上跑的还是旧前台：`crates/server/src/mother.rs` + `aginx agent create` 那种 workspace 化身。
- 仓里还没有烤进镜像的 `/home/SOUL.md` 默认树（FS.md 写了 `aginxos-next/home/`，磁盘上可能还没有）。
- 查找命令顺序（providers → tools → PATH）只是设计，router 未改。

所以：**文档是新的，活系统还是旧的。** 不要在 term/voice 里先做「切助理」。先把母体引擎接到 server，再让 Talk 只打母体。

---

## 真机（enchilada）

实验机 OnePlus 6，serial `b0d9f7fe`。槽 a = L0（`6.11.0-sdm845`，NCM `root@10.9.8.1`），槽 b = Lineage。

2026-09-22：曾卡在 fastboot，槽 a **unbootable**。`fastboot set_active a` 后 L0 回来，屏亮，pcmC0D0p/D1c 在。**succ_a 仍 0**——再冷启动失败会把 a 标死。

录音：喇叭通；slim 采集仍经常全 0。不要热换 `q6afe`、不要 unbind slim-ngd。现役 slim SET_PARAM 24 字节；16 槽已被否。详情只写在 `docs/HARDWARE.md`。

软件架构和麦是两条线。用户当时选：先记迁移、继续搞麦；麦未完，母体也未接线。

---

## 硬约束（抄自 AGENTS / 实验）

- 编译成功 ≠ 上机成功。上机结果只进 `HARDWARE.md`。
- 不提交 vendor 固件；不 wipe userdata；不 flash 除非这次需要。
- 闸 serial `b0d9f7fe` 再动 fastboot。
- 不发明没探针过的节点。
- 不把 `docs/FS.md` 再抄一份到别处当第二真相。改树只改 FS.md。

---

## 建议下一刀（等用户点）

1. **麦**（若继续 L0）：SSH 通了再测第一段 TX；CAF slim slave PORT 要盘上换 q6afe + 硬重启，禁止热加载。
2. **母体接线**（若切软件）：`crates/server` 链 `mother/`；废弃 workspace 化身入口；烤默认 `/home`；Talk 无名发言永远留在母体。
3. 不要两条同时大改。
