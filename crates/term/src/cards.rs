// cards — 定时任务产物的家（FS.md {home}/cards）。term 扫目录列小框；
// 点开 = POST /open，浏览器用自己的模板把 JSON 生成 HTML 上屏（屏幕
// 所有权 = show.html，见 aginxbrowser panel.rs）；删卡 = 删文件。
// 调用方只交 JSON + 模板名——模板和注册表都在浏览器目录里，term 不碰。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 浏览器持屏标记：aginxbrowser panel.rs 盯的是同一枚文件，它就是屏幕
/// （在且非空 = 浏览器持屏；消失/清空 = term 回位）。
pub const SHOW_PAGE: &str = "/run/aginxbrowser/show.html";

/// /open 的端口（aginxbrowser main.rs 的 REST 面）。const 数组形——
/// SocketAddr::from((&str,u16>) 不存在，([u8;4],u16) 有。
const OPEN_ADDR: ([u8; 4], u16) = ([127, 0, 0, 1], 8089);

pub struct Card {
    pub path: PathBuf,
    pub title: String,
    pub template: String,
    pub source: String,
    /// 刀4 页=会话：页上按住续话的路由靶（化身名；空=cron 产物，续话落
    /// 母体）。voice 出页时写；老卡/第三方卡没有此字段=空。
    pub session: String,
}

/// 母体 home：AGINX_HOME 覆写（试跑隔离）> AGINX_CARRIER_HOME > /home。
/// 与母体 home_dir()（types config.rs）同一张优先级表；不走 HOME 推导
/// ——term 由 init 拉起，环境里 HOME=/（设备实测）。
pub fn home_cards_dir() -> PathBuf {
    let root = match std::env::var("AGINX_HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => match std::env::var("AGINX_CARRIER_HOME") {
            Ok(h) if !h.is_empty() => PathBuf::from(h),
            _ => PathBuf::from("/home"),
        },
    };
    root.join("cards")
}

/// 扫目录：跳点开头的临时文件（写卡方 tmp+rename 原子写）与非 .json；
/// 按文件名倒序 = 时间戳前缀，新卡在前。坏 JSON / 无 title 的跳过——
/// 一张坏卡不该炸掉整列（正在改写或写坏了，下一轮 2s 重扫自愈）。
pub fn scan(dir: &Path) -> Vec<Card> {
    let mut out: Vec<Card> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let path = e.path();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let title = doc
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            continue;
        }
        out.push(Card {
            path,
            title: title.to_string(),
            template: doc
                .get("template")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            source: doc
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            session: doc
                .get("session")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
        });
    }
    out.sort_by(|a, b| b.path.cmp(&a.path));
    out
}

/// show.html 在且非空 = 浏览器持屏（panel.rs 同一谓词；空文件当没页，
/// 别拿白屏霸住屏——同 panel 的裁决）。
pub fn page_showing() -> bool {
    std::fs::read_to_string(SHOW_PAGE)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// 点卡：POST /open {"template","data"}。200 = 浏览器接单（show.html
/// 已落盘，term 的让位仲裁下一拍接管）；非 200 带回一行可读错误（比如
/// 404 unknown_template——该报母体安排写模板）。阻塞动作，~2s 上限，
/// 一次性点击，值得。
pub fn post_open(card: &Card) -> Result<(), String> {
    if card.template.trim().is_empty() {
        return Err("卡片没写模板名".into());
    }
    let raw = std::fs::read_to_string(&card.path).map_err(|e| format!("卡片读不出：{e}"))?;
    let data: serde_json::Value = serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(|e| format!("卡片不是 JSON：{e}"))?
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let body = serde_json::json!({ "template": card.template, "data": data }).to_string();
    let mut s = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(OPEN_ADDR),
        Duration::from_secs(2),
    )
    .map_err(|e| format!("浏览器没在：{e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let req = format!(
        "POST /open HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    s.write_all(req.as_bytes()).map_err(|e| format!("发不出去：{e}"))?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| format!("没回应：{e}"))?;
    let status = resp.lines().next().unwrap_or("").to_string();
    if status.contains(" 200 ") {
        return Ok(());
    }
    let why = resp
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.trim())
        .unwrap_or("")
        .to_string();
    Err(format!("{status} {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aginx-cards-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 点文件（tmp 残留）、非 json、坏 JSON、无 title 全跳过；文件名倒序
    /// = 新卡在前；缺目录 = 空列不炸。
    #[test]
    fn scan_skips_dots_and_bad_json_sorts_newest_first() {
        let dir = fixture("scan");
        std::fs::write(dir.join(".2026-09-23-abc.json"), "{}").unwrap();
        std::fs::write(dir.join("notjson.txt"), "x").unwrap();
        std::fs::write(
            dir.join("2026-09-22-morning.json"),
            r#"{"title":"昨天的晨报","template":"morning","data":{},"source":"cron"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("2026-09-23-morning.json"),
            r#"{"title":"今天的晨报","template":"morning","data":{},"source":"cron","session":"me"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("2026-09-23-broken.json"), "{ not json").unwrap();
        std::fs::write(dir.join("2026-09-23-notitle.json"), r#"{"template":"x"}"#).unwrap();
        let cards = scan(&dir);
        assert_eq!(cards.len(), 2, "dot/txt/bad-json/no-title all skipped");
        assert_eq!(cards[0].title, "今天的晨报", "filename-descending = newest first");
        assert_eq!(cards[1].title, "昨天的晨报");
        assert_eq!(cards[0].template, "morning");
        assert_eq!(cards[0].source, "cron");
        // 刀4：session 读出；老卡（昨天的晨报）没有该字段=空
        assert_eq!(cards[0].session, "me");
        assert_eq!(cards[1].session, "");
        assert!(scan(Path::new("/nonexistent-aginx-cards")).is_empty());
    }

    /// 所有权谓词：文件缺席 = term 持屏（host 上 /run 不存在）。
    #[test]
    fn page_showing_false_without_the_file() {
        assert!(!page_showing());
    }
}
