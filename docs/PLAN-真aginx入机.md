# 真 aginx 入机方案（已批，2026-09-27「可以的」）

2026-09-27 起草。**已获用户批准**——五刀按序开工；本文件是五刀的
章程（刀1 出包 2026-09-27 当日落账）。

---

## 一、设计裁定（09-27，用户原话/转述）

1. **系统即 agent，无 `me` 实体**。系统本身没有 workflows 也能干活；
   workflows/<名>/ 只是让活更专业。`home/workflows.md` = 助理能力注册表、
   **唯一真源**（为快：一次 file_read 替代扫目录+N 个 template.json）；
   安装一个助理时**更新数据库**并同步写注册表。
2. **agent:// 泛解析在 aginx 里设置**。`agent://<机>.relay.aginx.net/<名>`
   由 aginx 的注册表解析到 workflows/<名>/（类似泛解析+二级域名）；
   **根是否对外是 aginx 里的权限设置，正常情况下不对外的**。
3. **真 aginx 原样入机，不搞 aginx-gateway**。aginx / agc / aginxbrowser /
   aginxbrain 是四个独立项目、各有专人开发；AginxOS 是装配工（出包、
   灌配置、写注册表），不是仿制者。`crates/gateway`（N5 第六单元）是
   aginx 的仿制品 → 退役。
4. **ACP 的家在 aginx，不在 aginx-carrier**。aginx 通过方言翻译器直接
   驱动任意 CLI——codex 只需 `~/.aginx/agents/codex/aginx.toml` 一份配置
   即入网（`output="codex-exec-json"`），零码、不经过 carrier、不经过母体。
   aginx-carrier 只是接入包之一。
5. **系统就是系统，系统本身就是智能体**（09-27 三次澄清定型）。有了
   系统才可以安装 codex；不装 codex 系统也是智能体——只是当前系统
   智能体的能力（母体+brain）没有 codex 强。**codex / workflows 助理
   都是装进系统的装备，不是平级智能体**：装上=系统变强/更专业，卸掉
   系统照旧是智能体。对外对话的主角永远是系统本人；`/<名>` 条目只是
   「直接使用某件装备」的便捷直通面。

---

## 二、现状证据

### 2.1 手机侧问题（仿制品的账）

