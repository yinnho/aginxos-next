//! UTF-8-safe string utilities.

// Re-export from carrier-types to avoid duplicate implementation.
// See audit R-1: truncate_str (types) and safe_truncate_str (runtime) were identical.
pub use carrier_types::truncate_str as safe_truncate_str;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_within_limit() {
        let s = "hello";
        assert_eq!(safe_truncate_str(s, 10), "hello");
    }

    #[test]
    fn ascii_exact_limit() {
        let s = "hello";
        assert_eq!(safe_truncate_str(s, 5), "hello");
    }

    #[test]
    fn ascii_truncated() {
        let s = "hello world";
        assert_eq!(safe_truncate_str(s, 5), "hello");
    }

    #[test]
    fn multibyte_chinese() {
        // Each Chinese character is 3 bytes in UTF-8
        let s = "\u{4f60}\u{597d}\u{4e16}\u{754c}"; // "hello world" in Chinese, 12 bytes
                                                    // Truncating at 7 bytes should not split the 3rd char (bytes 6..9)
        let t = safe_truncate_str(s, 7);
        assert_eq!(t, "\u{4f60}\u{597d}"); // 6 bytes, 2 chars
        assert!(t.len() <= 7);
    }

    #[test]
    fn multibyte_emoji() {
        let s = "\u{1f600}\u{1f601}\u{1f602}"; // 3 emoji, 4 bytes each = 12 bytes
        let t = safe_truncate_str(s, 5);
        assert_eq!(t, "\u{1f600}"); // 4 bytes, 1 emoji
    }

    #[test]
    fn zero_limit() {
        let s = "hello";
        assert_eq!(safe_truncate_str(s, 0), "");
    }

    #[test]
    fn empty_string() {
        assert_eq!(safe_truncate_str("", 10), "");
    }
}
