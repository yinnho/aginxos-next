//! Stripping `[DELIVER:…]` markers from agent replies.
//!
//! The marker once carried a content key + field overrides that the engine
//! resolved and pushed to a channel. Channel delivery left the engine with the
//! #68 knives, so only the strip remains — a raw marker must not leak into card
//! or webhook text.

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

/// Strip every `[DELIVER:…]` marker from `text`, returning the count stripped
/// and the cleaned text. A marker whose body has no unescaped closing `]` is
/// left verbatim (malformed markers are not silently swallowed).
pub(crate) fn strip_deliver_markers(text: &str) -> (usize, String) {
    let marker = "[DELIVER:";
    let mut count = 0;
    let mut cleaned = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(marker) {
        cleaned.push_str(&rest[..start]);
        let after = &rest[start + marker.len()..];
        match find_unescaped(after, ']') {
            Some(end) => {
                count += 1;
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
    (count, cleaned)
}
