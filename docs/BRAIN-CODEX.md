# brain.aginx.net 推理后端怪癖档案 + codex 上脑实验需求

> 写于 2026-09-26（收据：docs/HARDWARE.md #395/#396，本地档）。
> 本文档交给 brain 侧处理：证据在册、风险在册、行动项在文末待勾。

## 一、已收据的事实（不要重新怀疑，直接用）

### 事实 1：brain 是推理型后端，max_tokens 压不住 reasoning

2026-09-26 母体 turn 延迟解剖（#395）：给两字回信（「收到」）写 turn 摘要，
请求 `max_tokens=150`，brain 仍先产出 **637 字 reasoning**（设备日志
`Captured reasoning_content len=637`），实际耗时 20-36s/次。

推论：
- `max_tokens` 对 reasoning 段**无效**（要么不占预算，要么只约束最终
  答案段）——任何「我用小 max_tokens 探测过，后端很快」的结论都是假的。
- 辅助性小调用（摘要/压缩/分类）在 brain 上会被 reasoning 放大成
  20-36s 档的大调用。

### 事实 2：探测结论必须用真 turn 验证

curl + 小 max_tokens 探测给出「backend is fast」假象；真 turn（母体
send 路径）实测 20-30s（主机）/ 30-36s（设备）。**判断 brain 行为只能
用真实对话路径收据，不能用裸 curl。**

### 事实 3：母体已落地的两个缓解模式（可复用）

针对推理型后端的工程护栏，已在 aginx-server 进程内落地（提交
0948972，包 v0.1.7）：

1. **机械快路径**：短进短出零工具轮（≤120 字、无工具调用）直接用
   原文当摘要，零 LLM 调用——推理后端对小任务不划算，能不打就不打。
2. **硬预算超时**：仍有 LLM 的辅助调用一律 5s 预算（`SUMMARY_BUDGET_SECS`），
   超时即弃、走降级路径（回退原文），**绝不门控主回复**。

效果：母体 turn 稳态 49-65s → 4-6s（设备包件复验 4.49/5.99/3.88s）。

## 二、codex 上 brain 实验（定位：实验，非主力）

### 机会

codex CLI 支持自定义 model provider（OpenAI 格式兼容）。理论上可把
codex 指到 brain.aginx.net——等于开发 agent 吃系统自己的脑（dogfood）。

配置样例（以 codex 官方 config.toml 文档为准，字段名以实测为准）：

```toml
# ~/.codex/config.toml —— 样例，未实测
model_provider = "brain"

[model_providers.brain]
name = "brain"
base_url = "https://brain.aginx.net/v1"
env_key = "BRAIN_API_KEY"   # key 走环境变量，绝不写进任何文件
wire_api = "chat"           # chat completions 面（vs "responses"）
```

### 预期风险（为什么不当主力）

1. **预算失真**：codex 的 token 预算/上下文管理假设 max_tokens 可控；
   brain 的 reasoning 不受约束（事实 1），codex 的开销模型直接失真。
2. **延迟放大**：codex 一个任务几十次 LLM 调用，每次都可能被 reasoning
   拖到 20-36s 档（母体实测），乘起来不可用。
3. **reasoning_content 传递行为未知**：codex 是否解析/丢弃该字段、
   是否把它计入上下文，未收据。
4. **流式+工具调用组合未收据**：codex 重度依赖 streaming + tool calls；
   brain 在该组合下的行为（断流、tool call 与 reasoning 交错）未验证。

### 实验阶梯与判据

- **L0 连通**：codex 在 brain 上回答一次仓库只读问题（如「FS.md 的
  助理放在哪个目录」）。判据：答案正确 + 单次调用 <30s。
- **L1 小修**：codex 在 brain 上完成一次单文件小改动。判据：改动正确
  + 全程 token/延迟记录在案。
- **L2 工具链**：连续多轮 tool calls 不断流。判据：10 轮以上无人工
  干预完成。
- **任一档判死**：延迟不可接受（单 turn >60s）/ 流断 / 预算机制报错。
- **主力位永远保留给原生 provider**；brain 位只是 dogfood 实验。

## 三、行动项（交 brain 侧处理）

