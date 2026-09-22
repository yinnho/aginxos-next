//! Hub client — unified interface for all Hub API calls.
//!
//! All Hub HTTP requests go through this module. External callers should
//! never use raw reqwest to call Hub endpoints.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

// === URL validation ===

/// SECURITY: Validate that a Hub URL is safe to fetch (not an internal/metadata endpoint).
/// Uses the shared carrier_types::ssrf module for comprehensive SSRF protection.
pub fn validate_hub_url(url: &str) -> Result<()> {
    carrier_types::ssrf::check_ssrf(url).map_err(|e| anyhow::anyhow!(e))
}

// === Auth helpers ===

/// Read API key from the configured env var. Falls back to reading ~/.aginx/carrier/.env directly.
pub fn read_api_key(env_var: &str) -> Result<String> {
    if let Ok(v) = std::env::var(env_var) {
        if !v.trim().is_empty() {
            return Ok(v);
        }
    }
    {
        let env_path = carrier_types::config::home_dir().join(".env");
        if let Ok(content) = std::fs::read_to_string(&env_path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(value) = trimmed.strip_prefix(&format!("{}=", env_var)) {
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        return Ok(value);
                    }
                }
            }
        }
    }
    anyhow::bail!(
        "API Key 未设置。请设置环境变量 {} (从 Hub 的 Keys 页面获取)",
        env_var
    )
}

#[derive(Deserialize)]
struct AuthResponse {
    token: String,
}

#[derive(Deserialize)]
struct KeyResponse {
    key: String,
}

/// Register a new account on Hub. Returns JWT token.
pub async fn register(
    hub_url: &str,
    username: &str,
    email: &str,
    password: &str,
) -> Result<String> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/register", base))
        .json(&serde_json::json!({
            "username": username,
            "email": email,
            "password": password,
        }))
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("注册失败 ({}): {}", status, body);
    }

    let auth: AuthResponse = resp.json().await.context("解析注册响应失败")?;
    Ok(auth.token)
}

/// Login to Hub. Returns JWT token.
pub async fn login(hub_url: &str, login: &str, password: &str) -> Result<String> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/login", base))
        .json(&serde_json::json!({
            "login": login,
            "password": password,
        }))
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("登录失败 ({}): {}", status, body);
    }

    let auth: AuthResponse = resp.json().await.context("解析登录响应失败")?;
    Ok(auth.token)
}

/// Create an API key on Hub using JWT token. Returns the plain key.
pub async fn create_api_key(hub_url: &str, jwt: &str, name: &str) -> Result<String> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/keys", base))
        .bearer_auth(jwt)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("创建 API Key 失败: {}", body);
    }

    let data: KeyResponse = resp.json().await.context("解析响应失败")?;
    Ok(data.key)
}

// === Templates ===

#[derive(Deserialize)]
struct SearchResponse {
    templates: Vec<TemplateItem>,
    total: usize,
}

#[derive(Deserialize)]
struct TemplateItem {
    name: String,
    description: String,
    #[allow(dead_code)]
    author: String,
    latest_version: String,
    download_count: i64,
    rating_avg: f64,
}

/// Search templates on Hub. Returns formatted table string.
pub async fn search_templates(hub_url: &str, api_key: &str, query: &str) -> Result<String> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let url = if query.is_empty() {
        format!("{}/api/templates?limit=20", base)
    } else {
        format!(
            "{}/api/templates?q={}&limit=20",
            base,
            urlencoding::encode(query)
        )
    };

    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        bail!("Hub 返回错误: {}", resp.status());
    }

    let data: SearchResponse = resp.json().await.context("解析 Hub 响应失败")?;

    if data.templates.is_empty() {
        return Ok("没有找到匹配的模版".to_string());
    }

    let mut out = format!("找到 {} 个模版:\n\n", data.total);
    out.push_str(&format!(
        "{:<25} {:<12} {:<8} {:<6} {}\n",
        "名称", "版本", "下载", "评分", "描述"
    ));
    out.push_str(&format!("{}\n", "-".repeat(80)));

    for t in &data.templates {
        let desc = if t.description.chars().count() > 30 {
            format!("{}…", t.description.chars().take(30).collect::<String>())
        } else {
            t.description.clone()
        };
        let stars = format_stars(t.rating_avg);
        out.push_str(&format!(
            "{:<25} {:<12} {:<8} {:<6} {}\n",
            t.name, t.latest_version, t.download_count, stars, desc
        ));
    }

    Ok(out)
}

