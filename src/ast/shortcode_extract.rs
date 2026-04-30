//! Pre-parse extraction of `:::shortcode` blocks from markdown source.
//!
//! Walks the markdown line-by-line, tracking fenced code blocks (so
//! `:::buttons` inside a code fence stays inert) and recognizing
//! `:::name ...args` / `:::` openers/closers. Each block is replaced with
//! a sentinel HTML comment (`<!--MOSS_SHORTCODE_N-->`) that pulldown-cmark
//! emits as a `Block::Other` raw HTML; the final parser pass walks the
//! AST and substitutes the sentinels with typed [`Shortcode`] variants.
//!
//! Why this design:
//!
//! - `:::` block syntax is not standard CommonMark; pulldown-cmark sees
//!   it as plain text inside a paragraph. Post-parse text-matching is
//!   fragile (works only when the shortcode is the entire paragraph).
//! - Pre-parse extraction with a sentinel is the same pattern Zola uses
//!   and preserves parsing correctness for adjacent content.
//! - The sentinel is an HTML comment so it survives pulldown-cmark intact
//!   (pulldown-cmark passes HTML comments through `Event::Html` as
//!   `Block::HtmlBlock`).

use super::shortcode::{Shortcode, SubscribeShortcode};

/// One extracted shortcode block, with its body parsed into a typed variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedShortcode {
    /// 0-based index used in the placeholder sentinel.
    pub index: usize,
    /// Parsed shortcode (typed variants per Phase B).
    pub shortcode: Shortcode,
}

/// Result of pre-parse extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    /// Markdown source with `:::shortcode` blocks replaced by sentinel
    /// HTML comments. Pulldown-cmark sees this as the input.
    pub markdown_with_placeholders: String,
    /// One entry per extracted block, indexed by sentinel number.
    pub extracted: Vec<ExtractedShortcode>,
}

/// Recognized shortcode names (Phase B Task 7+ adds variants here).
fn parse_shortcode_block(name: &str, body: &str) -> Option<Shortcode> {
    match name {
        "subscribe" => Some(Shortcode::Subscribe(parse_subscribe_body(body))),
        // Phase B 8-11 add: buttons, gallery, hero, grid.
        _ => None,
    }
}

fn parse_subscribe_body(body: &str) -> SubscribeShortcode {
    let mut description: Option<String> = None;
    let mut button: Option<String> = None;
    for line in body.lines() {
        let trim = line.trim();
        if trim.is_empty() {
            continue;
        }
        if let Some(idx) = trim.find(':') {
            let key = trim[..idx].trim();
            let value = trim[idx + 1..].trim();
            match key {
                "description" if !value.is_empty() => description = Some(value.to_string()),
                "button" if !value.is_empty() => button = Some(value.to_string()),
                _ => {} // unknown / empty value — silently ignored, matches existing rewriter
            }
        }
    }
    SubscribeShortcode {
        description,
        button,
    }
}

/// The sentinel HTML comment used to mark an extracted shortcode in the
/// markdown source. Pulldown-cmark emits these as [`Event::Html`] inside
/// a [`Tag::HtmlBlock`], which surfaces as [`Block::Other`] in our AST.
pub fn placeholder_for(index: usize) -> String {
    format!("<!--MOSS_SHORTCODE_{index}-->")
}

/// Try to interpret a [`Block::Other`] payload as a shortcode placeholder.
/// Returns the `index` if it matches.
pub fn parse_placeholder(html: &str) -> Option<usize> {
    let trim = html.trim();
    let inner = trim.strip_prefix("<!--MOSS_SHORTCODE_")?;
    let inner = inner.strip_suffix("-->")?;
    inner.parse::<usize>().ok()
}

/// Walk the markdown line-by-line, replace `:::name` blocks with sentinels.
///
/// Tracks fenced code blocks (` ``` ` and `~~~`) so `:::buttons` inside a
/// code fence stays inert. Currently recognizes `:::subscribe`; other
/// shortcodes are added in Phase B Tasks 8-11. Unrecognized `:::name`
/// blocks pass through verbatim (the legacy string-rewriter still
/// processes them during the staged migration).
pub fn extract_shortcodes(markdown: &str) -> ExtractionResult {
    let mut output = String::with_capacity(markdown.len());
    let mut extracted: Vec<ExtractedShortcode> = Vec::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    let mut in_code_fence = false;
    let mut fence_marker = String::new();

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Track code fences first; do not parse shortcodes inside them.
        if in_code_fence {
            output.push_str(line);
            output.push('\n');
            if trimmed.starts_with(&fence_marker)
                && trimmed.trim_start_matches(fence_marker.chars().next().unwrap()).trim().is_empty()
            {
                in_code_fence = false;
                fence_marker.clear();
            }
            i += 1;
            continue;
        }
        if let Some(marker) = detect_code_fence_open(trimmed) {
            in_code_fence = true;
            fence_marker = marker;
            output.push_str(line);
            output.push('\n');
            i += 1;
            continue;
        }

        // Try to recognize a `:::name` opener.
        if let Some((name, _args)) = parse_shortcode_opener(trimmed) {
            // Look for the matching `:::` closer on a subsequent line.
            let mut body_lines: Vec<&str> = Vec::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() {
                let body_trim = lines[j].trim();
                if body_trim == ":::" {
                    closed = true;
                    break;
                }
                body_lines.push(lines[j]);
                j += 1;
            }

            if !closed {
                // Unclosed block: emit verbatim, let the legacy rewriter
                // surface the syntax error.
                output.push_str(line);
                output.push('\n');
                i += 1;
                continue;
            }

            // Try to parse the block as a known shortcode.
            let body = body_lines.join("\n");
            if let Some(sc) = parse_shortcode_block(name, &body) {
                let index = extracted.len();
                output.push_str(&placeholder_for(index));
                output.push('\n');
                extracted.push(ExtractedShortcode {
                    index,
                    shortcode: sc,
                });
                // Skip past the closer.
                i = j + 1;
                continue;
            }

            // Unrecognized name: emit verbatim. The legacy rewriter
            // may still process it (e.g. `:::buttons` until Phase B Task 8).
            output.push_str(line);
            output.push('\n');
            i += 1;
            continue;
        }

        // Regular content line.
        output.push_str(line);
        output.push('\n');
        i += 1;
    }

    ExtractionResult {
        markdown_with_placeholders: output,
        extracted,
    }
}

