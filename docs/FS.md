# AginxOS 文件结构（2026-09-24）

交接入口（做到哪、别读哪）：`docs/HANDOFF-母体.md`。本页只定机上树。

人只对**母体**说话。一个人有很多**助理**：母体是总管，助理是编制。外面另有别人的 `agent://`。

| | 是什么 | 不是什么 |
|---|---|---|
| **母体** | `/home` 根上的 SOUL / MEMORY / 对人会话。总管 + 门面 | 助理之一、再套一层 mother 目录 |
| **助理** | `/home/workflows/<名>/`。分身格式的翻本：性格、干什么、自己的 flows | 用户直接切过去聊天的「第二张脸」；不是 skill 包 |
| **tools** | 普通 CLI | 助理、provider |
| **providers** | Codex / Claude / Grok 这类 agent CLI | 助理、对话对象 |
| **peers** | 别人的 `agent://` | 自己家里的助理 |

母体**设置**每个助理：干什么（profile / default_flow）、性格（SOUL / system_prompt）。派活是母体的事。用户不对助理点名当操作系统，也不对 Codex 说话。

没有 skill。分身格式已拒收 `skills/`，流程在助理自己的 `flows/<name>/flow.md`。

workspace 仍是执行工位（`run/`），用户脸上没有。

旧 `~/.aginx/workspaces/<化身>/` 退役。二进制落位细节见 `rootfs/README.md`。

机上 **`AGINX_HOME=/home`**。这台手机没有第二个用户，家目录就是母体，不必再藏一层 `.aginx`。开发机可把 `AGINX_HOME` 指到别处（例如 `~/aginx-home`），和手机不同名。

---

## 机上三层

```
/usr/bin            出厂 tool（aginx、aginx-term…）+ .aginxmd
/usr/libexec/aginx  守护（server / runtime / gateway / secretd）
/var                机器账：日志、模型、热换 bin
/home               母体的家：总管人设 + 助理编制 + tools / providers / peers
```

OTA 不覆盖 `/home`。

---

## `/home`

```
/home/
├── SOUL.md                 # 母体：对主人是总管，对外是门面
├── MEMORY.md               # 母体的长期记忆索引
├── config.toml             # 预留（v0 未接线——brain 真源=brain.json/env 桥）
├── sessions/               # 人对母体的会话账
│   └── main.jsonl
├── data/                   # kernel 自账（carrier.db：会话史/记忆/计量）
├── cards/                  # 首页卡片信封（定时任务产物；term 扫目录列���框，删卡片=删文件）
├── photos/                 # 相册（相机照片归档）
├── files/                  # 文件（普通文档，母体可见可管）
│
├── tools/                  # 普通 CLI
│   └── <name>/
│       ├── <name>
│       └── <name>.aginxmd
│
├── providers/              # Codex / Claude / Grok …
│   └── <name>/
│       ├── <name>
│       └── <name>.aginxmd
│
├── workflows/              # 助理编制。一个目录 = 一个助理（分身翻本）
│   └── <name>/
│       ├── template.json   # 干什么、default_flow
│       ├── profile.md      # 名称、职责
│       ├── SOUL.md         # 性格。母体可改
│       ├── system_prompt.md
│       ├── MEMORY.md       # 这个助理自己的知识索引
│       ├── EVOLUTION.md
│       ├── knowledge/
│       ├── flows/          # 这个助理会的流程（不是 skill）
│       │   └── <flow>/
│       │       ├── flow.md
│       │       ├── references/
│       │       ├── examples/
│       │       └── scripts/
│       └── agents/         # 可选：助理内部的子代理
│
├── run/                    # 某次执行的工位，冷了可删
│   └── <run-id>/
│
├── peers/                  # 别人的 agent://
│   └── <id>.toml
│
└── secret/
```

出厂 CLI 已经在 `/usr/bin`（同样 `.aginxmd`）。母体找命令：**先 `providers/`，再 `tools/`，再 PATH**。不必把 `aginx-qr` 再复制一份进 `/home`。

### cards — 首页卡片 + 结果信封

定时任务自备数据落盘 JSON 进 `cards/`（信封 `title/template/data/created/source`），term 扫目录列小框，删卡片=删文件。按住说话链的**结果信封**不落盘：母体 send 回包整段是 JSON 对象、带非空 `template` 与 `data`（可选 `say` 上脸一句话）即信封，voice 原样 POST /open 交浏览器按模板出页；非 JSON 回包走 `reply` 模板兜底。不许把数据拆成 markdown 改写成文章再包回 HTML。浏览器缺模板（/open 返 `unknown_template`）→ voice 报母体安排写一次并登记。

