//! aginx-efi-hello — Stage 1 第一级：无 Linux 内核，aginx 面开口。
//! u-boot 的 EFI_LOADER 即固件；本 app 即"操作系统"。
//! uefi 0.41 起 #[entry] 无参——系统表由入口宏注册为全局，
//! firmware 等读数走 uefi::system:: 免参函数。

#![no_main]
#![no_std]

use uefi::prelude::*;

#[entry]
fn main() -> Status {
    uefi::helpers::init().expect("helpers init");

    uefi::println!("aginx-efi-hello: no kernel, just us");
    uefi::println!("firmware vendor: {}", uefi::system::firmware_vendor());
    uefi::println!("firmware rev: {:#x}", uefi::system::firmware_revision());

    // 亮灯即证：停在这里，串口/HDMI 上有字=无 Linux 启动链成立。
    loop {
        core::hint::spin_loop();
    }
}
