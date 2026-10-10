//! 日循环腿（daily-review，设计=docs/日循环.md）：系统每晚自己整理当天。
//!
//! Leg A `day`：确定性材料化（只读）——当日 codex 会话（/root 与 /home
//! 两世界）、晨报执行账、研究账、频道日志、已知索引，stdout 出 JSON；
//! `--prompt` 在 JSON 外再出整装 prompt（材料清单内嵌 + lifecycle
//! evolution 的 JSON 契约原核），Leg B 的 codex exec 直接吃。
//! Leg C `digest`：stdin 吃 codex 原样输出——严格验 JSON（坏件拒收、
//! 缺口件标 degraded、exit 1），parse_analysis_response + apply_evolution
//! 落 knowledge/（lifecycle 原核：双层格式+版本留痕+MEMORY.md 索引重建），
//! 汇总写晨报输入件 inputs/day-review.md。
//!
//! 纪律：day 材料绝不出设备——本工具只在设备上跑，输出只进本机
//! codex 回合；测试用 AGINX_MEM_DAY_ROOT 把全部根路径搬到临时树。

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 根路径表：设备缺省；测试经 AGINX_MEM_DAY_ROOT 前缀整体搬树。
#[derive(Debug, Clone)]
pub struct Roots {
    pub base: PathBuf,
    pub workspace: PathBuf,
    pub digest_out: PathBuf,
}

impl Roots {
    pub fn from_env() -> Self {
        match std::env::var("AGINX_MEM_DAY_ROOT") {
            Ok(base) if !base.is_empty() => {
                let b = PathBuf::from(base);
                Roots {
                    workspace: b.clone(),
                    digest_out: b
                        .join("home/workflows/morning-report/inputs/day-review.md"),
                    base: b,
                }
            }
            _ => Roots {
                base: PathBuf::from("/"),
                workspace: PathBuf::from("/home"),
                digest_out: PathBuf::from(
                    "/home/workflows/morning-report/inputs/day-review.md",
                ),
            },
        }
    }

    fn p(&self, abs: &str) -> PathBuf {
        // 设备路径全是 / 开头的绝对路径；搬树时剥前导 / 挂 base 下。
        self.base.join(abs.trim_start_matches('/'))
    }
}

/// `aginx-mem day [--date YYYY-MM-DD] [--prompt]`
pub async fn run_day(date: Option<&str>, prompt: bool, ws_flag: Option<&Path>) -> Result<()> {
    let roots = Roots::from_env();
    let workspace = ws_flag.map(|p| p.to_path_buf()).unwrap_or(roots.workspace.clone());
    let day = date
        .map(|d| d.to_string())
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    if !day.chars().take(10).all(|c| c.is_ascii_digit() || c == '-') || day.len() < 10 {
        bail!("--date 需要 YYYY-MM-DD 形状，得到 {day:?}");
    }

    let materials = gather_materials(&roots, &workspace, &day).await?;
    if prompt {
        println!("{}", build_prompt(&materials, &day).await);
    } else {
        println!("{}", serde_json::to_string_pretty(&materials)?);
    }
    Ok(())
}

