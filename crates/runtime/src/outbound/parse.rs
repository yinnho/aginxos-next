//! Parsing for agent reply markers: NOTIFY, DELIVER.

/// A parsed `[DELIVER:key|field=value|...]` marker. The key resolves a base
/// [`carrier_types::content::ContentDescriptor`] from the agent's `content.toml`;
/// `overrides` let the agent supply dynamic fields (e.g. a per-order
/// miniprogram pagepath) without falling back to channel-specific tools.
#[derive(Debug, Clone)]
pub(crate) struct DeliverMarker {
    pub key: String,
    pub overrides: Vec<(String, String)>,
}

/// Generic `[OPEN:key]content[/CLOSE]` marker parser.
pub(crate) fn parse_markers(
    text: &str,
    open: &str,
    close: &str,
) -> (Vec<(String, String)>, String) {
    let mut out = Vec::new();
    let mut cleaned = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        cleaned.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        // key ends at the first ']'
        match after_open.find(']') {
            Some(type_end) => {
                let key = after_open[..type_end].trim().to_string();
                let after_type = &after_open[type_end + 1..];
                match after_type.find(close) {
                    Some(content_end) => {
                        let content = after_type[..content_end].trim().to_string();
                        if !key.is_empty() {
                            out.push((key, content));
                        }
                        rest = &after_type[content_end + close.len()..];
                    }
                    None => {
                        // No closing tag — emit as-is and stop
                        cleaned.push_str(open);
                        cleaned.push_str(after_open);
                        rest = "";
                    }
                }
            }
            None => {
                cleaned.push_str(open);
                cleaned.push_str(after_open);
                rest = "";
            }
        }
    }
    cleaned.push_str(rest);
    (out, cleaned)
}

/// Parse `[NOTIFY:type]content[/NOTIFY]` markers from agent reply text.
pub(crate) fn parse_notify_markers(text: &str) -> (Vec<(String, String)>, String) {
    parse_markers(text, "[NOTIFY:", "[/NOTIFY]")
}

/// Find the first occurrence of ASCII `c` in `s` not preceded by a `\`.
/// Operates on bytes (safe because `c` is ASCII and never appears inside a
/// UTF-8 multibyte sequence). Used to locate the unescaped `]` that ends a
/// DELIVER marker body.
fn find_unescaped(s: &str, c: char) -> Option<usize> {
    let bytes = s.as_bytes();
    let needle = c as u8;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split `s` on unescaped ASCII `c` into borrowed slices. A `\c` sequence is
/// kept verbatim (the caller resolves the escape on the segment it cares
/// about). Byte-level, safe for ASCII `c`.
fn split_unescaped(s: &str, c: char) -> Vec<&str> {
    let bytes = s.as_bytes();
    let needle = c as u8;
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == needle {
            parts.push(&s[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

/// Resolve escapes in a DELIVER override value: `\|` -> `|`, `\]` -> `]`,
/// `\\` -> `\`. A lone backslash is kept as-is.
fn unescape_deliver(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.clone().next() {
                Some(n) if n == '|' || n == ']' || n == '\\' => {
                    out.push(n);
                    chars.next();
                    continue;
                }
                _ => {}
            }
        }
        out.push(ch);
    }
    out
}

/// Parse `[DELIVER:key]` and `[DELIVER:key|field=value|...]` markers.
///
/// The marker body is split on unescaped `|`: the first part is the content
/// key; each remaining part is `field=value`, where `field` may be a dotted
/// path such as `miniprogram.appid`. Values are taken literally after the first
/// `=`, so query strings (`pagepath=pages/x?token=abc`) are preserved. A value
/// containing `|` or `]` may be escaped as `\|` / `\]` so it does not end the
/// marker or split into another field.
pub(crate) fn parse_deliver_markers(text: &str) -> (Vec<DeliverMarker>, String) {
    let marker = "[DELIVER:";
    let mut markers = Vec::new();
    let mut cleaned = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(marker) {
        cleaned.push_str(&rest[..start]);
        let after = &rest[start + marker.len()..];
        match find_unescaped(after, ']') {
            Some(end) => {
                let body = after[..end].trim();
                let mut parts = split_unescaped(body, '|');
                let key = parts.remove(0);
                let key = key.trim().to_string();
                let mut overrides = Vec::new();
                for part in parts {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    if let Some(eq) = part.find('=') {
                        let field = part[..eq].trim().to_string();
                        let value = unescape_deliver(part[eq + 1..].trim());
                        if !field.is_empty() {
                            overrides.push((field, value));
                        }
                    }
                }
                if !key.is_empty() {
                    markers.push(DeliverMarker { key, overrides });
                }
                rest = &after[end + 1..];
            }
            None => {
                // Malformed (no closing ]), emit as-is and stop.
                cleaned.push_str(marker);
                cleaned.push_str(after);
                rest = "";
            }
        }
    }
    cleaned.push_str(rest);
    (markers, cleaned)
}
