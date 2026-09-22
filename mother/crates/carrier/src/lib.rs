//! aginx-carrier 共享库：分身 OS 的组装层。
//!
//! bin（`aginx-carrier` CLI）与桌面形态（crates/desktop Tauri 壳）共用
//! kernel boot + 通道接线。HTTP API 服务器形态在 Phase 7 落地时同样走这里。

pub mod envelope;
pub mod wiring;
