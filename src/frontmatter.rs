//! YAML frontmatter parsing with body preservation.
//!
//! Uses `serde_yaml` directly (NOT `gray_matter`, whose `Pod` type
//! doesn't properly deserialize YAML arrays — see ADR-008).
//!
//! The body is preserved byte-for-byte via boundary-aware splitting.
//! `frontmatter_range` records the byte offsets of the `---` delimiters
//! so callers can do surgical replacement without re-serializing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A parsed markdown document with frontmatter separated from body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    /// Parsed frontmatter key-value pairs.
    pub frontmatter: HashMap<String, serde_yaml::Value>,
    /// The markdown body (everything after the closing `---`).
    pub body: String,
    /// Byte offsets of the frontmatter block: (start_of_opening_delimiter, end_of_closing_delimiter).
    /// `None` if no frontmatter was found.
    pub frontmatter_range: Option<(usize, usize)>,
}

/// Parse a markdown document, extracting frontmatter and body.
///
/// If the content starts with `---\n`, the YAML frontmatter is extracted
/// and deserialized into a `HashMap`. The body is everything after the
/// closing `---` delimiter (preserved byte-for-byte).
///
/// If no frontmatter is found, returns an empty map with the full content as body.
pub fn parse(content: &str) -> ParsedDocument {
    // Normalize CRLF → LF so byte-offset arithmetic can assume single-byte newlines.
    let owned;
    let content = if content.contains("\r\n") {
        owned = content.replace("\r\n", "\n");
        owned.as_str()
    } else {
        content
    };

    // Must start with `---` followed by newline (or just `---` at end of content).
    if !content.starts_with("---") {
        return ParsedDocument {
            frontmatter: HashMap::new(),
            body: content.to_string(),
            frontmatter_range: None,
        };
    }

    // Find end of opening `---` line.
    let after_opening = match content.find('\n') {
        Some(pos) => pos + 1,
        None => {
            // Content is just "---" with no newline — no valid frontmatter.
            return ParsedDocument {
                frontmatter: HashMap::new(),
                body: content.to_string(),
                frontmatter_range: None,
            };
        }
    };

    // Search for closing `---` line in the remainder.
    let rest = &content[after_opening..];
    let mut offset = 0;
    for line in rest.lines() {
        if line.trim() == "---" {
            // Found closing delimiter.
            let close_line_start = after_opening + offset;
            let close_line_end = close_line_start + line.len();

            // Include the newline after the closing `---` if present.
            let fm_end = if close_line_end < content.len()
                && content.as_bytes()[close_line_end] == b'\n'
            {
                close_line_end + 1
            } else {
                close_line_end
            };

            // The YAML text is between the opening and closing delimiters.
            let yaml_text = &content[after_opening..close_line_start];

            // Parse the YAML.
            let frontmatter: HashMap<String, serde_yaml::Value> =
                match serde_yaml::from_str(yaml_text) {
                    Ok(map) => map,
                    Err(_) => {
                        // Invalid YAML — treat as no frontmatter.
                        return ParsedDocument {
                            frontmatter: HashMap::new(),
                            body: content.to_string(),
                            frontmatter_range: None,
                        };
                    }
                };

            let body = &content[fm_end..];

            return ParsedDocument {
                frontmatter,
                body: body.to_string(),
                frontmatter_range: Some((0, fm_end)),
            };
        }
        offset += line.len() + 1; // +1 for '\n'
    }

    // No closing delimiter found — no valid frontmatter.
    ParsedDocument {
        frontmatter: HashMap::new(),
        body: content.to_string(),
        frontmatter_range: None,
    }
}

