//! aginx-pair — 双档入口：设备侧 `apply`（无特性门）+ host 侧铸码
//! （feature `mint`）。同一二进制两副面孔：
//!
//! * **设备档**（`--no-default-features`，第三次 zigbuild）：argv 只有
//!   `apply` 一个形状——payload 从 stdin 进，五步自举 + boot.state 刷新
//!   全在 `apply.rs`。mint 代码一字符不进设备二进制（qrcodegen/
//!   jpeg-encoder/aginx-qr jpeg 全被裁掉）。
//! * **host 档**（default）：`apply` 照样可用；其余 argv 全是铸码
//!   旗标（M42f⑤ 形状不动）。
//!
//! argv 分派恒定：`apply` 走 stdin 路径，其余才落进 mint——所以设备上
//! argv 永远两词，ps 里永远干净。

mod apply;

#[cfg(feature = "mint")]
mod mint;

use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("apply") => {
            // 零位置参数：payload 只走 stdin（argv 泄密是硬红线）。
            if args.len() > 1 {
                eprintln!("aginx-pair: apply takes no arguments (payload on stdin)");
                exit(2);
            }
            exit(apply::run(&apply::PairPaths::from_env()));
        }
        #[cfg(feature = "mint")]
        _ => mint::mint(&args),
        #[cfg(not(feature = "mint"))]
        _ => {
            eprintln!("usage: aginx-pair apply (payload on stdin)");
            exit(2);
        }
    }
}