fn detect_code_fence_open(trimmed: &str) -> Option<String> {
    if trimmed.starts_with("```") {
        Some("```".to_string())
    } else if trimmed.starts_with("~~~") {
        Some("~~~".to_string())
    } else {
        None
    }
}

fn parse_shortcode_opener(trimmed: &str) -> Option<(&str, &str)> {
    let rest = trimmed.strip_prefix(":::")?;
    if rest.is_empty() || rest.starts_with(':') {
        // Just `:::` (closer) or `::::...` — not an opener.
        return None;
    }
    // Name = letters/digits/underscores; rest of line is args.
    let name_end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    if name_end == 0 {
        return None;
    }
    let name = &rest[..name_end];
    let args = rest[name_end..].trim();
    Some((name, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_shortcodes_round_trips_input() {
        let md = "# Heading\n\npara with [link](u).\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.markdown_with_placeholders, md);
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn extracts_subscribe_block_with_description_and_button() {
        let md = ":::subscribe\ndescription: Get updates\nbutton: Sign me up\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.description.as_deref(), Some("Get updates"));
                assert_eq!(args.button.as_deref(), Some("Sign me up"));
            }
        }
        assert!(result
            .markdown_with_placeholders
            .contains("<!--MOSS_SHORTCODE_0-->"));
        assert!(!result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn extracts_subscribe_block_with_only_description() {
        let md = ":::subscribe\ndescription: Get updates\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.description.as_deref(), Some("Get updates"));
                assert!(args.button.is_none());
            }
        }
    }

    #[test]
    fn extracts_subscribe_block_with_no_args() {
        let md = ":::subscribe\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert!(args.description.is_none());
                assert!(args.button.is_none());
            }
        }
    }

    #[test]
    fn ignores_unknown_keys_in_subscribe_body() {
        let md = ":::subscribe\nweird: thing\nbutton: Go\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.button.as_deref(), Some("Go"));
                assert!(args.description.is_none());
            }
        }
    }

    #[test]
    fn subscribe_inside_code_fence_is_not_extracted() {
        // Adversarial: `:::subscribe` inside a fenced code block is just
        // documentation text. The extractor must not treat it as a
        // shortcode.
        let md = "```\n:::subscribe\ndescription: doc\n:::\n```\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn subscribe_inside_tilde_fence_is_not_extracted() {
        let md = "~~~\n:::subscribe\n:::\n~~~\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn unclosed_subscribe_block_emits_verbatim() {
        // Unclosed: emit source verbatim so the author sees the typo.
        let md = ":::subscribe\nbutton: Go\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn unknown_shortcode_passes_through_verbatim() {
        // `:::buttons` is not yet migrated (Phase B Task 8); the
        // extractor must NOT consume it. Legacy rewriter still handles it.
        let md = ":::buttons\n[t](u)\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result.markdown_with_placeholders.contains(":::buttons"));
    }

    #[test]
    fn extracts_multiple_subscribes_with_increasing_indices() {
        let md = ":::subscribe\ndescription: a\n:::\n\nsome text\n\n:::subscribe\nbutton: b\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 2);
        assert_eq!(result.extracted[0].index, 0);
        assert_eq!(result.extracted[1].index, 1);
        assert!(result
            .markdown_with_placeholders
            .contains("<!--MOSS_SHORTCODE_0-->"));
        assert!(result
            .markdown_with_placeholders
            .contains("<!--MOSS_SHORTCODE_1-->"));
    }

    #[test]
    fn parse_placeholder_round_trips_index() {
        for index in [0, 1, 5, 99] {
            let s = placeholder_for(index);
            assert_eq!(parse_placeholder(&s), Some(index));
        }
    }

    #[test]
    fn parse_placeholder_rejects_non_placeholder_html() {
        assert!(parse_placeholder("<div>hi</div>").is_none());
        assert!(parse_placeholder("<!--just a comment-->").is_none());
    }

    #[test]
    fn parse_shortcode_opener_recognizes_simple_name() {
        assert_eq!(
            parse_shortcode_opener(":::subscribe"),
            Some(("subscribe", ""))
        );
    }

    #[test]
    fn parse_shortcode_opener_extracts_args() {
        assert_eq!(
            parse_shortcode_opener(":::grid 3 1:2:1"),
            Some(("grid", "3 1:2:1"))
        );
    }

    #[test]
    fn parse_shortcode_opener_rejects_bare_closer() {
        assert!(parse_shortcode_opener(":::").is_none());
    }

    #[test]
    fn parse_shortcode_opener_rejects_quadruple_colon() {
        assert!(parse_shortcode_opener("::::buttons").is_none());
    }
}
