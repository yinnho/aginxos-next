//! Public file view URLs for agent outputs.
//!
//! When a tool writes a file under a sender's data dir (typically `output/…`),
//! callers should attach a `view_url` so the model can paste a clickable link
//! without per-clone prompt engineering.
//!
//! URL shape (matches `GET /api/files/view/{agent}/{*path}?sender_id=…`):
//! ```text
//! {external_url}/api/files/view/{agent_name}/output/foo.md?sender_id={sid}&render=markdown
//! ```

/// Build a browser-viewable URL for a file under the sender data directory.
///
/// `rel_under_sender` is relative to `sender_data_dir` (e.g. `output/image.png`
/// or `output/剧本.md`). Returns `None` if `external_url` is missing/empty or
/// required identity fields are empty.
pub fn build_file_view_url(
    external_url: Option<&str>,
    agent_name: &str,
    rel_under_sender: &str,
    sender_id: &str,
) -> Option<String> {
    let base = external_url?.trim().trim_end_matches('/');
    if base.is_empty() || agent_name.is_empty() || sender_id.is_empty() {
        return None;
    }
    let rel = normalize_rel_path(rel_under_sender)?;
    let path_enc = encode_path_segments(&rel);
    let mut url = format!(
        "{base}/api/files/view/{agent}/{path}?sender_id={sid}",
        agent = urlencoding::encode(agent_name),
        path = path_enc,
        sid = urlencoding::encode(sender_id),
    );
    if rel.ends_with(".md") {
        url.push_str("&render=markdown");
    }
    Some(url)
}

/// Build view URLs for each relative path; skips entries that cannot be encoded.
pub fn build_file_view_urls(
    external_url: Option<&str>,
    agent_name: &str,
    rel_paths: &[String],
    sender_id: &str,
) -> Vec<String> {
    rel_paths
        .iter()
        .filter_map(|p| build_file_view_url(external_url, agent_name, p, sender_id))
        .collect()
}

/// Relative path under sender dir for a file written via user path like `output/x.md`.
/// Non-output catch-all paths are treated as under `output/`.
pub fn rel_path_for_user_write(raw_path: &str) -> Option<String> {
    let normalized = raw_path.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let rel = normalized.trim_start_matches('/');
    if rel.is_empty() || rel.contains("..") {
        return None;
    }
    if rel.starts_with("output/") || rel == "output" {
        return Some(rel.to_string());
    }
    if rel.starts_with("memory/") || rel == "memory" {
        return Some(rel.to_string());
    }
    // Catch-all user paths land in output/
    Some(format!("output/{rel}"))
}

fn normalize_rel_path(rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim().trim_start_matches('/');
    if rel.is_empty() || rel.contains("..") {
        return None;
    }
    Some(rel.to_string())
}

fn encode_path_segments(rel: &str) -> String {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| urlencoding::encode(s).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_markdown_view_url() {
        let u = build_file_view_url(
            Some("https://file.yinnho.cn"),
            "short-drama-writer",
            "output/剧本-第1集.md",
            "user123",
        )
        .unwrap();
        assert!(u.starts_with("https://file.yinnho.cn/api/files/view/short-drama-writer/"));
        assert!(u.contains("sender_id=user123"));
        assert!(u.contains("render=markdown"));
        assert!(u.contains("%E5%89%A7%E6%9C%AC") || u.contains("output/"));
    }

    #[test]
    fn builds_image_view_url_no_markdown() {
        let u = build_file_view_url(
            Some("https://carrier.yinnho.cn/"),
            "ai-writer",
            "output/image_20260101.png",
            "sid",
        )
        .unwrap();
        assert_eq!(
            u,
            "https://carrier.yinnho.cn/api/files/view/ai-writer/output/image_20260101.png?sender_id=sid"
        );
    }

    #[test]
    fn none_without_external_url() {
        assert!(build_file_view_url(None, "a", "output/x.md", "s").is_none());
    }

    #[test]
    fn user_write_rel_paths() {
        assert_eq!(
            rel_path_for_user_write("output/foo.md").as_deref(),
            Some("output/foo.md")
        );
        assert_eq!(
            rel_path_for_user_write("bar.md").as_deref(),
            Some("output/bar.md")
        );
        assert_eq!(
            rel_path_for_user_write("memory/note.md").as_deref(),
            Some("memory/note.md")
        );
    }
}
