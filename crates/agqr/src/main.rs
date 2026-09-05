//! agqr CLI — 拍下的 JPEG 里解 QR，payload 一行一个打到 stdout。
//!
//! 用法：`agqr <jpg>`。exit 0=至少解出一个 / 1=没码 / 2=错误（读图失败等）。
//! voiced 不经本 CLI（lib 直连 WIFI 解析），这是人手调试面 + 验收脚本入口。
//! bin 需要 jpeg 特性档（default）；关掉特性时给出明确报错而不是链接错误。

#[cfg(not(feature = "jpeg"))]
fn main() {
    eprintln!("agqr: bin 需要 jpeg 特性档（default），当前构建不含 JPEG 解码");
    std::process::exit(2);
}

#[cfg(feature = "jpeg")]
fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    if path.is_empty() {
        eprintln!("usage: agqr <jpg>");
        std::process::exit(2);
    }
    let jpeg = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("agqr: read {path}: {e}");
            std::process::exit(2);
        }
    };
    match agqr::decode_jpeg(&jpeg) {
        Ok(payloads) if payloads.is_empty() => std::process::exit(1),
        Ok(payloads) => {
            for p in &payloads {
                println!("{p}");
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("agqr: {e}");
            std::process::exit(2);
        }
    }
}
