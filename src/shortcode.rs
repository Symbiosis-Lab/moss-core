//! Configurable-delimiter shortcode parser.
//!
//! Extracts shortcode blocks from markdown content using caller-specified
//! delimiters (e.g. `:::` / `:::`). This is a **structural** parser only —
//! it identifies block boundaries and byte offsets but does NOT generate HTML.
//!
//! # Example
//!
//! ```text
//! ::: warning
//! Be careful with this.
//! :::
//! ```
//!
//! With `open_delim = ":::"` and `close_delim = ":::"`, this produces a
//! `ShortcodeSpan` with `name = "warning"`, `content = Some("Be careful with this.\n")`.

use serde::{Deserialize, Serialize};

/// A shortcode block found in content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShortcodeSpan {
    /// The shortcode name (first word after the opening delimiter).
    pub name: String,
    /// Everything after the name on the opening line (arguments/attributes).
    pub args: String,
    /// Inner content between open and close delimiters, if any.
    pub content: Option<String>,
    /// Byte offset of the opening delimiter in the source.
    pub start: usize,
    /// Byte offset just past the closing delimiter (or end of opening line for self-closing).
    pub end: usize,
}

/// Extract shortcode spans from content using the given delimiters.
///
/// Scans line by line. When a line starts with `open_delim` followed by a
/// name, it opens a shortcode block. A subsequent line that is exactly
/// `close_delim` (trimmed) closes it.
///
/// Self-closing shortcodes (opening delimiter with name but no matching
/// close) are NOT emitted — only properly paired blocks are returned.
pub fn extract_shortcodes(
    content: &str,
    open_delim: &str,
    close_delim: &str,
) -> Vec<ShortcodeSpan> {
    // Normalize CRLF → LF so byte-offset arithmetic can assume single-byte newlines.
    let owned;
    let content = if content.contains("\r\n") {
        owned = content.replace("\r\n", "\n");
        owned.as_str()
    } else {
        content
    };

    let mut spans = Vec::new();

    // State for the currently-open shortcode block.
    let mut open_name: Option<String> = None;
    let mut open_args = String::new();
    let mut open_start: usize = 0;
    let mut content_start: usize = 0;

    let mut byte_offset: usize = 0;

    for line in content.split('\n') {
        let trimmed = line.trim();
        let line_byte_len = line.len();
        // +1 for the '\n' that split consumed (except possibly the last line)
        let next_offset = byte_offset + line_byte_len + 1;

        if open_name.is_some() {
            // We're inside an open block — check for closing delimiter.
            if trimmed == close_delim {
                // Close the block.
                let inner = &content[content_start..byte_offset];
                // `end` is just past the closing line (including newline if present)
                let end = if next_offset - 1 <= content.len() {
                    // Check if there was actually a newline after this line
                    std::cmp::min(next_offset, content.len())
                } else {
                    content.len()
                };

                spans.push(ShortcodeSpan {
                    name: open_name.take().unwrap(),
                    args: std::mem::take(&mut open_args),
                    content: Some(inner.to_string()),
                    start: open_start,
                    end,
                });
            }
        } else {
            // Not inside a block — check for opening delimiter.
            if trimmed.starts_with(open_delim) {
                let after_delim = trimmed[open_delim.len()..].trim_start();
                if !after_delim.is_empty() {
                    // There's a name after the delimiter — this opens a block.
                    let mut parts = after_delim.splitn(2, char::is_whitespace);
                    let name = parts.next().unwrap_or("").to_string();
                    let args = parts.next().unwrap_or("").trim().to_string();

                    if !name.is_empty() {
                        open_name = Some(name);
                        open_args = args;
                        open_start = byte_offset;
                        content_start = std::cmp::min(next_offset, content.len());
                    }
                }
            }
        }

        byte_offset = next_offset;
    }

    // If a block was opened but never closed, we do NOT emit it.

    spans
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_block() {
        let input = "::: warning\nBe careful.\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "warning");
        assert_eq!(spans[0].args, "");
        assert_eq!(spans[0].content.as_deref(), Some("Be careful.\n"));
        assert_eq!(spans[0].start, 0);
    }

    #[test]
    fn test_block_with_args() {
        let input = "::: note title=\"Important\"\nContent here.\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "note");
        assert_eq!(spans[0].args, "title=\"Important\"");
        assert_eq!(spans[0].content.as_deref(), Some("Content here.\n"));
    }

    #[test]
    fn test_multiple_blocks() {
        let input = "Intro text.\n\n::: tip\nTip content.\n:::\n\nMiddle.\n\n::: warning\nWarning content.\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "tip");
        assert_eq!(spans[0].content.as_deref(), Some("Tip content.\n"));
        assert_eq!(spans[1].name, "warning");
        assert_eq!(spans[1].content.as_deref(), Some("Warning content.\n"));
    }

    #[test]
    fn test_no_shortcodes() {
        let input = "Just plain markdown.\n\nNo shortcodes here.";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert!(spans.is_empty());
    }

    #[test]
    fn test_unclosed_block_not_emitted() {
        let input = "::: warning\nThis block is never closed.\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert!(spans.is_empty());
    }

    #[test]
    fn test_delimiter_only_line_not_opened() {
        // A line with just ":::" and no name should not open a block.
        let input = ":::\nSome text.\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert!(spans.is_empty());
    }

    #[test]
    fn test_custom_delimiters() {
        let input = "{% highlight ruby %}\nputs 'hello'\n{% end %}\n";
        let spans = extract_shortcodes(input, "{%", "{% end %}");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "highlight");
        assert_eq!(spans[0].args, "ruby %}");
    }

    #[test]
    fn test_multiline_content() {
        let input = "::: details\nLine 1\nLine 2\nLine 3\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].content.as_deref(),
            Some("Line 1\nLine 2\nLine 3\n")
        );
    }

    #[test]
    fn test_empty_content() {
        let input = "::: empty\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "empty");
        assert_eq!(spans[0].content.as_deref(), Some(""));
    }

    #[test]
    fn test_byte_offsets() {
        let prefix = "Hello.\n\n";
        let input = format!("{}::: note\nContent.\n:::\n", prefix);
        let spans = extract_shortcodes(&input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, prefix.len());
        assert_eq!(spans[0].end, input.len());
    }

    #[test]
    fn test_nested_delimiters_not_supported() {
        // Nesting is not supported — the inner closing delimiter
        // closes the outer block.
        let input = "::: outer\n::: inner\nNested.\n:::\nMore outer.\n:::\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        // The first ":::" close matches the outer open.
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "outer");
        // Content includes the inner opening but stops at first close.
        assert!(spans[0].content.as_ref().unwrap().contains("::: inner"));
    }

    #[test]
    fn test_crlf_single_block() {
        let input = "::: warning\r\nBe careful.\r\n:::\r\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "warning");
        assert_eq!(spans[0].args, "");
        assert_eq!(spans[0].content.as_deref(), Some("Be careful.\n"));
        assert_eq!(spans[0].start, 0);
    }

    #[test]
    fn test_crlf_byte_offsets() {
        let prefix = "Hello.\r\n\r\n";
        let input = format!("{}::: note\r\nContent.\r\n:::\r\n", prefix);
        let spans = extract_shortcodes(&input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        // After CRLF normalization, "Hello.\n\n" is 8 bytes.
        assert_eq!(spans[0].start, 8);
        // Normalized: "Hello.\n\n::: note\nContent.\n:::\n" = 30 bytes
        assert_eq!(spans[0].end, 30);
    }

    #[test]
    fn test_crlf_multiple_blocks() {
        let input = "Intro.\r\n\r\n::: tip\r\nTip content.\r\n:::\r\n\r\n::: warning\r\nWarning content.\r\n:::\r\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "tip");
        assert_eq!(spans[0].content.as_deref(), Some("Tip content.\n"));
        assert_eq!(spans[1].name, "warning");
        assert_eq!(spans[1].content.as_deref(), Some("Warning content.\n"));
    }

    #[test]
    fn test_crlf_multiline_content() {
        let input = "::: details\r\nLine 1\r\nLine 2\r\nLine 3\r\n:::\r\n";
        let spans = extract_shortcodes(input, ":::", ":::");

        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].content.as_deref(),
            Some("Line 1\nLine 2\nLine 3\n")
        );
    }
}
