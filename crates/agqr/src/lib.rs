//! agqr — QR 解码（M42b 眼分支）。
//!
//! 产品的眼进第一段：cam-shot 盲拍一张灰 JPEG → 本 CLI 解出 QR payload。
//! 解码器是 quircs（quirc 的纯 Rust 移植，拉 num-traits/num-derive/
//! thiserror，全纯 Rust——musl 静态无碍）；JPEG 解码复用 agimg
//! （vendored libjpeg-turbo，DCT 缩放直出小图）。
//!
//! lib 面还给 voiced 复用 `parse_wifi_payload`（WIFI: 载荷解析，纯字符串，
//! 无重依赖——voiced 以 default-features=false 引本 crate 时不拉 agimg）。

/// 送进解码器的图像边长上限。2016×1136 的后摄帧走 agimg DCT 1/2 缩放后
/// ~1008×568；QR 占半幅时 ~10px/模块，远超 quirc 的采样需求。
#[cfg(feature = "jpeg")]
pub const MAX_DECODE_SIDE: u32 = 1280;

/// 灰度图里解全部 QR，返回每个码的 payload（UTF-8 lossy）。
/// 解不出的码跳过（拍照噪声里混半个码是常态），不整体失败。
///
/// 原图直接解空时换 Bradley 局部二值化再解一轮——quircs 的 identify
/// 内部是全图 Otsu，暗房拍亮屏（屏幕外的黑房间占像素大头）时阈值落在
/// 「房间↔屏幕」之间，比房间亮的 QR 黑模块全被划成白、finder 全灭
/// （2026-09-04 实拍收据：满屏清晰 QR，`no candidate patterns at all`）。
#[cfg(feature = "decode")]
pub fn decode_luma(w: usize, h: usize, luma: &[u8]) -> Vec<String> {
    let mut out = identify_all(w, h, luma);
    if out.is_empty() {
        let bin = bradley_binarize(w, h, luma);
        out = identify_all(w, h, &bin);
    }
    out
}

#[cfg(feature = "decode")]
fn identify_all(w: usize, h: usize, luma: &[u8]) -> Vec<String> {
    assert_eq!(luma.len(), w * h);
    let mut out = Vec::new();
    let mut quirc = quircs::Quirc::default();
    for code in quirc.identify(w, h, luma) {
        match code.map_err(|e| e.to_string()).and_then(|c| {
            c.decode().map_err(|e| e.to_string())
        }) {
            Ok(data) => out.push(String::from_utf8_lossy(&data.payload).into_owned()),
            Err(_) => continue,
        }
    }
    out
}

/// Bradley/Wellner 自适应二值化（积分图，偏置 15%）。输出只有 0/255 两档
/// ——quircs 内部 Otsu 对双极图会把阈值落在两峰之间，等效我们自定义了
/// 阈值，绕过它只认全图直方图的限制。
///
/// 窗口必须**远大于**码里最大的整块黑区（finder 中心 3×3、对齐环 5×5
/// ≈ 5 模块）：判黑条件是「低于窗均值 85%」，均匀黑区里像素≈窗均值，
/// 窗口小于黑块时整个黑块会被判白（2026-09-04 实拍收据：finder 黑环
/// 反转成白、只剩边缘线框）。缺省半窗 = min(w,h)/4，夹在 [40,320]。
#[cfg(feature = "decode")]
pub fn bradley_binarize(w: usize, h: usize, luma: &[u8]) -> Vec<u8> {
    bradley_with_half(w, h, luma, (w.min(h) as u32 / 4).clamp(40, 320) as i64)
}

