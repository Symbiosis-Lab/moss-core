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
    /// serde_yaml error message when a delimited `---...---` block failed to
    /// parse as YAML. `None` when the block parsed cleanly or there was no
    /// delimited block. When `Some`, `frontmatter` is empty and `body` still
    /// holds the WHOLE document (so the editor can show/repair the bad block);
    /// use `render_body()` for the HTML-render view that excludes it.
    ///
    /// `#[serde(default)]` is defensive: the type derives Deserialize but is
    /// transient (no code deserializes INTO it).
    #[serde(default)]
    pub frontmatter_error: Option<String>,
}

impl ParsedDocument {
    /// Body suitable for RENDERING to HTML (build pipeline): excludes a
    /// delimited frontmatter block that FAILED to parse, so malformed YAML never
    /// leaks verbatim into output. Equal to `body` when the frontmatter parsed
    /// cleanly or there was no block.
    ///
    /// The EDITOR must NOT use this — it needs the raw `body` so the author can
    /// see/repair the bad block and a full-reserialize save preserves it.
    #[allow(clippy::string_slice)]
    // `fm_end` is a line-boundary offset (the `frontmatter_range` contract); on
    // the error path `body` == the CRLF-normalized content that `fm_end` indexes,
    // so the slice is char-aligned and CRLF-safe.
    pub fn render_body(&self) -> &str {
        match (self.frontmatter_error.as_ref(), self.frontmatter_range) {
            (Some(_), Some((_, fm_end))) => &self.body[fm_end..],
            _ => &self.body,
        }
    }
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
            frontmatter_error: None,
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
                frontmatter_error: None,
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
                    Err(e) => {
                        // Invalid YAML. Record the block range + surface the
                        // error instead of silently swallowing it (which used to
                        // dump the raw `---...---` block into `body`, leaking it
                        // verbatim into rendered HTML with no warning — the
                        // "Europe - A Prophecy.md" bug). `body` stays the WHOLE
                        // document so the editor can still show/repair the block
                        // and a re-serialize save preserves the file; the build
                        // renders `render_body()` (block-excluded) so nothing
                        // leaks. See ADR-020.
                        return ParsedDocument {
                            frontmatter: HashMap::new(),
                            body: content.to_string(),
                            frontmatter_range: Some((0, fm_end)),
                            frontmatter_error: Some(e.to_string()),
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
                frontmatter_error: None,
            };
        }
        offset += line.len() + 1; // +1 for '\n'
    }

    // No closing delimiter found — no valid frontmatter.
    ParsedDocument {
        frontmatter: HashMap::new(),
        body: content.to_string(),
        frontmatter_range: None,
        frontmatter_error: None,
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
///
/// The shared control-char stripper: this is the same string-level primitive
/// `strip_control_chars` (above) applies recursively to `serde_yaml::Value`
/// trees. It is `pub` so other crates (e.g. `src-tauri`'s scrape/email write
/// paths) can apply the identical defense-in-depth strip at their own
/// hand-rolled or `serde_yaml`-based frontmatter funnels — see
/// `tauri-apps/tauri#10194`.
pub fn strip_control_chars_str(s: &str) -> String {
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

// ── Structural asset paths in frontmatter ────────────────────────────────

/// Byte spans of frontmatter values that name a project file.
///
/// The field set is derived from the schema
/// ([`crate::schema_fields::asset_field_names`]), never listed here.
///
/// # Why this does not use `parse`
///
/// [`parse`] CRLF-normalizes before computing `frontmatter_range`, so those
/// offsets index a COPY and are unsafe for rewriting a CRLF source. This
/// locates the block on its own raw line table instead. It agrees with
/// `parse` on what a block is (any content starting with `---`, ending at
/// the next line that trims to `---`), so it adds no false-positive surface
/// relative to the parser that already reads these files.
///
/// # Why this does not use the inert mask
///
/// Frontmatter is YAML, not markdown. A 4-space-indented `cover:` under
/// `cascade:` after a blank line reads as an indented code block to
/// [`crate::inert_regions`] and would be silently dropped.
///
/// Nothing is deserialized and nothing re-serialized: only the value span is
/// ever replaced, so YAML comments, key order and quoting style survive.
pub fn frontmatter_asset_spans(source: &str) -> Vec<crate::resolve::md_extract::AssetPathSpan> {
    use crate::resolve::md_extract::{AssetPathSpan, PathContainer};

    let mut out = Vec::new();
    let table = crate::resolve::md_extract::line_table(source);
    let line_at = |k: usize| -> &str {
        let (base, content, _) = table[k];
        #[allow(clippy::string_slice)]
        // Line boundaries from `line_table`, which splits on ASCII '\n'/'\r'.
        &source[base..base + content]
    };

    // The block must open on line 0 and close on a later `---` line.
    if table.is_empty() || line_at(0).trim() != "---" {
        return out;
    }
    let Some(close) = (1..table.len()).find(|&k| line_at(k).trim() == "---") else {
        return out;
    };

    let keys: Vec<&str> = crate::schema_fields::asset_field_names().collect();
    // Indent of the key that opened a `|`/`>` block scalar, if we are inside
    // one. Every deeper-indented line below it is content, not a mapping —
    // this is what stops a `description: |` body containing `cover: x.png`
    // from being rewritten.
    let mut block_scalar_indent: Option<usize> = None;

    for k in 1..close {
        let (base, content_len, term_len) = table[k];
        let line = line_at(k);
        let indent = line.len() - line.trim_start().len();

        if let Some(bi) = block_scalar_indent {
            if line.trim().is_empty() || indent > bi {
                continue;
            }
            block_scalar_indent = None;
        }
        if line.trim().is_empty() {
            continue;
        }

        let Some(colon) = line.find(':') else { continue };
        #[allow(clippy::string_slice)]
        // `find` on ASCII ':' → char boundary; `indent` counts leading ASCII
        // whitespace.
        let key = line[indent..colon].trim();
        #[allow(clippy::string_slice)]
        let after = &line[colon + 1..];

        // `key: |` / `key: >-` / `key: |2` opens a block scalar.
        let t = after.trim();
        if t.starts_with('|') || t.starts_with('>') {
            #[allow(clippy::string_slice)]
            // '|' and '>' are ASCII.
            let tail = t[1..].trim_start_matches(['+', '-']);
            if tail.chars().all(|c| c.is_ascii_digit()) {
                block_scalar_indent = Some(indent);
                continue;
            }
        }

        if !keys.contains(&key) {
            continue;
        }

        // Value start: first non-space after the colon.
        let vrel = colon + 1 + (after.len() - after.trim_start().len());
        if vrel >= content_len {
            continue;
        }
        #[allow(clippy::string_slice)]
        let raw_tail = &line[vrel..];
        let first = raw_tail.as_bytes()[0];
        // Flow collections are not asset paths.
        if first == b'[' || first == b'{' {
            continue;
        }

        let (value_len, quote) = match first {
            b'"' => (scan_quoted(raw_tail, '"'), Some('"')),
            b'\'' => (scan_quoted(raw_tail, '\''), Some('\'')),
            _ => (scan_plain(raw_tail), None),
        };
        let Some(value_len) = value_len else { continue };
        #[allow(clippy::string_slice)]
        // `scan_quoted` / `scan_plain` return char-boundary lengths.
        let raw = &raw_tail[..value_len];
        let inner = match quote {
            Some('"') => unescape_double(raw),
            Some('\'') => raw
                .trim_matches('\'')
                .replace("''", "'"),
            _ => raw.to_string(),
        };
        if inner.trim().is_empty() {
            continue;
        }
        let (path, attrs) = crate::media::split_pipe(&inner);
        let path = crate::media::strip_wikilink(path).trim().to_string();
        if path.is_empty() {
            continue;
        }

        out.push(AssetPathSpan {
            path,
            attrs: attrs.to_string(),
            quote,
            value: base + vrel..base + vrel + value_len,
            outer: base..base + content_len + term_len,
            container: PathContainer::FrontmatterField {
                key: key.to_string(),
            },
        });
    }

    out
}

/// Byte length of a quoted YAML scalar starting at `s[0]` (the open quote),
/// INCLUDING both quotes. `None` when the quote never closes on this line.
fn scan_quoted(s: &str, q: char) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        if q == '"' && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == q as u8 {
            // YAML single quotes escape by doubling.
            if q == '\'' && bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// Byte length of a plain (unquoted) YAML scalar: to end of line, minus a
/// trailing ` #` comment, minus trailing whitespace.
fn scan_plain(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' && i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            end = i;
            break;
        }
    }
    while end > 0 && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    if end == 0 {
        None
    } else {
        Some(end)
    }
}

/// Decode a double-quoted YAML scalar's inner text (`\"` and `\\`).
fn unescape_double(raw: &str) -> String {
    let inner = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;
