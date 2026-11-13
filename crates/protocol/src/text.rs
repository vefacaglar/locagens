//! Char-boundary-safe string truncation shared by every crate.
//!
//! `String::truncate(n)` and `&s[..n]` panic when `n` falls inside a
//! multibyte UTF-8 sequence (Turkish text, `→`, curly quotes, `U+FFFD` from
//! lossy decoding). Every byte-budget cut goes through these helpers instead.

/// The largest index `<= index` that lies on a char boundary of `text`
/// (stable stand-in for `str::floor_char_boundary`).
pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut cut = index;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

/// The longest prefix of `text` that fits in `max_bytes` without splitting a
/// character.
pub fn prefix_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    &text[..floor_char_boundary(text, max_bytes)]
}

/// The longest suffix of `text` that fits in `max_bytes` without splitting a
/// character.
pub fn suffix_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Truncates `text` in place to at most `max_bytes`, backing off to the
/// previous char boundary. Returns whether anything was cut.
pub fn truncate_at_char_boundary(text: &mut String, max_bytes: usize) -> bool {
    if text.len() <= max_bytes {
        return false;
    }
    let cut = floor_char_boundary(text, max_bytes);
    text.truncate(cut);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_back_to_the_previous_boundary() {
        // `→` is 3 bytes: cutting at 2 must not split it.
        let mut text = "a→b".to_string();
        assert!(truncate_at_char_boundary(&mut text, 2));
        assert_eq!(text, "a");
        let mut text = "ğüşİ".to_string();
        assert!(truncate_at_char_boundary(&mut text, 3));
        assert_eq!(text, "ğ");
        let mut short = "ok".to_string();
        assert!(!truncate_at_char_boundary(&mut short, 10));
        assert_eq!(short, "ok");
    }

    #[test]
    fn prefix_and_suffix_never_split_characters() {
        assert_eq!(prefix_at_char_boundary("é€x", 4), "é");
        assert_eq!(prefix_at_char_boundary("abc", 99), "abc");
        assert_eq!(suffix_at_char_boundary("x€é", 3), "é");
        assert_eq!(suffix_at_char_boundary("abc", 99), "abc");
        assert_eq!(floor_char_boundary("\u{FFFD}\u{FFFD}", 4), 3);
    }
}
