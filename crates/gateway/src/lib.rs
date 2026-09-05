//! aginx-gateway — 手机上的远端通道守护（N5⑤，D13 宪法定名）。
//!
//! 常驻注册到 relay（第 0 层，relay_secret 单门），把外部 JSON-RPC
//! （initialize/prompt，经 relay 管道）收口到本机 aginx-server 的 UDS
//! 前台。服务器侧零工作量：骨干 relay.aginx.net:8443 已在跑（同箱
//! 86quan，LE 证书验签过）。
//!
//! 模块：relay（第 0 层传输核）/ agent（第 1 层收口桥）/ config /
//! secret。协议权威 = 生态仓 aginx/ACP.md（金样本互锁，tests/golden.rs）。
//! bin 在 src/bin/aginx-gateway.rs（lib+bin 形态同 aginx-secret——
//! 集成测试要导入）。

pub mod agent;
pub mod config;
pub mod relay;
pub mod secret;

pub use agent::{error_frame, initialize_result, turn_frames_ok};
pub use relay::{Bridge, RelayMessage, Wire};