async fn gather_materials(roots: &Roots, workspace: &Path, day: &str) -> Result<Value> {
    let (y, m, d) = split_ymd(day);

    // codex 会话：/root 与 /home 两世界（root 手跑 vs wrapper/服务）。
    let mut codex_sessions = Vec::new();
    for home in ["/root/.codex/sessions", "/home/.codex/sessions"] {
        let dir = roots.p(&format!("{home}/{y}/{m}/{d}"));
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().map(|x| x == "jsonl").unwrap_or(false) {
                    codex_sessions.push(session_stats(&path));
                }
            }
        }
    }
    codex_sessions.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));

    // 会话索引（分诊层）：网关远/近两本。
    let sessions_index: Vec<Value> = ["/var/lib/aginx/gateway/sessions.json", "/var/lib/aginx/gateway-local/sessions.json"]
        .iter()
        .map(|p| {
            let path = roots.p(p);
            let meta = fs::metadata(&path);
            json!({
                "path": path.to_string_lossy(),
                "exists": meta.is_ok(),
                "bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            })
        })
        .collect();

    // 晨报执行账：当日 wrapper 日志 + 归档落没落 + 末行状态。
    let morning = morning_state(roots, day);

    // 研究账：/home/research/logs/<名>.log 的当日轮数/成败/末次活动。
    let research = research_state(roots, day);

    // 频道日志（体量+错误行数；细读留给 codex）。
    let channels = {
        let path = roots.p("/var/log/aginx-svc/aginx-channels.log");
        let meta = fs::metadata(&path);
        let errors = fs::read_to_string(&path)
            .map(|t| t.lines().filter(|l| l.contains("ERROR")).count())
            .unwrap_or(0);
        json!({"path": path.to_string_lossy(), "bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0), "error_lines": errors})
    };

    // 已知索引（对账面：已存在的不提取）。
    let known_index = crate::knowledge::knowledge_list(Some(workspace))
        .await
        .unwrap_or_else(|_| "（索引不可用）".into());
    let known_count = known_index
        .lines()
        .filter(|l| l.starts_with("- "))
        .count();

    let activity = !codex_sessions.is_empty()
        || morning["report_archived"].as_bool().unwrap_or(false)
        || research
            .iter()
            .any(|r| r["rounds"].as_u64().unwrap_or(0) > 0);

    Ok(json!({
        "date": day,
        "generated_at": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        "workspace": workspace.to_string_lossy(),
        "activity": activity,
        "codex_sessions": codex_sessions,
        "sessions_index": sessions_index,
        "morning": morning,
        "research": research,
        "channels": channels,
        "known_index_count": known_count,
    }))
}

fn split_ymd(day: &str) -> (String, String, String) {
    let mut it = day.split('-');
    let y = it.next().unwrap_or_default().to_string();
    let m = it.next().unwrap_or_default().to_string();
    let d = it.next().unwrap_or_default().to_string();
    (y, m, d)
}

fn session_stats(path: &Path) -> Value {
    let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (lines, user_msgs, assistant_msgs) = count_turns(path);
    json!({
        "path": path.to_string_lossy(),
        "bytes": bytes,
        "lines": lines,
        "user_msgs": user_msgs,
        "assistant_msgs": assistant_msgs,
    })
}

/// 逐行数回合（codex rollout：response_item 行内 `"role":"user"` 等）。
/// 只做子串计数，不解析整行 JSON——材料化要便宜，细读是 codex 的活。
fn count_turns(path: &Path) -> (u64, u64, u64) {
    let mut lines = 0u64;
    let mut user = 0u64;
    let mut assistant = 0u64;
    if let Ok(content) = fs::read_to_string(path) {
        for l in content.lines() {
            lines += 1;
            if l.contains("\"role\":\"user\"") {
                user += 1;
            } else if l.contains("\"role\":\"assistant\"") {
                assistant += 1;
            }
        }
    }
    (lines, user, assistant)
}

fn morning_state(roots: &Roots, day: &str) -> Value {
    let compact = day.replace('-', "");
    let logs_dir = roots.p("/home/workflows/morning-report/logs");
    let mut logs = Vec::new();
    if let Ok(entries) = fs::read_dir(&logs_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("codex-{compact}")) {
                logs.push(json!(e.path().to_string_lossy()));
            }
        }
    }
    logs.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    let last_status = logs
        .last()
        .and_then(|p| fs::read_to_string(Path::new(p.as_str().unwrap())).ok())
        .and_then(|t| t.lines().rev().find(|l| !l.trim().is_empty()).map(String::from))
        .unwrap_or_else(|| "（无当日日志）".into());

    // 归档名：晨报-YYYY年M月D日.src.md（月/日不补零，设备实测形状）。
    let (y, m, d) = split_ymd(day);
    let mn: u32 = m.parse().unwrap_or(0);
    let dn: u32 = d.parse().unwrap_or(0);
    let archived = roots.p(&format!(
        "/home/workflows/morning-report/output/晨报-{y}年{mn}月{dn}日.src.md"
    ));
    json!({
        "logs": logs,
        "last_status": last_status,
        "report_archived": archived.exists(),
        "report_path": archived.to_string_lossy(),
    })
}

fn research_state(roots: &Roots, day: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let dir = roots.p("/home/research/logs");
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(inst) = name.strip_suffix(".log") else { continue };
        let Ok(text) = fs::read_to_string(e.path()) else { continue };
        let mut rounds = 0u64;
        let mut rc0 = 0u64;
        let mut failed = 0u64;
        let mut in_day = false;
        let mut last_activity = String::new();
        for l in text.lines() {
            if let Some(rest) = l.strip_prefix("=== ") {
                in_day = rest.starts_with(day);
                if in_day {
                    rounds += 1;
                    last_activity = rest[..rest.len().min(19)].to_string();
                }
            } else if in_day {
                if l.starts_with("--- rc=0") {
                    rc0 += 1;
                } else if l.starts_with("--- rc=") {
                    failed += 1;
                }
            }
        }
        out.push(json!({
            "name": inst,
            "log": e.path().to_string_lossy(),
            "rounds": rounds,
            "rc0": rc0,
            "failed": failed,
            "last_activity": last_activity,
        }));
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

/// Leg B 的整装 prompt：日批包一层 + evolution 契约原核。
async fn build_prompt(materials: &Value, day: &str) -> String {
    let ws = Path::new(materials["workspace"].as_str().unwrap_or("/home"));
    let known = crate::knowledge::knowledge_list(Some(ws))
        .await
        .unwrap_or_else(|_| "（索引不可用）".into());
    format!(
        r#"你是系统的日循环整理官。整理 {day} 一天（材料清单里的「昨天」）的系统活动，产出知识更新。

材料清单（JSON；路径全在本机，需要细看就直接读文件，大文件可只读首尾）：
{materials}

已知知识索引（已存在的不提取）：
{known}

{}

流程要求：
1. 主料=codex 会话（当天全部对话回合）；晨报/研究/频道账是辅证
2. 对照已知索引只提取新增：业务事实、裁决、配方、踩坑、偏好都算知识
3. gaps=系统应该知道但材料显露它不知道的缺口
4. 只返回 JSON，不要其他文字"#,
        carrier_lifecycle::evolution::build_analysis_prompt()
    )
}

/// `aginx-mem digest [--out FILE]`（workspace 走 --workspace 旗标）。
/// 返回进程退出码：0=落账（含空跑）；1=degraded。
pub async fn run_digest(ws_flag: Option<&Path>, out_flag: Option<&Path>) -> Result<i32> {
    let roots = Roots::from_env();
    let workspace = ws_flag.map(|p| p.to_path_buf()).unwrap_or(roots.workspace.clone());
    let out = out_flag.map(PathBuf::from).unwrap_or(roots.digest_out.clone());

    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("digest 从 stdin 读 codex 输出失败")?;
    let (rc, _) = apply_and_summarize(&workspace, &out, &raw).await?;
    Ok(rc)
}

/// digest 核心：验件→落账→汇总件。抽出来供测试直吃（不经 stdin）。
pub async fn apply_and_summarize(workspace: &Path, out: &Path, raw: &str) -> Result<(i32, Value)> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // 严格验 JSON：坏件拒收（不落任何 knowledge），缺口件标 degraded。
    let strict = strict_parse(&raw);
    let (analysis, degraded, reason) = match strict {
        Ok(()) => (
            carrier_lifecycle::evolution::parse_analysis_response(&raw)
                .unwrap_or(carrier_lifecycle::evolution::EvolutionAnalysis {
                    knowledge: vec![],
                    gaps: vec![],
                    trivial: true,
                }),
            false,
            String::new(),
        ),
        Err(e) => (
            carrier_lifecycle::evolution::EvolutionAnalysis {
                knowledge: vec![],
                gaps: vec![],
                trivial: true,
            },
            true,
            e,
        ),
    };

    let empty_day = !degraded && analysis.knowledge.is_empty() && analysis.gaps.is_empty();
    let saved = if empty_day {
        Vec::new()
    } else {
        carrier_lifecycle::evolution::apply_evolution(&workspace, &analysis, None, None, None)
    };

    let lint = crate::knowledge::knowledge_lint(Some(&workspace))
        .await
        .unwrap_or_else(|_| "（lint 不可用）".into());
    let lint_errors = lint.lines().filter(|l| l.to_lowercase().contains("error")).count();

    let status = if degraded {
        "degraded"
    } else if empty_day {
        "空跑（无新知识无缺口，幂等零产出）"
    } else {
        "ok"
    };
    let mut body = format!("# 日循环自查 · {today}\n\n- 状态：{status}\n");
    if degraded {
        body.push_str(&format!("- 原因：codex 输出不是合法分析 JSON（{reason}）；本日知识未落账\n"));
    }
    body.push_str(&format!("- 新学 {} 条\n", saved.len()));
    for p in &saved {
        body.push_str(&format!("  - {}\n", p.file_name().map(|f| f.to_string_lossy()).unwrap_or_default()));
    }
    body.push_str(&format!("- 知识缺口 {} 条：\n", analysis.gaps.len()));
    for g in &analysis.gaps {
        body.push_str(&format!("  - {g}\n"));
    }
    body.push_str(&format!("- lint：{lint_errors} 项错误待修\n"));
    body.push_str("\n（本文件=次日晨报「昨日自查」段的输入件；degraded=引擎坏了要人看）\n");

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, body).with_context(|| format!("写 {}", out.display()))?;

    let summary = json!({
        "ok": !degraded,
        "status": status,
        "saved": saved.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "gaps": analysis.gaps,
        "lint_errors": lint_errors,
        "out": out.to_string_lossy(),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok((if degraded { 1 } else { 0 }, summary))
}

/// 严格校验：剥壳后必须是合法 JSON 对象，且带三个契约键之一。
fn strict_parse(raw: &str) -> std::result::Result<(), String> {
    let start = raw.find('{').ok_or("输出里没有 JSON 对象")?;
    let end = raw.rfind('}').ok_or("输出里没有 JSON 对象")?;
    let v: Value = serde_json::from_str(raw[start..=end].trim())
        .map_err(|e| format!("JSON 解析失败：{e}"))?;
    if !v.is_object() {
        return Err("JSON 不是对象".into());
    }
    let has_key = ["has_new_knowledge", "knowledge", "gaps"]
        .iter()
        .any(|k| v.get(k).is_some());
    if !has_key {
        return Err("JSON 缺契约键（has_new_knowledge/knowledge/gaps）".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tree() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn day_materializes_and_flags_empty_activity() {
        let t = tree();
        std::env::set_var("AGINX_MEM_DAY_ROOT", t.path());
        let roots = Roots::from_env();
        // 造一个 codex 会话：/root 世界当天一回合。
        let sess = roots.p("/root/.codex/sessions/2026/10/10");
        fs::create_dir_all(&sess).unwrap();
        fs::write(
            sess.join("rollout-x.jsonl"),
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[]}}\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n",
        )
        .unwrap();
        let ws = roots.workspace.clone();
        let m = futures_block_on(gather_materials(&roots, &ws, "2026-10-10"));
        let m = match m {
            Ok(v) => v,
            Err(e) => panic!("{e}"),
        };
        assert!(m["activity"].as_bool().unwrap());
        let s = &m["codex_sessions"][0];
        assert_eq!(s["user_msgs"], 1);
        assert_eq!(s["assistant_msgs"], 1);
        // 空跑日：无会话无晨报无研究轮。
        let m2 = futures_block_on(gather_materials(&roots, &ws, "2026-10-09"));
        assert!(!m2.unwrap()["activity"].as_bool().unwrap());
        std::env::remove_var("AGINX_MEM_DAY_ROOT");
    }

    #[test]
    fn strict_parse_rejects_garbage() {
        assert!(strict_parse("我觉得今天挺好的").is_err());
        assert!(strict_parse("{\"foo\": 1}").is_err());
        assert!(strict_parse("前言{\"has_new_knowledge\":false,\"knowledge\":[],\"gaps\":[]}后语").is_ok());
    }

    #[test]
    fn digest_end_to_end_three_shapes() {
        let t = tree();
        let mk = |name: &str| {
            let ws = t.path().join(name);
            fs::create_dir_all(&ws).unwrap();
            let out = ws.join("inputs/day-review.md");
            (ws, out)
        };
        let run = |ws: &Path, out: &Path, raw: &str| {
            futures_block_on(apply_and_summarize(ws, out, raw)).unwrap()
        };

        // ① 正常落账：一条知识 + 一个缺口。
        let (ws, out) = mk("ok");
        let (rc, summary) = run(
            &ws,
            &out,
            r#"{"has_new_knowledge":true,"knowledge":[{"title":"wifi-psk-law","content":"PSK 走 AGINX_WIFI_PSK env，不进 conf。","scope":"shared"}],"gaps":["短剧分账规则还没有条目"]}"#,
        );
        assert_eq!(rc, 0);
        assert_eq!(summary["status"], "ok");
        let kf = ws.join("knowledge/wifi-psk-law.md");
        let ktext = fs::read_to_string(&kf).unwrap();
        assert!(ktext.starts_with("---\nname: wifi-psk-law"));
        assert!(ktext.contains("AGINX_WIFI_PSK"));
        let mem = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
        assert!(mem.contains("wifi-psk-law.md"));
        let dr = fs::read_to_string(&out).unwrap();
        assert!(dr.contains("状态：ok"));
        assert!(dr.contains("短剧分账规则还没有条目"));
        // 版本留痕：JSONL 有当天行（timestamp 是 UTC rfc3339，日期断言用 UTC）。
        let vjson = fs::read_to_string(ws.join("history/versions.jsonl")).unwrap();
        assert!(vjson.contains("wifi-psk-law.md"));
        assert!(vjson.contains("\"source\":\"evolution\""));

        // ② 坏 JSON：拒收 + degraded，零 knowledge 落盘。
        let (ws, out) = mk("bad");
        let (rc, summary) = run(&ws, &out, "今天心情不错，没有 JSON");
        assert_eq!(rc, 1);
        assert_eq!(summary["status"], "degraded");
        assert!(!ws.join("knowledge").exists());
        let dr = fs::read_to_string(&out).unwrap();
        assert!(dr.contains("degraded"));
        assert!(dr.contains("未落账"));

        // ③ 空跑：幂等零产出。
        let (ws, out) = mk("idle");
        let (rc, summary) = run(
            &ws,
            &out,
            r#"{"has_new_knowledge":false,"knowledge":[],"gaps":[]}"#,
        );
        assert_eq!(rc, 0);
        assert_eq!(summary["status"], "空跑（无新知识无缺口，幂等零产出）");
        assert!(!ws.join("knowledge").exists());
        let dr = fs::read_to_string(&out).unwrap();
        assert!(dr.contains("空跑"));
    }

    // 薄 runtime 桥：测试里没有 tokio 上下文时的阻塞执行器。
    fn futures_block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }
}