/// Search templates on Hub. Returns the raw JSON response.
/// `page` is 1-based on the Hub side; `None` means page 1 (param omitted).
pub async fn search_templates_json(
    hub_url: &str,
    api_key: &str,
    query: Option<&str>,
    limit: Option<u32>,
    page: Option<u32>,
) -> Result<serde_json::Value> {
    validate_hub_url(hub_url)?;
    let url = search_url(hub_url, query, limit, page);

    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        bail!("Hub 返回错误: {}", resp.status());
    }

    resp.json().await.context("解析 Hub 响应失败")
}

/// Pure URL builder for the Hub template listing (unit-testable).
pub fn search_url(hub_url: &str, query: Option<&str>, limit: Option<u32>, page: Option<u32>) -> String {
    let base = hub_url.trim_end_matches('/');
    let mut params: Vec<String> = Vec::new();
    if let Some(q) = query {
        params.push(format!("q={}", urlencoding::encode(q)));
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(p) = page.filter(|p| *p > 1) {
        params.push(format!("page={p}"));
    }
    if params.is_empty() {
        format!("{}/api/templates", base)
    } else {
        format!("{}/api/templates?{}", base, params.join("&"))
    }
}

/// Get a single template's detail from Hub.
pub async fn get_template(hub_url: &str, api_key: &str, name: &str) -> Result<serde_json::Value> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let url = format!("{}/api/templates/{}", base, urlencoding::encode(name));

    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        bail!("Hub 返回错误: {}", resp.status());
    }

    resp.json().await.context("解析 Hub 响应失败")
}

/// Hub template key used for download/push (name or hub_template_id).
/// DupHub version endpoints accept the template **name**; we also store that
/// as `hub_template_id` so push/update-checks do not require a separate UUID.
pub fn hub_template_key(name: &str, hub_template_id: Option<&str>) -> String {
    hub_template_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(name)
        .to_string()
}

/// Download and install a template from Hub via the file-level dup endpoints.
/// Returns the clone name on success.
///
/// Writes `clone_source.hub_template_id` = Hub template name so later
/// update-checks / push resolve the same Hub key without re-install.
pub async fn install_template(
    hub_url: &str,
    api_key: &str,
    name: &str,
    version: Option<&str>,
    workspace_dir: &Path,
) -> Result<String> {
    tracing::info!("正在从 Hub 文件级拉取 {} ...", name);
    let (_remote_manifest, files) = fetch_dup_files(hub_url, api_key, name, version).await?;
    tracing::info!("已拉取 {} 个文件", files.len());

    crate::manifest::write_files_to_workspace(&files, workspace_dir)?;

    // hub_template_id = template name (name-based Hub API)
    let hub_id = hub_template_key(name, None);
    let manifest = crate::build_manifest_from_workspace(workspace_dir, name, Some(hub_id))?;
    let toml_str = toml::to_string_pretty(&manifest).context("Failed to serialize agent.toml")?;
    std::fs::write(workspace_dir.join("agent.toml"), toml_str)?;

    tracing::info!("分身 '{}' 安装完成 (hub_template_id set)", name);
    Ok(name.to_string())
}

