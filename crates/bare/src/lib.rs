//! bare — 无内核底座平台库（D14：零机器串，全部参数化）。
//!
//! 机器事实（MMIO 基址、scratch 规则）由设备侧裸件 crate 传入；
//! 本库只含通用契约：UFSHCI 事务、GPT 校验、CRC32、日志环记录、异常
//! 现场读取、MMU 恒等映射、时延仪器。
//! 契约真源 = devices/<codename>/bringup/bare/TRUTH.md（首台机器实证）。

#![cfg_attr(not(test), no_std)]

pub mod crc32;
pub mod exc;
pub mod ext4;
pub mod gic;
pub mod gpt;
pub mod instrument;
pub mod logring;
pub mod mmu;
pub mod psci;
pub mod spmi;
pub mod time;
pub mod ufs;
