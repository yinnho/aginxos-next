//! WIFI: 码铸码器（host-only 测试夹具，M42g 眼输入测试用）。
//!
//!   cargo run -p aginx-qr --example wifi-qr -- 'WIFI:T:WPA;S:Legrand AP;P:1234567890;;'
//!
//! 终端块打印（Mac 满屏窗 + 后摄 ~20cm，M42b 已证配方）。payload 从 argv
//! 传入原样渲染——WIFI:/AGINXPAIR1/任意文本都行，本工具���解释内容；
//! 载荷合法性用 aginx-qr 本体解一遍自校验。
use std::env;

fn main() {
    let payload = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("用法: wifi-qr <payload>   例: 'WIFI:T:WPA;S:Legrand AP;P:1234567890;;'");
        std::process::exit(2);
    });
    let qr =
        qrcodegen::QrCode::encode_text(&payload, qrcodegen::QrCodeEcc::Medium).expect("encode");
    let n = qr.size() as usize;
    let quiet = 4; // 4 模块静区，扫码必需
    let total = n + quiet * 2; // 含静区总模块宽
    println!(
        "# {payload}  （v{:?}，{}×{} 模块，EC-M）",
        qr.version(),
        n,
        n
    );

    // 每输出行 = 2 模块高（▀▄█ 组合近似方形）；上下各补 2 行静区
    let blank = " ".repeat(total);
    println!("{blank}");
    println!("{blank}");
    let mut y = quiet;
    while y < quiet + n {
        let mut line = String::new();
        for x in 0..total {
            let in_x = x >= quiet && x < quiet + n;
            let up = in_x && qr.get_module((x - quiet) as i32, (y - quiet) as i32);
            let down = in_x
                && y + 1 < quiet + n
                && qr.get_module((x - quiet) as i32, (y + 1 - quiet) as i32);
            line.push(match (up, down) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        println!("{line}");
        y += 2;
    }
    println!("{blank}");
    println!("{blank}");
}
