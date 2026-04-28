//! Standard markdown link resolution.
//!
//! Scans markdown for `[text](target)` patterns and resolves `target` via
//! [`ContentGraph::resolve_path`](crate::content_graph::ContentGraph::resolve_path)
//! when `target` is a relative reference (not an external URL, anchor, or
//! protocol link).
//!
//! This is the standard-link counterpart to [`markdown_refs`](super::markdown_refs),
//! which handles bare-filename image references `![](photo.jpg)`. Both modules
//! delegate resolution to the same `ContentGraph::resolve_path` entry point --
//! keeping link-syntax handling uniform regardless of which bracket shape the
//! author used.
//!
//! Content inside fenced code blocks and inline code spans is left untouched.
//!
//! Resolved output uses the `moss-resolved:` scheme so downstream URL-transform
//! code (markdown.rs's `resolve_link` closure) can recognize pre-resolved links
//! and convert them to relative pretty URLs.

use crate::content_graph::ContentGraph;

use super::fuzzy_path::{resolve_reference, ResolvedRef};
use super::{Diagnostic, LinkType, OutgoingLink};

#[derive(Debug)]
pub struct MarkdownLinkResult {
    pub content: String,
    pub outgoing_links: Vec<OutgoingLink>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Return true if `url` is a reference we should hand to ContentGraph.
fn is_resolvable_target(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    // Fragment-only and protocol links are never resolved through the graph.
    if url.starts_with('#') {
        return false;
    }
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("//") {
        return false;
    }
    if url.starts_with("mailto:") || url.starts_with("tel:") || url.starts_with("data:") {
        return false;
    }
    // Wikilink-generated markers -- handled by wikilinks phase, not us.
    if url.starts_with("moss-resolved:") {
        return false;
    }
    // Absolute filesystem paths -- treat as opaque.
    if url.starts_with('/') {
        return false;
    }
    true
}

/// Split a URL into (path, suffix) where `suffix` is `?query` and/or `#fragment`
/// in source order. The path is what gets resolved against the content graph;
/// the suffix is reattached to the resolved URL verbatim.
///
/// Whichever of `?` or `#` appears first ends the path. RFC 3986 specifies
/// `?` before `#`, but mirroring wikilinks we accept either order — once the
/// path ends, everything else (including a later `?` or `#`) is opaque suffix.
fn split_path_suffix(url: &str) -> (&str, Option<&str>) {
    let q = url.find('?');
    let h = url.find('#');
    let cut = match (q, h) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    match cut {
        Some(pos) => (&url[..pos], Some(&url[pos..])),
        None => (url, None),
    }
}

/// Resolve `[text](target)` markdown links via ContentGraph.
///
/// Ignores image syntax (`![...](...)` -- those are `markdown_refs`'s job).
/// Leaves external, anchor-only, mailto/tel/data links untouched.
/// Leaves unresolvable targets as-is (diagnostic is emitted but the link stays).
pub fn resolve_markdown_links(
    content: &str,
    graph: &ContentGraph,
    from_path: &str,
) -> MarkdownLinkResult {
    let mut outgoing_links: Vec<OutgoingLink> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut output_lines: Vec<String> = Vec::new();

    let mut fence_char: Option<char> = None;

    for line in content.lines() {
        // Fence tracking -- mirrors markdown_refs.rs so we don't rewrite code.
        if let Some(fc) = fence_char {
            let trimmed = line.trim_start();
            let closes = trimmed.starts_with(fc)
                && trimmed.chars().take(3).all(|c| c == fc)
                && trimmed.trim_matches(fc).trim().is_empty();
            if closes {
                fence_char = None;
            }
            output_lines.push(line.to_string());
            continue;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let candidate_char = trimmed.chars().next().unwrap();
            let rest = &trimmed[3..];
            if !rest.contains(candidate_char) {
                fence_char = Some(candidate_char);
                output_lines.push(line.to_string());
                continue;
            }
        }

        output_lines.push(rewrite_line(
            line,
            graph,
            from_path,
            &mut outgoing_links,
            &mut diagnostics,
        ));
    }

    let trailing = if content.ends_with('\n') { "\n" } else { "" };
    MarkdownLinkResult {
        content: output_lines.join("\n") + trailing,
        outgoing_links,
        diagnostics,
    }
}

fn rewrite_line(
    line: &str,
    graph: &ContentGraph,
    from_path: &str,
    outgoing: &mut Vec<OutgoingLink>,
    diags: &mut Vec<Diagnostic>,
) -> String {
    // Parser state: scan for `[` that is NOT preceded by `!` (image) and NOT inside `` `code` ``.
    // Walk by byte index. ASCII delimiters (`, !, [, ]) are always char-boundaries,
    // so byte-indexed slicing into `line` stays UTF-8 safe.
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Inline code span: match backtick runs. A run of N backticks is closed
        // only by another run of exactly N. Single ` does not close `` ... ``.
        if bytes[i] == b'`' {
            let run = count_byte(bytes, i, b'`');
            out.push_str(&line[i..i + run]);
            i += run;
            // Scan for a matching closing run of the same length.
            while i < len {
                if bytes[i] == b'`' {
                    let closing = count_byte(bytes, i, b'`');
                    out.push_str(&line[i..i + closing]);
                    i += closing;
                    if closing == run {
                        break;
                    }
                } else {
                    // Advance one UTF-8 char.
                    let ch_len = utf8_char_len(bytes[i]);
                    out.push_str(&line[i..i + ch_len]);
                    i += ch_len;
                }
            }
            continue;
        }

        // Skip images: `![...](...)` -- the entire span is left for markdown_refs.
        if bytes[i] == b'!' && i + 1 < len && bytes[i + 1] == b'[' {
            if let Some((.., consumed)) = parse_link_at(&line[i + 1..]) {
                // Emit the full `![...](...)` span and advance past it.
                out.push_str(&line[i..i + 1 + consumed]);
                i += 1 + consumed;
                continue;
            }
            // Not a well-formed image span -- emit `!` and continue.
            out.push('!');
            i += 1;
            continue;
        }

        if bytes[i] == b'[' {
            if let Some((text, url, consumed)) = parse_link_at(&line[i..]) {
                let mut handled = false;
                if is_resolvable_target(url) {
                    let (path_part, suffix) = split_path_suffix(url);
                    match resolve_reference(path_part, graph, from_path) {
                        ResolvedRef::Found(resolved) => {
                            let new_url = match suffix {
                                Some(s) => format!("moss-resolved:{}{}", resolved, s),
                                None => format!("moss-resolved:{}", resolved),
                            };
                            out.push_str(&format!("[{}]({})", text, new_url));
                            outgoing.push(OutgoingLink {
                                target_path: resolved,
                                display_text: text.to_string(),
                                link_type: LinkType::Standard,
                            });
                            handled = true;
                        }
                        ResolvedRef::Unresolved => {
                            diags.push(Diagnostic {
                                message: format!("unresolved link: {}", url),
                                source_path: from_path.to_string(),
                                reference: url.to_string(),
                            });
                        }
                    }
                }
                if !handled {
                    out.push_str(&line[i..i + consumed]);
                }
                i += consumed;
                continue;
            }
        }

        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Count consecutive occurrences of `b` in `bytes` starting at `start`.
fn count_byte(bytes: &[u8], start: usize, b: u8) -> usize {
    bytes[start..].iter().take_while(|&&x| x == b).count()
}

/// Length in bytes of the UTF-8 character whose leading byte is `b`.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xC0 {
        1 // continuation byte — shouldn't happen at a char boundary, but be safe
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

/// Parse `[text](url)` starting at index 0 of `s`. Returns (text, url, total_bytes_consumed).
/// Returns None if the shape isn't a standard markdown link.
fn parse_link_at(s: &str) -> Option<(&str, &str, usize)> {
    if !s.starts_with('[') {
        return None;
    }
    let bytes = s.as_bytes();
    // Find matching `]`.
    let mut depth = 0i32;
    let mut close_bracket = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close_bracket = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close_bracket = close_bracket?;
    if close_bracket + 1 >= bytes.len() || bytes[close_bracket + 1] != b'(' {
        return None;
    }
    // Find matching `)`.
    let mut paren_depth = 0i32;
    let mut close_paren = None;
    for (i, &b) in bytes[close_bracket + 1..].iter().enumerate() {
        match b {
            b'(' => paren_depth += 1,
            b')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    close_paren = Some(close_bracket + 1 + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close_paren = close_paren?;
    let text = &s[1..close_bracket];
    let url = &s[close_bracket + 2..close_paren];
    Some((text, url, close_paren + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;

    fn graph_with(paths: &[&str]) -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        for p in paths {
            b.add_file(p, p);
        }
        b.build()
    }

    #[test]
    fn folder_note_resolves() {
        let g = graph_with(&["index.md", "文字/文字.md"]);
        let r = resolve_markdown_links("[文字](文字.md)", &g, "index.md");
        assert!(
            r.content.contains("moss-resolved:文字/文字.md"),
            "got: {}",
            r.content
        );
    }

    #[test]
    fn external_link_untouched() {
        let g = graph_with(&["index.md"]);
        let r = resolve_markdown_links("[ex](https://example.com)", &g, "index.md");
        assert_eq!(r.content, "[ex](https://example.com)");
    }

    #[test]
    fn anchor_untouched() {
        let g = graph_with(&["index.md"]);
        let r = resolve_markdown_links("[top](#top)", &g, "index.md");
        assert_eq!(r.content, "[top](#top)");
    }

    #[test]
    fn image_left_for_markdown_refs() {
        let g = graph_with(&["index.md", "assets/photo.jpg"]);
        let r = resolve_markdown_links("![alt](photo.jpg)", &g, "index.md");
        // Images are handled by markdown_refs, not here.
        assert_eq!(r.content, "![alt](photo.jpg)");
    }

    #[test]
    fn unresolved_stays_as_is() {
        let g = graph_with(&["index.md"]);
        let r = resolve_markdown_links("[missing](missing.md)", &g, "index.md");
        assert_eq!(r.content, "[missing](missing.md)");
        assert_eq!(r.diagnostics.len(), 1);
    }

    #[test]
    fn inline_code_skipped() {
        let g = graph_with(&["index.md", "文字/文字.md"]);
        let r = resolve_markdown_links("`[文字](文字.md)`", &g, "index.md");
        assert_eq!(r.content, "`[文字](文字.md)`");
    }

    #[test]
    fn fragment_preserved() {
        let g = graph_with(&["index.md", "文字/文字.md"]);
        let r = resolve_markdown_links("[x](文字/文字.md#sec)", &g, "index.md");
        assert!(
            r.content.contains("moss-resolved:文字/文字.md#sec"),
            "got: {}",
            r.content
        );
        assert_eq!(r.outgoing_links.len(), 1);
        assert_eq!(r.outgoing_links[0].target_path, "文字/文字.md");
    }

    #[test]
    fn multiple_links_on_one_line() {
        let g = graph_with(&["index.md", "foo.md", "bar.md"]);
        let r = resolve_markdown_links("[a](foo.md) and [b](bar.md)", &g, "index.md");
        assert!(r.content.contains("moss-resolved:foo.md"), "got: {}", r.content);
        assert!(r.content.contains("moss-resolved:bar.md"), "got: {}", r.content);
        assert_eq!(r.outgoing_links.len(), 2);
    }

    #[test]
    fn double_backtick_inline_code_skipped() {
        let g = graph_with(&["index.md", "y.md"]);
        // A single ` inside a `` ... `` span should NOT close the span.
        let r = resolve_markdown_links("``[x](y.md)``", &g, "index.md");
        assert_eq!(r.content, "``[x](y.md)``");
        assert!(r.outgoing_links.is_empty());
    }

    #[test]
    fn outgoing_link_fields_populated() {
        let g = graph_with(&["index.md", "foo.md"]);
        let r = resolve_markdown_links("[hello](foo.md)", &g, "index.md");
        assert_eq!(r.outgoing_links.len(), 1);
        assert_eq!(r.outgoing_links[0].link_type, LinkType::Standard);
        assert_eq!(r.outgoing_links[0].target_path, "foo.md");
        assert_eq!(r.outgoing_links[0].display_text, "hello");
    }

    #[test]
    fn fence_open_with_inline_close_is_not_fence() {
        // Sibling rule: a line with the fence char after the leading run
        // isn't a fence *opening*. Content outside should still rewrite.
        let g = graph_with(&["index.md", "foo.md"]);
        let input = "```inline``` [a](foo.md)";
        let r = resolve_markdown_links(input, &g, "index.md");
        assert!(r.content.contains("moss-resolved:foo.md"), "got: {}", r.content);
    }

    #[test]
    fn query_string_preserved() {
        let g = graph_with(&["index.md", "assets/scale-compare.html"]);
        let r = resolve_markdown_links(
            "[demo](scale-compare.html?a=major_pent&r=major_pent%3AD)",
            &g,
            "index.md",
        );
        assert!(
            r.content
                .contains("moss-resolved:assets/scale-compare.html?a=major_pent&r=major_pent%3AD"),
            "got: {}",
            r.content
        );
        assert_eq!(r.outgoing_links.len(), 1);
        assert_eq!(r.outgoing_links[0].target_path, "assets/scale-compare.html");
    }

    #[test]
    fn query_and_fragment_preserved() {
        let g = graph_with(&["index.md", "assets/app.html"]);
        let r = resolve_markdown_links("[d](app.html?x=1#sec)", &g, "index.md");
        assert!(
            r.content.contains("moss-resolved:assets/app.html?x=1#sec"),
            "got: {}",
            r.content
        );
    }

    #[test]
    fn fragment_before_query_preserved() {
        // Nonstandard order — preserved as-is.
        let g = graph_with(&["index.md", "assets/app.html"]);
        let r = resolve_markdown_links("[d](app.html#sec?x=1)", &g, "index.md");
        assert!(
            r.content.contains("moss-resolved:assets/app.html#sec?x=1"),
            "got: {}",
            r.content
        );
    }

    /// Regression: `[![alt](image-path)](target?query)` — markdown link wrapping
    /// a markdown image (the shape produced by `[![[image.png]]](target.html?q)`
    /// after the wikilinks pass rewrites the embed to `![alt](path)`).
    #[test]
    fn link_wrapping_image_with_query_resolves() {
        let g = graph_with(&["index.md", "assets/scale-compare.html", "assets/scale-compare.png"]);
        let input = "[![scale-compare](assets/scale-compare.png)](scale-compare.html?a=major_pent&r=major_pent%3AD)";
        let r = resolve_markdown_links(input, &g, "index.md");
        assert!(
            r.content.contains("moss-resolved:assets/scale-compare.html?a=major_pent&r=major_pent%3AD"),
            "outer link not resolved or query dropped. got: {}",
            r.content
        );
    }

    #[test]
    fn fenced_block_skipped() {
        let g = graph_with(&["index.md", "文字/文字.md"]);
        let input = "```\n[文字](文字.md)\n```\n";
        let r = resolve_markdown_links(input, &g, "index.md");
        assert!(r.content.contains("[文字](文字.md)"));
        assert!(!r.content.contains("moss-resolved:"));
    }
}
