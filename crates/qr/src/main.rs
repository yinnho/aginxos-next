//! aginx-qr CLI — 拍下的 JPEG 里解 QR，payload 一行一个打到 stdout。
//!
//! 用法：`aginx-qr <jpg>`。exit 0=至少解出一个 / 1=没码 / 2=错误（读图失败等）。
//! aginx-voice 不经本 CLI（lib 直连 WIFI 解析），这是人手调试面 + 验收脚本入口。
//! bin 需要 jpeg 特性档（default）；关掉特性时给出明确报错而不是链接错误。

#[cfg(not(feature = "jpeg"))]
fn main() {
    eprintln!("aginx-qr: bin 需要 jpeg 特性档（default），当前构建不含 JPEG 解码");
    std::process::exit(2);
}

#[cfg(feature = "jpeg")]
fn main() {
    // M47⑤t: self-pin to the little cluster {cpu0..cpu5}. This CLI is
    // spawned by aginx-voice's QR poll (2 Hz, 100-300 ms bursts on a big
    // core) exactly while cam-shot owns {cpu6,cpu7} with the viewfinder
    // pixel chain — an unpinned decode lands mid-chain and shows up as a
    // frame hitch. On an A55 the burst runs ~2.5x longer but costs the
    // chain nothing (⑤i core-class probe).
    #[cfg(target_os = "linux")]
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for i in 0..6 {
            libc::CPU_SET(i, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
    let path = std::env::args().nth(1).unwrap_or_default();
    if path.is_empty() {
        eprintln!("usage: aginx-qr <jpg>");
        std::process::exit(2);
    }
    let jpeg = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("aginx-qr: read {path}: {e}");
            std::process::exit(2);
        }
    };
    match aginx_qr::decode_jpeg(&jpeg) {
        Ok(payloads) if payloads.is_empty() => std::process::exit(1),
        Ok(payloads) => {
            for p in &payloads {
                println!("{p}");
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("aginx-qr: {e}");
            std::process::exit(2);
        }
    }
}
