# 交接：系统线——母体迁移刀法（2026-09-22）

给下一棒：**麦归别人，这条线只管系统。** 刀0–刀2 已完结（三提交未推送），刀3
进行到一半，断点在文内，照着做即可。先读 `docs/HANDOFF-母体.md` 定产品调子，
再读这份接刀。

---

## 以哪份为准

| 文件 | 管什么 |
|------|--------|
| `docs/FS.md` | **唯一**的机上文件树真源（刀3 会补三行，见下） |
| `docs/DECISIONS.md` | 已锁决策 |
| `docs/HARDWARE.md` | 真机目击。只本地提交 |
| `docs/ARCH.md` / `ARCHITECTURE.md` | 旧模型，不当现行产品读 |
| `docs/HANDOFF-母体.md` | 产品定调交接。**部分已被刀法超越**，见下节 |

**HANDOFF-母体.md 已过时段落**（别被带偏）：
- 「mother/ 没链进 crates/server」——刀2 已直调，`mother.rs` 已改名 `host.rs`。
- 「仓里还没有 home/ 默认树（烤进镜像）」——刀3 已建，但形态变了：**不是烤进
  镜像，是 `include_str!` 内嵌进 aginx-server 二进制**（理由见「刀3 布线理由」）。
- 「router 查找序只是设计」——仍是刀4 待办，没变。

---

## 定调（用户 2026-09-22 原话，宪法级）

1. **这个系统就是智能体，必须以最直接的运行方式跑**——OS 进程=agent 进程、
   盒内零 hop、间接只许在真边界、账自己记。直调、自落账皆其推论。
2. 麦（音频线）其他人做，系统线不碰麦线文件。
3. 以前的分身（化身）直接变成现在的 workflow：CLONE-FORMAT 文件夹套住
   `{AGINX_HOME}/workflows/<名>/`。aginx-carrier 迁移入系统，不再单独存在。
   人只对母体说话（Talk 圆=母体=OS）。

机上 `AGINX_HOME=/home`（刀3 落地中）。

---

## 刀法进度

| 刀 | 内容 | 状态 |
|----|------|------|
| 0 | `mother/` 子树入库（gitignore+FS.md+HANDOFF） | ✅ |
| 1 | 引擎对齐 FS.md（me 归 home 根、workspaces 残留、ACP 桥摘除） | ✅ `4fde120` |
| 2a | kernel TurnObserver（工具帧回流） | ✅ `d6ce9a8` |
| 2b | server 直调 kernel（mother.rs→host.rs、turn.rs 退役、D8 帧账、brain env 桥、外层 workspace exclude mother/） | ✅ `5743305` |
| 3 | 出厂 /home 内嵌进 server；AGINX_HOME=/home | ✅ `ed13bc5`。redfin 首启种树已目击；进程因无 brain.json 退出，已换回旧二进制 |
| 4 | router 查找序 providers→tools→PATH | 待开 |
| 5 | 手机构建面（musl+L0 尺寸；aginx-runtime 物理删除） | 待开 |

三提交（4fde120 / d6ce9a8 / 5743305）**均未推送**——推送须用户明示。

---

## 刀3 断点（六件布线，用户已批「同意」）

### 已落（编辑完成，未跑编译）

① **repo `home/` 两件** ✅
- `home/SOUL.md`：出厂母体人设。双角色——对内（主人）自留地+总管：记事、
  全景看得见 workflows/ 全体助理、派活收结果、建议主人建新助理；对外（访客）
  门面+接待：代表主人应答、编制不外泄、不越权承诺。铁律 3 条（永远是「我」、
  远程派活花主人 token 先确认、同意流只读不代批）。
- `home/MEMORY.md`：空知识索引（`# 知识索引` / `## 知识` / 暂无——随使用沉淀）。
- 用「助理」新词（旧档的「分身/克隆大师」词汇退役）。

② **`crates/server/src/host.rs`：单真源内嵌 + boot 种树** ✅
- `HOME_SOUL` / `HOME_MEMORY` 常量：`include_str!("../../../../home/SOUL.md")`。
- `fn seed_home(home)`：建 `sessions/` + 两文件缺则写、**存在即跳过**（用户编辑
  绝不覆盖）、失败只 eprintln 不炸。
