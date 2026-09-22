# aginx-carrier — Agent Instructions

## 项目定位

**aginx-carrier 是分身 OS**：托管数字分身的 Agent 运行时，aginx Agent 互联网上"网站"之一（aginx=nginx，本仓=网站）。从 OpenCarrier **搬运移植**而来（独立仓，非 fork——零共享 git 历史，见下），定位/分层/借用机制见 `../docs/AGINX-CARRIER-VISION.md`。

**与 OpenCarrier 的关系（铁律）**：
- 源仓库 `~/Documents/opencarrier/opencarrier/` 是**只读参考**——要搬代码从那里 cp，绝不反向修改
- 本仓是**独立移植仓，不是 fork**：git 取证过——commit 全始于 init 骨架、逐 crate 搬运改名、零共享 git 对象、无 merge-base、无 opencarrier remote。单操作者（无多租户）、私有化部署、aginx 原生接入；**各自进化，不追平，无 cherry-pick 义务**
- 搬运策略：一边建一边复制——每个 crate 按依赖顺序搬，搬运时就完成改名+剥多租户+适配，每步 `cargo build && cargo test` 全绿

## 约定

- **crate 命名**：`carrier-*`（carrier-types / carrier-memory / carrier-runtime / carrier-kernel / carrier-lifecycle / carrier-clone / carrier-ilink / carrier-webhook / carrier-gateway），bin 叫 `aginx-carrier`
- **配置/数据目录**：`~/.aginx/carrier/`
- **git 历史**：纯 cp 不带历史，opencarrier 仓库是档案（历史在那里查）
- **通道范围**：入站两条——iLink（人→agent）+ webhook（机器→agent，`carrier-webhook`，daemon `start` 专属 HTTP 监听，默认关、移动端不起）；weixin-oa/企微kf 后置
- **AginxOS 融合（2026-08-30 定调）**：全力配合 AginxOS（智能体手机），化身即 app、carrier 是化身 runtime（需求文档 `~/Documents/aginxos/docs/CARRIER.md`）。**webui/`web` 子命令已退役**——浏览器面不再有；化身管理走 CLI：`aginx-carrier agent install|list|remove|update`（市场机制在 `carrier-clone::market`，UI 无关层）。远程化身句柄（§3.3 远程类）：`agent remote add|remove` 注册 agent:// 地址，注册表 `~/.aginx/carrier/remote-agents.json`（0600，含访客 token），`agent list` 同构合并（STATE=remote），acp/ask 对远程别名不 boot kernel、经 carrier-gateway agent_client 转发。用户可见文案用「化身」，内部代码 clone 不改名

## Build & Verify Workflow

```bash
cargo build --workspace            # Must compile
cargo test --workspace             # All tests must pass
cargo clippy --workspace --all-targets -- -D warnings  # Zero warnings
```

## Phase 索引（搬运顺序）

0. 骨架（本仓初始化）✅
1. types → memory
2. runtime
3. kernel + lifecycle（剥多租户重灾区：senders/admins/api key 分层）
4. clone（CLONE-FORMAT.md 金样本测试同批搬；clone_install 写 ~/.aginx/agents/<clone>/aginx.toml 入网钩子）
5. `aginx-carrier acp` stdio ACP 桥 + aginx 网关联调，打通第一个 agent:// 分身地址
6. iLink 通道
7. 三形态（桌面 Tauri / 移动 UniFFI / 服务器）

## Common Gotchas（继承自 opencarrier，搬运时留意）

- Config 字段加进 struct 必须同批加 `Default` impl，否则 build 挂
- `AgentLoopResult` 字段是 `.response` 不是 `.response_text`
- flow 的 frontmatter description 非空是硬门槛（空 description → collect_flow_summaries 跳过 → flow 不可见）
- 中文文本切片用 char_boundary 回退，禁止裸 `&s[..N]`