- [x] **brain 侧开关**（2026-09-26 已上线，提交 242b084，部署 brain.aginx.net）：
  显式客户端意图压过路由 reasoning-tag 默认，三面统一：
  - chat 面：`reasoning_effort: "none"|"minimal"` → 关；`"low"|"medium"|"high"`
    → 开（budget 4000/10000/24000）；也认智谱式 `thinking:{"type":...}` 和
    `enable_thinking` 布尔
  - Anthropic 面：`thinking: {"type":"disabled"}` / `{"type":"enabled","budget_tokens":N}`
  - Responses 面：`reasoning.effort` 同 chat 面映射
  - 不传参数 → 维持旧行为（reasoning-tag 路由自动开思考）
  实测（chat 面、同一句提示）：deepseek-flash reasoning 107→0 token；
  caizhipu glm-5.3 reasoning 142→0 token、1.68s→0.64s。**母体辅助调用
  加一个 `"reasoning_effort":"none"` 即可彻底关思考**，5s 硬预算护栏可降级为兜底。
- [x] **语义澄清**：Anthropic 格式 provider 上 `max_tokens` 只约束最终答案段，
  思考段走独立的 `budget_tokens`（不占 max_tokens）；且 spec 要求
  max_tokens > budget_tokens——brain 的 tag 注入会把客户端较小的
  max_tokens 抬到 budget+6000（母体 150 被抬到 16000 的原因，已随开关
  修复：显式关思考后不再抬）。OpenAI 格式 provider 上 reasoning token
  是否计入 max_tokens 由 provider 决定（deepseek 不计入）。
- [x] **OpenAI 兼容面**：关思考后 provider 不再产出 reasoning_content
  （实测 deepseek-flash、glm-5.3 均为 0）；无法逐请求关的纯推理模型
  （若接入）仍会回 reasoning_content——目前 brain 路由表内无此死角。
- [ ] **实测回填**：按第二节阶梯跑 L0，把真实延迟/行为数据回填本文档
  （只接受真 turn 收据，见事实 2）。
- [ ] codex provider 配置样例实测校对后更新本节。

## 四、v0.4.2 musl 版五问的回复（aginxos-next 侧，2026-09-26）

对应 `aginxbrain/docs/BRAIN-RELEASE-v0.4.2.md` 第三节，逐条：

1. **架构**：都要，aarch64-musl 优先。全部在役设备（redfin/enchilada）
   是 aarch64；ORIN-NX「本地大脑档」（docs/DEVICE-ORIN-NX.md，选型
   中未立项）也是 aarch64——若立项，本机 brain 是该档核心件。
   x86_64-musl 次优先（服务器/容器腿；86quan 现跑 glibc Ubuntu，
   x86_64-musl 是容器化备胎，不阻塞任何线）。
2. **形态**：GitHub Release 出 Linux 惯例的 tar.gz 即可；**我方安装器
   拒收 gzip**（ustar 嗅探+gzip 拒绝，M32c 收据），镜像侧会放解压后
   的**裸二进制**（grok 先例：上游件→我方镜像裸件→manifest 钉裸件
   sha，flat-binary 包味直拷 /var/bin）。pkgs/ 树包配方（pkg.toml
   +[service] 单元）是我侧的活，真上机时配；上游保持干净裸件。
3. **命名**：`aginxbrain-v0.4.2-aarch64-unknown-linux-musl.tar.gz` 可；
   包内二进制名 `aginxbrain`（D13 姓氏，与 aginxbrowser 同族）。
   我方镜像路径将是 `pkgs.aginx.net/aginxbrain/v0.4.2/`。
4. **配置路径**：设备上**必须可指定**（硬需求）——env `AGINXBRAIN_HOME`
   （或 --home 旗标），默认 `~/.aginxbrain/` 兼容服务器不动。设备侧
   目标形态：config+db 住 `/var/lib/aginx/aginxbrain`（state 世界统一
   /var/lib/aginx，N5 裁决），provider 密钥走 `/etc/aginx/env`
   （env_file；密钥永不进包进库——aginx-gateway 同款纪律）。
5. **per-API-key 默认思考档**：排期做，值=兜底而非主路径。母体辅助
   调用（摘要/压缩/分类）将逐请求带 `"reasoning_effort":"none"`
   （引擎侧改，已立案的延迟优化点）；主答轮保默认思考（要质量）。
   per-key default 的正确优先序：**请求参数 > per-key default >
   全局默认**——这样 key 设 none 只兜住忘带参数的调用，主答显式
   带 high 仍能思考。若你们实现成 per-key 强压请求参数，请说明。

另：母体 me 的 agent:// 地址 = `agent://redfin.relay.aginx.net/me`
（agc 配置腿需 AGC_RELAY_SECRET，Mac 侧已配）。brain 侧可直接发问；
多设备铁律：relay 活≠id 不变，认准 id=redfin。