- `Mother::boot` 开头 `seed_home(&home);` 插在 `bridge_brain_json` **之前**
  （kernel boot 失败退进程时树也已在位）。

③ **`host.rs` ensure_agent 关 identity 脚手架** ✅
- 一律 `manifest.generate_identity_files = false`——kernel 的 identity 7 件套
  （USER/TOOLS/AGENTS/BOOTSTRAP/IDENTITY/HEARTBEAT…）不落树。
- me 特判：`display_name="我"`、`description="母体 — 对主人是总管，对外是门面（家根身份）"`
  （对齐 wiring::seed_system_me 语义；server 直调路径不背 carrier crate，自种）。

④ **host.rs 种子测试** ✅（已写入 tests 模块，未跑）
- `home_seed_idempotent_and_no_identity_junk`：预置用户 SOUL 脏内容→boot 后原样；
  MEMORY 缺→种出厂版含「知识索引」；sessions/ 在位；me 轮跑过后家根与
  workflows/小满/ 均无 USER/TOOLS/AGENTS/BOOTSTRAP/IDENTITY/HEARTBEAT。

`include_str!` 路径按源文件 `crates/server/src/host.rs` 是三级
（`../../../home/`）。交接初稿写的四级会编出仓外，已改。

### 待做（就剩这些）——已收口

⑤ **`pkgs/aginx/pkg.toml` envs 翻转** ✅
- 现：`envs = ["AGINX_HOME=/home/.aginx", "AGINX_BIN=/var/bin/aginx",
  "AGINX_RUNTIME_BIN=/var/lib/aginx/pkgfiles/aginx/bin/aginx-runtime"]`
- 改：`envs = ["AGINX_HOME=/home", "AGINX_BIN=/var/bin/aginx"]`
- 摘 RUNTIME_BIN 的理由：刀2 直调后 server 不再 spawn aginx-runtime 子进程，
  该 env 无人读。头注顺手把「fast-agi runtime（UDS…）」等措辞续成直调口径。

⑥ **`docs/FS.md` 三处** ✅
- /home 树 `sessions/` 之后补一行：`├── data/  # kernel 自账（carrier.db：会话史/记忆/计量）`
- `├── config.toml` 行注改为「预留（v0 未接线——brain 真源=brain.json/env 桥）」
- 仓布局段 `├── home/` 描述同步为「出厂 SOUL/MEMORY 两件，server include_str! 内嵌种子」

⑦ **验证 + 原子提交** ✅
- `cargo test -p aginx-server`：25 过。
- `./scripts/check.sh`：红在 `term::tests::recover_requires_line_match_and_wraps_raw`（`background:#000`），铁律允许，未修。
- `./scripts/check.sh lint`：绿。
- `cd /Users/sophiehe/Documents/aginxos-next && CARGO_INCREMENTAL=0 cargo test -p aginx-server`
- `./scripts/check.sh`（唯一允许的红=麦线 term 自坏，见铁律）
- 提交圈：`home/` + `crates/server/` + `pkgs/aginx/pkg.toml` + `docs/FS.md`。
  带 `Co-Authored-By: Claude <noreply@anthropic.com>`，**不推送**。

---

## 刀3 布线理由（防走样）

- **出厂树长在母体进程里**：repo home/ 真源 → include_str! 内嵌进二进制。
  镜像 TREE/home 保持空、aginx 包不带 share 文件。三承诺零机制成本：裸 L0
  装包首启即出树、重刷（SKIP_STATE 清 /home）自动重建、OTA 永不碰 /home
  （state tar 打包 /home 保用户树，seed 只补缺不覆盖）。
- **config.toml 不烤**：它的读者是老 carrier CLI 的 `load_config`（config.rs），
  server 直调路径根本不读——烤进去是假档。FS.md 标「预留」。
- **brain 真源链**：kernel boot 只认 `home/brain.json`（无则 Hub 拉，再无
  BootFailed）。env 桥（AGINX_BRAIN_URL+AGINXBRAIN_API_KEY 缺席文件时一次性
  合成）已在刀2 落地（host.rs `bridge_brain_json`）。
- **identity 7 件套是污染**：`AgentManifest::default()` 的
  `generate_identity_files` 默认 true，spawn 时往 workspace 写七件杂件——FS.md
  的家根与助理形状都不认。母体 SOUL/MEMORY 由 seed_home 种出厂版，助理性格由
  create 面（soul 参数）写。
