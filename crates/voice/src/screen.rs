//! 按住链收尾：结果信封 → 调起浏览器（HANDOFF-显示 ④）。语音只把
//! JSON 交给浏览器（POST /open 模板名+JSON）；HTML 由浏览器用自己目录
//! 里的模板生成。对话框本身归 term（刀C paint_dialog）——浏览器不画
//! 过程，只画结果；屏幕所有权 = show.html（panel.rs 同一谓词）。

use std::time::Duration;

const OPEN: &str = "http://127.0.0.1:8089/open";

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

/// Chat 臂收尾：信封 → POST /open；非 JSON 回包 → reply 模板兜底（母体
/// 正文也是一页）。返回 (上脸句, 缺模板报告)：上脸句信封用 `say`、兜底
/// 用原文——脸行永远不 dump JSON；缺模板报告 = (模板名, 在册清单)，报
/// 母体的 spawn 与去重在 main.rs（本模块只管判据）。浏览器不在/盘满等
/// 环境错不报——重试同一个调用没用，文本就是结果（降级一等）。
pub fn show_reply(question: &str, reply: &str) -> (String, Option<(String, Vec<String>)>) {
    if let Some(env) = parse_envelope(reply) {
        let line = env
            .say
            .clone()
            .unwrap_or_else(|| "做好了，结果在屏幕上。".into());
        return match open_result(&env.template, &env.data) {
            Ok(()) => (line, None),
            Err(err) => {
                eprintln!("aginx-voice: open {}: {}", env.template, why(&err));
                (line, missing_report(&env.template, &err))
            }
        };
    }
    let data = serde_json::json!({ "question": question, "body": reply });
    match open_result("reply", &data) {
        Ok(()) => (reply.to_string(), None),
        Err(err) => {
            eprintln!("aginx-voice: open reply: {}", why(&err));
            (reply.to_string(), missing_report("reply", &err))
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
}
