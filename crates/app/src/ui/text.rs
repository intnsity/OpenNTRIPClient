//! Small text utilities shared across UI windows. One implementation, one
//! test suite: the filter boxes in different windows must never drift apart
//! in matching semantics.

use std::time::Duration;

/// ASCII-case-insensitive substring test, allocation-free. Bytewise windows
/// are exact for any UTF-8 needle; only ASCII letters fold case, which is
/// the right trade for protocol text (headers, mountpoints, sourcetable
/// fields, status lines). The empty needle matches everything, so a cleared
/// filter box shows the full list.
pub fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// "just now", "5 min ago", "3 h ago", "2 days ago": a file's age needs no
/// timezone, and recency is what picks the right file from a list.
pub fn age_label(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..60 => "just now".to_string(),
        60..3_600 => format!("{} min ago", s / 60),
        3_600..86_400 => format!("{} h ago", s / 3_600),
        86_400..172_800 => "1 day ago".to_string(),
        _ => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_like_a_person_would_say_them() {
        assert_eq!(age_label(Duration::from_secs(5)), "just now");
        assert_eq!(age_label(Duration::from_secs(300)), "5 min ago");
        assert_eq!(age_label(Duration::from_secs(7_200)), "2 h ago");
        assert_eq!(age_label(Duration::from_secs(90_000)), "1 day ago");
        assert_eq!(age_label(Duration::from_secs(3 * 86_400)), "3 days ago");
    }

    #[test]
    fn is_ascii_case_insensitive() {
        assert!(contains_ignore_ascii_case("ICY 200 OK", "icy"));
        assert!(contains_ignore_ascii_case("GET /RTCM32 HTTP/1.1", "rtcm32"));
        assert!(contains_ignore_ascii_case("RTCM 3.2", "rtcm"));
        assert!(contains_ignore_ascii_case("ogden", "OGD"));
        assert!(!contains_ignore_ascii_case("ICY 200 OK", "401"));
        assert!(!contains_ignore_ascii_case("RTCM", "rtcm 3"));
    }

    #[test]
    fn empty_needle_matches_everything() {
        assert!(contains_ignore_ascii_case("anything", ""));
        assert!(contains_ignore_ascii_case("", ""));
    }

    #[test]
    fn needle_longer_than_haystack_never_matches() {
        assert!(!contains_ignore_ascii_case("", "x"));
        assert!(!contains_ignore_ascii_case("short", "much longer needle"));
    }

    #[test]
    fn multibyte_utf8_haystack_keeps_byte_windows_sound() {
        assert!(contains_ignore_ascii_case("Grussformel \u{00e4}", "FORMEL"));
        // Exact match on a non-ASCII needle still works (no case folding).
        assert!(contains_ignore_ascii_case(
            "Grussformel \u{00e4}",
            "\u{00e4}"
        ));
    }
}
