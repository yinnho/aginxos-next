//! 化身市场机制（UI 无关层）——DupHub 列表 / 权限预览 / 一键安装的数据面。
//!
//! 从 webui/market.rs 抽出（webui 随 `web` 子命令退役，2026-08-30 AginxOS
//! 融合裁决）：CLI（`aginx-carrier agent install`）与未来任何宿主共用。
//! 拉取走本 crate `hub` 的 dup 文件级端点（带 SSRF 防护）；安装本体走
//! kernel 的 `clone_install_files` 正规管线（校验→落盘→spawn→入网钩子）。
//!
//! 权限预览是消费级硬需求：安装 = 装代码，必须在安装前看到化身声明了
//! 什么（flows/tools/shell_allow/mcp_servers）。

use std::collections::BTreeMap;

use futures::StreamExt;

use crate::hub;

/// 预览子集文件上限（防滥用：只拉 template.json + flows + SOUL.md）。
pub const PREVIEW_MAX_FILES: usize = 40;
const PREVIEW_CONCURRENCY: usize = 4;
/// 安装全量文件并行度（fetch_dup_files 是串行逐文件，大化身不可接受）。
const INSTALL_CONCURRENCY: usize = 8;

/// Hub 交互错误的稳定分类（CLI 与宿主按 code 提示，不认 HTTP 状态码）。
pub struct HubError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// hub.rs 的 bail 文本稳定携带 StatusCode Display（"拉取 manifest 失败
/// {name}: 401 Unauthorized - ..."）。扫第一个 ": NNN" 段做分类。
fn status_in_err(text: &str) -> Option<u16> {
    let mut from = 0usize;
    while let Some(p) = text[from..].find(": ") {
        let s = from + p + 2;
        let digits: String = text[s..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() == 3 {
            if let Ok(n) = digits.parse::<u16>() {
                return Some(n);
            }
        }
        from = s;
    }
    None
}

/// 把 hub 底层错误文本分类成稳定 code + 友好文案。
pub fn classify_hub_err(text: &str) -> HubError {
    let (code, message) = match status_in_err(text) {
        Some(401) => ("bad_key", "Hub API key 无效或已失效，请更新后重试".to_string()),
        Some(402) => ("paid", "付费模版——需先在 DupHub 购买后再安装".to_string()),
        Some(403) => ("private", "私有化身，当前 key 无权访问".to_string()),
        Some(404) => ("not_found", "Hub 上不存在这个化身".to_string()),
        _ => ("hub_unreachable", format!("Hub 通信失败: {text}")),
    };
    HubError { code, message }
}

/// clone_install_files 的 name 规则前置预检（安装端是硬门禁，提前友好提示）。
pub fn valid_clone_name(name: &str) -> bool {
    let n = name.len();
    (1..=64).contains(&n)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// 并行拉一组 dup 文件（fetch_dup_files 是串行逐文件，这里带并发度）。
/// 错误返回原始 bail 文本，由 classify_hub_err 统一分类。
async fn fetch_parallel(
    hub_url: &str,
    key: &str,
    name: &str,
    paths: &[String],
    concurrency: usize,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let results: Vec<(String, Result<Vec<u8>, String>)> =
        futures::stream::iter(paths.to_vec())
            .map(|p| {
                let hub = hub_url.to_string();
                let key = key.to_string();
                let name = name.to_string();
                async move {
                    let r = hub::fetch_dup_file(&hub, &key, &name, &p, None)
                        .await
                        .map_err(|e| format!("{e:#}"));
                    (p, r)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;
    let mut out = BTreeMap::new();
    for (p, r) in results {
        out.insert(p, r?);
    }
    Ok(out)
}

/// 拉安装全量文件：manifest 全路径 → 并行取回。安装器（kernel
/// `clone_install_files`）的输入形状。
pub async fn fetch_install_files(
    hub_url: &str,
    key: &str,
    name: &str,
) -> Result<BTreeMap<String, Vec<u8>>, HubError> {
    let manifest = hub::fetch_dup_manifest(hub_url, key, name, None)
        .await
        .map_err(|e| classify_hub_err(&format!("{e:#}")))?;
    let paths: Vec<String> = manifest.files.keys().cloned().collect();
    fetch_parallel(hub_url, key, name, &paths, INSTALL_CONCURRENCY)
        .await
        .map_err(|e| classify_hub_err(&e))
}

/// 拉权限预览子集文件（template.json + flows frontmatter + SOUL.md；
/// knowledge/references 等大文件不拉）。
pub async fn fetch_preview_files(
    hub_url: &str,
    key: &str,
    name: &str,
) -> Result<BTreeMap<String, Vec<u8>>, HubError> {
    let manifest = hub::fetch_dup_manifest(hub_url, key, name, None)
        .await
        .map_err(|e| classify_hub_err(&format!("{e:#}")))?;
    let paths = preview_paths(&manifest.files);
    fetch_parallel(hub_url, key, name, &paths, PREVIEW_CONCURRENCY)
        .await
        .map_err(|e| classify_hub_err(&e))
}

/// 预览要拉的文件子集（上限 PREVIEW_MAX_FILES）。
fn preview_paths(files: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for p in files.keys() {
        let wanted = p == "template.json"
            || p == "SOUL.md"
            || p == "profile.md"
            || (p.starts_with("flows/") && (p.ends_with("flow.md") || p.ends_with("SKILL.md")))
            || p.starts_with("skills/");
        if wanted {
            out.push(p.clone());
            if out.len() >= PREVIEW_MAX_FILES {
                break;
            }
        }
    }
    out
}

/// Hub 上该化身的最新版本号（duphub 的"更新" = 定义版本 diff 的对端）。
pub async fn hub_latest_version(
    hub_url: &str,
    key: &str,
    name: &str,
) -> Result<String, HubError> {
    let detail = hub::get_template(hub_url, key, name)
        .await
        .map_err(|e| classify_hub_err(&format!("{e:#}")))?;
    Ok(detail
        .get("latest_version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string())
}

/// 权限预览提取（纯函数，主单测点）。
///
/// 输入：预览子集文件（template.json + flows/*/[flow|SKILL].md + skills/**）。
/// 输出：flows 权限清单 + mcp_servers/plugins/tags + 安装期格式校验错误。
pub fn extract_preview(files: &BTreeMap<String, Vec<u8>>) -> serde_json::Value {
    let mut display_name = String::new();
    let mut description = String::new();
    let mut tags: Vec<String> = Vec::new();
    let mut mcp_servers: Vec<String> = Vec::new();
    let mut plugins: Vec<String> = Vec::new();

    if let Some(b) = files.get("template.json") {
        if let Some(t) = crate::parse_template_manifest_lenient(&String::from_utf8_lossy(b)) {
            if !t.display_name.is_empty() {
                display_name = t.display_name;
            }
            if !t.description.is_empty() {
                description = t.description;
            }
            tags = t.tags;
            mcp_servers = t.mcp_servers;
            plugins = t.plugins;
        }
    }

    let mut flows = Vec::new();
    for (path, bytes) in files {
        let is_flow = path.starts_with("flows/")
            && (path.ends_with("flow.md") || path.ends_with("SKILL.md"));
        if !is_flow {
            continue;
        }
        let def = carrier_types::flow::parse_flow_def(&String::from_utf8_lossy(bytes));
        // frontmatter name 为空时用目录名（flows/<dir>/flow.md → <dir>）
        let dir = path
            .trim_start_matches("flows/")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let name = if def.name.is_empty() {
            dir
        } else {
            def.name.clone()
        };
        let privilege = match def.privilege {
            carrier_types::flow::FlowPrivilege::System => "system",
            carrier_types::flow::FlowPrivilege::Agent => "agent",
        };
        flows.push(serde_json::json!({
            "path": path,
            "name": name,
            "description": def.description,
            "tools": def.tools,
            "shell_allow": def.shell_allow,
            "deny_tools": def.deny_tools,
            "privilege": privilege,
            "entry": def.entry.unwrap_or(true),
            "elevates": def.elevates(),
            "max_level": format!("{:?}", def.required_max_tool_level()).to_lowercase(),
        }));
    }

    // 安装期是同一套校验的硬门禁——预览先跑一遍，错误非空则前端禁装。
    let format_errors = crate::validate_install_format(files).unwrap_or_default();

    serde_json::json!({
        "display_name": display_name,
        "description": description,
        "tags": tags,
        "mcp_servers": mcp_servers,
        "plugins": plugins,
        "flows": flows,
        "format_errors": format_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn extract_preview_golden() {
        let template_json = r#"{
            "version": "3", "name": "demo", "display_name": "演示化身",
            "description": "演示用", "tags": ["测试"],
            "mcp_servers": ["aginxbrowser"], "plugins": []
        }"#;
        let good_flow = "---\nname: writer\ndescription: 写文章的流程说明\ntools:\n  - file_read\n  - shell_exec\nshell_allow:\n  - python3 output/scripts/*\n---\n正文内容";
        let bad_desc_flow = "---\ndescription: \ntools:\n  - file_read\n---\n正文";
        let f = files(&[
            ("template.json", template_json),
            ("flows/writer/flow.md", good_flow),
            ("flows/broken/flow.md", bad_desc_flow),
            ("skills/old/legacy.md", "老布局"),
            ("knowledge/big.md", "不该进预览"),
        ]);
        let v = extract_preview(&f);

        assert_eq!(v["display_name"], "演示化身");
        assert_eq!(v["mcp_servers"][0], "aginxbrowser");
        assert_eq!(v["tags"][0], "测试");

        let flows = v["flows"].as_array().unwrap();
        assert_eq!(flows.len(), 2);
        let writer = flows
            .iter()
            .find(|f| f["name"] == "writer")
            .expect("writer flow");
        assert_eq!(writer["elevates"], true);
        assert_eq!(writer["privilege"], "agent");
        assert_eq!(writer["shell_allow"][0], "python3 output/scripts/*");
        assert_eq!(writer["tools"][1], "shell_exec");

        // 空名字 flow 兜底目录名；空 description 被格式校验抓住
        assert!(flows.iter().any(|f| f["name"] == "broken"));
        let errs = v["format_errors"].as_array().unwrap();
        assert!(
            errs.iter().any(|e| e.as_str().unwrap_or("").contains("broken")),
            "空 description 应产生格式错误: {errs:?}"
        );
        // skills/ 废弃布局告警
        assert!(
            errs.iter().any(|e| e.as_str().unwrap_or("").contains("skills")),
            "skills/ 废弃布局应告警: {errs:?}"
        );
    }

    #[test]
    fn status_in_err_finds_codes() {
        assert_eq!(
            status_in_err("拉取 manifest 失败 demo: 402 Payment Required - body"),
            Some(402)
        );
        assert_eq!(
            status_in_err("拉取文件失败 demo [a.md]: 403 Forbidden - x"),
            Some(403)
        );
        assert_eq!(status_in_err("无法连接 Hub: tcp connect error"), None);
        // body 里带 404 不应误报（": 404" 只在状态位出现）
        assert_eq!(
            status_in_err("拉取 manifest 失败 demo: 401 Unauthorized - err 404 found"),
            Some(401)
        );
    }

    #[test]
    fn classify_maps_all_codes() {
        assert_eq!(classify_hub_err("x: 401 Unauthorized").code, "bad_key");
        assert_eq!(classify_hub_err("x: 402 Payment Required").code, "paid");
        assert_eq!(classify_hub_err("x: 403 Forbidden").code, "private");
        assert_eq!(classify_hub_err("x: 404 Not Found").code, "not_found");
        assert_eq!(classify_hub_err("tcp closed").code, "hub_unreachable");
    }

    #[test]
    fn clone_name_rules() {
        assert!(valid_clone_name("ai-writer"));
        assert!(valid_clone_name("a"));
        assert!(!valid_clone_name("AI_Writer"));
        assert!(!valid_clone_name("-bad"));
        assert!(!valid_clone_name(""));
        assert!(!valid_clone_name(&"x".repeat(65)));
    }

    #[test]
    fn hub_default_url_is_duphub() {
        assert_eq!(
            carrier_types::config::HubConfig::default().url,
            "https://duphub.com"
        );
    }
}
