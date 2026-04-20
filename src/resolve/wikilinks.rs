//! Wikilink parsing and resolution.
//!
//! Transforms Obsidian-style `[[wikilinks]]` and `![[embeds]]` in markdown
//! content into standard markdown links, resolving targets against a
//! [`ContentGraph`](crate::content_graph::ContentGraph).
//!
//! # Supported Syntax
//!
//! | Input | Output |
//! |-------|--------|
//! | `[[target]]` | `[target](moss-resolved:target.md)` |
//! | `[[target\|alias]]` | `[alias](moss-resolved:target.md)` |
//! | `[[target#heading]]` | `[target > heading](moss-resolved:target.md#anchor)` |
//! | `[[target#^block-id]]` | `[target > ^block-id](moss-resolved:target.md#block-id)` |
//! | `![[image.png]]` | `![image](resolved/url/image.png)` |
//! | `![[file.md]]` | `<!-- moss-embed:resolved/path.md -->` |
//!
//! Wikilinks inside fenced code blocks and inline code spans are preserved
//! unchanged.

use crate::content_graph::ContentGraph;
use crate::heading_anchor::obsidian_heading_anchor;
use crate::media::{format_img_tag, is_all_display_keywords, parse_media_attrs};

use super::fuzzy_path::{relative_asset_path, resolve_reference, ResolvedRef};
use super::{Diagnostic, LinkType, OutgoingLink};

