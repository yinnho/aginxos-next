//! carrier-gateway — agent:// 客户端与联系人台账（webui 第三刀/第六~九刀抽出）。
//!
//! [`agent_client`]：经 relay 的 ACP 协议客户端（连接/鉴权/prompt/chunk 流/
//! sessionId 收割/配对与同意流管理法）。[`tool_store`]：`external-tools.json`
//! 台账（added 清单 + 联系人记忆 + 远程网关地址簿双槽凭证）。
//!
//! 独立成 crate 的原因：runtime 的 gateway_hub 工具需要复用这两件，而
//! runtime 不能依赖 webui（webui→kernel→runtime 会循环）。两文件零
//! carrier 内部依赖，原样搬运。

pub mod agent_client;
pub mod tool_store;
