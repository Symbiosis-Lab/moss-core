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
    // Char-aligned: `after_opening = pos + 1` where `pos = content.find('\n')`,
    // and '\n' is a single ASCII byte, so the index lands on a char boundary.
    #[allow(clippy::string_slice)]
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
            // Char-aligned: `after_opening` follows '\n' (ASCII), and
            // `close_line_start = after_opening + offset` where `offset`
            // accumulates `line.len() + 1` per line returned by `lines()`
            // (each line is a complete-char slice and '\n' is one byte).
            #[allow(clippy::string_slice)]
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

            // Char-aligned: `fm_end` is `close_line_end` (= line-aligned via `lines()`
            // + ASCII '---') optionally + 1 for an ASCII '\n'.
            #[allow(clippy::string_slice)]
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
///
/// String values that look like YAML numbers (integers, floats, scientific
/// notation like `753659e7`) are forced to `serde_yaml::Value::String` before
/// serialization so that serde_yaml quotes them. This prevents silent data
/// corruption on the next parse.
pub fn serialize(
    frontmatter: &HashMap<String, serde_yaml::Value>,
    body: &str,
) -> Result<String, String> {
    if frontmatter.is_empty() {
        return Ok(body.to_string());
    }

    // Ensure string values that look numeric are serialized as quoted strings.
    // Also strip stray control characters (defense-in-depth mirror of the
    // frontend `beforeinput` guard, see below) before either transform.
    let safe_fm: HashMap<String, serde_yaml::Value> = frontmatter
        .iter()
        .map(|(k, v)| (k.clone(), ensure_strings_quoted(&strip_control_chars(v))))
        .collect();

    let yaml =
        serde_yaml::to_string(&safe_fm).map_err(|e| format!("YAML serialize error: {}", e))?;

    // serde_yaml adds a trailing newline; no need to add another.
    Ok(format!("---\n{}---\n{}", yaml, body))
}

/// Recursively strip stray C0/C1 control characters from `serde_yaml::Value`
/// strings (write-boundary defense-in-depth).
///
/// ── Why this exists (root cause) ─────────────────────────────────────────
/// On macOS, Tauri v2's multiwebview path (moss enables the `unstable`
/// feature and creates the editor as a child webview via `window.add_child`)
/// hits an unfixed wry bug: arrow keys forward into AppKit's
/// `interpretKeyEvents:` -> `insertText:`, which types the arrow key's
/// legacy control code (Left 0x1C, Right 0x1D, Up 0x1E, Down 0x1F) into a
/// plain `<input>`/`<textarea>` instead of only moving the caret. See
/// `tauri-apps/tauri#10194` (open upstream issue).
///
/// The frontend guards this at the DOM `beforeinput` boundary (see
/// `frontend/app/ui/control-char-guard.ts`), but this Rust strip mirrors it
/// at the write boundary as defense-in-depth — any control char that reaches
/// this point (e.g. a value set before the guard was installed, or via a
/// path that bypasses the DOM entirely) is stripped before it is ever
/// persisted to disk.
///
/// Removes C0 controls (0x00-0x1F) EXCEPT TAB (0x09), LF (0x0A), CR (0x0D);
/// DEL (0x7F); and C1 controls (0x80-0x9F). This numeric-range approach
/// mirrors the frontend guard's `CONTROL_RANGES` table exactly.
fn strip_control_chars(value: &serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::String(s) => serde_yaml::Value::String(strip_control_chars_str(s)),
        serde_yaml::Value::Sequence(seq) => {
            serde_yaml::Value::Sequence(seq.iter().map(strip_control_chars).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut new_map = serde_yaml::Mapping::new();
            for (k, v) in map {
                new_map.insert(k.clone(), strip_control_chars(v));
            }
            serde_yaml::Value::Mapping(new_map)
        }
        // Leave other types as-is
        other => other.clone(),
    }
}

/// True if `c` is a C0/C1 control character that must never survive into
/// saved frontmatter (excludes TAB/LF/CR, which are legitimate whitespace).
fn is_stray_control_char(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x08 | 0x0b..=0x0c | 0x0e..=0x1f | 0x7f..=0x9f
    )
}

/// Remove all stray C0/C1 control characters (excluding TAB/LF/CR) from `s`.
fn strip_control_chars_str(s: &str) -> String {
    s.chars().filter(|c| !is_stray_control_char(*c)).collect()
}

/// Recursively ensure that `serde_yaml::Value::Number` values that were
/// originally strings (e.g., UIDs like "753659e7") remain as strings.
///
/// This is a defensive measure: if a value is already a `String`, leave it.
/// If it's a `Number`, convert it to `String` representation so serde_yaml
/// will quote it. This handles the case where a previous parse already
/// corrupted a hex-like UID into a float.
///
/// For sequences and mappings, recurse.
fn ensure_strings_quoted(value: &serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Sequence(seq) => {
            serde_yaml::Value::Sequence(seq.iter().map(ensure_strings_quoted).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut new_map = serde_yaml::Mapping::new();
            for (k, v) in map {
                new_map.insert(k.clone(), ensure_strings_quoted(v));
            }
            serde_yaml::Value::Mapping(new_map)
        }
        // Leave other types as-is
        other => other.clone(),
    }
}

