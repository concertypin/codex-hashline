//! Core hashline utility: content-anchored line addressing.
//!
//! Each line is tagged with a 2-hex content hash so edits stay stable
//! across line insertions/deletions.  Format: `LINE:HEX|content`.

use std::path::Path;

/// Compute a 2-hex content hash for a line.
///
/// Whitespace is stripped before hashing (indentation-insensitive).
pub fn line_anchor(line: &str) -> String {
    let stripped: String = line
        .trim_end_matches('\r')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let hash = fxhash32(&stripped);
    format!("{:02x}", hash)
}

/// Format a single line with its hashline anchor.
pub fn format_line(num: usize, content: &str) -> String {
    format!("{}:{}|{}", num, line_anchor(content), content)
}

/// Format a file-level snapshot header.
pub fn file_header(path: &Path) -> String {
    format!("[{}]", path.display())
}

/// Simple 32-bit hash (FNV-1a variant) for hashline anchors.
/// Deterministic, platform-independent, no external deps.
fn fxhash32(data: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for byte in data.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash & 0xff // keep low byte → 2 hex chars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_anchor_stable() {
        let a = line_anchor("  fn hello() {");
        let b = line_anchor("fn hello() {");
        assert_eq!(a, b, "whitespace differences should produce same hash");
    }

    #[test]
    fn test_line_anchor_different() {
        let a = line_anchor("return 1;");
        let b = line_anchor("return 2;");
        assert_ne!(a, b, "different content should produce different hash");
    }

    #[test]
    fn test_format_line() {
        let line = format_line(1, "hello");
        assert!(line.starts_with("1:"));
        assert!(line.ends_with("|hello"));
        assert_eq!(line.len(), "1:xx|hello".len());
    }

    #[test]
    fn test_carriage_return_stripped() {
        let a = line_anchor("hello\r");
        let b = line_anchor("hello");
        assert_eq!(a, b);
    }
}
