//! 按住链收尾：结果信封 → 调起浏览器（HANDOFF-显示 ④）。语音只把
//! JSON 交给浏览器（POST /open 模板名+JSON）；HTML 由浏览器用自己目录
//! 里的模板生成。对话框本身归 term（刀C paint_dialog）——浏览器不画
//! 过程，只画结果；屏幕所有权 = show.html（panel.rs 同一谓词）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const OPEN: &str = "http://127.0.0.1:8089/open";

/// 刀4 页=会话：当前页的卡路径。开页者写——term 点卡写、voice 出页写；
/// term 让位期按住时读它当 hold 靶（「在哪个页就是哪个对话的延续」）。
pub const PAGE_FILE: &str = "/run/aginx-voice/page";

/// 结果信封（灵魂文件规程，FS.md）：母体 send 回包是 JSON 对象且含非空
/// `template` 字符串与 `data` 字段即信封——workflow 的产出就是这份 JSON，
/// 不许再拆成 markdown/文章包回 HTML。
pub struct Envelope {
    pub template: String,
    pub data: serde_json::Value,
    /// 可选：上脸的一句话摘要。缺省「做好了，结果在屏幕上。」
    pub say: Option<String>,
}

/// 回包是不是结果信封。trim 后整段解析：信封是 workflow 的最终产物，
/// 不是嵌在闲聊里的一段 JSON。字段不齐/不是对象 = 不是信封，调用方走
/// reply 模板兜底。
pub fn parse_envelope(reply: &str) -> Option<Envelope> {
    let doc = serde_json::from_str::<serde_json::Value>(reply.trim()).ok()?;
    if !doc.is_object() {
        return None;
    }
    let template = doc.get("template")?.as_str()?.trim().to_string();
    if template.is_empty() {
        return None;
    }
    let data = doc.get("data")?.clone();
    let say = doc
        .get("say")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    Some(Envelope { template, data, say })
}

/// 上页内容（刀4 落卡用）：template+data——信封页或 reply 兜底页。
/// /open 没开成（浏览器不在/缺模板）= 没有页 = 没有卡。
pub struct Shown {
    pub template: String,
    pub data: serde_json::Value,
}

/// Chat 臂收尾：信封 → POST /open；非 JSON 回包 → reply 模板兜底（母体
/// 正文也是一页）。返回 (上脸句, 缺模板报告, 上页内容)：上脸句信封用
/// `say`、兜底用原文——脸行永远不 dump JSON；缺模板报告 = (模板名, 在册
/// 清单)，报母体的 spawn 与去重在 main.rs（本模块只管判据）。浏览器不在
/// /盘满等环境错不报——重试同一个调用没用，文本就是结果（降级一等）。
pub fn show_reply(
    question: &str,
    reply: &str,
) -> (String, Option<(String, Vec<String>)>, Option<Shown>) {
    if let Some(env) = parse_envelope(reply) {
        let line = env
            .say
            .clone()
            .unwrap_or_else(|| "做好了，结果在屏幕上。".into());
        let shown = Shown {
            template: env.template.clone(),
            data: env.data.clone(),
        };
        return match open_result(&env.template, &env.data) {
            Ok(()) => (line, None, Some(shown)),
            Err(err) => {
                eprintln!("aginx-voice: open {}: {}", env.template, why(&err));
                (line, missing_report(&env.template, &err), None)
            }
        };
    }
    let data = serde_json::json!({ "question": question, "body": reply });
    let shown = Shown {
        template: "reply".into(),
        data: data.clone(),
    };
    match open_result("reply", &data) {
        Ok(()) => (reply.to_string(), None, Some(shown)),
        Err(err) => {
            eprintln!("aginx-voice: open reply: {}", why(&err));
            (reply.to_string(), missing_report("reply", &err), None)
        }
    }
}

/// /open 错误体 → 缺模板报告。只有 unknown_template 算「没模板→报母体
/// 安排写一次并登记」；在册清单从错误体捞（浏览器给的机器判据，不用
/// 解析自由文本猜）。
fn missing_report(tpl: &str, err: &serde_json::Value) -> Option<(String, Vec<String>)> {
    if err.get("error").and_then(|v| v.as_str()) != Some("unknown_template") {
        return None;
    }
    let name = err
        .get("template")
        .and_then(|v| v.as_str())
        .unwrap_or(tpl)
        .to_string();
    let known = err
        .get("known")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some((name, known))
}

