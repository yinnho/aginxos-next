//! HTTP client for the file-level dup sync remote.
//!
//! The remote is stateless and git-style: the client fetches the current
//! manifest + individual files (pull) and posts fast-forward changes
//! (push). Bearer-auth via the remote API key. Endpoint shape follows
//! `remote.api`: "templates" = duphub `/api/templates/{name}/dup/*`
//! (default), "clones" = runtime `/api/clones/{name}/dup/*` (upstream
//! dup's shape against a carrier runtime).

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use base64::Engine;

use crate::manifest::Manifest;

/// Build a dup endpoint for the configured api shape.
fn endpoint(url: &str, api: &str, name: &str, tail: &str) -> String {
    let scope = if api == "clones" { "clones" } else { "templates" };
    format!("{url}/api/{scope}/{name}/dup/{tail}")
}

/// Fetch the remote's current definition-layer manifest.
pub async fn get_manifest(url: &str, api_key: &str, api: &str, name: &str) -> Result<Manifest> {
    let endpoint = endpoint(url, api, name, "manifest");
    let resp = reqwest::Client::new()
        .get(&endpoint)
        .bearer_auth(api_key)
        .send()
        .await
        .context("无法连接 remote")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("获取 manifest 失败 ({status}): {body}");
    }
    resp.json::<Manifest>().await.context("解析 manifest 失败")
}

/// Fetch a single file's raw bytes from the remote.
pub async fn get_file(url: &str, api_key: &str, api: &str, name: &str, path: &str) -> Result<Vec<u8>> {
    let endpoint = endpoint(url, api, name, &format!("file/{path}"));
    let resp = reqwest::Client::new()
        .get(&endpoint)
        .bearer_auth(api_key)
        .send()
        .await
        .context("无法连接 remote")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("获取文件 {path} 失败 ({status}): {body}");
    }
    let bytes = resp.bytes().await.context("读取文件内容失败")?;
    Ok(bytes.to_vec())
}

/// Post a fast-forward push: base_hash + changed files + deletes.
///
/// Returns the remote's new manifest after apply. Errors (e.g. 409 remote
/// evolved) bubble up as a bail with the server message.
pub async fn post_push(
    url: &str,
    api_key: &str,
    api: &str,
    name: &str,
    base_hash: &str,
    files: &BTreeMap<String, Vec<u8>>,
    deletes: &[String],
) -> Result<Manifest> {
    let endpoint = endpoint(url, api, name, "push");
    // base64-encode file contents (handles binary like data.db).
    let files_b64: BTreeMap<String, String> = files
        .iter()
        .map(|(p, b)| {
            (
                p.clone(),
                base64::engine::general_purpose::STANDARD.encode(b),
            )
        })
        .collect();
    let payload = serde_json::json!({
        "base_hash": base_hash,
        "files": files_b64,
        "deletes": deletes,
    });

    let resp = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await
        .context("无法连接 remote")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("推送失败 ({status}): {body}");
    }
    let v: serde_json::Value = serde_json::from_str(&body).context("解析推送响应失败")?;
    let manifest = v.get("manifest").context("推送响应缺 manifest")?;
    serde_json::from_value(manifest.clone()).context("解析 manifest 失败")
}
