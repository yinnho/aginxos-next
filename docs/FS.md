# AginxOS 文件结构（2026-09-24 立；2026-10-02 修宪换代）

世界观真源 = `docs/DESIGN.md`（数字世界里的人；#68 刀③ 修宪，本页
世界观段已折叠进去）。本页只定**机上树**——树细节的唯一权威。

一个**系统**——这个人在数字世界里的本体（分身+助理同一实体）。
它的家就是 `/home`。能力面是它的侧面（`workflows/`），在场是它的门
（`channels/`），引擎是雇来的手（`providers/` 真身在 `/var/bin`）。
外面另有别人的 `agent://`（`peers/`）。

| | 是什么 | 不是什么 |
|---|---|---|
| **系统** | `/home` 根人格（AGENTS.md，命名定谳 2026-10-02）+ MEMORY + 对人会话。默认应答者 + 对外门面；本体，不是管理员 | 管人派活的「总管」、再套一层 mother 目录 |
| **能力面** | `/home/workflows/<名>/`。系统的角色侧面：晨报面、斥候面、模板面——AGENTS.md=人格+规程 | 员工名册、用户直接切过去聊的「第二张脸」、skill 包 |
| **tools** | 普通 CLI | 能力面、provider |
| **providers** | Codex / Claude / Grok 这类 agent CLI 的人格壳层：`providers/<名>/<名>`（几行 shim + inline 卡） | 能力面、对话对象 |
| **channels** | 在场：`channels/<名>/`，一频道=一目录+一包（DESIGN.md §四） | 引擎本体、人的操作面 |
| **peers** | 别人的 `agent://` | 自己家里的能力面 |

能力面**是系统的侧面**：干什么（AGENTS.md 规程 / default_flow）、
什么性格（人格段），系统自己设置、可改。派发面（gateway + `agents/`
名册）是**机器结构**不是人格：拉起引擎、记账、管权限，自己不思考。
用户不对能力面点名当操作系统，也不对 Codex 说话。

没有 skill。能力面格式拒收 `skills/`，流程在自己的 `flows/<name>/flow.md`。

工位仍是 `run/`，用户脸上没有。

旧 `~/.aginx/workspaces/<化身>/` 退役。二进制落位细节见 `rootfs/README.md`。

机上 **`AGINX_HOME=/home`**。这台机器没有第二个用户，家目录就是系统本体，不必再藏一层 `.aginx`。开发机可把 `AGINX_HOME` 指到别处（例如 `~/aginx-home`），和设备不同名。

---

## 机上三层

```
/usr/bin            出厂 tool（aginx、aginx-term…）+ .aginxmd
/usr/libexec/aginx  守护（server / runtime / gateway / secretd）
/var                机器账：日志、模型、热换 bin
/home               系统本体的家：人格 + 能力面 + 在场 + peers（provider 真身在 /var/bin）
```

OTA 不覆盖 `/home`。

---

## `/home`

```
/home/
├── AGENTS.md               # 系统人格（定谳 2026-10-02 统一此名；SOUL.md 物理改名随刀④）
├── MEMORY.md               # 人格真源索引（长期记忆）
├── config.toml             # 预留（v0 未接线——brain 真源=brain.json/env 桥）
├── sessions/               # 人对系统的会话账
│   └── main.jsonl
├── data/                   # kernel 自账（carrier.db：会话史/记忆/计量；人格真源，拆迁后独立成一等件）
├── cards/                  # 首页卡片信封（定时任务产物；term 扫目录列���框，删卡片=删文件）
├── photos/                 # 相册（相机照片归档）
├── files/                  # 文件（普通文档，系统可见可管）
│
├── tools/                  # 普通 CLI
│   └── <name>/
│       ├── <name>
│       └── <name>.aginxmd
│
├── providers/              # agent CLI 的人格壳层（轻 shim 指到 /var/bin 真身）
│   └── <name>/
│       └── <name>          # 几行脚本 + inline 卡，不放大件
│
├── channels/               # 在场（DESIGN.md §四；一频道=一目录+一包）
│   └── <名>/
│       ├── channel.toml    # 类型 + 默认绑定 + 策略
│       └── senders/<uid>/  # 会话状态（session.json 绑定=字段；gw.json 桥会话）
│
├── workflows/              # 能力面。一个目录 = 一个角色侧面（系统的工作位）
│   └── <name>/
│       ├── AGENTS.md       # 人格+规程（codex 工位原生名，刀1 起在役；对外叫「助理」）
│       ├── template.json   # 干什么、default_flow（clone 遗产，随刀④拆迁处置）
│       ├── profile.md      # 名称、职责（clone 遗产）
│       ├── SOUL.md         # 旧名人格文件（→AGENTS.md，物理迁移随刀④）
│       ├── system_prompt.md
│       ├── MEMORY.md       # 这个能力面自己的知识索引
│       ├── EVOLUTION.md
│       ├── knowledge/
│       ├── flows/          # 这个能力面会的流程（不是 skill）
│       │   └── <flow>/
│       │       ├── flow.md
│       │       ├── references/
│       │       ├── examples/
│       │       └── scripts/
│       └── agents/         # 可选：能力面内部的子代理
│
├── run/                    # 某次执行的工位，冷了可删
│   └── <run-id>/
│
├── peers/                  # 别人的 agent://
│   └── <id>.toml
│
└── secret/
```