### tools — 普通 CLI

git、aginxbrowser 客户端、扫码装的小命令。二进制 + sidecar，不是人格。用户不对它说话。

### providers — agent CLI

从 Omarchy 拿来的分层：Codex / Claude / Grok 是 **provider**，不是分身。各有自己的二进制；session/用量留在该目录下或 `/home/run/`，不写进 `SOUL.md`。

```
/home/providers/codex/
  codex
  codex.aginxmd
```

母体说「用 Codex 改这个」→ spawn `/home/providers/codex`。

### workflows — 助理（分身翻本）

一个人很多助理。目录形态与 carrier `CLONE-FORMAT.md` 同一套，只是所有权在母体：

- **干什么**：`profile.md`、`template.json` 的 description / default_flow  
- **性格**：`SOUL.md`、`system_prompt.md` —— 母体设置、可改  
- **会做的事**：`flows/<flow>/flow.md`（`SKILL.md` 只作旧名回退，不再生成）  
- **知识**：`knowledge/`、`MEMORY.md`

用户跟母体说话。母体看得见编制、派活、收回结果。助理不是第二张系统脸，也不是 `agent://` 上的别人。

新助理 = 在 `workflows/` 下多一个目录（扫码装的是助理包，不是人设操作系统）。顶层 `skills/` 拒收。流程步骤可以调用 tool 或 provider 名，二进制不进这个目录。

出厂助理（clone-creator）随镜像烤进 `workflows/`——真源是仓里 `home/workflows/clone-creator`，金样本闸钉死件数与安装校验；内嵌种子机制已退役。

`/home/SOUL.md` 是总管自己，不要再做一个 `workflows/me`。

### peers

```toml
url = "agent://shop.example.relay.aginx.net/front"
name = "某店"
```

对方的 tool/workflow/记忆在他家。

### run/

这一次任务的 cwd、中间文件、CLI 的 scratch。不是 `providers/codex/workspace`。

---

## 系统树（身体）

```
/
├── usr/bin/                # 出厂 tools + 路由器
├── usr/libexec/aginx/      # 母体进程
├── usr/share/aginx/
├── etc/aginx/
├── var/log/aginx-svc/
├── var/models/
├── var/bin/                # 热换中的 term/voice 等
└── home/                   # AGINX_HOME
```

机型：`devices/<codename>/`，不进家目录人设文件。

---

## 母体引擎（原 aginx-carrier）

独立仓 `~/Documents/aginx/aginx-carrier` **停作产品**。源码已并入本仓 `crates/`（2026-09-24 workspace 合一，`carrier-*` 各件进根 workspace），引擎跑在 `crates/server`（aginx-server）进程内，不再安装 `aginx-carrier` 二进制。

默认家目录已改为 `AGINX_HOME` → `/home`；助理根已改为 `{home}/workflows`。按 clone 名的 ACP 桥不再作为 OS 入口。

---

## 仓（aginxos-next）

```
aginxos-next/
├── crates/                 # 单一 workspace（OS 面 + 母体引擎 carrier-* 面）
│   ├── server/             # 母体前台，引擎在进程内
│   ├── kernel/ runtime/ …  # 母体引擎件（carrier-kernel / carrier-runtime / …）
│   ├── gateway/            # agent://
│   ├── router/             # /usr/bin 宇宙
│   └── term/ voice/ …
├── home/                   # 出厂整树真源，烤线整树拷进机上 /home
│   ├── SOUL.md MEMORY.md   # 母体人格（总管 + 门面）
│   ├── photos/ files/      # 相册、文件
│   └── workflows/          # 出厂助理编制（clone-creator）
├── devices/<codename>/
├── rootfs/
├── pkgs/                   # 出厂 tool 的签名包 → /usr/bin
└── docs/FS.md
```

用户后来装的 Codex 不进 git，进机上 `/home/providers/`。普通 CLI 进 `/home/tools/`。

---

## 对照

| 旧 | 新 |
|---|---|
| 化身 = 用户切来切去的脸 | 助理 = 母体编制，用户只对母体 |
| 分身目录（SOUL / profile / flows） | **`/home/workflows/<名>/` 同一套文件** |
| Codex = 分身 | **provider** |
| `skills/` | 助理自己的 `flows/<flow>/flow.md` |
| 点名切化身 | 对母体说话；母体设置助理、派活；别人走 `agent://` |
