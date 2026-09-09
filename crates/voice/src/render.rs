//! v4⑥ 结果页活体化（#246⑥）：voice = 纯内容生产者。brain 文本回复 →
//! markdown-lite → 磷光 HTML → 原子写 result.html；上屏归 term 面板客户端
//! （CDP attach 引擎 screencast → DRM 直写），voice 不再触碰引擎。
//!
//! face{result:true} 的翻旗点唯一：run_outs 尾部 flush_pending()。Chat 臂
//! 只 stage（写文件+暂存）不早翻——早翻会被本回合后续的 face::write 清掉
//! （v4④ 靠后台线程晚翻旗侥幸避开，同步化后必踩，故整体后移到出口）。
//! 世代线同步退役：任何新用户动作都会 face::write(result=false)，term
//! 下降沿自己拆台——失效机制就是旗子本身。

use std::sync::Mutex;

use crate::face;

/// 结果页 HTML（v4⑥）。voice 原子换名写；term face 假→真沿读（同 face
/// mtime 先例）。
pub const RESULT_HTML: &str = "/run/aginx-voice/result.html";

/// 本回合已暂存结果页，等 run_outs 尾部统一翻旗。值=翻旗时用的 state 名。
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// Chat 臂：同步写 result.html（毫秒级，无线程无引擎往返），暂存 state
/// 待尾部翻旗。文本先行不变——调用方必须已 set_line(Q\nA)。问句块
/// #283 常驻：面板上问句在答句上方，与光标面同形状。
pub fn stage_reply(state: &str, question: &str, markdown: &str) {
    let p = hwd::load_or_exit();
    let html = page_html(question, markdown, p.panel.width, p.panel.height);
    let tmp = format!("{RESULT_HTML}.tmp");
    if std::fs::write(&tmp, html).is_ok() && std::fs::rename(&tmp, RESULT_HTML).is_ok() {
        *PENDING.lock().unwrap() = Some(state.to_string());
    } else {
        eprintln!("aginx-voice: result.html write failed");
    }
}

/// run_outs 尾部（followups 循环后）调用：全程序唯一翻旗点。无暂存=空转。
pub fn flush_pending() {
    if let Some(state) = PENDING.lock().unwrap().take() {
        face::write_result(&state);
    }
}

/// 行内转换：`^\d+\. ` 识别有序列表项，返回项文本。
fn ordered_item(l: &str) -> Option<&str> {
    let digits = l.len() - l.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    l[digits..].strip_prefix(". ")
}

