# 外部观察 Watchlist（2026-09 起）

> 2026-09-09 起本文件为活文档（一代仓 aginxos 8a502ef 封仓后迁入）。
> 文中 VIOLOOP-STUDY / EXTERNAL-SCAN-2026-09 / TERMUX-STUDY /
> OPENLOGI-STUDY 链接指一代仓 `docs/` 同名文件。

原则：**只记不依赖**——已休眠/强传染/不同赛道的外部项目，各留
一句「是什么 + 为什么记 + 触发再评估的条件」。

## OpenDuck——数据面的 MotherDuck 形态

[CITGuru/openduck](https://github.com/CITGuru/openduck)（569★，
2026-05 起休眠，MIT）：`ATTACH 'openduck:mydb'` 挂远程库、单查询
LOCAL/REMOTE 双端执行、差分存储（append-only 层+快照读）。

**记它**：卡上 8GB RAM 跑不动全量历史分析（session-events 积累/
跨分身记忆挖掘）是真实缺口，「重活上移」只答了 ffmpeg/LLM，
数据查询类重活无答案——dual execution 是这个问题的漂亮形态
（agent 写 SQL 不用知道数据在哪）。

**再评估触发**：项目复活；或 MotherDuck 模式出现成熟开源实现。
在那之前的第一动作是朴素解：大表放服务器 PG/duckdb，卡上远程查。

## TabTin——人+多 Agent 团队协作平台

[tabtin-ai/TabTin](https://github.com/tabtin-ai/TabTin)（237★，
2026-08 开源，Public Preview，**AGPL-3.0 强传染——只看思路不动
代码**）：任务交接（冻结上下文+有权限的文件引用）、人机共编
文档/表格、Agent 角色带规则/模型/Skill/记忆、治理三件套。

**记它**：①架构第三次撞款——「移动端无独立执行环境，配合
电脑 daemon」= 界面设备≠执行设备，与我们的「人手机=微信界面、
卡=身体」同构（前两次：Violoop RK3576、BYOK，见
[VIOLOOP-STUDY.md](VIOLOOP-STUDY.md)）；②**任务交接包**概念正中
我们链式 cron 流水线痛点——现在 output/<pipeline_id>/状态.md
台账是贫民版，升级方向=「上下文快照+文件引用+权限检查」正式化
成接续包，cron 链接续/分身间转交/人中途接管吃同一格式。

**再评估触发**：链式 cron 管线下次迭代（把交接包正式化时参考
其冻结-引用-权限三件结构）；或其 Community Server 生态成势。

## Cloudflare OS——Gatekeeper 模拟式异步确认 + Blueprints 第五次撞款

[cloudflare/cloudflare-os](https://github.com/cloudflare/cloudflare-os)（9.7k★，
2026-08 开源 v2，Apache-2.0，Workers/TS 栈）：Cloudflare 内部全员在用的
AI 生产力 OS——agent 工作台 + AI 现场造应用（Gadgets）+ 能力制安全层
（Gatekeepers）。栈与我们是零运行时交集，偷机制不偷代码。

**记它**：①**Gatekeeper 模拟式异步确认**——agent 碰到需批准的不可逆
动作时，Gatekeeper 本地模拟该动作结果放行 agent 继续排队干活（读结果
也给模拟值），跑完由用户批量/逐条终审。把「人等机器」翻成「机器等人」，
直接取代同步挂起式 confirm gate（同步闸门会把链式 cron 卡死在第一步，
逼人去 skip-permissions）。我们已有原始版先例：[PUBLISH] 只建草稿=
模拟值，公众号后台终审=批量批准。②**Blueprints 第五次撞款**——「分享
代码不分享托管实例、每个用户跑自己的私有副本、SaaS 中心化模型在 AI
时代失效」= DupHub 装分身同一主张，9.7k★ 大厂把它当产品地基（前四次
见 [VIOLOOP-STUDY.md](VIOLOOP-STUDY.md) 与本文件 TabTin 条）。③
能力引入制（agent 默认零权限、按任务「介绍」资源、agent 可主动请求）
= 我们 flow_load 提权的精细版；「agents 不是 users」的第三家
（Violoop 双芯片、TabTin 治理之后）。

**再评估触发**：carrier confirm gate 立项时（模拟式取代同步挂起，
VIOLOOP-STUDY 可学清单第 3 条已按此修订）。

## AGIROS——具身智能 OS 的社区标准赛道（生态雷达）

[agiros.org.cn](https://agiros.org.cn/)（开放原子/openEuler 体系，
月报 2026-08）：ROS 生态机器人 OS 社区——DDS 中间件、物理仿真、
WAM→轻量 VLA 决策迁移、平台×硬件兼容性认证。

**记它**：①「兼容性认证」模式（认证申请→收录→选型参考）是
agpkg 包×设备验证矩阵未来的呈现层模板；②WAM→VLA「迁移决策
机制而非最终动作」与 Violoop 权重蒸馏、我们 flow/validator
同构——第三家印证「通用智能越来越便宜，专用能力才需要沉淀」，
我们的蒸馏层是显式数据（flow/validator/clone 知识），比权重
黑盒可控；③具身 OS 的社区标准赛道存在且在组织化（世界机器人
大会/人才认证），agent 硬件品类起来后可能有标准位窗口。

**再评估触发**：agpkg 装机量上来需要兼容性呈现层；或考虑
社区/标准位站位时。

## 2026-09-05 主动扫描批次（五路并行，详见 [EXTERNAL-SCAN-2026-09.md](EXTERNAL-SCAN-2026-09.md)）

**专文候选池**（按优先级，未排期）：①Anthropic Agent Skills 生态
（380 万 SKILL.md 无中央仓库=agpkg 对齐题+DupHub 站位）②Bitwarden
agent-access（Rust 凭证 JIT 租赁，发凭证=confirm gate 审批点）
③RK3576 NPU 三件套（0.5-0.8B 舒适区实测，数字已录 DEVICE.md）
④MemOS+letta-code（MemCube 受控共享=分身记忆互通参照）⑤OpenHands
V1 SDK（事件溯源重放=session-events 升格）。

**一句话 watchlist 条目**（带触发）：

- **letta-code**（3.2k★，TS/Bun）：sleep-time 离线重整已产品化=
  compactor 离线版——触发=MemOS 专文时并看
- **basic-memory**（3.9k★）：md 真源+wiki-link 同步建 SQLite
  图谱——触发=aginxMemory 迭代
- **A2A**（25.6k★，Linux 基金会）：agent↔agent 任务互发标准，
  Agent Card 放 `/.well-known/agent-card.json`——触发=分身互发
  对外开放
- **AGNTCY**（LF「Internet of Agents」，Cisco/Google 坐镇）：
  agent 公网发现+身份标准化——触发=分身对外营业
- **USC agent-audit**（arXiv 2603.22853，repo 未熟）：审计三检查
  点+防篡改 trail——触发=audit 防篡改/session-events 异常发现立项
- **Cline Focus Chain**：任务锚自维护清单——触发=task_plan 迭代
- **moondream**（10k★，Apache-2.0，2B/0.5B 小 VLM）：**五原语
  视觉 API——caption/VQA/detect/point/count**=periph eye 的现成
  词汇表（agent 眼睛要结构化感知，不是对图聊天）；0.5B 档明确定位
  「边端蒸馏目标」=蒸馏立法的权重版反面（我们蒸馏层是显式数据）。
  触发=periph eye API 设计；本地 mini-VLM 档是否要填再议（候选
  SmolVLM/Qwen2.5-VL-0.5B）

**反向验证四条**（Goose 砍多模型 / E2B-KVM 不适配 / candle 不敌
NPU / cargo-dist 停摆）详见扫描文。

## anydoc——文档→Markdown 统一转换（性质例外：可依赖）

[firecrawl/anydoc](https://github.com/firecrawl/anydoc)（20.6k★，
MIT，纯 Rust 无 ML 无网络，2026-09 活跃）：14 格式（doc/docx/ppt/
pptx/xls/xlsx/odt/ods/odp/rtf/epub/csv/pdf）统一转 GFM Markdown
——各格式解析进共享文档模型、单一 GFM 序列化器出稿（修一处全
格式受益），中位 4.4ms。格式从字节内容探测不认扩展名；不做 OCR，
扫描页退出码 3 结构化上报，opt-in 才送 Firecrawl Parse 云端——
本地确定性快活/云 ML 重活的切线与我们耳嘴同构。

**记它但性质不同——本清单第一条「可依赖」**：MIT+纯 Rust+CLI=
直接可用（cargo add 或装 CLI 当外部件），zigbuild aarch64-musl
直接上卡。CLI 契约（argv 进/stdout 出/退出码 0 成功 1 转不了/
2 用法错 3 缺 OCR/never prompts）= D12 外部件四件套教科书。
填真缺口：文档生成有 document_generate、xlsx 读有 calamine，
但 docx/pptx/pdf→markdown 的 agent 阅读面没有统一答案——微信收
文件进分身、知识摄入管线都堵这。随仓 40 行 SKILL.md 是 Agent
Skill 金样本（触发语义 description+退出码契约+「大文档落盘按需
读别灌 context」），已录 [EXTERNAL-SCAN-2026-09.md](EXTERNAL-SCAN-2026-09.md)
专文①弹药。

**行动触发**：知识摄入/文档阅读能力立项时直接采用；aginxos-next
外部件清单填充时。

## Lightpanda——从零写的 agent 用 headless 浏览器

[lightpanda-io/browser](https://github.com/lightpanda-io/browser)（35.1k★，
2026-09 活跃两三周一版 v0.4.0，**AGPL-3.0 强传染——只看思路不动代码
不当库**）：Zig 从零写（非 Chromium 分支），html5ever 解析+V8 跑 JS
+libcurl 取页；官方基准 933 页爬取峰值内存 123MB vs headless Chrome
2GB（16 倍省）、快 9 倍；aarch64-linux glibc 官方二进制（nightly
+deb）直接跑。

**记它**：①**卡上浏览器缺口**——8GB 卡跑 headless Chrome 峰值 2GB
是奢侈，123MB 让「卡上本地浏览器」从不可行变可行（JS 重/SPA 页
curl 啃不动的那类）；CDP+Webdriver Bidi 双协议 = Playwright/Puppeteer
生态现成驱动（理念三）；②**结构化感知路线再验证**——`fetch --dump
markdown` 活页面 JS 渲染后直接出 md，agent 用结构读网页不截图不
vision，绕开 AginxBrain vision 两个坑（reasoning 死循环/URL 被拒），
与 moondream 五原语同哲学；③**`lightpanda agent` 产出 PandaScript**
——LLM 会话原型→纯 JS 确定性脚本→生产 `run` 无模型重放 token 零耗
=「LLM 原型→确定性工件」第三家独立撞款（flow 录制重放、AGIROS
WAM→VLA 之后）；④原生 MCP server 按 Mcp-Session-Id 每会话独立
page/cookies/内存、可显式共享上下文——多分身共用浏览器的会话隔离
现成设计；⑤`fetch --dump markdown` 与 anydoc（死文件→md）拼成
agent 阅读面两半。旁证弹药：anydoc（文档）/lightpanda（浏览器）/
moondream（视觉）三家同时「从零重造、为 agent 不为人」= 2026 生态
方向独立旁证。

**再评估触发**：分身卡本地浏览器 / periph browser 轻量化选型立项时。
届时注意：当外部二进制跑合法（连镜像分发都行，保持其开源+补丁公开），
禁抄代码禁库链接；上卡必须 `LIGHTPANDA_DISABLE_TELEMETRY=true`（默认
开遥测）；glibc 链接无 musl 版，musl 镜像要自编且 V8 依赖重；重
canvas 应用/Cloudflare 指纹/验证码页会挂（WPT 通过率页公开）。

## Scrapling——agent 阅读面的自适应爬虫框架（同赛道工具箱）

[D4Vinci/Scrapling](https://github.com/D4Vinci/Scrapling)（79.5k★，
2026 活跃，BSD-3-Clause 无传染雷区——但同为只借思路）：Python 爬虫
框架三层——fetchers（TLS 指纹伪装 + Camoufox 隐身浏览器，破
Cloudflare Turnstile 级反爬）/ parser（自适应自愈选择器：auto_save
存页快照、DOM 变更后按相似度重定位元素）/ spiders（AutoThrottle
自适应限速、断点续爬 checkpoint、多会话路由）。

**记它**：①**喂模型前剥 prompt injection**——MCP server 把页面交给
模型前先剥注入内容，配 CSS 选择器收窄视野；与 aginxbrowser MCP/
skills 路线同构，注入剥离正是我们要补的件；②**自愈选择器**——爬虫
最脆的是网站改版选择器全挂，auto_save+相似度重定位让长任务不脆断；
③**capture_xhr 后台 API 截获**——页面背后的 XHR 才是干净结构化
数据面，比啃渲染后 DOM 便宜一个量级（aginxbrowser session_network
同方向，可对齐打磨）；④**AutoThrottle + 断点续爬 + dev 缓存回放**——
长爬任务的限速礼仪与可恢复性；⑤**第四家收敛**——MCP server +
Agent Skill + page.markdown() 三面俱全，继 anydoc（文档）/
lightpanda（浏览器）/moondream（视觉）之后再证「为 agent 不为人」
的阅读面生态方向；79.5k★ + 代理商赞助墙验证赛道付费能力。

**再评估触发**：aginxbrowser 选择器自愈 / prompt-injection 剥离 /
长爬任务恢复立项时。届时注意：不抄 Python 库形态与 Spider 框架——
它站真浏览器引擎，我们站自研 diting 引擎（10× 资源效率是分身卡
立身之本），定位=同赛道工具箱非竞品；隐身军备（TLS 伪装/Camoufox）
不进——对抗性泥潭，轻量伪装够用。

## 附：已完成研究（不在 watchlist，已转行动）

- Termux → [TERMUX-STUDY.md](TERMUX-STUDY.md)（recipes 仓/fallback
  已进行动项）
- OpenLogi → [OPENLOGI-STUDY.md](OPENLOGI-STUDY.md)（Apache-2.0
  可白嫖，periph 立项参考）
- Violoop → [VIOLOOP-STUDY.md](VIOLOOP-STUDY.md)（carrier 五条
  可学清单）
