//! `aginx-carrier qr-login` — iLink 扫码登录（headless 服务器形态）。
//!
//! 终端直接渲染 ASCII 二维码（手机微信扫屏幕即可），轮询到确认后落
//! `workflows/<bind_agent>/senders/<user_id>/session.json`（绑定即路由：
//! 会话住在分身下；一次性进程无 DB 回调 → JSON 旁路，daemon 的 respawn
//! watcher 每 5s 扫描收编）。此后 daemon 与 `notify` 一次性告警都有会话可发。
//!
//! `--screen`（AginxOS 手机形态）：二维码不进终端——POST 本机
//! aginxbrowser `/open`（qr 模板）直接上屏，扫完确认换 reply 成功页。
//! 显示面缺席（headless 复跑）只 stderr，登录流不死。
//!
//! 日志走 stderr，二维码/提示走 stdout（终端是给人扫的）。

use carrier_ilink::auth;

pub fn run(bot_id: String, bind_agent: String, screen: bool) -> anyhow::Result<()> {
    let http = carrier_ilink::build_http_client();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let on_qr_screen = |url: &str| {
        open_qr_page(url, &bot_id, &bind_agent);
        println!("\n二维码已上屏（bot_id={bot_id}，绑定分身={bind_agent}）。");
        println!("扫不了时手机浏览器打开此链接同样生效: {url}");
        // stdout 走管道（ssh 重定向）时是块缓冲——不 flush 提示不出
        let _ = std::io::Write::flush(&mut std::io::stdout());
    };
    let on_qr_term = |url: &str| {
        println!("\n微信扫码登录（bot_id={bot_id}，绑定分身={bind_agent}）：\n");
        print_ascii_qr(url);
        println!("\n手机浏览器打开此链接同样生效（E2E 验证过的形态）: {url}");
        println!("↑ 扫码或点链接均可（8 分钟超时，过期自动刷新）");
        // stdout 走管道（ssh 重定向）时是块缓冲——不 flush 二维码出不来
        let _ = std::io::Write::flush(&mut std::io::stdout());
    };

    let on_qr: &dyn Fn(&str) = if screen { &on_qr_screen } else { &on_qr_term };
    let msg = runtime.block_on(auth::qr_login(&http, &bot_id, Some(&bind_agent), Some(on_qr)))?;
    if screen {
        open_page(
            "reply",
            &serde_json::json!({
                "question": "微信登录",
                "body": format!("{msg}\n\n现在可以直接用微信给这个手机发消息了。")
            }),
        );
    }
    println!("\n{msg}");
    Ok(())
}

/// 把短 URL 铸成白底黑模块的 SVG 二维码（4 模块静区，viewBox 以模块
/// 计）。纯 ASCII——base64 后过模板转义槽、页内 atob 还原均无损。
fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
    let w = code.width();
    let quiet = 4usize;
    let total = (w + quiet * 2) as u32;
    let mut d = String::new();
    for y in 0..w {
        for x in 0..w {
            if code[(x, y)] == qrcode::Color::Dark {
                d.push_str(&format!("M{},{}h1v1h-1z", x + quiet, y + quiet));
            }
        }
    }
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {t} {t}\" shape-rendering=\"crispEdges\"><rect width=\"{t}\" height=\"{t}\" fill=\"#ffffff\"/><path d=\"{d}\" fill=\"#000000\"/></svg>",
        t = total,
        d = d
    ))
}

/// QR 页上屏：svg 铸不出（文本超容量等）时 svg_b64 空串——qr 模板有
/// 兜底文案 + 链接文本，流程照走。
fn open_qr_page(url: &str, bot_id: &str, bind_agent: &str) {
    let svg_b64 = qr_svg(url)
        .map(|svg| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(svg)
        })
        .unwrap_or_default();
    open_page(
        "qr",
        &serde_json::json!({
            "title": "微信扫码登录",
            "subtitle": format!("bot={bot_id} · 绑定化身={bind_agent} · 扫完即接通"),
            "svg_b64": svg_b64,
            "url": url,
        }),
    );
}

