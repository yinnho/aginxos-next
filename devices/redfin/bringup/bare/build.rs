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
    // s59：wlanmdsp.mbn 真身进 OUT_DIR（main.rs include_bytes 消费——
    // elf_loader PD dlopen("wlanmdsp.mbn",4) 四级搜索第 1 级的应答体）。
    // 唯一真源 .local/device/redfin/modem/wlanmdsp.mbn（gitignored；
    // vendor.img /firmware 提取配方=S4-LINUX-PATH.md 收据二十四）。缺件
    // 即 build 失败——不退占位（半截真身喂 modem 比不喂更糟）。
    {
        let src = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../../../../.local/device/redfin/modem/wlanmdsp.mbn");
        println!("cargo:rerun-if-changed={}", src.display());
        let bytes = fs::read(&src)
            .unwrap_or_else(|e| panic!("wlanmdsp.mbn asset missing: {e}（提取配方见 S4-LINUX-PATH.md 收据二十四）"));
        // 真身四验防漂移：尺寸 + ELF32 LE + Hexagon + ET_EXEC（静态
        // 装载路径的硬断言值——错件早毙，别等 modem 侧断言）。
        assert_eq!(bytes.len(), 3_811_096, "wlanmdsp.mbn size drift");
        assert_eq!(&bytes[0..4], b"\x7fELF", "wlanmdsp.mbn not ELF");
        assert_eq!(bytes[4], 1, "wlanmdsp.mbn not ELF32");
        assert_eq!(bytes[5], 1, "wlanmdsp.mbn not LE");
        assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 2, "wlanmdsp.mbn not ET_EXEC");
        assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), 0xa4, "wlanmdsp.mbn not Hexagon");
        let dest = Path::new(&env::var("OUT_DIR").unwrap()).join("wlanmdsp.mbn");
        fs::write(&dest, &bytes).unwrap();
    }
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