/// Fetch a template's file-level manifest from Hub (dup file-level install).
///
/// Returns the same `{files, hash}` shape opencarrier uses locally, so the
/// hash is directly comparable to a local `build_manifest`. `version=None`
/// resolves to latest on the Hub side.
pub async fn fetch_dup_manifest(
    hub_url: &str,
    api_key: &str,
    name: &str,
    version: Option<&str>,
) -> Result<crate::manifest::Manifest> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let key = urlencoding::encode(name);
    let mut url = format!("{}/api/templates/{}/dup/manifest", base, key);
    if let Some(v) = version {
        url.push_str("?version=");
        url.push_str(&urlencoding::encode(v));
    }
    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("拉取 manifest 失败 {}: {} - {}", name, status, body);
    }
    let manifest: crate::manifest::Manifest = resp.json().await.context("解析 manifest 失败")?;
    Ok(manifest)
}

/// Fetch a single definition-layer file's raw bytes from Hub (dup file-level
/// install). `path` is relative to the workspace root (e.g. `knowledge/x.md`).
pub async fn fetch_dup_file(
    hub_url: &str,
    api_key: &str,
    name: &str,
    path: &str,
    version: Option<&str>,
) -> Result<Vec<u8>> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let key = urlencoding::encode(name);
    // Encode each segment but keep `/` so the Hub's catch-all route matches.
    let encoded_path = path
        .split('/')
        .map(urlencoding::encode)
        .collect::<Vec<_>>()
        .join("/");
    let mut url = format!("{}/api/templates/{}/dup/file/{}", base, key, encoded_path);
    if let Some(v) = version {
        url.push_str("?version=");
        url.push_str(&urlencoding::encode(v));
    }
    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("拉取文件失败 {} [{}]: {} - {}", name, path, status, body);
    }
    let bytes = resp.bytes().await.context("读取响应失败")?;
    Ok(bytes.to_vec())
}

/// Fetch the manifest + every file for a template via the dup file-level
/// endpoints. Convenience for the install path. Returns `(manifest, path -> bytes)`.
pub async fn fetch_dup_files(
    hub_url: &str,
    api_key: &str,
    name: &str,
    version: Option<&str>,
) -> Result<(
    crate::manifest::Manifest,
    std::collections::BTreeMap<String, Vec<u8>>,
)> {
    let manifest = fetch_dup_manifest(hub_url, api_key, name, version).await?;
    let mut files: std::collections::BTreeMap<String, Vec<u8>> = std::collections::BTreeMap::new();
    for path in manifest.files.keys() {
        let bytes = fetch_dup_file(hub_url, api_key, name, path, version).await?;
        files.insert(path.clone(), bytes);
    }
    Ok((manifest, files))
}

/// Push a clone's definition-layer files to Hub as a new template version via
/// the dup file-level write endpoint (`POST /api/templates/{name}/dup/push`).
///
/// `files` is `path -> bytes`; `hash` is the manifest state id (server logs it
/// for verification - the endpoint does NOT enforce fast-forward; each push is a
/// new version snapshot, with client-side debounce handled by the caller).
/// Returns the template name.
pub async fn push_dup_files(
    hub_url: &str,
    api_key: &str,
    name: &str,
    files: &std::collections::BTreeMap<String, Vec<u8>>,
    hash: &str,
    changelog: Option<&str>,
    visibility: Option<&str>,
) -> Result<String> {
    validate_hub_url(hub_url)?;
    use base64::Engine;
    let base = hub_url.trim_end_matches('/');
    let url = format!("{}/api/templates/{}/dup/push", base, name);

    let mut files_b64 = serde_json::Map::new();
    for (path, bytes) in files {
        files_b64.insert(
            path.clone(),
            serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
    }
    let mut payload = serde_json::json!({
        "files": serde_json::Value::Object(files_b64),
        "hash": hash,
    });
    if let Some(c) = changelog {
        payload["changelog"] = serde_json::Value::String(c.to_string());
    }
    if let Some(vis) = visibility {
        payload["visibility"] = serde_json::Value::String(vis.to_string());
    }

    let total_kb = files.values().map(|b| b.len()).sum::<usize>() as f64 / 1024.0;
    tracing::info!(
        "正在推送到 Hub 文件级 ({} files / {:.1} KB)...",
        files.len(),
        total_kb
    );

    let resp = hub_post(&url, api_key)
        .json(&payload)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("文件级推送失败: {} — {}", status, body);
    }

    let body: serde_json::Value = resp.json().await.context("解析 Hub 响应失败")?;
    let resp_name = body["name"].as_str().unwrap_or("unknown");
    let resp_version = body["version"].as_str().unwrap_or("unknown");
    tracing::info!("文件级推送成功: {} v{}", resp_name, resp_version);
    Ok(resp_name.to_string())
}

