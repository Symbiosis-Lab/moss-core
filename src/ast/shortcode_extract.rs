//! Pre-parse extraction of `:::shortcode` blocks from markdown source.
//!
//! Walks the markdown line-by-line, tracking fenced code blocks (so
//! `:::buttons` inside a code fence stays inert) and recognizing
//! `:::name ...args` / `:::` openers/closers. Each block is replaced with
//! a sentinel HTML comment (`<!--MOSS_SC_{nonce}_N-->`) that pulldown-cmark
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

use super::shortcode::{
    ButtonItem, ButtonsShortcode, GalleryItem, GalleryShortcode, Shortcode, SubscribeShortcode,
};
use super::url::Url;

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
    /// Per-extraction nonce (8 hex chars). Derived from a hash of the
    /// input markdown so it's deterministic but collision-resistant
    /// against authored content. The placeholder format is
    /// `<!--MOSS_SC_{nonce}_{index}-->`; an authored markdown comment
    /// matching that exact shape would have to embed the same hash of
    /// itself, which is computationally improbable for any input shorter
    /// than the SHA universe.
    pub nonce: String,
}

/// Recognized shortcode names (Phase B Task 7+ adds variants here).
///
/// `args` is the trailing text after `:::name ` on the opening line
/// (e.g. for `:::buttons {.primary}`, args is `{.primary}`).
fn parse_shortcode_block(name: &str, args: &str, body: &str) -> Option<Shortcode> {
    match name {
        "subscribe" => Some(Shortcode::Subscribe(parse_subscribe_body(body))),
        "buttons" => Some(Shortcode::Buttons(parse_buttons_body(args, body))),
        "gallery" => Some(Shortcode::Gallery(parse_gallery_body(args, body))),
        // Phase B 10-11 add: hero, grid.
        _ => None,
    }
}

fn parse_gallery_body(args: &str, body: &str) -> GalleryShortcode {
    // Args: `N {.classes}` where N is optional columns count.
    let (positional, classes) = split_positional_and_classes(args);
    let columns = if positional.is_empty() {
        None
    } else {
        positional.parse::<u32>().ok()
    };
    let mut items: Vec<GalleryItem> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Each line: `path|attrs`, `![alt](path)|attrs`, or bare `path`.
        // The pipe split (if any) is BEFORE the markdown-image pattern check.
        let (src_raw, attrs) = split_pipe(trimmed);
        let (src_url, alt) = match parse_markdown_image(src_raw) {
            Some((alt, path)) => (path, alt),
            None => (src_raw.trim().to_string(), String::new()),
        };
        items.push(GalleryItem {
            src: Url::unresolved(src_url),
            alt,
            attrs: attrs.to_string(),
        });
    }
    GalleryShortcode {
        columns,
        classes,
        items,
    }
}

/// Split `args` into `(positional_text, classes)` from `{.foo .bar}` syntax.
/// Mirrors `extract_attrs` in `src-tauri/src/build/shortcode.rs`.
fn split_positional_and_classes(args: &str) -> (String, String) {
    let trimmed = args.trim();
    if let Some(brace_start) = trimmed.find('{') {
        if let Some(brace_end) = trimmed[brace_start..].find('}') {
            let positional = trimmed[..brace_start].trim().to_string();
            let inner = &trimmed[brace_start + 1..brace_start + brace_end];
            let mut classes = Vec::new();
            for token in inner.split_whitespace() {
                if let Some(class) = token.strip_prefix('.') {
                    if !class.is_empty() {
                        classes.push(class);
                    }
                }
            }
            return (positional, classes.join(" "));
        }
    }
    (trimmed.to_string(), String::new())
}

/// Split `s` on `|` into `(before, after)`. If no pipe, returns `(s, "")`.
fn split_pipe(s: &str) -> (&str, &str) {
    match s.find('|') {
        Some(pos) => (&s[..pos], s[pos + 1..].trim()),
        None => (s, ""),
    }
}

/// Parse `![alt](path)` into `(alt, path)`. Returns `None` if not a
/// markdown image. Mirrors the legacy parser at shortcode.rs:1615.
fn parse_markdown_image(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let rest = s.strip_prefix("![")?;
    let close_bracket = rest.find("](")?;
    let alt = &rest[..close_bracket];
    let after = &rest[close_bracket + 2..];
    let close_paren = after.rfind(')')?;
    let path = &after[..close_paren];
    if path.contains('(') {
        return None;
    }
    Some((alt.to_string(), path.to_string()))
}