出厂 CLI 已经在 `/usr/bin`（同样 `.aginxmd`）。系统找命令走 router 三层路由（resolve.rs 实装）：**先 `<home>/providers/<名>/`，再 `<home>/tools/<名>/`，再 PATH 的 `aginx-*`**——provider 走裸名路由（`codex` 就是 `codex`），aginx 宇宙带姓。不必把 `aginx-qr` 再复制一份进 `/home`。

### cards — 首页卡片 + 结果信封

定时任务自备数据落盘 JSON 进 `cards/`（信封 `title/template/data/created/source`），term 扫目录列小框，删卡片=删文件。按住说话链的**结果信封**不落盘：系统 send 回包整段是 JSON 对象、带非空 `template` 与 `data`（可选 `say` 上脸一句话）即信封，voice 原样 POST /open 交浏览器按模板出页；非 JSON 回包走 `reply` 模板兜底。不许把数据拆成 markdown 改写成文章再包回 HTML。浏览器缺模板（/open 返 `unknown_template`）→ voice 报系统安排写一次并登记。

### tools — 普通 CLI

git、aginxbrowser 客户端这类。**首选走 pkg 进 `/var/bin`**（与 provider
同路，只是 sidecar 无 kind 标记）；`/home/tools/` 仅作扫码装小命令的
选装位。二进制 + sidecar，不是人格。用户不对它说话。

### providers — agent CLI

从 Omarchy 拿来的分层：Codex / Claude / Grok 是 **provider**，不是分身。
2026-09-27 裁决（**Mac 方式**）——真身与壳层分离：

- **真身走 pkg 进 `/var/bin`**（`aginx-pkg opt-in codex`），与 Mac 同构
  （npm 装、binary 在 PATH）；pkg 线自带签名/版本/更新/镜像。
  233MB 大件不进 /home——backup/state tar 不背，与「模型不进 tar」
  同纪律。
- **`providers/<名>/<名>` 是人格壳**：几行 shim + inline `# aginx:` 卡，
  exec 到 /var/bin 真身；router tier-1 裸名路由由此生效（系统按名
  spawn）。没人在意按名 spawn 前可以不建壳。
- **状态家在 `/home/.<名>`**（如 `/home/.codex`）；session/用量留在
  状态家或 `/home/run/`，不写进人格文件（AGENTS.md）。

系统说「用 Codex 改这个」→ 按名 spawn（tier-1 壳层解析到真身）。

**模型分工**（2026-09-27 立法，#403 实践定型）：

- **产出是对话的，系统自己答**：回答、摘要、检索取材、markdown 成稿——
  brain 直答，会话内收口。例外：系统自己的工具到不了的墙外数据
  （x.com 等直连必死），派**斥候能力面**（`workflows/scout`：它派 grok
  走隧道带浏览器 MCP，只当眼睛），原文带回后成稿仍系统亲笔（#408）。
- **产出是工件的，派 provider**：HTML 模板、代码生成这类文件工件长活
  （预计超 4 分钟）——派 codex（grok 不接工件活：模板质量不过关，#402）。
- **派工必须异步**：flow 只跑派工脚本立即返回，后台腿干活，完成以卡片
  回话。工具层 shell 300s 硬顶、relay turn gate 110s 都撑不下长活；
  轮询等待、系统手写工件（替 provider 代笔 HTML）都违宪。斥候取材
  同款异步（派工秒回、回头取果，后台腿自带 280s 闸）。

### channels — 在场

一频道 = 一目录 + 一包（DESIGN.md §四；#69 改形，2026-10-02）。目录只有
数据和绑定，协议腿住 opt-in 包（`aginx-channels` 唯一机身，weixin=
第一腿）；裸 L0 零频道，装频道 = 落目录 + opt-in。文件夹即注册表。

- **绑定以频道为轴**：会话住 `channels/<名>/senders/<uid>/`，绑哪个能力面
  是记录里的字段（`bind_agent`）——换绑=改字段，不搬家（推翻旧
  `workflows/<分身>/senders/` 以分身为轴的搬家律）。
- **频道不感知引擎**：桥只认「把文本交给某 agent、拿回文本」窄接口，按
  channel.toml 的 `default_agent` + 会话绑定路由到 gateway 名册（本地
  ACP 口 `127.0.0.1:8686`，`aginx-gateway-local` 包），引擎随名册条目换。

### workflows — 能力面（角色侧面）