/// /open 错误体的 why 字段（日志用；缺字段给占位，不当机）。
fn why(err: &serde_json::Value) -> &str {
    err.get("why").and_then(|v| v.as_str()).unwrap_or("?")
}

/// 工作流已经拿出 JSON：交给浏览器按模板出页。show.html 落盘后 term
/// 的让位仲裁下一拍接管，对话框随 term 放屏自然关掉。
fn open_result(template: &str, data: &serde_json::Value) -> Result<(), serde_json::Value> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_recv_body(Some(Duration::from_secs(8)))
            .build(),
    );
    let raw = serde_json::json!({"template": template, "data": data}).to_string();
    let resp = agent
        .post(OPEN)
        .header("content-type", "application/json")
        .send(raw)
        .map_err(|e| serde_json::json!({"why": e.to_string()}))?;
    let body: serde_json::Value = resp
        .into_body()
        .read_to_string()
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| serde_json::json!({"why": "open: unreadable response"}))?;
    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    Err(body)
}

// ---- 刀4 页=会话：卡 = 页 = 会话 ----

/// 母体 home（与 term cards::home_cards_dir 同一张优先级表）。voice 不
/// 链 term crate，十行就地复刻——改优先级要两边同步。
pub fn cards_dir() -> PathBuf {
    let root = std::env::var("AGINX_HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::env::var("AGINX_CARRIER_HOME")
                .ok()
                .filter(|h| !h.is_empty())
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home"));
    root.join("cards")
}

/// 卡标题 = 问句截 16 字（卡带自己再裁宽）。空问句给占位——title 空
/// 的卡 term 扫不出来（scan 跳过无 title），一回合不能白干。
fn clip_title(t: &str) -> String {
    let s: String = t.chars().take(16).collect();
    let s = s.trim();
    if s.is_empty() {
        "对话".into()
    } else {
        s.to_string()
    }
}

/// 文件名时间戳：date 子进程（status_text 同款——机上无 chrono，busybox
/// date 是已验活体）；失败退 unix 秒（排序吃亏但唯一性在）。
fn date_stamp() -> String {
    Command::new("date")
        .arg("+%F-%H%M%S")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "0".into())
        })
}

/// 会话卡落盘（刀4 页=会话：页 = 卡 = 会话）。新任务（page=None）= 新卡；
/// 页上追问（page=当前页卡路径）= 原地重写同卡——该会话的页永远显示
/// 最新结果，卡带不膨胀。返回卡路径（调用方写 PAGE_FILE 记账）。
/// tmp+rename 原子写（tmp 后缀不是 .json，term 扫描天然跳过）。
pub fn save_card(
    dir: &Path,
    page: Option<&Path>,
    title: &str,
    template: &str,
    data: &serde_json::Value,
    session: &str,
) -> Option<PathBuf> {
    let doc = serde_json::json!({
        "title": clip_title(title),
        "template": template,
        "data": data,
        "source": "chat",
        "session": session,
    });
    let path = match page {
        Some(p) => p.to_path_buf(),
        None => dir.join(format!("{}-chat-{}.json", date_stamp(), std::process::id())),
    };
    let _ = std::fs::create_dir_all(dir);
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let raw = doc.to_string();
    if std::fs::write(&tmp, raw).is_ok() {
        std::fs::rename(&tmp, &path).ok()?;
    } else {
        return None;
    }
    Some(path)
}

/// 读卡的 session 字段（路由靶）。读不出（卡没了/坏 JSON/无 session）
/// → None——调用方落母体。
pub fn card_session(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let doc = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let s = doc
        .get("session")
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    if s.is_empty() {
        return None;
    }
    Some(s)
}