fn parse_buttons_body(args: &str, body: &str) -> ButtonsShortcode {
    let (_positional, classes) = split_positional_and_classes(args);
    let mut items: Vec<ButtonItem> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((text, url)) = extract_markdown_link(trimmed) {
            items.push(ButtonItem {
                text,
                url: Url::unresolved(url),
            });
        }
        // Non-link lines silently ignored (matches legacy behavior).
    }
    ButtonsShortcode { classes, items }
}

/// Extract a markdown link `[text](url)` from a single trimmed line.
/// Returns `(text, url)` if the line is a single link, else `None`.
fn extract_markdown_link(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if !s.starts_with('[') {
        return None;
    }
    let close_bracket = s.find(']')?;
    let text = &s[1..close_bracket];
    let after = &s[close_bracket + 1..];
    if !after.starts_with('(') || !after.ends_with(')') {
        return None;
    }
    let url = &after[1..after.len() - 1];
    if url.is_empty() {
        return None;
    }
    Some((text.to_string(), url.to_string()))
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
///
/// `nonce` is the per-extraction hash from [`ExtractionResult::nonce`],
/// which forecloses the namespace-collision case where an author writes
/// `<!--MOSS_SC_*-->` literally in their markdown.
pub fn placeholder_for(nonce: &str, index: usize) -> String {
    format!("<!--MOSS_SC_{nonce}_{index}-->")
}

/// Try to interpret a [`Block::Other`] payload as a shortcode placeholder
/// matching the given `nonce`. Returns the `index` if it matches.
///
/// Any sentinel with a different (or absent) nonce is rejected — that's
/// what makes authored content with a similar comment shape inert.
pub fn parse_placeholder(nonce: &str, html: &str) -> Option<usize> {
    let trim = html.trim();
    let prefix = format!("<!--MOSS_SC_{nonce}_");
    let inner = trim.strip_prefix(&prefix)?;
    let inner = inner.strip_suffix("-->")?;
    inner.parse::<usize>().ok()
}

/// Compute the per-extraction nonce from the input markdown. Uses
/// `std::hash::DefaultHasher` (FxHash-like; not cryptographic, but good
/// enough to make a literal authored-content collision computationally
/// improbable for any short input). Returns 8 hex characters.
fn compute_nonce(input: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    // Truncate to 32 bits for an 8-char hex; collisions across two
    // sites are not a concern (each extraction uses its own nonce
    // for its own substitution). Per-extraction collision-resistance
    // requires only that the nonce differs from any literal string
    // in the same input — 32 bits is overkill for that.
    let h = hasher.finish() as u32;
    format!("{h:08x}")
}

/// Walk the markdown line-by-line, replace `:::name` blocks with sentinels.
///
/// Tracks fenced code blocks (` ``` ` and `~~~`) so `:::buttons` inside a
/// code fence stays inert. Currently recognizes `:::subscribe`; other
/// shortcodes are added in Phase B Tasks 8-11. Unrecognized `:::name`
/// blocks pass through verbatim (the legacy string-rewriter still
/// processes them during the staged migration).
pub fn extract_shortcodes(markdown: &str) -> ExtractionResult {
    let nonce = compute_nonce(markdown);
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

        // Try to recognize a `:::name` (or `::::name`, etc.) opener.
        if let Some((arity, name, args)) = parse_shortcode_opener(trimmed) {
            // Look for the matching closer (same arity) on a subsequent line.
            let mut body_lines: Vec<&str> = Vec::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() {
                if is_close_fence(lines[j].trim(), arity) {
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
            if let Some(sc) = parse_shortcode_block(name, args, &body) {
                let index = extracted.len();
                output.push_str(&placeholder_for(&nonce, index));
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
            // still handles this kind until its Phase B migration commit.
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
        nonce,
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

/// Parse an opening fence line into (colon_count, name, args). Returns
/// `None` if the line is not an opener.
///
/// Accepts any colon count >= 3 (`:::name`, `::::name`, `:::::name`, ...).
/// The colon count is preserved so the closer must match the same arity
/// (allows nested shortcodes like `::::buttons` inside `:::grid`).
fn parse_shortcode_opener(trimmed: &str) -> Option<(usize, &str, &str)> {
    let colons = trimmed.chars().take_while(|&c| c == ':').count();
    if colons < 3 {
        return None;
    }
    let rest = &trimmed[colons..];
    // Name = letters/digits/underscores/hyphens; rest of line is args.
    let name_end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    if name_end == 0 {
        return None;
    }
    let name = &rest[..name_end];
    let args = rest[name_end..].trim();
    Some((colons, name, args))
}

/// True if `trimmed` is a closing fence with the specified arity (`:::`
/// for arity 3, `::::` for arity 4, etc.).
///
/// Matches the legacy `parse_fence_close` semantics in
/// `src-tauri/src/build/shortcode.rs:857`: a closer is N colons followed
/// by optional whitespace only. A line `::: extra` is NOT a closer
/// (it's content). Without this match, authors who pasted text after
/// the closer would see different behavior between the typed-AST path
/// and the legacy grid parser, surfacing as silent content corruption.
///
/// Implemented via char iteration (NOT `split_at(arity)`) because the
/// `arity` is a count of `:` characters (always ASCII, 1 byte each), but
/// the `trimmed` line might start with multi-byte UTF-8 characters
/// (e.g. `[申请测试版](...)` from Chinese-language buttons). `split_at`
/// is byte-indexed and would panic mid-character on such lines. Char
/// iteration sidesteps the issue and is also slightly faster — we early-exit
/// on the first non-`:` character.
fn is_close_fence(trimmed: &str, arity: usize) -> bool {
    let mut chars = trimmed.chars();
    for _ in 0..arity {
        match chars.next() {
            Some(':') => {}
            _ => return false,
        }
    }
    // Remaining chars (if any) must all be whitespace.
    chars.all(char::is_whitespace)
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
            other => panic!("expected Subscribe, got {other:?}"),
        }
        assert!(result
            .markdown_with_placeholders
            .contains(&placeholder_for(&result.nonce, 0)));
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
            other => panic!("expected Subscribe, got {other:?}"),
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
            other => panic!("expected Subscribe, got {other:?}"),
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
            other => panic!("expected Subscribe, got {other:?}"),
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
        // `:::hero` is not yet migrated (Phase B Task 10 lands later);
        // the extractor must NOT consume it. Legacy rewriter still
        // handles it.
        let md = ":::hero\n![[bg.jpg]]\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result.markdown_with_placeholders.contains(":::hero"));
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
            .contains(&placeholder_for(&result.nonce, 0)));
        assert!(result
            .markdown_with_placeholders
            .contains(&placeholder_for(&result.nonce, 1)));
    }

    #[test]
    fn parse_placeholder_round_trips_index() {
        let nonce = "deadbeef";
        for index in [0, 1, 5, 99] {
            let s = placeholder_for(nonce, index);
            assert_eq!(parse_placeholder(nonce, &s), Some(index));
        }
    }

    #[test]
    fn parse_placeholder_rejects_non_placeholder_html() {
        let nonce = "deadbeef";
        assert!(parse_placeholder(nonce, "<div>hi</div>").is_none());
        assert!(parse_placeholder(nonce, "<!--just a comment-->").is_none());
    }

    #[test]
    fn parse_placeholder_rejects_wrong_nonce() {
        // Authored content collision case: an author writes a literal
        // <!--MOSS_SC_*_0--> in their markdown. parse_placeholder requires
        // the same nonce as this extraction's session, so a mismatched
        // nonce returns None. This forecloses the authored-content
        // namespace collision.
        let s = placeholder_for("aaaa1111", 5);
        assert_eq!(parse_placeholder("bbbb2222", &s), None);
    }

    #[test]
    fn extract_uses_content_derived_nonce() {
        // The nonce is deterministic per input — calling extract_shortcodes
        // twice on the same input produces the same nonce.
        let md = ":::subscribe\n:::\n";
        let r1 = extract_shortcodes(md);
        let r2 = extract_shortcodes(md);
        assert_eq!(r1.nonce, r2.nonce);
        // Different inputs produce different nonces (with overwhelming
        // probability — collision impossible to exhibit here).
        let r3 = extract_shortcodes(":::subscribe\ndescription: x\n:::\n");
        assert_ne!(r1.nonce, r3.nonce);
    }

    #[test]
    fn nonce_makes_authored_collision_inert() {
        // If an author writes a literal placeholder-shape comment, my
        // nonce will differ from theirs, so the substitution leaves
        // their text alone.
        let md = ":::subscribe\n:::\n\nLook: <!--MOSS_SC_00000000_0-->\n";
        let result = extract_shortcodes(md);
        // The author's comment survives because the embedded nonce
        // differs from the computed one (probability of collision = 1/2^32).
        assert_ne!(result.nonce, "00000000");
        assert!(result
            .markdown_with_placeholders
            .contains("MOSS_SC_00000000_0"));
    }

    #[test]
    fn parse_shortcode_opener_recognizes_simple_name() {
        assert_eq!(
            parse_shortcode_opener(":::subscribe"),
            Some((3, "subscribe", ""))
        );
    }

    #[test]
    fn parse_shortcode_opener_extracts_args() {
        assert_eq!(
            parse_shortcode_opener(":::grid 3 1:2:1"),
            Some((3, "grid", "3 1:2:1"))
        );
    }

    #[test]
    fn parse_shortcode_opener_recognizes_quadruple_colon() {
        // ::::buttons is the standard way to nest a shortcode inside
        // a :::grid cell. The arity is preserved so the closer matches.
        assert_eq!(
            parse_shortcode_opener("::::buttons"),
            Some((4, "buttons", ""))
        );
    }

    #[test]
    fn parse_shortcode_opener_rejects_two_colons() {
        // Two colons is not a fence.
        assert!(parse_shortcode_opener("::name").is_none());
    }

    #[test]
    fn extracts_quadruple_colon_buttons() {
        // ::::buttons inside hypothetical grid context. We just test the
        // extractor in isolation; grid integration lands in Task 11.
        let md = "::::buttons\n[Tickets](go/)\n::::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Tickets");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_quadruple_buttons_inside_unrecognized_grid() {
        // The realistic SoCiviC pattern: `::::buttons` (4-colon) nested
        // inside a `:::grid` (3-colon, not yet migrated). The extractor
        // must recognize the 4-colon buttons block while leaving the
        // 3-colon grid markers verbatim for the legacy grid rewriter.
        let md = ":::grid 2\n::::buttons\n[Tickets](go/)\n::::\n---\nfooter cell\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        assert!(matches!(
            result.extracted[0].shortcode,
            Shortcode::Buttons(_)
        ));
        // The grid markers must survive verbatim for the legacy rewriter.
        assert!(result.markdown_with_placeholders.contains(":::grid 2"));
        assert!(result.markdown_with_placeholders.contains("---\nfooter"));
        assert!(!result.markdown_with_placeholders.contains("::::buttons"));
    }

    #[test]
    fn arity_mismatch_does_not_close_block() {
        // A `:::` closer inside a `::::buttons` block must NOT close it.
        // Body content can contain `:::` strings as text (in code blocks
        // or grid cell separators when nested differently).
        let md = "::::buttons\n[t](u)\n:::\n[t2](u2)\n::::\n";
        let result = extract_shortcodes(md);
        // Only one extraction (the ::::buttons block).
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                // Both links should be captured (the `:::` was just body text).
                assert_eq!(args.items.len(), 2);
            }
            _ => panic!("expected Buttons"),
        }
    }

    // ---- Buttons (Phase B Task 8) ----

    #[test]
    fn extracts_buttons_block_with_one_link() {
        let md = ":::buttons\n[Documentation](docs/)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert!(args.classes.is_empty());
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Documentation");
                match &args.items[0].url {
                    Url::Unresolved(s) => assert_eq!(s, "docs/"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_buttons_block_with_multiple_links() {
        let md = ":::buttons\n[Docs](docs/)\n[GitHub](https://github.com)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].text, "Docs");
                assert_eq!(args.items[1].text, "GitHub");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_buttons_block_with_class_attrs() {
        let md = ":::buttons {.primary .large}\n[Go](go/)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.classes, "primary large");
                assert_eq!(args.items.len(), 1);
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_buttons_with_moss_resolved_url_intact() {
        // The upstream resolve pipeline rewrites internal links to
        // moss-resolved:foo.md before the AST sees them. The extractor
        // must preserve the prefix verbatim — visit_urls_mut classifies it.
        let md = ":::buttons\n[Docs](moss-resolved:docs/index.md)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => match &args.items[0].url {
                Url::Unresolved(s) => assert_eq!(s, "moss-resolved:docs/index.md"),
                _ => panic!("expected Unresolved"),
            },
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn buttons_skips_non_link_lines() {
        // Non-link lines (commentary, blank lines) are silently skipped
        // — matches the legacy rewriter behavior.
        let md = ":::buttons\nNot a link, just text.\n[Real](real/)\n\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Real");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn buttons_inside_code_fence_is_not_extracted() {
        let md = "```\n:::buttons\n[t](u)\n:::\n```\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn extract_markdown_link_rejects_text_with_close_bracket() {
        // Pinning test (code-review P2): the parser uses find(']') for the
        // first close-bracket. A link text containing ']' silently fails
        // to parse and the line is skipped (silently — matches legacy
        // shortcode.rs::extract_markdown_link). If/when this is relaxed,
        // this test fails and the change is deliberate.
        let md = ":::buttons\n[a]b](u)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => assert!(args.items.is_empty()),
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extract_markdown_link_requires_trailing_paren() {
        // Pinning test: trailing content after `)` causes the link to be
        // rejected (matches legacy behavior).
        let md = ":::buttons\n[t](u) <!-- trailing -->\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => assert!(args.items.is_empty()),
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn close_fence_with_trailing_whitespace_is_recognized() {
        // Whitespace after the colons is allowed (matches legacy
        // parse_fence_close at shortcode.rs:857).
        let md = ":::subscribe\nbutton: x\n:::   \n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
    }

    #[test]
    fn is_close_fence_handles_multibyte_utf8_lines() {
        // Regression test for the moss-releases panic at byte index 3
        // (inside `申`) of `[申请测试版](#青苔正在封闭测试)`. The buggy
        // `split_at(arity)` was byte-indexed; this line happens to be
        // longer than 3 bytes but the first 3 bytes land mid-character
        // because `[` (1 byte) + `申` (3 bytes, bytes 1..4). Char-based
        // iteration sidesteps the issue.
        assert!(!is_close_fence("[申请测试版](#青苔正在封闭测试)", 3));
        assert!(!is_close_fence("[申请测试版](#青苔正在封闭测试)", 4));
        // CJK lines that would have panicked the old split_at variant.
        assert!(!is_close_fence("中文内容", 3));
        assert!(!is_close_fence("日本語", 3));
        // Truly closing lines still match.
        assert!(is_close_fence(":::", 3));
        assert!(is_close_fence("::::", 4));
    }

    #[test]
    fn extract_shortcodes_handles_buttons_with_cjk_link_text() {
        // End-to-end regression for the moss-releases site bug: a
        // :::buttons block containing a markdown link with CJK text
        // and a CJK URL anchor. The extractor must not panic, must
        // extract the buttons block, and must capture both items.
        let md = ":::buttons\n[申请测试版](#青苔正在封闭测试)\n[文档](docs/)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].text, "申请测试版");
                match &args.items[0].url {
                    Url::Unresolved(s) => assert_eq!(s, "#青苔正在封闭测试"),
                    _ => panic!("expected Unresolved"),
                }
                assert_eq!(args.items[1].text, "文档");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extract_shortcodes_does_not_panic_on_arbitrary_cjk_content() {
        // Smoke test against the shape that triggered the moss-releases
        // panic: a document with mixed CJK content INCLUDING lines that
        // start with multi-byte characters but happen to have byte
        // length ≥ arity. None of these are close-fence candidates;
        // the extractor must scan past them without panic.
        let md = "# 标题\n\n中文段落，混合 English 单词。\n\n:::buttons\n[申请测试版](#锚点)\n:::\n\n## 二级标题\n\n更多内容。\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
    }

    #[test]
    fn close_fence_with_trailing_text_is_not_recognized() {
        // P1 #2 fix: `::: more text` does NOT close the block. Without
        // this match against legacy semantics, an author who pasted text
        // after the closer would see different behavior between the
        // typed-AST path and the legacy grid parser.
        let md = ":::subscribe\nbutton: x\n::: more text\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        // The first `:::` is body content; the second `:::` is the closer.
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => assert_eq!(args.button.as_deref(), Some("x")),
            _ => panic!("expected Subscribe"),
        }
    }

    // ---- Gallery (Phase B Task 9) ----

    #[test]
    fn extracts_gallery_with_bare_paths() {
        let md = ":::gallery\nphoto1.jpg\nphoto2.png\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert!(args.columns.is_none());
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].alt, "");
                match &args.items[0].src {
                    Url::Unresolved(s) => assert_eq!(s, "photo1.jpg"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_columns_arg() {
        let md = ":::gallery 4\na.jpg\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => assert_eq!(args.columns, Some(4)),
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_classes() {
        let md = ":::gallery 3 {.showcase}\na.jpg\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.columns, Some(3));
                assert_eq!(args.classes, "showcase");
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_markdown_image_syntax() {
        let md = ":::gallery\n![A photo](photo.jpg)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.items[0].alt, "A photo");
                match &args.items[0].src {
                    Url::Unresolved(s) => assert_eq!(s, "photo.jpg"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_pipe_attrs() {
        let md = ":::gallery\nphoto.jpg|cover top\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.items[0].attrs, "cover top");
                match &args.items[0].src {
                    Url::Unresolved(s) => assert_eq!(s, "photo.jpg"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn gallery_skips_blank_lines() {
        let md = ":::gallery\n\na.jpg\n\nb.jpg\n\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => assert_eq!(args.items.len(), 2),
            _ => panic!("expected Gallery"),
        }
    }
}
