//! UTF-8 boundary helpers compatible with the workspace's MSRV (1.74).
//!
//! `str::floor_char_boundary` would be the obvious tool, but it stabilized in
//! Rust 1.83. Until the workspace MSRV catches up, every truncation site
//! routes through [`floor_char_boundary`] below.

/// Return the largest byte index `<= max` that is a valid UTF-8 char boundary
/// in `s`. Mirrors `str::floor_char_boundary` exactly (clamps `max` to
/// `s.len()`, walks backwards from there).
///
/// This is used wherever output needs to be truncated to a byte budget without
/// splitting a multi-byte char. Since UTF-8 lead bytes are at most 4 bytes
/// apart, the loop runs at most 4 iterations.
#[inline]
pub fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        // Walking down from a valid byte index < len cannot underflow:
        // index 0 is always a boundary, so the loop terminates first.
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_truncation() {
        assert_eq!(floor_char_boundary("hello", 3), 3);
        assert_eq!(floor_char_boundary("hello", 0), 0);
        assert_eq!(floor_char_boundary("hello", 5), 5);
    }

    #[test]
    fn over_len_clamps() {
        assert_eq!(floor_char_boundary("hi", 99), 2);
    }

    #[test]
    fn walks_back_to_boundary_in_multibyte() {
        // "é" is two bytes (0xC3 0xA9). max=1 lands inside it; floor returns 0.
        let s = "é";
        assert_eq!(s.len(), 2);
        assert_eq!(floor_char_boundary(s, 1), 0);
        assert_eq!(floor_char_boundary(s, 2), 2);
    }

    #[test]
    fn four_byte_char() {
        // 🎉 is 4 bytes. From any inner index we should land at the start.
        let s = "a🎉b";
        // indexes: a=0, 🎉=1..5, b=5
        assert_eq!(floor_char_boundary(s, 4), 1);
        assert_eq!(floor_char_boundary(s, 3), 1);
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(floor_char_boundary(s, 1), 1);
        assert_eq!(floor_char_boundary(s, 0), 0);
    }

    #[test]
    fn matches_std_floor_char_boundary_on_known_cases() {
        // Boundaries in this string live at 0, 3, 6, 10, 14, 18, 22, 26.
        // ("❤" = 3 bytes, U+FE0F variation selector = 3 bytes, then 4-byte emoji.)
        let s = "❤️🧡💛💚💙💜";
        assert_eq!(s.len(), 26);
        assert_eq!(floor_char_boundary(s, 13), 10);
        assert_eq!(floor_char_boundary(s, 14), 14);
        assert_eq!(floor_char_boundary(s, 15), 14);
        assert_eq!(floor_char_boundary(s, 26), 26);
    }
}
