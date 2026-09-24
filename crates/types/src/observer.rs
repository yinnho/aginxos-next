//! TurnObserver — D8 帧账在引擎侧的观察面。
//!
//! server 直调 kernel 跑轮（宪法：最直接的运行方式），D8 会话账（agi
//! jsonl）由 server 记——但工具帧发生在引擎循环内部，server 看不见。
//! 这个 trait 就是那双眼睛：kernel 轮里每次工具执行前后回调，server
//! 侧实现把回调折成帧落账（tool_call / tool_result / steer）。
//!
//! 设计约束：
//! - **只观察，不影响轮**：回调拿不到任何能改变执行路径的把手。工具
//!   照常执行，观测失败也不拖死轮（实现自行吞错记日志）。
//! - **Request/Done 帧不在这里**：轮的边界帧由调用方（server）在调
//!   kernel 前后自己记——它知道回合号，不需要进循环。
//! - **drain_steers 是唯一例外**：它有副作用语义（把排队的中途输入
//!   折成 user 轮），但折页动作由循环完成，observer 只供货。工具步
//!   边界是唯一下发点（刚回过账的轮必然还要再调 brain——与 fast-agi
//!   的 steer 帧语义同源：运行中插入 = steer，空闲插入 = 下一回合）。
//!
//! 挂线：`KernelHandle::turn_observer()`（默认 None）；CarrierKernel 持
//! 字段，`set_turn_observer` 注入。不进 run_agent_loop 参数表——挂在
//! handle 上随 kernel 走，全部现有调用点零改动。

use serde_json::Value;

/// 一轮执行的旁观者。实现必须 Send + Sync 且非阻塞（回调在轮的关键
/// 路径上）。
pub trait TurnObserver: Send + Sync {
    /// 工具即将执行（normalize 之后、执行之前）——「先记账、再执行」。
    /// `args` 是模型给的原始输入（JSON 对象或数组）。
    fn on_tool_call(&self, agent: &str, id: &str, tool: &str, args: &Value);

    /// 工具执行完毕（含失败/被钩子拦下/超时——只要有了结果就有回账）。
    /// `content` 是执行结果原文（成功载荷或错误说明）。
    fn on_tool_result(&self, agent: &str, id: &str, ok: bool, content: &str);

    /// 工具步边界取排队的中途输入。返回的每条文本会被折成 user 轮，
    /// 下一次 brain 调用可见。空 = 无插入。
    fn drain_steers(&self, agent: &str) -> Vec<String>;
}
