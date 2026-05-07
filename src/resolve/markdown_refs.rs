//! Bare-filename resolution for standard markdown image syntax.
//!
//! Scans markdown content for `![alt](url)` patterns where `url` is a
//! "bare filename" (no path separators, no protocol, no relative prefix)
//! and resolves them via the [`ContentGraph`](crate::content_graph::ContentGraph).
//!
//! This handles the case where users write `![](photo.jpg)` in their markdown
//! and the actual file lives at `assets/photo.jpg`. The bare filename is
//! resolved through the content graph's fuzzy matching, and the URL is
//! replaced with the correct relative path.
//!
//! Content inside fenced code blocks and inline code spans is left untouched.

use crate::content_graph::ContentGraph;
use crate::media::{format_img_tag, parse_media_attrs};

use super::fuzzy_path::{relative_asset_path, resolve_reference, ResolvedRef};
use super::{Diagnostic, LinkType, OutgoingLink};

/// Result of resolving bare-filename markdown image references.
#[derive(Debug)]
pub struct MarkdownRefResult {
    /// The transformed content with bare filenames replaced by resolved paths.
    pub content: String,
    /// Outgoing links found during resolution.
    pub outgoing_links: Vec<OutgoingLink>,
    /// Diagnostics (currently unused — unresolvable bare filenames are left as-is).
    pub diagnostics: Vec<Diagnostic>,
}

/// Check whether a URL string is a "bare filename" that should be resolved
/// via the ContentGraph.
///
/// A bare filename:
/// - Has no path separators (`/` or `\`)
/// - Has no `./` or `../` prefix
/// - Has no protocol (`http://`, `https://`, `//`, `data:`, `mailto:`)
/// - Is not a fragment-only reference (`#`)
/// - Has a file extension (contains `.` with non-empty suffix)
fn is_bare_filename(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }

    // Fragment-only
    if url.starts_with('#') {
        return false;
    }

    // Explicit relative prefix
    if url.starts_with("./") || url.starts_with("../") {
        return false;
    }

    // Protocol prefixes
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("//")
        || url.starts_with("data:")
        || url.starts_with("mailto:")
    {
        return false;
    }

    // Path separators
    if url.contains('/') || url.contains('\\') {
        return false;
    }

    // Must have a file extension (dot with non-empty suffix)
    if let Some(dot_pos) = url.rfind('.') {
        dot_pos > 0 && dot_pos < url.len() - 1
    } else {
        false
    }
}

/// Resolve bare-filename references in standard markdown image syntax.
///
/// Scans `content` for `![alt](url)` patterns. For each one where `url`
/// is a bare filename, resolves it via the ContentGraph and replaces the
/// URL with a relative path from the source file's parent directory to the
/// resolved target.
///
/// Content inside fenced code blocks and inline code spans is preserved
/// unchanged.
///
/// # Arguments
///
/// * `content`   - the markdown source text
/// * `graph`     - the content graph for path resolution
/// * `from_path` - the file containing the image references (for relative URLs)
pub fn resolve_markdown_refs(
    content: &str,
    graph: &ContentGraph,
    from_path: &str,
) -> MarkdownRefResult {
    let mut outgoing_links: Vec<OutgoingLink> = Vec::new();
    let diagnostics: Vec<Diagnostic> = Vec::new();
    let mut output_lines: Vec<String> = Vec::new();

    let mut fence_char: Option<char> = None;

    for line in content.lines() {
        // --- Fenced code block tracking ---
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
        let fence_rest = trimmed
            .strip_prefix("```")
            .map(|r| ('`', r))
            .or_else(|| trimmed.strip_prefix("~~~").map(|r| ('~', r)));
        if let Some((candidate_char, rest)) = fence_rest {
            if !rest.contains(candidate_char) {
                fence_char = Some(candidate_char);
                output_lines.push(line.to_string());
                continue;
            }
        }

        // --- Process line for markdown image refs, respecting inline code ---
        let transformed =
            process_line(line, graph, from_path, &mut outgoing_links);
        output_lines.push(transformed);
    }

    let mut output = output_lines.join("\n");

    // Restore trailing newline if the original had one.
    if content.ends_with('\n') {
        output.push('\n');
    }

    MarkdownRefResult {
        content: output,
        outgoing_links,
        diagnostics,
    }
}

