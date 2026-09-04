// aginx-runtime — fast-agi 引擎（宪法 D5：化身 = 文件夹，单引擎冷热两态）。
//
// 模块图：
//   transport  — 协议线 IO 面（stdio 生产 / 内存测试）
//   agent_loop — 轮状态机（carrier agent_loop 设计搬入 + 两处手术）
//   brain      — OpenAI 格式 brain 客户端（carrier llm_driver 搬入）
//   tools      — 工具面发现（`aginx commands --json`，D12 注册表即工具）
//   avatar     — 化身文件夹读面（SOUL.md + D8 会话日志冷恢复）
//   message    — 会话消息模型（OpenAI 形状最小集）
//   loop_state — 轮内计数与循环/无进展/上下文压力检测器

pub mod agent_loop;
pub mod avatar;
pub mod brain;
pub mod loop_state;
pub mod message;
pub mod tools;
pub mod transport;