一个系统很多能力面：晨报面、斥候面、模板面——同一人格的侧面，对外
叫「助理」。**在役形态 = codex 工位**：`AGENTS.md`（人格+规程）+
`flows/`，gateway 名册条目 `folder=` 指到这里，引擎随条目换（#428
spike、刀1/刀2 已证）。carrier clone 格式件（`template.json`/
`profile.md`/`SOUL.md`/`system_prompt.md`，格式真源
`crates/clone/CLONE-FORMAT.md`）是过渡遗产，随刀④拆迁处置：

- **干什么**：AGENTS.md 规程（旧：profile.md、template.json 的
  description / default_flow）
- **性格**：AGENTS.md 人格段（旧：SOUL.md、system_prompt.md）——
  系统设置、可改
- **会做的事**：`flows/<flow>/flow.md`（`SKILL.md` 只作旧名回退，不再生成）
- **知识**：`knowledge/`、`MEMORY.md`

用户跟系统说话。系统看得见自己的能力面、派活、收回结果。能力面不是第二张系统脸，也不是 `agent://` 上的别人。

新能力面 = 在 `workflows/` 下多一个目录（扫码装的是助理包，不是人设操作系统）。顶层 `skills/` 拒收。流程步骤可以调用 tool 或 provider 名，二进制不进这个目录。

出厂能力面（clone-creator）随镜像烤进 `workflows/`——真源是仓里 `home/workflows/clone-creator`，金样本闸钉死件数与安装校验；内嵌种子机制已退役。

`/home/AGENTS.md` 是系统本体自己，不要再做一个 `workflows/system`（me→system 已收，#413）。

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
├── usr/libexec/aginx/      # 系统进程（server 等守护）
├── usr/share/aginx/
├── etc/aginx/
├── var/log/aginx-svc/
├── var/models/
├── var/bin/                # 热换中的 term/voice 等
└── home/                   # AGINX_HOME
```

机型：`devices/<codename>/`，不进家目录人设文件。

---

## 引擎（商品）

引擎=商品劳力（DESIGN.md）：codex/grok/开源 CLI，真身走 pkg 进
`/var/bin`，按名 spawn。每个干活位（workflows 能力面、channels 频道
路由的 agent）由 gateway 名册条目（`agents/<名>/aginx.toml`）指认
引擎与方言（output=raw / codex-exec-json / …）——**换引擎=改条目**，
OS 面不动。刀1 晨报、刀2 模板匠已迁 codex（#429/#430）；微信频道
绑定 codex（#431，用户裁决不用 carrier）。

carrier-* 各件（2026-09-24 并入的引擎面）**退役中**：aginx-server
=前台+派活台，不是引擎；设备���仅 system 本体过渡期仍骑
aginx-carrier acp 腿（迁外部引擎待本体设计），#428 spike 已证
workflows=codex 工作目录全通（退役判据达成）。通道/词表面已随 #69
清场——channels/ 唯一家，系统不认识频道。

---

## 仓（aginxos-next）

```
aginxos-next/
├── crates/                 # 单一 workspace（OS 面 + 引擎/频道面）
│   ├── server/             # 系统前台 aginx-server（引擎退役中：前台+派活台）
│   ├── kernel/ runtime/ …  # carrier-* 引擎件（退役中，见「引擎」章）
│   ├── channels/           # 频道体系唯一家（机身+weixin 腿，#69）
│   ├── router/             # /usr/bin 宇宙
│   └── term/ voice/ call/ …
├── home/                   # 出厂整树真源，烤线整树拷进机上 /home
│   ├── SOUL.md MEMORY.md   # 系统人格（→AGENTS.md 随刀④改名）+ 真源索引
│   ├── photos/ files/      # 相册、文件
│   └── workflows/          # 出厂能力面（clone-creator）
├── devices/<codename>/
├── rootfs/
├── pkgs/                   # 签名包（tool/引擎/频道，/usr/bin 与 /var/bin 两面）
└── docs/FS.md
```

用户后来装的 Codex 不进 git，走 pkg 进机上 `/var/bin`（状态家
`/home/.codex`；要按名 spawn 时补 `providers/codex/` 壳）。普通 CLI 同路。

---

## 对照（世界观换代，2026-10-02 修宪）

与 DESIGN.md §九同源；本表只列影响树上措辞的行：

| 旧 | 新 |
|---|---|
| 母体（总管） | **系统**=数字世界里的人（本体，`/home` 根人格） |
| 助理=编制（员工） | workflows=**能力面**（同一人格的侧面；对外仍叫「助理」） |
| 通道=bot 集成 | channels=**在场**（各平台的本人） |
| 记忆=会话账 | memory=**人格真源**（越用越懂你的模型） |
| 引擎=自研母体 | 引擎=**商品劳动力**（codex/grok，可换） |
| 对外两脸 | **N 个频道** |
| SOUL.md（人格文件名） | **AGENTS.md**（定谳 2026-10-02；物理迁移随刀④） |