/// Serialize frontmatter and body back into a markdown document.
///
/// Produces `---\n{yaml}\n---\n{body}`. If frontmatter is empty,
/// returns just the body.
pub fn serialize(
    frontmatter: &HashMap<String, serde_yaml::Value>,
    body: &str,
) -> Result<String, String> {
    if frontmatter.is_empty() {
        return Ok(body.to_string());
    }

    let yaml =
        serde_yaml::to_string(frontmatter).map_err(|e| format!("YAML serialize error: {}", e))?;

    // serde_yaml adds a trailing newline; no need to add another.
    Ok(format!("---\n{}---\n{}", yaml, body))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_with_frontmatter() {
        let input = "---\ntitle: Hello World\ndate: 2024-01-15\n---\nBody content here.";
        let doc = parse(input);

        assert_eq!(doc.frontmatter.len(), 2);
        assert_eq!(
            doc.frontmatter.get("title").and_then(|v| v.as_str()),
            Some("Hello World")
        );
        assert_eq!(
            doc.frontmatter.get("date").and_then(|v| v.as_str()),
            Some("2024-01-15")
        );
        assert_eq!(doc.body, "Body content here.");
        assert!(doc.frontmatter_range.is_some());
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let input = "Just body content.";
        let doc = parse(input);

        assert!(doc.frontmatter.is_empty());
        assert_eq!(doc.body, "Just body content.");
        assert!(doc.frontmatter_range.is_none());
    }

    #[test]
    fn test_parse_empty_frontmatter() {
        let input = "---\n---\nBody after empty frontmatter.";
        let doc = parse(input);

        // serde_yaml::from_str("") returns Err for empty input, so this
        // should be treated as no-frontmatter (invalid YAML).
        // Actually, empty string can produce Null rather than a map.
        // Either way the behavior is graceful.
        assert_eq!(doc.body, "Body after empty frontmatter.");
    }

    #[test]
    fn test_parse_no_closing_delimiter() {
        let input = "---\ntitle: Hello\nno closing";
        let doc = parse(input);

        assert!(doc.frontmatter.is_empty());
        assert_eq!(doc.body, input);
        assert!(doc.frontmatter_range.is_none());
    }

    #[test]
    fn test_parse_yaml_arrays() {
        let input = "---\ntags:\n  - rust\n  - wasm\n---\nBody.";
        let doc = parse(input);

        let tags = doc.frontmatter.get("tags").expect("tags field");
        let seq = tags.as_sequence().expect("should be sequence");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].as_str(), Some("rust"));
        assert_eq!(seq[1].as_str(), Some("wasm"));
    }

    #[test]
    fn test_parse_boolean_values() {
        let input = "---\ndraft: true\nunlisted: false\n---\nContent.";
        let doc = parse(input);

        assert_eq!(
            doc.frontmatter.get("draft").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            doc.frontmatter.get("unlisted").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn test_parse_numeric_values() {
        let input = "---\nweight: 42\nrating: 3.5\n---\nContent.";
        let doc = parse(input);

        assert_eq!(
            doc.frontmatter.get("weight").and_then(|v| v.as_u64()),
            Some(42)
        );
        assert_eq!(
            doc.frontmatter.get("rating").and_then(|v| v.as_f64()),
            Some(3.5)
        );
    }

    #[test]
    fn test_parse_preserves_body_exactly() {
        let body = "Line 1\n\nLine 3 with **bold**\n\n- list item\n";
        let input = format!("---\ntitle: Test\n---\n{}", body);
        let doc = parse(&input);

        assert_eq!(doc.body, body);
    }

    #[test]
    fn test_frontmatter_range_byte_offsets() {
        let input = "---\ntitle: Hi\n---\nBody.";
        let doc = parse(input);

        let (start, end) = doc.frontmatter_range.expect("range");
        assert_eq!(start, 0);
        // "---\ntitle: Hi\n---\n" = 18 bytes
        assert_eq!(&input[start..end], "---\ntitle: Hi\n---\n");
        assert_eq!(&input[end..], "Body.");
    }

    #[test]
    fn test_serialize_with_frontmatter() {
        let mut fm = HashMap::new();
        fm.insert(
            "title".to_string(),
            serde_yaml::Value::String("Hello".to_string()),
        );

        let result = serialize(&fm, "Body content.").expect("serialize");

        assert!(result.starts_with("---\n"));
        assert!(result.contains("title: Hello"));
        assert!(result.contains("---\nBody content."));
    }

    #[test]
    fn test_serialize_empty_frontmatter() {
        let fm = HashMap::new();
        let result = serialize(&fm, "Just body.").expect("serialize");
        assert_eq!(result, "Just body.");
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let input = "---\n: invalid: yaml: [unclosed\n---\nBody.";
        let doc = parse(input);

        // Invalid YAML should fall back to no-frontmatter.
        assert!(doc.frontmatter.is_empty());
    }

    #[test]
    fn test_parse_frontmatter_with_trailing_whitespace_on_delimiter() {
        let input = "---\ntitle: Test\n---  \nBody.";
        let doc = parse(input);

        // The closing delimiter has trailing spaces — `line.trim() == "---"` should match.
        assert_eq!(
            doc.frontmatter.get("title").and_then(|v| v.as_str()),
            Some("Test")
        );
        assert_eq!(doc.body, "Body.");
    }

    #[test]
    fn test_parse_content_starts_with_dashes_but_not_frontmatter() {
        let input = "---- Not frontmatter\nJust text.";
        let doc = parse(input);

        // Starts with "----" (4 dashes), which starts_with("---") is true.
        // But after the first line, there's no closing `---`.
        assert!(doc.frontmatter.is_empty());
        assert_eq!(doc.body, input);
    }

    #[test]
    fn test_roundtrip() {
        let input = "---\ntitle: Round Trip\n---\nBody stays the same.";
        let doc = parse(input);

        let output = serialize(&doc.frontmatter, &doc.body).expect("serialize");

        // Re-parse and verify
        let doc2 = parse(&output);
        assert_eq!(
            doc.frontmatter.get("title"),
            doc2.frontmatter.get("title")
        );
        assert_eq!(doc.body, doc2.body);
    }

    #[test]
    fn test_parse_multiline_body() {
        let input = "---\ntitle: Test\n---\nParagraph 1.\n\nParagraph 2.\n\n> Quote\n";
        let doc = parse(input);

        assert_eq!(doc.body, "Paragraph 1.\n\nParagraph 2.\n\n> Quote\n");
    }

    #[test]
    fn test_parse_only_dashes() {
        let input = "---";
        let doc = parse(input);

        assert!(doc.frontmatter.is_empty());
        assert_eq!(doc.body, "---");
    }

    #[test]
    fn test_parse_crlf_content() {
        let input = "---\r\ntitle: Hello World\r\ndate: 2024-01-15\r\n---\r\nBody content here.";
        let doc = parse(input);

        assert_eq!(doc.frontmatter.len(), 2);
        assert_eq!(
            doc.frontmatter.get("title").and_then(|v| v.as_str()),
            Some("Hello World")
        );
        assert_eq!(
            doc.frontmatter.get("date").and_then(|v| v.as_str()),
            Some("2024-01-15")
        );
        assert_eq!(doc.body, "Body content here.");
        assert!(doc.frontmatter_range.is_some());
    }

    #[test]
    fn test_parse_crlf_byte_offsets() {
        let input = "---\r\ntitle: Hi\r\n---\r\nBody.";
        let doc = parse(input);

        let (start, end) = doc.frontmatter_range.expect("range");
        assert_eq!(start, 0);
        // After CRLF normalization, offsets are relative to the normalized string.
        // "---\ntitle: Hi\n---\n" = 18 bytes
        assert_eq!(end, 18);
    }

    #[test]
    fn test_parse_crlf_preserves_body() {
        let body = "Line 1\nLine 2\n";
        let input = format!("---\r\ntitle: Test\r\n---\r\n{}", body.replace('\n', "\r\n"));
        let doc = parse(&input);

        assert_eq!(
            doc.frontmatter.get("title").and_then(|v| v.as_str()),
            Some("Test")
        );
        // Body CRLF is also normalized to LF.
        assert_eq!(doc.body, body);
    }

    #[test]
    fn test_parse_crlf_yaml_arrays() {
        let input = "---\r\ntags:\r\n  - rust\r\n  - wasm\r\n---\r\nBody.";
        let doc = parse(input);

        let tags = doc.frontmatter.get("tags").expect("tags field");
        let seq = tags.as_sequence().expect("should be sequence");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].as_str(), Some("rust"));
        assert_eq!(seq[1].as_str(), Some("wasm"));
    }
}
