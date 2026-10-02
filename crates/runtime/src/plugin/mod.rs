//! 插件树残部：只剩 admin_store（cron/flow 的分身管理员判定）。
//!
//! 通道管理器、插件加载器、内置通道注册表、工具分发器、桥管理器
//! 已随 #68 刀4 退役（频道体系整线住 crates/channels；母体不认识
//! 频道——出站富媒体走 outbound/，cron 投递走 kernel 通道钩子）。

pub mod admin_store;