/// Extract a frontmatter value as a string, handling the case where YAML
/// parsed a hex-like string (e.g., `753659e7`) as a number.
///
/// Returns `Some(string)` if the value is a String or a Number that can be
/// converted to string. Returns `None` for other types.
pub fn value_as_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(format!("{}", n)),
        serde_yaml::Value::Bool(b) => Some(format!("{}", b)),
        _ => None,
    }
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
        let input = "---\ndraft: true\n---\nContent.";
        let doc = parse(input);

        assert_eq!(
            doc.frontmatter.get("draft").and_then(|v| v.as_bool()),
            Some(true)
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
        // "---\ntitle: Hi\n---\n" = 18 bytes. The slices below assert the
        // byte-offset contract of `frontmatter_range`: each offset lands on a
        // line boundary (after `\n`), which is ASCII and therefore char-aligned.
        #[allow(clippy::string_slice)] // char-aligned: range returns line-boundary byte offsets
        {
            assert_eq!(&input[start..end], "---\ntitle: Hi\n---\n");
            assert_eq!(&input[end..], "Body.");
        }
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

    #[test]
    fn test_uid_scientific_notation_roundtrip() {
        // Regression test: UIDs like "753659e7" look like YAML scientific
        // notation and get parsed as floats. The serialize path must quote them.
        let input = "---\ntitle: Test\nuid: \"753659e7\"\n---\nBody.";
        let doc = parse(input);

        // When properly quoted, uid is parsed as a string
        let uid_val = doc.frontmatter.get("uid").expect("uid field");
        assert_eq!(uid_val.as_str(), Some("753659e7"));

        // Round-trip: serialize and re-parse
        let output = serialize(&doc.frontmatter, &doc.body).expect("serialize");
        let doc2 = parse(&output);
        let uid2 = doc2.frontmatter.get("uid").expect("uid field after roundtrip");
        assert_eq!(uid2.as_str(), Some("753659e7"));
    }

    #[test]
    fn test_value_as_string_handles_numbers() {
        // If a uid was already corrupted to a number by YAML parsing,
        // value_as_string should still extract a usable string.
        let num_val = serde_yaml::Value::Number(serde_yaml::Number::from(75365900));
        assert!(value_as_string(&num_val).is_some());

        let str_val = serde_yaml::Value::String("753659e7".to_string());
        assert_eq!(value_as_string(&str_val), Some("753659e7".to_string()));
    }

    #[test]
    fn test_unquoted_uid_parsed_as_number() {
        // Demonstrates the bug: unquoted hex-like UIDs are parsed as numbers
        let input = "---\ntitle: Test\nuid: 753659e7\n---\nBody.";
        let doc = parse(input);

        let uid_val = doc.frontmatter.get("uid").expect("uid field");
        // serde_yaml parses this as a number, not a string
        assert!(
            uid_val.as_str().is_none(),
            "Unquoted 753659e7 should NOT parse as string (it's a YAML number)"
        );

        // But value_as_string can still extract it
        assert!(value_as_string(uid_val).is_some());
    }

    #[test]
    fn test_serialize_strips_stray_control_chars() {
        // Regression test for the macOS Tauri multiwebview arrow-key bug
        // (tauri-apps/tauri#10194): a child webview's beforeinput/keyDown
        // path can insert the arrow key's legacy control code (Right =
        // U+001D GROUP SEPARATOR) into a plain input instead of just moving
        // the caret. The frontend guards this at `beforeinput`
        // (frontend/app/ui/control-char-guard.ts); this write-boundary strip
        // is the defense-in-depth backstop so a corrupted value can never
        // reach disk even if it slips past the DOM guard.
        let corrupted = format!("websites.{}", "\u{1D}".repeat(8));

        let mut fm = HashMap::new();
        fm.insert(
            "description".to_string(),
            serde_yaml::Value::String(corrupted),
        );

        let output = serialize(&fm, "Body.").expect("serialize");
        let doc = parse(&output);

        assert_eq!(
            doc.frontmatter.get("description").and_then(|v| v.as_str()),
            Some("websites."),
            "control chars must be stripped from the written value"
        );
    }

    #[test]
    fn test_strip_control_chars_str_keeps_tab_lf_cr() {
        // TAB/LF/CR are legitimate whitespace and must survive the strip —
        // mirrors the frontend guard's CONTROL_RANGES exclusions.
        let input = "a\tb\nc\rd\u{00}\u{7f}\u{85}e";
        assert_eq!(strip_control_chars_str(input), "a\tb\nc\rde");
    }
}