/// POST /open 到本机 aginxbrowser（8089）。失败只 stderr：显示面缺席
/// 不该弄死登录轮询（headless 复跑场景链接兜底还在）。
fn open_page(template: &str, data: &serde_json::Value) {
    use std::io::{Read, Write};
    let body = serde_json::json!({ "template": template, "data": data }).to_string();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 8089));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2))
    else {
        eprintln!("qr-login: aginxbrowser 不在 {addr}——页面上不了屏，链接兜底");
        return;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    let req = format!(
        "POST /open HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    if let Err(e) = s.write_all(req.as_bytes()) {
        eprintln!("qr-login: /open 发不出去: {e}");
        return;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let status = resp.lines().next().unwrap_or("").to_string();
    if !status.contains(" 200 ") {
        eprintln!("qr-login: /open {status}");
    }
}

/// 把短 URL 渲染成终端可扫的 ASCII 二维码：▀▄█ 半块字符一行装两个
/// 模块，四周留 4 模块静区。渲染失败不致命——URL 本身打给用户兜底。
fn print_ascii_qr(text: &str) {
    let code = match qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M) {
        Ok(c) => c,
        Err(_) => {
            println!("（二维码渲染失败；用手机浏览器打开: {text}）");
            return;
        }
    };
    let width = code.width() as i64;
    let quiet = 4i64;
    let dark = |x: i64, y: i64| -> bool {
        x >= 0 && y >= 0 && x < width && y < width && code[(x as usize, y as usize)] == qrcode::Color::Dark
    };
    for y in (-quiet..width + quiet).step_by(2) {
        let mut line = String::new();
        for x in -quiet..width + quiet {
            line.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 qr_svg 的 path（M{x},{y}h1v1h-1z 子路径）栅格化成 luma 位图。
    fn svg_to_luma(svg: &str, scale: usize) -> (usize, usize, Vec<u8>) {
        let total: usize = svg
            .split("viewBox=\"0 0 ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|t| t.parse().ok())
            .expect("viewBox 边长");
        let dim = total * scale;
        let mut luma = vec![255u8; dim * dim];
        for seg in svg.split("path d=\"").nth(1).unwrap_or("").split('M') {
            let Some((x, y)) = seg
                .split_once(',')
                .and_then(|(x, y)| y.split('h').next().map(|y| (x, y)))
                .and_then(|(x, y)| Some((x.parse::<usize>().ok()?, y.parse::<usize>().ok()?)))
            else {
                continue;
            };
            for py in y * scale..(y + 1) * scale {
                for px in x * scale..(x + 1) * scale {
                    luma[py * dim + px] = 0;
                }
            }
        }
        (dim, dim, luma)
    }

    /// 闭环：qr_svg 铸码 → 栅格化 → quircs 解码回原串。铸出的码必须
    /// 真的可扫（屏幕上是给人微信扫的），不只是格式合法。
    #[test]
    fn qr_svg_is_scannable() {
        let url = "https://open.weixin.qq.com/connect_confirm?scene=ilink&token=a1b2c3d4e5f6";
        let svg = qr_svg(url).expect("svg");
        assert!(svg.is_ascii(), "SVG 必须 ASCII：base64/atob 契约");
        let (w, h, luma) = svg_to_luma(&svg, 8);
        let decoded = aginx_qr::decode_luma(w, h, &luma);
        assert!(
            decoded.iter().any(|s| s == url),
            "解不回原串: {decoded:?}"
        );
    }

    /// 超容量文本（QR 上限）不炸：None 兜底，调用方走链接文本。
    #[test]
    fn qr_svg_over_capacity_is_none() {
        let huge = "x".repeat(4000);
        assert!(qr_svg(&huge).is_none());
    }
}