/// markdown-lite → 磷光整页 HTML。目标子集=真实 brain 回复里见过的形状：
/// ``` 围栏（原文捕获，内部 # 不成标题、缩进空白保留）、#/##/### 标题、
/// **粗体**、`code`、- 无序与 1. 有序列表、> 引用、--- 分隔线、| 表格、段落。
/// 先 HTML 转义再内联替换——内容永远是文本，不是标签。
/// 三钉烧死（引擎收据）：黑底、min-height 满屏高 px（引擎丢 vh 单位）、
/// 视口=面板宽；配色与字阶=磷光终端（黑底绿白字，P0 ASK_TMPL 语言）。
/// 问句块（#283 常驻）：用户原话转义直进，不进 markdown——问的是什么
/// 就显示什么。panel 尺寸是参数（D14）：产品走 stage_reply（hwd [panel]），
/// host 测试喂 fixture——纯函数两种调用方都不碰 /etc。
pub fn page_html(question: &str, md: &str, panel_w: u32, panel_h: u32) -> String {
    let mut body = String::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim();
        if l.is_empty() {
            i += 1;
        } else if l.starts_with("```") {
            // 围栏：原文行直收（保缩进），只转义不做任何 markdown 处理
            let mut code: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() {
                if lines[i].trim().starts_with("```") {
                    i += 1;
                    break;
                }
                code.push(lines[i]);
                i += 1;
            }
            body.push_str(&format!(
                "<pre><code>{}</code></pre>\n",
                escape(&code.join("\n"))
            ));
        } else if let Some(rest) = l.strip_prefix("### ") {
            body.push_str(&format!("<h3>{}</h3>\n", inline(rest)));
            i += 1;
        } else if let Some(rest) = l.strip_prefix("## ") {
            body.push_str(&format!("<h2>{}</h2>\n", inline(rest)));
            i += 1;
        } else if let Some(rest) = l.strip_prefix("# ") {
            body.push_str(&format!("<h1>{}</h1>\n", inline(rest)));
            i += 1;
        } else if l.starts_with('|') {
            let mut rows: Vec<Vec<String>> = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if !t.starts_with('|') {
                    break;
                }
                // |---|---| 分隔线跳过
                if t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                    i += 1;
                    continue;
                }
                rows.push(
                    t.trim_matches('|')
                        .split('|')
                        .map(|c| inline(c.trim()))
                        .collect(),
                );
                i += 1;
            }
            body.push_str("<table>");
            for (n, cells) in rows.iter().enumerate() {
                body.push_str("<tr>");
                for c in cells {
                    if n == 0 {
                        body.push_str(&format!("<th>{c}</th>"));
                    } else {
                        body.push_str(&format!("<td>{c}</td>"));
                    }
                }
                body.push_str("</tr>");
            }
            body.push_str("</table>\n");
        } else if l.starts_with("- ") {
            body.push_str("<ul>");
            while i < lines.len() {
                match lines[i].trim().strip_prefix("- ") {
                    Some(rest) => {
                        body.push_str(&format!("<li>{}</li>", inline(rest)));
                        i += 1;
                    }
                    None => break,
                }
            }
            body.push_str("</ul>\n");
        } else if ordered_item(l).is_some() {
            body.push_str("<ol>");
            while i < lines.len() {
                match ordered_item(lines[i].trim()) {
                    Some(rest) => {
                        body.push_str(&format!("<li>{}</li>", inline(rest)));
                        i += 1;
                    }
                    None => break,
                }
            }
            body.push_str("</ol>\n");
        } else if l.starts_with('>') {
            body.push_str("<blockquote>");
            while i < lines.len() {
                match lines[i].trim().strip_prefix('>') {
                    Some(rest) => {
                        let rest = rest.strip_prefix(' ').unwrap_or(rest);
                        if !rest.is_empty() {
                            body.push_str(&format!("<p>{}</p>", inline(rest)));
                        }
                        i += 1;
                    }
                    None => break,
                }
            }
            body.push_str("</blockquote>\n");
        } else if l.len() >= 3 && l.chars().all(|c| c == '-') {
            body.push_str("<hr>\n");
            i += 1;
        } else {
            body.push_str(&format!("<p>{}</p>\n", inline(l)));
            i += 1;
        }
    }
    let q = question.trim();
    let qblock = if q.is_empty() {
        String::new()
    } else {
        format!("<div class=\"q\">{}</div>\n", escape(q))
    };
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width={panel_w}\">\
<style>html,body{{background:#000}}\
body{{min-height:{panel_h}px;color:#8cffb0;font:36px/1.75 sans-serif;\
padding:56px 48px;margin:0}}\
h1{{font-size:52px;color:#eaffea;margin:0.8em 0 0.4em;font-weight:700}}\
h2{{font-size:46px;color:#eaffea;margin:0.8em 0 0.4em;font-weight:700}}\
h3{{font-size:40px;color:#eaffea;margin:0.8em 0 0.4em;font-weight:700}}\
p{{margin:18px 0}}\
ul,ol{{margin:18px 0;padding-left:48px}}\
li{{margin:10px 0}}\
b{{color:#ffffff}}\
code{{font:30px/1.5 monospace;color:#00ff41;background:#061006;\
padding:2px 10px;border-radius:6px}}\
pre{{background:#0a120a;border:1px solid #123f1f;border-radius:8px;\
padding:24px 28px;margin:24px 0;white-space:pre-wrap;overflow:hidden}}\
pre code{{font:28px/1.6 monospace;color:#c8ffd0;background:none;padding:0}}\
table{{border-collapse:collapse;width:100%;table-layout:fixed;font-size:28px;margin:24px 0}}\
td,th{{border:1px solid #005c1f;padding:12px 14px;text-align:left;word-wrap:break-word}}\
th{{background:#04170a;color:#eaffea}}\
blockquote{{margin:24px 0;padding:12px 28px;border-left:6px solid #1f6f3f;color:#9fd8ab}}\
blockquote p{{margin:10px 0}}\
hr{{border:none;border-top:2px solid #0f3d1e;margin:40px 0}}\
.meta{{font-size:26px;color:#3d6b4f}}\
.q{{color:#3d6b4f;font-size:32px;line-height:1.6;margin:0 0 44px;\
padding:20px 28px;border-left:6px solid #1f6f3f;white-space:pre-wrap}}\
</style></head><body>{qblock}{body}</body></html>"
    )
}