/// Image file extensions recognized for embed syntax (`![[…]]`).
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// Result of resolving all wikilinks in a document.
#[derive(Debug)]
pub struct WikilinkResult {
    /// The transformed content with wikilinks replaced by standard markdown.
    pub content: String,
    /// All outgoing links found in the document.
    pub outgoing_links: Vec<OutgoingLink>,
    /// Diagnostics for unresolved references.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse and resolve all wikilinks and embeds in `content`.
///
/// Scans for `[[…]]` and `![[…]]` patterns, resolves targets against `graph`,
/// and replaces them with standard markdown links.  Content inside fenced code
/// blocks and inline code spans is left untouched.
///
/// # Arguments
///
/// * `content`   — the markdown source text
/// * `graph`     — the content graph for path resolution
/// * `from_path` — the file containing the wikilinks (for relative URLs)
pub fn resolve_wikilinks(
    content: &str,
    graph: &ContentGraph,
    from_path: &str,
) -> WikilinkResult {
    let mut outgoing_links: Vec<OutgoingLink> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut output_lines: Vec<String> = Vec::new();

    let mut in_fence = false;
    let mut fence_char = ' ';

    for line in content.lines() {
        // --- Fenced code block tracking ---
        if in_fence {
            let trimmed = line.trim_start();
            let closes = trimmed.starts_with(fence_char)
                && trimmed.chars().take(3).all(|c| c == fence_char)
                && trimmed.trim_matches(fence_char).trim().is_empty();
            if closes {
                in_fence = false;
            }
            output_lines.push(line.to_string());
            continue;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let candidate_char = trimmed.chars().next().unwrap();
            let rest = &trimmed[3..];
            if !rest.contains(candidate_char) {
                fence_char = candidate_char;
                in_fence = true;
                output_lines.push(line.to_string());
                continue;
            }
        }

        // --- Process line for wikilinks, respecting inline code ---
        let transformed = process_line(line, graph, from_path, &mut outgoing_links, &mut diagnostics);
        output_lines.push(transformed);
    }

    let mut output = output_lines.join("\n");

    // Restore trailing newline if the original had one.
    if content.ends_with('\n') {
        output.push('\n');
    }

    WikilinkResult {
        content: output,
        outgoing_links,
        diagnostics,
    }
}

/// Process a single line, replacing wikilinks while preserving inline code.
fn process_line(
    line: &str,
    graph: &ContentGraph,
    from_path: &str,
    outgoing_links: &mut Vec<OutgoingLink>,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // --- Inline code span: skip everything until closing backtick(s) ---
        if chars[i] == '`' {
            let backtick_count = count_char(&chars, i, '`');
            // Push the opening backticks
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
                        // Not a matching close, just push the backticks
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

        // --- Embed wikilink: ![[…]] ---
        if chars[i] == '!' && i + 3 < len && chars[i + 1] == '[' && chars[i + 2] == '[' {
            if let Some((end, inner)) = find_wikilink_close(&chars, i + 3) {
                let replacement = resolve_embed(
                    &inner,
                    graph,
                    from_path,
                    outgoing_links,
                    diagnostics,
                );
                result.push_str(&replacement);
                i = end;
                continue;
            }
        }

        // --- Standard wikilink: [[…]] ---
        if chars[i] == '[' && i + 1 < len && chars[i + 1] == '[' {
            if let Some((end, inner)) = find_wikilink_close(&chars, i + 2) {
                let replacement = resolve_wikilink(
                    &inner,
                    graph,
                    from_path,
                    outgoing_links,
                    diagnostics,
                );
                result.push_str(&replacement);
                i = end;
                continue;
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

/// Find the closing `]]` starting from position `start`.
///
/// Returns `Some((position_after_close, inner_text))` or `None` if no `]]`
/// is found before end-of-line.
fn find_wikilink_close(chars: &[char], start: usize) -> Option<(usize, String)> {
    let mut j = start;
    let len = chars.len();
    while j + 1 < len {
        if chars[j] == ']' && chars[j + 1] == ']' {
            let inner: String = chars[start..j].iter().collect();
            return Some((j + 2, inner));
        }
        j += 1;
    }
    None
}

/// Parse wikilink inner text into (file_part, section, alias).
///
/// Syntax: `file#section|alias` — both `#section` and `|alias` are optional.
/// The section may start with `^` for block references.
fn parse_wikilink_inner(inner: &str) -> (&str, Option<&str>, Option<&str>) {
    // Split on `|` first (alias separator). Obsidian uses the first `|`.
    let (before_pipe, alias) = match inner.find('|') {
        Some(pos) => (&inner[..pos], Some(&inner[pos + 1..])),
        None => (inner, None),
    };

    // Split on `#` (section separator). Use the first `#`.
    let (file_part, section) = match before_pipe.find('#') {
        Some(pos) => (&before_pipe[..pos], Some(&before_pipe[pos + 1..])),
        None => (before_pipe, None),
    };

    (file_part, section, alias)
}

/// Check whether a path has an image extension.
fn is_image_path(path: &str) -> bool {
    if let Some(dot_pos) = path.rfind('.') {
        let ext = &path[dot_pos + 1..];
        IMAGE_EXTENSIONS.iter().any(|&e| e.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

/// Resolve a standard `[[…]]` wikilink and return its markdown replacement.
fn resolve_wikilink(
    inner: &str,
    graph: &ContentGraph,
    from_path: &str,
    outgoing_links: &mut Vec<OutgoingLink>,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let (file_part, section, alias) = parse_wikilink_inner(inner);

    // Build display text
    let display_text = if let Some(a) = alias {
        a.to_string()
    } else if let Some(sec) = section {
        if file_part.is_empty() {
            // Same-file section link: [[#heading]]
            format!("{}", sec)
        } else {
            format!("{} > {}", file_part, sec)
        }
    } else {
        file_part.to_string()
    };

    // Resolve the file part (if non-empty)
    let resolved = if file_part.is_empty() {
        // Same-file section link
        ResolvedRef::Found(from_path.to_string())
    } else {
        resolve_reference(file_part, graph, from_path)
    };

    match resolved {
        ResolvedRef::Found(target_path) => {
            outgoing_links.push(OutgoingLink {
                target_path: target_path.clone(),
                display_text: display_text.clone(),
                link_type: LinkType::Wikilink,
            });

            let anchor = build_anchor(section);

            if file_part.is_empty() {
                // Same-file link: use just the anchor (e.g. `#heading`)
                format!("[{}]({})", display_text, anchor)
            } else {
                format!("[{}](moss-resolved:{}{})", display_text, target_path, anchor)
            }
        }
        ResolvedRef::Unresolved => {
            diagnostics.push(Diagnostic {
                message: format!("Unresolved wikilink: [[{}]]", inner),
                source_path: from_path.to_string(),
                reference: inner.to_string(),
            });

            outgoing_links.push(OutgoingLink {
                target_path: file_part.to_string(),
                display_text: display_text.clone(),
                link_type: LinkType::Wikilink,
            });

            format!("[{}](moss-unresolved:{})", display_text, file_part)
        }
    }
}

/// Resolve an embed `![[…]]` and return its markdown replacement.
fn resolve_embed(
    inner: &str,
    graph: &ContentGraph,
    from_path: &str,
    outgoing_links: &mut Vec<OutgoingLink>,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let (file_part, section, alias) = parse_wikilink_inner(inner);

    let resolved = if file_part.is_empty() {
        ResolvedRef::Found(from_path.to_string())
    } else {
        resolve_reference(file_part, graph, from_path)
    };

    match resolved {
        ResolvedRef::Found(target_path) => {
            outgoing_links.push(OutgoingLink {
                target_path: target_path.clone(),
                display_text: file_part.to_string(),
                link_type: LinkType::Embed,
            });

            if is_image_path(&target_path) {
                let url = relative_asset_path(from_path, &target_path);
                match alias {
                    Some(alias_text) if is_all_display_keywords(alias_text) => {
                        // Alias is entirely display keywords → output raw HTML <img> with style.
                        let alt = file_stem(&target_path);
                        let attrs = parse_media_attrs(alias_text);
                        format_img_tag(&url, &alt, &attrs)
                    }
                    Some(alias_text) => {
                        // Alias is plain text → use as alt text.
                        format!("![{}]({})", alias_text, url)
                    }
                    None => {
                        // No alias → use filename stem as alt (existing behavior).
                        let alt = file_stem(&target_path);
                        format!("![{}]({})", alt, url)
                    }
                }
            } else {
                // Markdown embed: produce <!-- moss-embed:path#anchor -->
                // For embeds, preserve ^block-id so the embed resolver can
                // distinguish block refs from heading refs.
                let anchor = build_embed_anchor(section);
                format!("<!-- moss-embed:{}{} -->", target_path, anchor)
            }
        }
        ResolvedRef::Unresolved => {
            diagnostics.push(Diagnostic {
                message: format!("Unresolved embed: ![[{}]]", inner),
                source_path: from_path.to_string(),
                reference: inner.to_string(),
            });

            outgoing_links.push(OutgoingLink {
                target_path: file_part.to_string(),
                display_text: file_part.to_string(),
                link_type: LinkType::Embed,
            });

            format!("[{}](moss-unresolved:{})", file_part, file_part)
        }
    }
}

/// Build the anchor fragment (e.g. `#getting-started` or `#block-id`) from
/// a section reference.
fn build_anchor(section: Option<&str>) -> String {
    match section {
        None => String::new(),
        Some(s) if s.is_empty() => String::new(),
        Some(s) => {
            if let Some(block_id) = s.strip_prefix('^') {
                // Block reference: use as-is (no slug transform)
                format!("#{}", block_id)
            } else {
                // Heading reference: generate Obsidian-compatible anchor
                format!("#{}", obsidian_heading_anchor(s))
            }
        }
    }
}

/// Build the anchor fragment for an embed marker.
///
/// Unlike [`build_anchor`] (used for links), this preserves the `^` prefix
/// on block references so the embed resolver can distinguish them from headings.
fn build_embed_anchor(section: Option<&str>) -> String {
    match section {
        None => String::new(),
        Some(s) if s.is_empty() => String::new(),
        Some(s) => {
            if s.starts_with('^') {
                // Block reference: preserve ^prefix for embed resolver
                format!("#{}", s)
            } else {
                // Heading reference: generate Obsidian-compatible anchor
                format!("#{}", obsidian_heading_anchor(s))
            }
        }
    }
}

/// Extract the filename stem from a path (no directory, no extension).
fn file_stem(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    match filename.rfind('.') {
        Some(pos) if pos > 0 => filename[..pos].to_string(),
        _ => filename.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;

    /// Build a graph with common test files for wikilink tests.
    fn test_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("guide.md", "guide");
        b.add_file("posts/hello.md", "hello");
        b.add_file("notes/deep/secret.md", "secret");
        b.add_file("assets/photo.jpg", "photo");
        b.add_file("disclaimer.md", "disclaimer");
        b.add_headings(
            "guide.md",
            vec![("Getting Started".into(), "getting-started".into())],
        );
        b.add_blocks("guide.md", vec!["setup-step".into()]);
        b.build()
    }

    // 1. Basic wikilink
    #[test]
    fn test_basic_wikilink() {
        let graph = test_graph();
        let result = resolve_wikilinks("See [[guide]] for details.", &graph, "posts/hello.md");
        assert_eq!(result.content, "See [guide](moss-resolved:guide.md) for details.");
        assert!(result.diagnostics.is_empty());
    }

    // 2. Wikilink with alias
    #[test]
    fn test_wikilink_with_alias() {
        let graph = test_graph();
        let result = resolve_wikilinks("Read [[guide|the guide]].", &graph, "posts/hello.md");
        assert_eq!(result.content, "Read [the guide](moss-resolved:guide.md).");
    }

    // 3. Wikilink with heading
    #[test]
    fn test_wikilink_with_heading() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("See [[guide#Getting Started]].", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "See [guide > Getting Started](moss-resolved:guide.md#getting-started)."
        );
    }

    // 4. Wikilink with block reference
    #[test]
    fn test_wikilink_with_block_ref() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("See [[guide#^setup-step]].", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "See [guide > ^setup-step](moss-resolved:guide.md#setup-step)."
        );
    }

    // 5. Wikilink with heading and alias
    #[test]
    fn test_wikilink_heading_and_alias() {
        let graph = test_graph();
        let result = resolve_wikilinks(
            "See [[guide#Getting Started|setup]].",
            &graph,
            "posts/hello.md",
        );
        assert_eq!(
            result.content,
            "See [setup](moss-resolved:guide.md#getting-started)."
        );
    }

    // 6. Unresolved wikilink
    #[test]
    fn test_unresolved_wikilink() {
        let graph = test_graph();
        let result = resolve_wikilinks("See [[nonexistent]].", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "See [nonexistent](moss-unresolved:nonexistent)."
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].reference, "nonexistent");
        assert_eq!(result.diagnostics[0].source_path, "posts/hello.md");
    }

    // 7. Wikilink in code block preserved
    #[test]
    fn test_wikilink_in_code_block_preserved() {
        let graph = test_graph();
        let input = "Before.\n\n```\n[[guide]]\n```\n\nAfter [[guide]].";
        let result = resolve_wikilinks(input, &graph, "posts/hello.md");
        assert!(result.content.contains("```\n[[guide]]\n```"));
        assert!(result.content.contains("After [guide](moss-resolved:guide.md)."));
    }

    // 8. Wikilink in inline code preserved
    #[test]
    fn test_wikilink_in_inline_code_preserved() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("Use `[[guide]]` syntax.", &graph, "posts/hello.md");
        assert_eq!(result.content, "Use `[[guide]]` syntax.");
    }

    // 9. Multiple wikilinks on one line
    #[test]
    fn test_multiple_wikilinks_one_line() {
        let graph = test_graph();
        let result = resolve_wikilinks(
            "See [[guide]] and [[disclaimer]].",
            &graph,
            "posts/hello.md",
        );
        assert_eq!(
            result.content,
            "See [guide](moss-resolved:guide.md) and [disclaimer](moss-resolved:disclaimer.md)."
        );
        assert_eq!(result.outgoing_links.len(), 2);
    }

    // 10. Image embed
    #[test]
    fn test_image_embed() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg]]", &graph, "posts/hello.md");
        assert_eq!(result.content, "![photo](../assets/photo.jpg)");
    }

    // 10a. Image embed with position keyword → raw HTML <img> with style
    #[test]
    fn test_image_embed_position_keyword() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg|left]]", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"photo\" style=\"object-position:left\" />"
        );
    }

    // 10b. Image embed with fit keyword → raw HTML <img> with style
    #[test]
    fn test_image_embed_fit_keyword() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg|contain]]", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"photo\" style=\"object-fit:contain\" />"
        );
    }

    // 10c. Image embed with fit + position keywords → combined style
    #[test]
    fn test_image_embed_fit_and_position_keywords() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg|contain left]]", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"photo\" style=\"object-fit:contain;object-position:left\" />"
        );
    }

    // 10d. Image embed with two-word position keyword → combined style
    #[test]
    fn test_image_embed_two_word_position_keyword() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg|top left]]", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "<img src=\"../assets/photo.jpg\" alt=\"photo\" style=\"object-position:top left\" />"
        );
    }

    // 10e. Image embed with non-keyword alias → plain markdown with alias as alt text
    #[test]
    fn test_image_embed_alt_text() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg|A beautiful sunset]]", &graph, "posts/hello.md");
        assert_eq!(result.content, "![A beautiful sunset](../assets/photo.jpg)");
    }

    // 10f. Image embed with mixed alias (one known + one unknown token) → alt text
    #[test]
    fn test_image_embed_mixed_alias_as_alt_text() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[photo.jpg|left side]]", &graph, "posts/hello.md");
        assert_eq!(result.content, "![left side](../assets/photo.jpg)");
    }

    // 11. Markdown embed
    #[test]
    fn test_markdown_embed() {
        let graph = test_graph();
        let result = resolve_wikilinks("![[disclaimer]]", &graph, "posts/hello.md");
        assert_eq!(result.content, "<!-- moss-embed:disclaimer.md -->");
    }

    // 12. Heading-scoped embed
    #[test]
    fn test_heading_scoped_embed() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("![[guide#Getting Started]]", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "<!-- moss-embed:guide.md#getting-started -->"
        );
    }

    // 12b. Block-ref-scoped embed (preserves ^ for embed resolver)
    #[test]
    fn test_block_ref_scoped_embed() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("![[guide#^def-stem]]", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "<!-- moss-embed:guide.md#^def-stem -->"
        );
    }

    // 13. Outgoing links tracked
    #[test]
    fn test_outgoing_links_tracked() {
        let graph = test_graph();
        let result = resolve_wikilinks(
            "[[guide]] and ![[photo.jpg]]",
            &graph,
            "posts/hello.md",
        );
        assert_eq!(result.outgoing_links.len(), 2);

        let wikilink = &result.outgoing_links[0];
        assert_eq!(wikilink.target_path, "guide.md");
        assert_eq!(wikilink.link_type, LinkType::Wikilink);

        let embed = &result.outgoing_links[1];
        assert_eq!(embed.target_path, "assets/photo.jpg");
        assert_eq!(embed.link_type, LinkType::Embed);
    }

    // 14. No wikilinks — plain markdown unchanged
    #[test]
    fn test_no_wikilinks() {
        let graph = test_graph();
        let input = "Just a plain paragraph.\n\nAnother paragraph.";
        let result = resolve_wikilinks(input, &graph, "posts/hello.md");
        assert_eq!(result.content, input);
        assert!(result.outgoing_links.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    // -- Additional edge case tests --

    #[test]
    fn test_tilde_fence_preserved() {
        let graph = test_graph();
        let input = "Before.\n\n~~~\n[[guide]]\n~~~\n\nAfter [[guide]].";
        let result = resolve_wikilinks(input, &graph, "posts/hello.md");
        assert!(result.content.contains("~~~\n[[guide]]\n~~~"));
        assert!(result.content.contains("After [guide](moss-resolved:guide.md)."));
    }

    #[test]
    fn test_unclosed_wikilink_preserved() {
        let graph = test_graph();
        let result = resolve_wikilinks("See [[unclosed link.", &graph, "posts/hello.md");
        assert_eq!(result.content, "See [[unclosed link.");
        assert!(result.outgoing_links.is_empty());
    }

    #[test]
    fn test_empty_wikilink() {
        let graph = test_graph();
        let result = resolve_wikilinks("See [[]].", &graph, "posts/hello.md");
        // Empty inner text: file_part is empty, no section, produces same-file link
        assert_eq!(result.content, "See []().");
    }

    #[test]
    fn test_parse_wikilink_inner_basic() {
        assert_eq!(parse_wikilink_inner("guide"), ("guide", None, None));
    }

    #[test]
    fn test_parse_wikilink_inner_with_section() {
        assert_eq!(
            parse_wikilink_inner("guide#heading"),
            ("guide", Some("heading"), None)
        );
    }

    #[test]
    fn test_parse_wikilink_inner_with_alias() {
        assert_eq!(
            parse_wikilink_inner("guide|my alias"),
            ("guide", None, Some("my alias"))
        );
    }

    #[test]
    fn test_parse_wikilink_inner_full() {
        assert_eq!(
            parse_wikilink_inner("guide#heading|my alias"),
            ("guide", Some("heading"), Some("my alias"))
        );
    }

    #[test]
    fn test_is_image_path_detection() {
        assert!(is_image_path("photo.png"));
        assert!(is_image_path("photo.PNG"));
        assert!(is_image_path("dir/photo.jpg"));
        assert!(is_image_path("photo.jpeg"));
        assert!(is_image_path("photo.gif"));
        assert!(is_image_path("photo.svg"));
        assert!(is_image_path("photo.webp"));
        assert!(!is_image_path("file.md"));
        assert!(!is_image_path("file.pdf"));
        assert!(!is_image_path("noextension"));
    }

    #[test]
    fn test_trailing_newline_preserved() {
        let graph = test_graph();
        let input = "See [[guide]].\n";
        let result = resolve_wikilinks(input, &graph, "posts/hello.md");
        assert_eq!(result.content, "See [guide](moss-resolved:guide.md).\n");
    }

    #[test]
    fn test_double_backtick_inline_code() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("Use ``[[guide]]`` syntax.", &graph, "posts/hello.md");
        assert_eq!(result.content, "Use ``[[guide]]`` syntax.");
    }

    // -- is_all_display_keywords tests --

    #[test]
    fn test_is_all_display_keywords_single_position() {
        assert!(is_all_display_keywords("left"));
        assert!(is_all_display_keywords("right"));
        assert!(is_all_display_keywords("center"));
        assert!(is_all_display_keywords("top"));
        assert!(is_all_display_keywords("bottom"));
    }

    #[test]
    fn test_is_all_display_keywords_single_fit() {
        assert!(is_all_display_keywords("cover"));
        assert!(is_all_display_keywords("contain"));
        assert!(is_all_display_keywords("fill"));
        assert!(is_all_display_keywords("none"));
        assert!(is_all_display_keywords("scale-down"));
    }

    #[test]
    fn test_is_all_display_keywords_combined() {
        assert!(is_all_display_keywords("contain left"));
        assert!(is_all_display_keywords("cover right"));
    }

    #[test]
    fn test_is_all_display_keywords_two_word_position() {
        assert!(is_all_display_keywords("top left"));
        assert!(is_all_display_keywords("bottom right"));
    }

    #[test]
    fn test_is_all_display_keywords_not_keywords() {
        assert!(!is_all_display_keywords("A beautiful sunset"));
        assert!(!is_all_display_keywords("left side")); // "side" is unknown
        assert!(!is_all_display_keywords("my photo caption"));
    }

    #[test]
    fn test_is_all_display_keywords_empty() {
        assert!(!is_all_display_keywords(""));
        assert!(!is_all_display_keywords("   "));
    }

    // -- Language-tree aware wikilink resolution (Task 2.4) --

    /// Build a content graph from a simple filename -> content map.
    /// Used by language-tree tests below.
    fn build_graph_for_test(files: &std::collections::HashMap<String, String>) -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        for path in files.keys() {
            let slug = path.trim_end_matches(".md").to_string();
            b.add_file(path, &slug);
        }
        b.build()
    }

    #[test]
    fn test_wikilink_prefers_same_language_tree() {
        // Two footer.md files: one at root (EN), one under zh-hans/ (中文)
        // ![[footer]] from zh-hans/about.md should resolve to zh-hans/footer.md
        let mut files = std::collections::HashMap::new();
        files.insert("footer.md".to_string(), "EN footer content".to_string());
        files.insert("zh-hans/footer.md".to_string(), "ZH footer content".to_string());
        files.insert(
            "zh-hans/about.md".to_string(),
            "About\n\n![[footer]]".to_string(),
        );

        let graph = build_graph_for_test(&files);
        let result = resolve_wikilinks(
            &files["zh-hans/about.md"],
            &graph,
            "zh-hans/about.md",
        );

        assert!(
            result.content.contains("moss-embed:zh-hans/footer.md"),
            "Expected zh-hans/footer.md embed; got {}",
            result.content
        );
    }

    #[test]
    fn test_wikilink_falls_back_when_no_same_tree_match() {
        // Only one footer.md at root; zh-hans page's ![[footer]] falls back.
        let mut files = std::collections::HashMap::new();
        files.insert("footer.md".to_string(), "EN footer".to_string());
        files.insert(
            "zh-hans/about.md".to_string(),
            "About\n\n![[footer]]".to_string(),
        );

        let graph = build_graph_for_test(&files);
        let result = resolve_wikilinks(
            &files["zh-hans/about.md"],
            &graph,
            "zh-hans/about.md",
        );

        assert!(
            result.content.contains("moss-embed:footer.md"),
            "Expected fallback to root footer.md; got {}",
            result.content
        );
    }

    #[test]
    fn test_wikilink_root_prefers_root() {
        // When the source is at root, it prefers root-level siblings,
        // not a lang-prefixed namesake.
        let mut files = std::collections::HashMap::new();
        files.insert("footer.md".to_string(), "EN".to_string());
        files.insert("zh-hans/footer.md".to_string(), "ZH".to_string());
        files.insert("index.md".to_string(), "![[footer]]".to_string());

        let graph = build_graph_for_test(&files);
        let result = resolve_wikilinks(&files["index.md"], &graph, "index.md");

        assert!(
            result.content.contains("moss-embed:footer.md"),
            "Root source should resolve to root footer.md; got {}",
            result.content
        );
        assert!(
            !result.content.contains("moss-embed:zh-hans/footer.md"),
            "Should NOT prefer zh-hans sibling from root source; got {}",
            result.content
        );
    }

    #[test]
    fn test_explicit_path_overrides_language_preference() {
        // ![[zh-hans/footer]] from root should still resolve to zh-hans/footer.md
        // even though the root-tree preference would default to footer.md.
        let mut files = std::collections::HashMap::new();
        files.insert("footer.md".to_string(), "root".to_string());
        files.insert("zh-hans/footer.md".to_string(), "zh-hans version".to_string());
        files.insert(
            "index.md".to_string(),
            "![[zh-hans/footer]]".to_string(),
        );

        let graph = build_graph_for_test(&files);
        let result = resolve_wikilinks(&files["index.md"], &graph, "index.md");

        assert!(
            result.content.contains("moss-embed:zh-hans/footer.md"),
            "Explicit paths should be honored; got {}",
            result.content
        );
    }
}
