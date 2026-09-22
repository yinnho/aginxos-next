//! `aginx-carrier qr-login` — iLink 扫码登录（headless 服务器形态）。
//!
//! 终端直接渲染 ASCII 二维码（手机微信扫屏幕即可），轮询到确认后落
//! `workspaces/<bind_agent>/senders/<user_id>/session.json`（绑定即路由：
//! 会话住在分身下；一次性进程无 DB 回调 → JSON 旁路，daemon 的 respawn
//! watcher 每 5s 扫描收编）。此后 daemon 与 `notify` 一次性告警都有会话可发。
//!
//! 日志走 stderr，二维码/提示走 stdout（终端是给人扫的）。

use carrier_ilink::auth;

pub fn run(bot_id: String, bind_agent: String) -> anyhow::Result<()> {
    let http = carrier_ilink::build_http_client();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let on_qr = |url: &str| {
        println!("\n微信扫码登录（bot_id={bot_id}，绑定分身={bind_agent}）：\n");
        print_ascii_qr(url);
        println!("\n手机浏览器打开此链接同样生效（E2E 验证过的形态）: {url}");
        println!("↑ 扫码或点链接均可（8 分钟超时，过期自动刷新）");
        // stdout 走管道（ssh 重定向）时是块缓冲——不 flush 二维码出不来
        let _ = std::io::Write::flush(&mut std::io::stdout());
    };
    let msg = runtime.block_on(auth::qr_login(
        &http,
        &bot_id,
        Some(&bind_agent),
        Some(&on_qr),
    ))?;
    println!("\n{msg}");
    Ok(())
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