/// 纯 HTML 转义（围栏用：不做任何 markdown 内联处理）
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 行内：先转义，再 **粗体** 与 `code`（字面符号配对，奇数段在内侧）
fn inline(s: &str) -> String {
    let s = escape(s);
    let mut out = String::new();
    for (n, seg) in s.split("**").enumerate() {
        if n % 2 == 1 {
            out.push_str(&format!("<b>{seg}</b>"));
        } else {
            for (m, cseg) in seg.split('`').enumerate() {
                if m % 2 == 1 {
                    out.push_str(&format!("<code>{cseg}</code>"));
                } else {
                    out.push_str(cseg);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// host 测试统一入口：无问句整页 + fixture 面板（真实红皮尺寸，纯数据
    /// 不碰 /etc）。
    fn md(s: &str) -> String {
        page_html("", s, 1080, 2340) // D14-exempt: fixture panel geometry
    }

    #[test]
    fn html_has_three_pins() {
        let h = md("你好");
        assert!(h.contains("background:#000"));
        assert!(h.contains("min-height:2340px")); // D14-exempt: fixture pin
        assert!(h.contains("width=1080")); // D14-exempt: fixture pin
    }

    #[test]
    fn pins_follow_panel_params() {
        // 面板参数真的进了三钉——不是碰巧写死
        let h = page_html("", "x", 720, 1600); // D14-exempt: fixture panel
        assert!(h.contains("min-height:1600px"));
        assert!(h.contains("width=720"));
        assert!(!h.contains("2340")); // D14-exempt: fixture pin must not leak
    }

    #[test]
    fn headings_bold_code_list() {
        let h = md("# 大\n## 中\n**杭州** `26度`\n- 甲\n- 乙\n");
        assert!(h.contains("<h1>大</h1>"));
        assert!(h.contains("<h2>中</h2>"));
        assert!(h.contains("<b>杭州</b>"));
        assert!(h.contains("<code>26度</code>"));
        assert!(h.contains("<li>甲</li>"));
        assert!(h.contains("<li>乙</li>"));
        assert!(h.contains("<ul>"));
    }

    #[test]
    fn table_with_separator_row() {
        let h = md("| 城市 | 温度 |\n|---|---|\n| 杭州 | 26 |\n");
        assert!(h.contains("<th>城市</th>"));
        assert!(h.contains("<th>温度</th>"));
        assert!(h.contains("<td>杭州</td>"));
        assert!(h.contains("<td>26</td>"));
        assert!(!h.contains("---"));
    }

    #[test]
    fn content_is_escaped_never_tags() {
        let h = md("<script>&x</script>");
        assert!(h.contains("&lt;script&gt;&amp;x&lt;/script&gt;"));
        assert!(!h.contains("<script>"));
    }

    #[test]
    fn paragraph_wraps_plain_line() {
        let h = md("今天晴，26 度。");
        assert!(h.contains("<p>今天晴，26 度。</p>"));
    }

    #[test]
    fn fence_keeps_raw_lines_and_no_headings() {
        let md_src = "```python\n# 注释不是标题\n    缩进保留\nx = 1\n```\n后文\n";
        let h = md(md_src);
        assert!(h.contains("<pre><code>"));
        assert!(h.contains("# 注释不是标题"));
        assert!(!h.contains("<h1>"), "围栏内 # 绝不变成标题");
        assert!(h.contains("    缩进保留"), "围栏内缩进空白原样保留");
        assert!(h.contains("<p>后文</p>"), "围栏闭合后恢复正常解析");
        assert!(h.contains("white-space:pre-wrap"));
    }

    #[test]
    fn fence_escapes_html_no_inline() {
        let h = md("```\n<b>&i\n```");
        assert!(h.contains("&lt;b&gt;&amp;i"));
    }

    #[test]
    fn ordered_list_blockquote_hr() {
        let h = md("1. 甲\n2. 乙\n\n> 引用行\n\n---\n收尾\n");
        assert!(h.contains("<ol>"));
        assert!(h.contains("<li>甲</li>"));
        assert!(h.contains("<li>乙</li>"));
        assert!(h.contains("<blockquote><p>引用行</p></blockquote>"));
        assert!(h.contains("<hr>"));
        assert!(h.contains("<p>收尾</p>"));
    }

    #[test]
    fn reply_page_keeps_question_block_above_body() {
        // #283 问句常驻：问句块在正文上方，转义原文不进 markdown；
        // 无问句（render_html 老入口）不出现空块。
        let h = page_html("现在几点了？<b>", "**17** 点", 1080, 2340); // D14-exempt: fixture panel
        let q = h.find("class=\"q\"").expect("question block present");
        let body = h.find("<b>17</b>").expect("markdown body rendered");
        assert!(q < body, "question sits above the reply body");
        assert!(
            h.contains("现在几点了？&lt;b&gt;"),
            "question escaped verbatim, no markdown"
        );
        assert!(!page_html("", "答", 1080, 2340).contains("class=\"q\"")); // D14-exempt: fixture panel
    }
}