/// Outcome of `push_workspace_to_hub` - either a new version was pushed or the
/// content was unchanged vs Hub latest (debounced, no-op).
#[derive(Debug)]
pub enum PushOutcome {
    /// A new version was published.
    Pushed,
    /// Local content is identical to Hub latest - nothing to do.
    NoChange,
}

/// Push a running clone's definition layer to Hub as a new version. Closes the
/// loop: runtime evolution (knowledge_add / flow_update / MEMORY) -> Hub.
///
/// Builds the manifest + collects definition files in one walk (manifest hash
/// is computed from the collected bytes, so the two cannot diverge). Debounces
/// against Hub latest manifest by hash (no-op if identical; Hub fetch error is
/// treated as first publish and proceeds). Then pushes via the dup file-level
/// write endpoint.
///
/// `hub_id` is the Hub template key (name or UUID; read from clone_source).
pub async fn push_workspace_to_hub(
    hub_url: &str,
    api_key: &str,
    workspace: &Path,
    hub_id: &str,
) -> Result<PushOutcome> {
    let files = crate::manifest::collect_definition_files(workspace)?;
    let manifest_files: std::collections::BTreeMap<String, String> = files
        .iter()
        .map(|(p, b)| (p.clone(), crate::manifest::sha256_hex(b)))
        .collect();
    let hash = crate::manifest::manifest_hash(&manifest_files);

    // Debounce: compare local manifest hash vs Hub latest. If Hub doesn't have
    // the template yet (404 / Err), treat as first publish and proceed.
    let skip = match fetch_dup_manifest(hub_url, api_key, hub_id, None).await {
        Ok(hub_manifest) => hub_manifest.hash == hash,
        Err(e) => {
            tracing::info!(error = %e, "Hub manifest fetch failed, treating as first push");
            false
        }
    };
    if skip {
        return Ok(PushOutcome::NoChange);
    }

    push_dup_files(hub_url, api_key, hub_id, &files, &hash, None, None).await?;
    Ok(PushOutcome::Pushed)
}

// === Plugins ===

/// Search plugins on Hub. Returns the raw JSON value.
pub async fn search_plugins(
    hub_url: &str,
    api_key: &str,
    query: &str,
) -> Result<serde_json::Value> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let url = if query.is_empty() {
        format!("{}/api/plugins?limit=20", base)
    } else {
        format!(
            "{}/api/plugins?q={}&limit=20",
            base,
            urlencoding::encode(query)
        )
    };

    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        bail!("Hub 返回错误: {}", resp.status());
    }

    resp.json().await.context("解析 Hub 响应失败")
}

/// Detect the current platform string for plugin downloads.
pub fn current_platform() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
}