/// 开页记账：当前页卡路径写 PAGE_FILE。term 点卡 / voice 出页都写——
/// term 让位期按住读它当 hold 靶。host 上 /run 不在，写失败即忽略。
pub fn note_page(card: &Path) {
    if std::fs::create_dir_all("/run/aginx-voice").is_ok() {
        let _ = std::fs::write(PAGE_FILE, card.to_string_lossy().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 信封三态：字段齐=信封（say 可缺）；template 空/data 缺/非对象/
    /// 非 JSON=不是信封（走兜底）。
    #[test]
    fn parse_envelope_shape() {
        let e = parse_envelope(
            r#"{"template":"weather","data":{"city":"南京"},"say":"南京 26 度"}"#,
        )
        .expect("full envelope parses");
        assert_eq!(e.template, "weather");
        assert_eq!(e.data["city"], "南京");
        assert_eq!(e.say.as_deref(), Some("南京 26 度"));

        let e = parse_envelope(r#"  {"template":"weather","data":{}}  "#).expect("trim ok");
        assert_eq!(e.say, None, "say optional");

        assert!(parse_envelope(r#"{"template":"","data":{}}"#).is_none(), "empty template");
        assert!(parse_envelope(r#"{"template":"x"}"#).is_none(), "no data");
        assert!(parse_envelope(r#"["weather",{}]"#).is_none(), "array is not an envelope");
        assert!(parse_envelope("今天南京 26 度，晴。").is_none(), "plain text");
        assert!(parse_envelope("").is_none());
    }

    /// 缺模板报告只认 unknown_template，且把浏览器给的 known 清单原样
    /// 递回去；其他错误码（registry/盘满）不报——重试没用，不是「写模板」的事。
    #[test]
    fn missing_report_only_for_unknown_template() {
        let err = serde_json::json!({
            "ok": false, "error": "unknown_template",
            "why": "unknown template trip; known: weather, reply",
            "template": "trip", "known": ["weather", "reply"],
        });
        let (tpl, known) = missing_report("trip", &err).expect("reported");
        assert_eq!(tpl, "trip");
        assert_eq!(known, vec!["weather", "reply"]);

        assert!(missing_report("trip", &serde_json::json!({"error": "write_show"})).is_none());
        assert!(missing_report("reply", &serde_json::json!({"error": "unknown_template"})).is_some());
    }

    // ---- 刀4 页=会话：卡落盘 ----

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aginx-voice-card-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 新任务=新卡；页上追问=原地重写同卡（不膨胀）；title 截断+空占位。
    #[test]
    fn save_card_new_then_update_in_place() {
        let dir = tmpdir("save");
        let data = serde_json::json!({"city": "南京"});
        let p1 = save_card(&dir, None, "帮我看下南京的天气", "weather", &data, "me")
            .expect("new card saved");
        assert!(p1.starts_with(&dir), "card lives in the cards dir");
        let raw = std::fs::read_to_string(&p1).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(doc["title"], "帮我看下南京的天气");
        assert_eq!(doc["template"], "weather");
        assert_eq!(doc["data"]["city"], "南京");
        assert_eq!(doc["source"], "chat");
        assert_eq!(doc["session"], "me");

        // 追问：同卡重写，内容换新
        let data2 = serde_json::json!({"city": "北京"});
        let p2 = save_card(&dir, Some(&p1), "改成北京", "weather", &data2, "me")
            .expect("update saved");
        assert_eq!(p2, p1, "follow-up rewrites the same card");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p2).unwrap()).unwrap();
        assert_eq!(doc["title"], "改成北京");
        assert_eq!(doc["data"]["city"], "北京");
        assert_eq!(dir.read_dir().unwrap().count(), 1, "no second file");

        // 超长问句截 16 字；空问句给占位（title 空的卡 term 扫不出来）
        let long = "今".repeat(40);
        let p3 = save_card(&dir, None, &long, "reply", &data, "me").unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p3).unwrap()).unwrap();
        assert_eq!(doc["title"].as_str().unwrap().chars().count(), 16);
        let p4 = save_card(&dir, None, "  ", "reply", &data, "me").unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p4).unwrap()).unwrap();
        assert_eq!(doc["title"], "对话");
    }

    /// session 读取：有=Some，缺/空/坏 JSON/文件没了=None（调用方落母体）。
    #[test]
    fn card_session_reads_or_none() {
        let dir = tmpdir("sess");
        let p = save_card(&dir, None, "q", "t", &serde_json::json!({}), "小喜").unwrap();
        assert_eq!(card_session(&p), Some("小喜".into()));
        assert_eq!(card_session(&dir.join("gone.json")), None);
        let no_sess = dir.join("cron.json");
        std::fs::write(&no_sess, r#"{"title":"晨报"}"#).unwrap();
        assert_eq!(card_session(&no_sess), None);
        std::fs::write(&no_sess, "{ broken").unwrap();
        assert_eq!(card_session(&no_sess), None);
    }
}
