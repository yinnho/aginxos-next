// 手写解析 bare.toml → OUT_DIR/facts.rs（usize 常量）。
// 零依赖（无 toml crate）——裸件工具链不引入解析库；格式即
// 「section / key = value」，值支持 0x 十六进制与十进制。
use std::{env, fs, path::Path};

fn parse_val(s: &str) -> u64 {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).expect("bad hex")
    } else {
        t.parse().expect("bad int")
    }
}

fn main() {
    println!("cargo:rerun-if-changed=bare.toml");
    let src = fs::read_to_string("bare.toml").expect("bare.toml unreadable");
    let mut out = String::from("// 由 bare.toml 生成（build.rs），勿手改。\n");
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let (k, v) = line.split_once('=').expect("bare.toml: 非 section/key=value 行");
        let key = k.trim().to_uppercase().replace('-', "_");
        out.push_str(&format!("pub const {}: usize = {:#x};\n", key, parse_val(v)));
    }
    let dest = Path::new(&env::var("OUT_DIR").unwrap()).join("facts.rs");
    fs::write(dest, out).unwrap();
}