- **me 的 workspace=家根**：`KernelConfig::agent_workspace_dir` 特判
  （types/config.rs:1054）+ kernel.rs:795 spawn 特判（只建 sessions/）。
  me 的 SOUL/MEMORY 在家根直接进系统提示（kernel.rs:1290/1298 read_identity_file）。

---

## 铁律（违反=返工）

- **block_in_place**：kernel 轮路径（sender 历史富集）有 block_in_place +
  Handle::block_on 同步桥，只认 worker 线程。Mother 必须 boot 期起
  multi-thread RT（worker×2 全程复用）且轮 future 必须 `tokio::spawn` 上
  worker，外层 block_on 只等 JoinHandle；JoinError 折 BootFailed 走 done。
- **testkit 双车道**：kernel 每轮收口 inline 调 turn-summary（system 含
  "conversation summarizer"）——隐藏 LLM 消费者，stub 里与 agent 轮共吃一份
  script 必应答错位。固定应答 `INTENT/OUTCOME/FACTS: NONE` 分流（testkit.rs:67）。
- **麦线脏文件绝不碰绝不提交**：`crates/voice/`、`crates/term/`（含
  talk.rs）、`crates/hwd/`、`devices/enchilada/`（含 bringup/*）、
  `docs/HARDWARE.md`、`docs/HANDOFF-母体.md`。git add 时显式圈自己路径。
- **check.sh 唯一允许的红**＝`term::tests::recover_requires_line_match_and_wraps_raw`
  断言 `background:#000` 失败——麦线未提交改动砸了自己的测试，非本线问题，
  不碰不修。本线闸=`cargo test -p aginx-server` 全绿 + `check.sh lint` 绿。
- **双仓**：会话 cwd=/Users/sophiehe/Documents/aginxos（git 根、文档），代码在
  /Users/sophiehe/Documents/aginxos-next——**每条 Bash 命令内 cd**。
- 构建一律 `CARGO_INCREMENTAL=0`。秘密/PSK/密钥零回显。
- 设备侧（如需）：busybox awk 必 SIGSEGV（用 sed/set --）；杀进程禁
  pkill -f（用 pkill -x/pidof）。

---

## 刀4 / 刀5 剪影

- **刀4**：router 查找序 providers→tools→PATH——`agent://` 与外部件 CLI 的
  查找归位（D12：外部件一律 CLI，注册表=文件系统）。
- **刀5**：手机构建面——musl + L0 尺寸；跨 workspace 路径依赖已铺路（外层
  workspace `exclude=["mother"]`）；`aginx-runtime` 物理删除
  （build-pkg.sh aginx 分支三件 zigbuild 减两件；pkg.toml 的 RUNTIME_BIN
  已在刀3 ⑤摘掉）。

---

## 上机日备忘

- **样机是 Pixel 5（redfin）**，不是 OnePlus 6。adb `aginxosredfin`，
  fastboot / cmdline serial `13201FDD4001N8`。在跑的是槽 **b**，内核仍是
  机器自带的 4.19（`rdinit=/aginxos/trampoline`），不是 enchilada 那棵 6.11。
- 2026-09-22 收据（详见 HARDWARE.md 同日 redfin 条）：空的 `/home` 上，
  新 server 退出前种出 `SOUL.md`、`MEMORY.md`、`sessions/`，并写出
  `data/carrier.db`。没有 `brain.json`。这台机的 `/etc/aginx/env` 有
  `AGINXBRAIN_API_KEY`，没有 `AGINX_BRAIN_URL`，刀2 的桥没合成文件；
  kernel 接着要 `OPENCLONE_HUB_KEY`，进程退出码 1。
- 母体已换回 9 月 14 日的二进制，unit 仍是 `AGINX_HOME=/home/.aginx`。
  新二进制在机上 `aginx-server.seed`。种下的 `/home` 留着。旧进程继续听
  `/home/.aginx/workspaces`。要让新母体留在 ready，先有 `/home/brain.json`
  （或补上 `AGINX_BRAIN_URL` 让桥合成），再换回 `aginx-server.seed`。
- 裸 L0 装包首启的完整包安装还没做；这次是换二进制，不是重刷。

---

*本文件系统线交接档，本地维护；HANDOFF-母体.md 仍是产品调子的入口。*