/// Process a single line, resolving bare-filename markdown image references
/// while preserving inline code spans.
fn process_line(
    line: &str,
    graph: &ContentGraph,
    from_path: &str,
    outgoing_links: &mut Vec<OutgoingLink>,
) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // --- Inline code span: skip everything until closing backtick(s) ---
        if chars[i] == '`' {
            let backtick_count = count_char(&chars, i, '`');
            for _ in 0..backtick_count {
                result.push('`');
            }
            i += backtick_count;

            // Find closing sequence of the same length
            loop {
                if i >= len {
                    break;
                }
                if chars[i] == '`' {
                    let closing_count = count_char(&chars, i, '`');
                    if closing_count == backtick_count {
                        for _ in 0..closing_count {
                            result.push('`');
                        }
                        i += closing_count;
                        break;
                    } else {
                        for _ in 0..closing_count {
                            result.push('`');
                        }
                        i += closing_count;
                    }
                } else {
                    result.push(chars[i]);
                    i += 1;
                }
            }
            continue;
        }

        // --- Markdown image: ![alt](url) ---
        if chars[i] == '!' && i + 1 < len && chars[i + 1] == '[' {
            // Try to parse ![alt](url)
            if let Some((end, alt, url)) = parse_markdown_image(&chars, i) {
                let (path_part, attrs_str) = crate::media::split_pipe(&url);
                let attrs = parse_media_attrs(attrs_str);

                if is_bare_filename(path_part) {
                    match resolve_reference(path_part, graph, from_path) {
                        ResolvedRef::Found(target_path) => {
                            outgoing_links.push(OutgoingLink {
                                target_path: target_path.clone(),
                                display_text: alt.clone(),
                                link_type: LinkType::Standard,
                            });
                            let resolved_url = relative_asset_path(from_path, &target_path);
                            if !attrs.is_empty() {
                                result.push_str(&format_img_tag(&resolved_url, &alt, &attrs));
                            } else {
                                result.push_str(&format!("![{}]({})", alt, resolved_url));
                            }
                            i = end;
                            continue;
                        }
                        ResolvedRef::Unresolved => {
                            // Leave unchanged — could be a same-directory file
                            // But still apply attrs if present
                            if !attrs.is_empty() {
                                result.push_str(&format_img_tag(path_part, &alt, &attrs));
                            } else {
                                result.push_str(&format!("![{}]({})", alt, url));
                            }
                            i = end;
                            continue;
                        }
                    }
                } else {
                    // Not a bare filename
                    if !attrs.is_empty() {
                        // Has pipe attrs — output HTML with style
                        result.push_str(&format_img_tag(path_part, &alt, &attrs));
                    } else {
                        // No attrs — pass through unchanged (use original url to preserve any query/fragment)
                        result.push_str(&format!("![{}]({})", alt, url));
                    }
                    i = end;
                    continue;
                }
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Count consecutive occurrences of `ch` starting at position `start`.
fn count_char(chars: &[char], start: usize, ch: char) -> usize {
    chars[start..].iter().take_while(|&&c| c == ch).count()
}

/// Try to parse a markdown image starting at position `start` (the `!` char).
///
/// Expected syntax: `![alt](url)`
///
/// Returns `Some((end_position, alt_text, url))` or `None`.
fn parse_markdown_image(chars: &[char], start: usize) -> Option<(usize, String, String)> {
    let len = chars.len();

    // start should be '!', start+1 should be '['
    if start + 1 >= len || chars[start] != '!' || chars[start + 1] != '[' {
        return None;
    }

    // Find the closing ']' for alt text
    let mut j = start + 2;
    let mut depth = 1;
    while j < len && depth > 0 {
        if chars[j] == '[' {
            depth += 1;
        } else if chars[j] == ']' {
            depth -= 1;
        }
        if depth > 0 {
            j += 1;
        }
    }

    if depth != 0 {
        return None;
    }

    let alt: String = chars[start + 2..j].iter().collect();
    let after_bracket = j + 1;

    // Must be immediately followed by '('
    if after_bracket >= len || chars[after_bracket] != '(' {
        return None;
    }

    // Find the closing ')' for URL
    let mut k = after_bracket + 1;
    let mut paren_depth = 1;
    while k < len && paren_depth > 0 {
        if chars[k] == '(' {
            paren_depth += 1;
        } else if chars[k] == ')' {
            paren_depth -= 1;
        }
        if paren_depth > 0 {
            k += 1;
        }
    }

    if paren_depth != 0 {
        return None;
    }

    let url: String = chars[after_bracket + 1..k].iter().collect();
    Some((k + 1, alt, url))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;

    /// Build a graph with common test files for markdown ref tests.
    fn test_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("assets/photo.jpg", "photo");
        b.add_file(
            "assets/d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg",
            "uuid-photo",
        );
        b.add_file("assets/diagram.png", "diagram");
        b.add_file("guide.md", "guide");
        b.add_file("articles/intro.md", "intro");
        b.build()
    }

    // 1. Bare image filename resolves to correct relative path
    #[test]
    fn test_bare_image_resolves() {
        let graph = test_graph();
        let input = "![My Photo](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![My Photo](../assets/photo.jpg)");
    }

    // 2. UUID-style bare filename resolves correctly
    #[test]
    fn test_uuid_bare_filename_resolves() {
        let graph = test_graph();
        let input = "![](d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg)";
        let result = resolve_markdown_refs(
            input,
            &graph,
            "articles/\u{65e0}\u{7528}\u{4e4b}\u{65c5}/\u{771f}\u{6b63}\u{7684}\u{65c5}\u{7a0b}.md",
        );
        assert_eq!(
            result.content,
            "![](../../assets/d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg)"
        );
    }

    // 3. Deeply nested source file resolves to assets
    #[test]
    fn test_deeply_nested_source_resolves() {
        let graph = test_graph();
        let input = "![](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/sub/deep.md");
        assert_eq!(result.content, "![](../../assets/photo.jpg)");
    }

    // 4. Explicit relative path left unchanged
    #[test]
    fn test_explicit_relative_path_unchanged() {
        let graph = test_graph();
        let input = "![](./photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](./photo.jpg)");
    }

    // 5. Path with separator left unchanged
    #[test]
    fn test_path_with_separator_unchanged() {
        let graph = test_graph();
        let input = "![](assets/photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](assets/photo.jpg)");
    }

    // 6. External URL left unchanged
    #[test]
    fn test_external_url_unchanged() {
        let graph = test_graph();
        let input = "![](https://example.com/photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](https://example.com/photo.jpg)");
    }

    // 7. Data URI left unchanged
    #[test]
    fn test_data_uri_unchanged() {
        let graph = test_graph();
        let input = "![](data:image/png;base64,abc123)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](data:image/png;base64,abc123)");
    }

    // 8. Unresolvable bare filename left unchanged (no diagnostic)
    #[test]
    fn test_unresolvable_bare_filename_unchanged() {
        let graph = test_graph();
        let input = "![](nonexistent.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](nonexistent.jpg)");
        assert!(result.diagnostics.is_empty());
        assert!(result.outgoing_links.is_empty());
    }

    // 9. Code block content not processed
    #[test]
    fn test_code_block_not_processed() {
        let graph = test_graph();
        let input = "Before.\n\n```\n![](photo.jpg)\n```\n\nAfter ![](photo.jpg).";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        // Inside code block: unchanged
        assert!(result.content.contains("```\n![](photo.jpg)\n```"));
        // Outside code block: resolved
        assert!(result.content.contains("After ![](../assets/photo.jpg)."));
    }

    // 10. Inline code not processed
    #[test]
    fn test_inline_code_not_processed() {
        let graph = test_graph();
        let input = "Use `![](photo.jpg)` syntax.";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "Use `![](photo.jpg)` syntax.");
    }

    // 11. Alt text preserved
    #[test]
    fn test_alt_text_preserved() {
        let graph = test_graph();
        let input = "![My Photo](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![My Photo](../assets/photo.jpg)");
    }

    // 12. Multiple images on one line
    #[test]
    fn test_multiple_images_on_one_line() {
        let graph = test_graph();
        let input = "![a](photo.jpg) and ![b](diagram.png)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(
            result.content,
            "![a](../assets/photo.jpg) and ![b](../assets/diagram.png)"
        );
        assert_eq!(result.outgoing_links.len(), 2);
    }

    // 13. Empty alt text
    #[test]
    fn test_empty_alt_text() {
        let graph = test_graph();
        let input = "![](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](../assets/photo.jpg)");
    }

    // 14. Outgoing links tracked for resolved images
    #[test]
    fn test_outgoing_links_tracked() {
        let graph = test_graph();
        let input = "![pic](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.outgoing_links.len(), 1);
        assert_eq!(result.outgoing_links[0].target_path, "assets/photo.jpg");
        assert_eq!(result.outgoing_links[0].display_text, "pic");
        assert_eq!(result.outgoing_links[0].link_type, LinkType::Standard);
    }

    // --- Additional edge case tests ---

    #[test]
    fn test_tilde_fence_preserved() {
        let graph = test_graph();
        let input = "Before.\n\n~~~\n![](photo.jpg)\n~~~\n\nAfter ![](photo.jpg).";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert!(result.content.contains("~~~\n![](photo.jpg)\n~~~"));
        assert!(result.content.contains("After ![](../assets/photo.jpg)."));
    }

    #[test]
    fn test_double_backtick_inline_code() {
        let graph = test_graph();
        let input = "Use ``![](photo.jpg)`` syntax.";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "Use ``![](photo.jpg)`` syntax.");
    }

    #[test]
    fn test_protocol_relative_url_unchanged() {
        let graph = test_graph();
        let input = "![](//cdn.example.com/photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](//cdn.example.com/photo.jpg)");
    }

    #[test]
    fn test_mailto_unchanged() {
        let graph = test_graph();
        // Contrived but possible: mailto in an image URL
        let input = "![](mailto:test@example.com)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](mailto:test@example.com)");
    }

    #[test]
    fn test_fragment_only_unchanged() {
        let graph = test_graph();
        let input = "![](#section)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](#section)");
    }

    #[test]
    fn test_parent_relative_path_unchanged() {
        let graph = test_graph();
        let input = "![](../photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](../photo.jpg)");
    }

    #[test]
    fn test_no_extension_not_resolved() {
        let graph = test_graph();
        let input = "![](photo)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](photo)");
    }

    #[test]
    fn test_trailing_newline_preserved() {
        let graph = test_graph();
        let input = "![](photo.jpg)\n";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "![](../assets/photo.jpg)\n");
    }

    #[test]
    fn test_standard_markdown_links_not_affected() {
        let graph = test_graph();
        // Standard links [text](url) — NOT images — should be left alone
        let input = "[guide link](guide.md)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(result.content, "[guide link](guide.md)");
    }

    #[test]
    fn test_root_level_source_resolves() {
        let graph = test_graph();
        let input = "![](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "page.md");
        assert_eq!(result.content, "![](assets/photo.jpg)");
    }

    #[test]
    fn test_is_bare_filename_function() {
        // Should resolve
        assert!(is_bare_filename("photo.jpg"));
        assert!(is_bare_filename("d9512f2d-fdcf-4a22-b1d5-340f74ddedae.jpg"));
        assert!(is_bare_filename("file.png"));
        assert!(is_bare_filename("my-image.webp"));

        // Should NOT resolve
        assert!(!is_bare_filename(""));
        assert!(!is_bare_filename("#anchor"));
        assert!(!is_bare_filename("./photo.jpg"));
        assert!(!is_bare_filename("../photo.jpg"));
        assert!(!is_bare_filename("assets/photo.jpg"));
        assert!(!is_bare_filename("http://example.com/photo.jpg"));
        assert!(!is_bare_filename("https://example.com/photo.jpg"));
        assert!(!is_bare_filename("//cdn.example.com/photo.jpg"));
        assert!(!is_bare_filename("data:image/png;base64,abc"));
        assert!(!is_bare_filename("mailto:test@example.com"));
        assert!(!is_bare_filename("photo")); // no extension
        assert!(!is_bare_filename(".hidden")); // dot at position 0
    }

    // --- Pipe attr tests ---

    #[test]
    fn test_bare_filename_with_contain_attr() {
        let graph = test_graph();
        let input = "![](photo.jpg|contain)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"\" style=\"object-fit:contain\" />"
        );
    }

    #[test]
    fn test_bare_filename_with_position_attr() {
        let graph = test_graph();
        let input = "![My Photo](photo.jpg|left)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"My Photo\" style=\"object-position:left\" />"
        );
    }

    #[test]
    fn test_bare_filename_with_fit_and_position() {
        let graph = test_graph();
        let input = "![](photo.jpg|contain left)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"\" style=\"object-fit:contain;object-position:left\" />"
        );
    }

    #[test]
    fn test_bare_filename_no_attrs_unchanged() {
        let graph = test_graph();
        let input = "![](photo.jpg)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        // Same as before — no pipe, no change
        assert_eq!(result.content, "![](../assets/photo.jpg)");
    }

    #[test]
    fn test_relative_path_with_attrs() {
        let graph = test_graph();
        let input = "![](./photo.jpg|left)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        // Not bare filename, but has attrs — output HTML
        assert_eq!(
            result.content,
            "<img src=\"./photo.jpg\" alt=\"\" style=\"object-position:left\" />"
        );
    }

    #[test]
    fn test_path_with_separator_and_attrs() {
        let graph = test_graph();
        let input = "![](assets/photo.jpg|contain)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(
            result.content,
            "<img src=\"assets/photo.jpg\" alt=\"\" style=\"object-fit:contain\" />"
        );
    }

    #[test]
    fn test_external_url_with_attrs_unchanged() {
        let graph = test_graph();
        let input = "![](https://example.com/photo.jpg|left)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        // External URL with pipe — apply attrs
        assert_eq!(
            result.content,
            "<img src=\"https://example.com/photo.jpg\" alt=\"\" style=\"object-position:left\" />"
        );
    }

    #[test]
    fn test_unresolved_bare_with_attrs() {
        let graph = test_graph();
        let input = "![](nonexistent.jpg|left)";
        let result = resolve_markdown_refs(input, &graph, "articles/post.md");
        assert_eq!(
            result.content,
            "<img src=\"nonexistent.jpg\" alt=\"\" style=\"object-position:left\" />"
        );
    }
}
