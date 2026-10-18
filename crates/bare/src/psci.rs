//! PSCI 1.0 SMC 面（S0-6，NOLINUX-PLAN）。
//!
//! 参照 = 本机内核 drivers/firmware/psci.c：psci-0.2+ 标准 function id
//! （arm,psci-1.0 compatible、method="smc"），arm64 内核走 FN64 变体
//! `PSCI_FN_NATIVE(0_2, CPU_ON)`。EL1 发 `smc #0` 同步陷入 EL3（TZ
//! 固件受理——L0 内核拉副核同路径）。x0=fn，x1-x3=参数，x0=返回码。
//!
//! SMCCC 寄存器约定：x0-x3 进出、x4-x17 调用者保存——asm 声明 x4-x7
//! lateout 防 Rust 把活值错放在被陷坏寄存器里。

/// PSCI_VERSION（SMC32，唯一不回错误码的调用——回版本号本身）。
pub const PSCI_VERSION: u64 = 0x8400_0000;
/// CPU_ON（SMC64）：PSCI_FN_NATIVE(0_2, CPU_ON) = 0xC4000003。
/// 内核 psci_function_id[PSCI_FN_CPU_ON] 同值（psci.c:605）。
pub const CPU_ON_SMC64: u64 = 0xC400_0003;

/// 返回码（int32，负=错误）。
pub const SUCCESS: i64 = 0;
pub const ALREADY_ON: i64 = -1;
pub const NOT_PRESENT: i64 = -6;
/// PSCI_VERSION_NOT_SUPPORTED（-1 as u32）——无 PSCI 面。
pub const NOT_SUPPORTED: i64 = -1;

/// PSCI_VERSION：回 0x00010000|minor（1.0=0x10000）。
#[inline]
pub fn version() -> i64 {
    smc(PSCI_VERSION, 0, 0, 0)
}

/// CPU_ON(target_mpidr, entry_pa, context_id)：拉起目标核从 entry 跑
/// （AArch64 EL1、MMU off、x0=context_id、DAIF 掩）。
#[inline]
pub fn cpu_on(target: u64, entry: u64, ctx: u64) -> i64 {
    smc(CPU_ON_SMC64, target, entry, ctx)
}

/// SCM 64 位调用。x0=功能号，x1=arginfo，x2 起是参数。
/// 回 (x0, x1)。x0==1 时内核在 asm 里重试（SCM_INTERRUPTED）。
#[inline]
pub fn smc_ret2(fnid: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> (i64, u64) {
    let mut x0: i64 = fnid as i64;
    let mut x1 = a1;
    let mut x2 = a2;
    let mut x3 = a3;
    let mut x4 = a4;
    let mut x5 = a5;
    loop {
        unsafe {
            core::arch::asm!(
                "smc #0",
                inout("x0") x0,
                inout("x1") x1,
                inout("x2") x2,
                inout("x3") x3,
                inout("x4") x4,
                inout("x5") x5,
                lateout("x6") _,
                lateout("x7") _,
                options(nostack),
            );
        }
        if x0 != 1 {
            break;
        }
        x0 = fnid as i64;
        x1 = a1;
        x2 = a2;
        x3 = a3;
        x4 = a4;
        x5 = a5;
    }
    (x0, x1)
}

#[inline]
fn smc(fnid: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") fnid as i64 => ret,
            // x1-x3 参数位是 in 而非 inout 时，编译器可把调用侧活值
            // 缓存在这些寄存器跨 smc 复用——TZ 会踩参数位（SMCCC 允许
            // 返回未定义）。S0-6 首刷实证：`on aff=` 打出 0 即 target
            // 活值被踩。一律 inout=>_ 断根。
            inout("x1") a1 as i64 => _,
            inout("x2") a2 as i64 => _,
            inout("x3") a3 as i64 => _,
            lateout("x4") _,
            lateout("x5") _,
            lateout("x6") _,
            lateout("x7") _,
            options(nostack),
        );
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内核 uapi/psci.h：CPU_ON = fn(3) 的 FN64 形 = 0xC4000003（SMC64
    /// 位 31 置位即 0xC4 前缀；fn 号 3 不是直觉的 2——PSCI 0.2 编号
    /// 0=VERSION 1=SUSPEND 2=OFF **3=CPU_ON** 4=MIGRATE）。
    #[test]
    fn fn_ids_match_uapi() {
        assert_eq!(PSCI_VERSION, 0x8400_0000);
        assert_eq!(CPU_ON_SMC64, (0xC400_0000u64 | 3));
    }

    #[test]
    fn error_codes_match_spec() {
        assert_eq!(SUCCESS, 0);
        assert_eq!(ALREADY_ON, -1);
        assert_eq!(NOT_PRESENT, -6);
    }
}
