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

/// Split a URL into (path, fragment_including_hash).
fn split_fragment(url: &str) -> (&str, Option<&str>) {
    if let Some(pos) = url.find('#') {
        (&url[..pos], Some(&url[pos..]))
    } else {
        (url, None)
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
        } else {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                fence_char = Some('`');
                output_lines.push(line.to_string());
                continue;
            }
            if trimmed.starts_with("~~~") {
                fence_char = Some('~');
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
    // Walk by char to stay UTF-8 safe; ASCII delimiters always land on char boundaries so
    // slicing by byte index into `line` remains valid.
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut in_inline_code = false;
    let mut it = line.char_indices().peekable();

    while let Some((i, ch)) = it.next() {
        if ch == '`' {
            in_inline_code = !in_inline_code;
            out.push(ch);
            continue;
        }
        if in_inline_code {
            out.push(ch);
            continue;
        }
        // Skip images: `![...](...)` -- the entire span is left for markdown_refs.
        if ch == '!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // Try to parse the bracketed link span starting at i+1.
            if let Some((_text, _url, consumed)) = parse_link_at(&line[i + 1..]) {
                // Emit the full `![...](...)` span and advance past it.
                out.push_str(&line[i..i + 1 + consumed]);
                // Advance the iterator past the consumed bytes.
                while let Some(&(j, _)) = it.peek() {
                    if j < i + 1 + consumed {
                        it.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
            // Not a well-formed image span -- emit `!` and continue.
            out.push('!');
            continue;
        }
        if ch == '[' {
            if let Some((text, url, consumed)) = parse_link_at(&line[i..]) {
                let mut handled = false;
                if is_resolvable_target(url) {
                    let (path_part, frag) = split_fragment(url);
                    match resolve_reference(path_part, graph, from_path) {
                        ResolvedRef::Found(resolved) => {
                            let new_url = match frag {
                                Some(f) => format!("moss-resolved:{}{}", resolved, f),
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
                // Advance iterator past the consumed span.
                while let Some(&(j, _)) = it.peek() {
                    if j < i + consumed {
                        it.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(ch);
    }
    out
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
    fn fenced_block_skipped() {
        let g = graph_with(&["index.md", "文字/文字.md"]);
        let input = "```\n[文字](文字.md)\n```\n";
        let r = resolve_markdown_links(input, &g, "index.md");
        assert!(r.content.contains("[文字](文字.md)"));
        assert!(!r.content.contains("moss-resolved:"));
    }
}