/// Download and install a pre-compiled plugin from Hub.
pub async fn install_plugin(
    hub_url: &str,
    api_key: &str,
    name: &str,
    version: Option<&str>,
    plugins_dir: &Path,
) -> Result<String> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let platform = current_platform();
    let url = if let Some(v) = version {
        format!(
            "{}/api/plugins/{}/versions/{}?platform={}",
            base,
            urlencoding::encode(name),
            urlencoding::encode(v),
            platform
        )
    } else {
        format!(
            "{}/api/plugins/{}/versions/latest?platform={}",
            base,
            urlencoding::encode(name),
            platform
        )
    };

    tracing::info!("正在从 Hub 下载插件 {} (platform={})...", name, platform);

    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("下载插件失败 {}: {} — {}", name, status, body);
    }

    let bytes = resp.bytes().await.context("读取响应失败")?;
    tracing::info!("已下载插件 {} 字节", bytes.len());

    let plugin_dir = plugins_dir.join(name);
    std::fs::create_dir_all(&plugin_dir)
        .with_context(|| format!("创建插件目录失败: {}", plugin_dir.display()))?;

    let cursor = std::io::Cursor::new(&bytes[..]);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().with_context(|| "读取插件归档条目失败")? {
        let mut entry = entry.with_context(|| "读取归档条目失败")?;
        let path = entry.path().with_context(|| "获取归档条目路径失败")?;
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("插件归档包含不安全路径: {} (不允许使用 ..)", path.display());
        }
        let path_owned = path.to_path_buf();
        entry
            .unpack_in(&plugin_dir)
            .with_context(|| format!("解压归档条目失败: {}", path_owned.display()))?;
    }

    tracing::info!("插件 '{}' 安装完成 → {}", name, plugin_dir.display());
    Ok(name.to_string())
}

/// Check if a plugin is already installed in the plugins directory.
pub fn is_plugin_installed(plugins_dir: &Path, name: &str) -> bool {
    let plugin_dir = plugins_dir.join(name);
    if !plugin_dir.is_dir() {
        return false;
    }
    if !plugin_dir.join("plugin.toml").exists() {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if ["so", "dylib", "dll"].contains(&ext) {
                    return true;
                }
            }
        }
    }
    false
}

// === MCP Servers ===

// === Brain Config ===

/// Fetch brain configuration from Hub.
pub async fn fetch_brain_config(hub_url: &str, api_key: &str) -> Result<serde_json::Value> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let url = format!("{}/api/brain/config", base);

    let resp = hub_get(&url, api_key)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("获取 Brain 配置失败: {} — {}", status, body);
    }

    resp.json().await.context("解析 Brain 配置失败")
}

// === Feedback ===

/// Push feedback to Hub.
pub async fn push_feedback(
    hub_url: &str,
    api_key: &str,
    template_name: &str,
    title: &str,
    content: &str,
    source_template: Option<&str>,
) -> Result<()> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let url = format!("{}/api/feedback", base);

    let mut payload = serde_json::json!({
        "template_name": template_name,
        "title": title,
        "content": content,
    });
    if let Some(src) = source_template {
        payload["source_template"] = serde_json::Value::String(src.to_string());
    }

    let resp = hub_post(&url, api_key)
        .json(&payload)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!("推送反馈失败: {} — {}", status, body);
    }

    Ok(())
}

// === Releases ===

/// Check for updates on Hub. Returns the latest version string if newer than current.
pub async fn check_update(hub_url: &str, current_version: &str) -> Result<Option<String>> {
    validate_hub_url(hub_url)?;
    let base = hub_url.trim_end_matches('/');
    let url = format!("{}/api/releases", base);

    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("无法连接 Hub")?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let data: serde_json::Value = resp.json().await.ok().unwrap_or_default();
    let latest = match data["latest"].as_str() {
        Some(v) => v,
        None => return Ok(None),
    };

    if latest != current_version && !latest.is_empty() {
        Ok(Some(latest.to_string()))
    } else {
        Ok(None)
    }
}

// === Internal helpers ===

fn hub_request(method: reqwest::Method, url: &str, api_key: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .request(method, url)
        .bearer_auth(api_key)
}

fn hub_get(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    hub_request(reqwest::Method::GET, url, api_key)
}

fn hub_post(url: &str, api_key: &str) -> reqwest::RequestBuilder {
    hub_request(reqwest::Method::POST, url, api_key)
}

fn format_stars(avg: f64) -> String {
    let full = (avg / 1.0).round() as i32;
    (0..5)
        .map(|i| if i < full { "★" } else { "☆" })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_template_key_prefers_id() {
        assert_eq!(hub_template_key("ai-writer", Some("uuid-1")), "uuid-1");
        assert_eq!(hub_template_key("ai-writer", Some("  ")), "ai-writer");
        assert_eq!(hub_template_key("ai-writer", None), "ai-writer");
    }
}