| 问题 | 证据 |
|---|---|
| 唯一门=relay_secret，无 per-client 鉴权 | crates/gateway/src/agent.rs 头注「本网关的鉴权是第 0 层 relay_secret 单门，外部层无 per-client 鉴权态」；initialize 恒回 `authenticated:false` |
| 只实现 initialize+prompt，其余 -32601 | 同上 |
| **裸根可达**：prompt 的 avatar 可选 → UDS `{"op":"send"}` → 母体光标 | gateway/src/agent.rs:92-96；server/src/ops.rs「send 不带 avatar = 住（当前光标）」 |
| 无 access/权限字段 | crates/gateway/src/config.rs（Config 只有 host/port/tls/heartbeat/reconnect/turn_timeout） |
| `me` 实体残留三处 | ①`seed_system_me`（crates/carrier/src/wiring.rs:201，workspace=家根）②入站兜底按名找 me（runtime/src/plugin/bridge.rs resolve_fallback）③weixin 会话 `bind_agent="me"`（设备实测 workflows/me/senders/…） |
| 花名册=扫目录，非注册表 | server/src/host.rs:224-248 对账扫 workflows/*；front.rs roster()=目录清单 |

### 2.2 生态 aginx（正解，~/Documents/aginx/aginx）

- **权限模型**：`AccessMode { public / protected / private }`，**默认
  private**（src/config/mod.rs:39-49）。三档身份：Bound（配对设备）/
  Authorized（scoped token：`allowed_agents` + `allowed_methods` +
  `allow_system`）/ consent 流（requestAccess + auto_approve）。鉴权链
  is_allowed（src/acp/handler.rs:90-137）：admin 五法 owner-only → safe
  法放行 → methods 白名单 → system 法须 allow_system → prompt 须
  agent ∈ allowed_agents。**「默认不对外」在真品是结构性的。**
- **裸根结构性不可 prompt**：prompt 的 `agent` 参数必填，Unknown agent
  直接 -32601（handler.rs:580-607）。「根=系统本人」不是现成选项。
- **执行面 = 每 prompt spawn 一次进程**（PromptAdapter，src/acp/adapter/
  mod.rs）：stdin 喂消息 → stdout 逐行按方言翻译成 ACP chunk → 终帧
  （收割真 session_id/成本/轮数）；超时杀进程、断连 ≤1s 杀进程、
  收件先落盘（丢件柜台 spool）+ 成功轮记账（台账 ledger）。
- **方言翻译器注册表**（src/acp/adapter/translate.rs）：`raw` /
  `claude-stream-json` / **`codex-exec-json`**（codex `exec --json`
  JSONL：thread.started 收割 thread_id、item.completed 的 agent_message
  出文本、turn.failed 报 error；按 codex 0.151 实测形状，带全套测试）。
  头注立场：「方言知识全部住本模块的翻译器注册表里——网关核心只认
  OutputTranslator 接口，零 CLI 字样」。ACP.md §2.8 立法：**方言是形状
  契约不是身份契约**；新增方言=aginx 项目加翻译器，不归 OS 仓。
- **会话/绑定 CLI 面**：`aginx pair / devices / unbind / auth / auths /
  revoke`（main.rs）——`auth` 发 scoped token（allowed_agents 可点名）。
- **原生 AcpBackend 未完工**（acp_backend.rs 自报 not implemented）——
  方言路 + carrier 双模桥已够用，不构成阻塞。

### 2.3 Mac 本机=活样本（标准装配早已存在）

```
~/.aginx/config.toml        [server] mode=relay access=private
                            [relay] id=qi7o6bj5 domain=relay.aginx.net
~/.aginx/agents/ai-writer/aginx.toml   ← aginx-carrier clone_install 自动写
                            「分身装好即入网，删除分身时自动移除」
    id="ai-writer"  agent_type="aginx-carrier"  output="claude-stream-json"
    [command] path=…/aginx-carrier  args=["acp","--clone","ai-writer"]
    [session] resume_args=["--session","${SESSION_ID}"]
~/.aginx/agents/claude/aginx.toml     ← 纯配置接 CLI：path="claude"
                                       args=["--print","--output-format","stream-json",…]
（copilot / gemini / opencarrier / tiny-pilot … 同理）
```

**clone_install 自动写 agents toml 就是裁定 1「安装更新注册表」的既有
实现**——手机 install 链复刻此行为即可。

### 2.4 手机已有配套（不用新造的）

- **carrier 双模桥**：crates/carrier/src/acp.rs——网关按 aginx.toml 拉起，
  stdin 嗅探：裸行=raw 方言一次问；`{jsonrpc}`=ACP ndjson 桥（内核
  lazy boot）。workflows 助理入网用这条。
- **codex 已打包在役**（第 10 包，brain 真答）——入网只差一份 toml。
- **agc 已有配对面**：`--bind <配对码>`（bindDevice → token 入本地钥匙串，
  per-网关身份）与 `--request-access`（consent 流）——private 模式切流
  后 agc 侧无需改码，走绑定即可。

---

## 三、目标架构

```
                     relay.aginx.net:8443（独立项目）
                            │ 唯一注册方 id=redfin
        ┌───────────────────┴──────────────────────┐
        │  真 aginx 守护（生态仓原样，包化）            │  ← 对外面
        │  access=private：默认不对外=挡外人            │
        │  （对外与否是权限设置；主人 Bound 永远可对话）    │
        │  · Bound=主人配对 / Authorized=scoped token  │
        │  · 每 prompt spawn 一次接入包进程              │
        │  · 台账+丢件柜台+方言翻译                      │
        └───┬───────────────────┬────────────────────┘
            │ 根=系统本人(本体)    │ /<名>=装备直通面(装了才有)
   ┌────────┴───────────┐  ┌─────┴──────────────────────┐
   │ 系统智能体（永远在）    │  │ 装备条目（可装可卸）           │
   │ 引擎=母体+brain       │  │ codex=强臂(codex-exec-json)  │
   │ 当前弱于 codex        │  │ workflows/<名>=专业化         │
   │ 但可把活派给右边的装备  │  │ 条目=一份 aginx.toml          │
   └────────┬───────────┘  └────────────────────────────────┘
            │ 同体（系统智能体的常驻形态）
        ┌───┴───────────────────────────────────────┐
        │  常驻母体 aginx-server（UDS 前台不动）         │  ← 本地面
        │  term / voice / weixin 通道 / cron 晨报      │
        │  （无 me 门牌；发现面=workflows.md）           │
        └────────────────────────────────────────────┘
```

要点：

- **本体与装备**：系统智能体=本体，永远在、根上必可对话；codex/助理=
  装备，装了才有直通面、卸了系统照旧干活（不装 codex 的机器根轮与
  本地面一切照常）。「默认不对外」挡的是**外人**（private+配对/token），
  不是把系统本人藏起来——主人 Bound 永远可对话。
- **系统智能体自己也能用装备**：根轮由系统接，系统可再把活派给
  codex/助理（agent_* 派活线，刀5 之后的引擎工作）——这才是「装了
  codex 系统变强」的完整含义。
- **外面/内面分家**：外部流量不再直达母体 UDS 前台（面收窄）；根轮=
  系统条目按同一接入包机制拉起的一轮系统对话（一轮一进程、aginx
  台账续话）；常驻母体仍是本地面+定时面（voice/weixin/cron）的宿主。
  常驻与按需轮都是系统智能体的形态。
- **注册表=泛解析的实体**：`agent://redfin.relay.aginx.net/<名>` 的 <名>
  就是 agents 注册表条目 id。新装备落一份 toml 即有直通面，地址层
  零维护。两类条目并存：装备直连（codex…）与 carrier（workflows 助理）。
- **安装期三写对齐**（裁定 1）：安装 workflows/<名> → ①kernel/DB 照常
  记账 ②`home/workflows.md` 加一行（能力注册表）③agents 目录落
  `<名>.toml`（对外入网）。卸载反之。
- **根的地址机制**（裸根 vs /system 条目）见 §五-1——**本体已定**，
  待选的只是机制。

---

## 四、整改方案（五刀，各带验收门）

### 刀1 出包——生态 aginx 上 aarch64-musl

- 生态仓 `~/Documents/aginx/aginx` **只构建不改码**（cargo-zigbuild
  aarch64-unknown-linux-musl 静态；纯 Rust+tokio，无 musl 障碍预期，
  以实构建为准）。若遇编译问题：写需求文档给专人，不在生态仓动手。
- 包形态（**同包换芯**，沿用 opt-in 结构）：
  `pkgs/aginx-gateway/` 改造——真身落 `/usr/libexec/aginx/aginx-gateway`
  （daemon 区，避让 D13 裸命令位）；面 `/var/bin/aginx-gateway` →
  symlink + `.aginxmd` sidecar（pair/auth/devices 运维面从这里走）；
  `[service]` 单元保留 env_file=/etc/aginx/env + secretd 弱依赖结构。
- 数据/配置落位（全部现成机制，零源码改动）：
  - 配置 `-c /etc/aginx/gateway.toml`（0600：relay id=redfin、
    relay_secret、access=private、auth.jwt_secret 新生成）——secrets
    不进包不进库，刷机日灌注。
  - 状态 `AGINX_DATA_DIR=/var/lib/aginx/gateway`（绑定台账、丢件柜台，
    归 N5 状态世界）。
- **验收**：host `./scripts/check.sh` 绿；包四件套可签可装；musl 静态
  ldd 自证。

### 刀2 redfin 切流换装

- 纪律（relay 按 id 路由，redfin 同时只能一个注册方）：
  ①停旧 aginx-gateway 单元 → ②装新包 → ③写 /etc/aginx/gateway.toml
  （relay_secret 从旧 env/config 搬入）→ ④`aginx-gateway pair` 出码 →
  ⑤Mac `agc agent://redfin.relay.aginx.net --bind <码>` 入钥匙串。
- **验收（含负例）**：
  - 正例：Mac agc prompt 真答（先拿 codex 或 scout 条目试）；
    sessionId 续话两轮。
  - 负例 A：无配对无 token 的客户端 prompt 被拒（private 生效）；
  - 负例 B：裸根（不带 agent）prompt 被拒（agent 参数必填）；
  - 负例 C：scoped token 只放行 allowed_agents 点名的助理。
- 收据落 `docs/HARDWARE.md`（只本地）。

### 刀3 注册表铺条目（装备入网）

- **codex 条目先行**（管路验收：验证真 aginx 全链——spawn、方言翻译、
  thread_id 续话、private 挡外人）：`agents/codex/aginx.toml`
  （`output="codex-exec-json"`、args `["exec","--json"]`、resume_args
  `["resume","${SESSION_ID}"]`）。**定性：codex=装进系统的强臂，条目是
  它的直通面**；不装它系统照旧是智能体（根轮与本地面不受影响）。
- **系统条目=根的执行面，主角**（依赖刀5 系统本人化）：注册表落系统
  条目（id 见 §五-1），command=「一轮系统对话」的 CLI 面——外部根轮
  由系统本人接。codex 条目只是管路验收，系统条目才是产品本体。
- **install 链三写**：crates/clone 安装链在装 workflows/<名> 时同步写
  ①DB ②`home/workflows.md` 行 ③`agents/<名>.toml`（carrier 条目：
  `command=aginx-carrier acp --clone <名>`，复刻 Mac clone_install）。
- **验收**：codex 直通两轮续话；装一个测试助理 → agc 直寻址真答；
  卸载 → 条目与注册表行同步消失；**不装 codex 时根轮/本地面照常**
  （系统智能体不依赖任何装备）。

### 刀4 旧线退役

- `crates/gateway` 删除；镜像 manifest 与公共镜像收尾（aginx-gateway
  包被刀1 换芯吸收，包名不变、版本跳 v0.2.0）。
- enchilada：回网后同配方换装（挂账）。
- **验收**：仓内 grep 无 crates/gateway 引用；check.sh 绿；设备无旧
  单元残留。

### 刀5 no-me 线（引擎侧，可并行）

- me 退场三处：`seed_system_me` 退场（系统本人不是注册助理）、入站
  兜底改「系统本人」语义、weixin `bind_agent` 默认值改掉。
- 花名册发现面改读 `home/workflows.md`（唯一真源；DB 只在安装时写）。
- 根语义落位：按 §五-1 拍板结果实施。
- **验收**：设备上无 me 目录/无 me 注册行；workflows.md 增删助理后
  派活面即变；现有验收脚本全绿。

---

## 五、待拍板（动手前需要用户确认）

1. **根=系统本人的地址机制**（本体已由裁定 5 定死：根必是系统本人、
   必可对话——「暂不表达根」出局，根不可对话=产品缺本体。只选机制）：
   - A. 先行：注册表落系统条目（id 待定，如 `system`），command 指向
     「一轮系统对话」的 CLI 面——`agent://redfin.relay.aginx.net/system`
     即刻可用，零上游依赖，随刀5 落地；
   - B. 归宿：提请 aginx 项目（专人）做「默认条目/泛解析根」特性——
     裸根 `agent://redfin.relay.aginx.net/` 直达系统本人，语义最纯，
     依赖上游排期；
   - 建议路线 **A→B**（先可用、后归纯）。「默认不对外」与根可对话不
     矛盾：挡的是外人，主人 Bound 永远可对话系统本人。
2. **同包换芯 vs 新包名**：方案取同包名 `aginx-gateway`（opt-in 结构、
     secretd 依赖、env 灌注全沿用）；若嫌名字带仿制品原罪，可改名
     （如 `aginx-relay`），连带镜像/manifest/两机操作。
3. **切流时机**：redfin 专日（停机窗口短、可回退=重起旧单元）vs 随下
     一 bake 一起。
4. **workflows 助理对外默认姿态**：Bound（主人设备）全放行；第三方走
     `aginx auth` 点名 allowed_agents 白名单——默认是否给任何助理开对外？
     （建议：默认只 Bound，第三方按需发 token。）
5. **enchilada 同步**：随刀4 一起，还是继续挂账。
6. **生态仓纪律确认**：遇 bug/缺特性一律写需求文档（docs/REQ-*.md），
     本仓与设备侧只做构建、包装、配置——确认无例外。

---

## 六、风险与纪律

- **relay id 唯一注册方**：切流序=先停旧再起新（刀2 已固化）；两机
  （redfin/enchilada）各持各 id，互不串。
- **secrets**：relay_secret / jwt_secret 落 `/etc/aginx/gateway.toml`
  （0600），刷机日灌注不进包不进库；泄漏即换（AGINXBRAIN key 前科
  在册，换装即轮换）。
- **行为变化明示**：外部根轮=**跟系统本人对话**（系统条目拉起一轮
  系统对话，系统可再把活派给装备）；装备条目=直接使用该装备（独立
  进程、aginx 台账+resume_args 续话）。旧的「外部轮一律打到母体光标
  D16 派活」退场——主角是系统本人，装备各就各位。晨报/weixin 等
  cron 面不受影响（仍走母体进程内）。
- **冷启开销观察项**：装备直通轮每轮 spawn 一次接入包进程（Mac 同款
  模型）；carrier 条目含内核 boot，首轮延迟待实测，重了再议（如
  AcpBackend 完工后切长驻——那是上游进度）。根轮同受一轮一进程的
  冷启影响，同口径实测。
- **AcpBackend 未完工**：走方言路（codex-exec-json / raw / 双模桥），
  不构成阻塞；若要用票据借用轮（prompt_borrowed）已可用。
- **每刀过 `./scripts/check.sh`**；设备行为变化收据落 docs/HARDWARE.md
  （只本地提交）；推送只推代码尖。
- **D13/D14/L0 合规**：包形态、真身居 libexec、面带 sidecar；仓内零
  机器串；不动 image svc.d（2 单元上限不变）。

---

*确认方式：逐刀点头或改批；§五 六问逐条拍板后，按刀序开工。*
