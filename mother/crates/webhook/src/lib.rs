//! Webhook HTTP 入站通道——aginx-carrier 的第二条入站渠道（机器→agent）。
//!
//! iLink 是人→agent（微信消息）；这条是机器→agent：外部系统（n8n/cron/
//! 监控/GitHub/CI）POST 一个事件到 `POST /hook/{name}`，触发所绑分身一轮。
//! 语义细节立法在 `~/Documents/aginx/ARCHITECTURE.md`（§7 用法块）。
//!
//! - 默认 `?` 无 wait：202 立即返回，轮后台跑，回复文本进日志（agent 靠
//!   工具做事）；`?wait=N`：同步阻塞拿回复（机器请求-响应流）。
//! - 会话：`sender_id = webhook:{name}` → label `user:webhook:{name}`，同一
//!   hook 的连续事件一个连续会话；数据目录 `workspaces/{agent}/senders/{name}/`。
//! - 路由：DirectBind + 启动时种路由（`cm.set_sender_route(name, agent)`）。

pub mod channel;
pub mod dedup;
pub mod server;

pub use channel::WebhookChannel;
pub use server::serve;