/// 参数化版（诊断用）：显式给半窗半径。
#[cfg(feature = "decode")]
pub fn bradley_with_half(w: usize, h: usize, luma: &[u8], half: i64) -> Vec<u8> {
    assert_eq!(luma.len(), w * h);
    // 积分图（前导 0 行/列，省边界判断）
    let stride = w + 1;
    let mut ii = vec![0u64; stride * (h + 1)];
    for y in 0..h {
        let mut row_sum = 0u64;
        for x in 0..w {
            row_sum += luma[y * w + x] as u64;
            ii[(y + 1) * stride + x + 1] = ii[y * stride + x + 1] + row_sum;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let y0 = (y as i64 - half).max(0) as usize;
        let y1 = (y as i64 + half).min(h as i64 - 1) as usize;
        for x in 0..w {
            let x0 = (x as i64 - half).max(0) as usize;
            let x1 = (x as i64 + half).min(w as i64 - 1) as usize;
            let cnt = ((y1 - y0 + 1) * (x1 - x0 + 1)) as u64;
            let sum = ii[(y1 + 1) * stride + x1 + 1] + ii[y0 * stride + x0]
                - ii[y0 * stride + x1 + 1]
                - ii[(y1 + 1) * stride + x0];
            // 黑：像素值 * 20 < 窗和 * 17（即低于窗均值 15%）
            if (luma[y * w + x] as u64) * 20 * cnt < sum * 17 {
                out[y * w + x] = 0;
            } else {
                out[y * w + x] = 255;
            }
        }
    }
    out
}

/// XRGB8888（0x00RRGGBB，行主序——agimg Bitmap 的布局）→ 8-bit 灰度。
/// 整数权重 77/150/29（和 256）：BT.601 luma，无浮点。
#[cfg(feature = "jpeg")]
pub fn luma_from_xrgb(w: u32, h: u32, pix: &[u32]) -> Vec<u8> {
    assert_eq!(pix.len(), (w * h) as usize);
    pix.iter()
        .map(|&p| {
            let r = (p >> 16) & 0xff;
            let g = (p >> 8) & 0xff;
            let b = p & 0xff;
            ((77 * r + 150 * g + 29 * b) >> 8) as u8
        })
        .collect()
}

/// JPEG 字节 → 全部 QR payload。尺度阶梯逐档试（每档内部 decode_luma
/// 自带 raw→Bradley 两轮），首个出码的档位直接返回。
///
/// 档位是 2026-09-04 设备收据定的：5/8 DCT（≤1280 请求）是扫屏甜点
/// （模块 ~34px、拍频纹波被 DCT 低通压住），同一张照片全分辨率与 1/8
/// 都解不出——成功率对尺度非单调，不能只解一档。3/4 与 1/2 兜别的
/// 距离/拍频组合。
#[cfg(feature = "jpeg")]
pub fn decode_jpeg(jpeg: &[u8]) -> Result<Vec<String>, String> {
    let mut last = Vec::new();
    let mut decoded_any = false;
    for max_side in [MAX_DECODE_SIDE, 1600, 1008] {
        if let Some(bm) = agimg::decode_scaled(jpeg, max_side, max_side) {
            decoded_any = true;
            let luma = luma_from_xrgb(bm.w, bm.h, &bm.pix);
            let out = decode_luma(bm.w as usize, bm.h as usize, &luma);
            if !out.is_empty() {
                return Ok(out);
            }
            last = out;
        }
    }
    if decoded_any {
        Ok(last)
    } else {
        Err("jpeg decode failed".into())
    }
}

// ---------------- WIFI: 载荷（ZXing Wi-Fi network config 格式）----------------

/// WiFi QR 解析结果。`auth`: "WPA" | "WEP" | "nopass"。
#[derive(Debug, Clone, PartialEq)]
pub struct WifiQr {
    pub ssid: String,
    pub psk: String,
    pub auth: String,
    pub hidden: bool,
}

/// 解析 `WIFI:T:WPA;S:my ssid;P:p\;ss;;`：
/// - 前缀 `WIFI:`（大小写敏感，规范如此）后是 `K:V;` 字段，乱序容忍；
/// - 转义：`\;` `\,` `\"` `\\`（反斜杠后的这四个字符取字面值）；
/// - `T` 缺省/未知按 WPA（有密码）或 nopass（无密码）收敛；
/// - 缺 `S` → None（没有 SSID 的 WiFi 码没有意义）。
pub fn parse_wifi_payload(p: &str) -> Option<WifiQr> {
    let rest = p.strip_prefix("WIFI:")?;
    // 逐字符拆顶层 ';'，同时处理转义——转义字符永远属于当前字段的值。
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut cur = String::new();
    let mut chars = rest.chars().peekable();
    let mut parts: Vec<String> = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // 取转义目标；行尾孤立反斜杠按字面收
                if let Some(&n) = chars.peek() {
                    if matches!(n, ';' | ',' | '"' | '\\') {
                        cur.push(n);
                        chars.next();
                        continue;
                    }
                }
                cur.push('\\');
            }
            ';' => {
                parts.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    parts.push(cur); // 尾段（规范以 `;;` 结尾，末段为空）
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once(':')?;
        fields.push((k.to_string(), v.to_string()));
    }
    let get = |key: &str| -> Option<&str> {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let ssid = get("S")?.to_string();
    if ssid.is_empty() {
        return None;
    }
    let psk = get("P").unwrap_or("").to_string();
    let auth = match get("T").unwrap_or("").to_uppercase().as_str() {
        "WEP" => "WEP".to_string(),
        "NOPASS" => "nopass".to_string(),
        _ if psk.is_empty() => "nopass".to_string(),
        _ => "WPA".to_string(),
    };
    let hidden = get("H")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    Some(WifiQr {
        ssid,
        psk,
        auth,
        hidden,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// qrcodegen 模块网格 → luma 位图（scale px/模块 + 4 模块静区）。
    #[cfg(feature = "decode")]
    fn render(text: &str, scale: usize) -> (usize, usize, Vec<u8>) {
        let qr = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium)
            .expect("encode");
        let quiet = 4;
        let n = qr.size() as usize; // qrcodegen 全程 i32，这里换回 usize 世界
        let side = (n + quiet * 2) * scale;
        let mut luma = vec![255u8; side * side];
        for y in 0..n {
            for x in 0..n {
                if qr.get_module(x as i32, y as i32) {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = (x + quiet) * scale + dx;
                            let py = (y + quiet) * scale + dy;
                            luma[py * side + px] = 0;
                        }
                    }
                }
            }
        }
        (side, side, luma)
    }

    #[test]
    #[cfg(feature = "decode")]
    fn roundtrip_plain_payload() {
        for scale in [8, 4] {
            let (w, h, luma) = render("hello aginxos", scale);
            assert_eq!(
                decode_luma(w, h, &luma),
                vec!["hello aginxos".to_string()],
                "scale {scale}"
            );
        }
    }

    #[test]
    #[cfg(feature = "decode")]
    fn roundtrip_wifi_payload_with_escapes() {
        // 密码含 ; 与 \，SSID 含 , 与 " —— 全是转义字符
        let payload = r#"WIFI:T:WPA;S:my\, \"cool\" ssid;P:p\;ss\\w;;"#;
        let (w, h, luma) = render(payload, 8);
        assert_eq!(decode_luma(w, h, &luma), vec![payload.to_string()]);
        let wq = parse_wifi_payload(payload).unwrap();
        assert_eq!(wq.ssid, r#"my, "cool" ssid"#);
        assert_eq!(wq.psk, r"p;ss\w");
        assert_eq!(wq.auth, "WPA");
        assert!(!wq.hidden);
    }

    #[test]
    fn wifi_parse_forms() {
        // 最小形态 + 字段乱序 + nopass + hidden
        let wq = parse_wifi_payload("WIFI:T:WPA;S:home;P:secret123;;").unwrap();
        assert_eq!(
            wq,
            WifiQr {
                ssid: "home".into(),
                psk: "secret123".into(),
                auth: "WPA".into(),
                hidden: false
            }
        );
        let wq = parse_wifi_payload("WIFI:P:pw;S:ap2;H:true;T:nopass;;").unwrap();
        assert_eq!(wq.auth, "nopass");
        assert!(wq.hidden);
        let wq = parse_wifi_payload("WIFI:S:opennet;;").unwrap();
        assert_eq!(wq.auth, "nopass");
        assert_eq!(wq.psk, "");
        // T 未知但有密码 → WPA
        let wq = parse_wifi_payload("WIFI:S:x;P:y;T:sae;;").unwrap();
        assert_eq!(wq.auth, "WPA");
    }

    #[test]
    fn wifi_parse_rejects() {
        assert!(parse_wifi_payload("https://aginx.net").is_none());
        assert!(parse_wifi_payload("WIFI:P:only;;").is_none()); // 无 SSID
        assert!(parse_wifi_payload("WIFI:S:;P:x;;").is_none()); // SSID 空
        assert!(parse_wifi_payload("wifi:T:WPA;S:a;P:b;;").is_none()); // 前缀大小写
    }

    #[test]
    #[cfg(feature = "jpeg")]
    fn luma_weights() {
        // 白 → 255，黑 → 0，纯绿 → (150*255)>>8 = 149（整数截断）
        let l = luma_from_xrgb(3, 1, &[0x00ffffff, 0x00000000, 0x0000ff00]);
        assert_eq!(l, vec![255, 0, 149]);
    }

    #[test]
    #[cfg(feature = "decode")]
    fn blank_image_no_codes() {
        let luma = vec![128u8; 64 * 64];
        assert!(decode_luma(64, 64, &luma).is_empty());
    }

    /// 暗房纸质码（Bradley 兜底的真实场景）：暗房画布 + 暗纸 + 更暗的墨。
    /// 三电平（房 10 / 纸 110 / 墨 25）下全图 Otsu 会切在房↔纸之间，墨
    /// (25) 落到亮侧、整码消失；Bradley 大窗（远大于模块）按局部对比度
    /// 二值化必须救回。decode_luma 整链出码 = 两轮兜底按序生效。
    #[test]
    #[cfg(feature = "decode")]
    fn dark_room_dim_paper_qr() {
        let payload = "WIFI:T:WPA;S:aginx;P:12345678;;";
        let (qw, _qh, qr_luma) = render(payload, 4);
        let cw = qw * 2;
        let ch = qw * 2;
        let mut canvas = vec![10u8; cw * ch];
        for y in 0..qw {
            for x in 0..qw {
                let v = qr_luma[y * qw + x];
                // 白(255)→暗纸(110)，黑(0)→墨(25)
                canvas[(y + qw / 2) * cw + (x + qw / 2)] = if v == 0 { 25 } else { 110 };
            }
        }
        assert_eq!(
            decode_luma(cw, ch, &canvas),
            vec![payload.to_string()],
            "dim paper on dark room must decode via the Bradley fallback"
        );
    }

    /// Bradley 极性回归：大窗下黑模块必须判黑（窗口小于黑块时经典失效
    /// 是整块判白——2026-09-04 实拍收据，finder 反转成白线框）。
    #[test]
    #[cfg(feature = "decode")]
    fn bradley_black_blob_stays_black() {
        let (qw, _qh, qr_luma) = render("polarity", 4);
        let bin = bradley_binarize(qw, qw, &qr_luma);
        // 黑模块中心像素必须为 0（白底为 255）
        let n = (qr_luma.len() / 4) * 3; // 大致取一个黑模块位置
        let _ = n;
        let blacks: usize = qr_luma
            .iter()
            .zip(&bin)
            .filter(|(&src, &b)| src == 0 && b == 0)
            .count();
        let total_black: usize = qr_luma.iter().filter(|&&v| v == 0).count();
        assert!(
            blacks * 10 >= total_black * 9,
            ">=90% of black modules must stay black under big-window Bradley (got {blacks}/{total_black})"
        );
    }
}
