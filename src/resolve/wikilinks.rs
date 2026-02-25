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
//! | `[[target]]` | `[target](resolved/url/)` |
//! | `[[target\|alias]]` | `[alias](resolved/url/)` |
//! | `[[target#heading]]` | `[target > heading](resolved/url/#anchor)` |
//! | `[[target#^block-id]]` | `[target > ^block-id](resolved/url/#block-id)` |
//! | `![[image.png]]` | `![image](resolved/url/image.png)` |
//! | `![[file.md]]` | `<!-- moss-embed:resolved/path.md -->` |
//!
//! Wikilinks inside fenced code blocks and inline code spans are preserved
//! unchanged.

use crate::content_graph::ContentGraph;
use crate::heading_anchor::obsidian_heading_anchor;

use super::fuzzy_path::{relative_url, resolve_reference, ResolvedRef};
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

/// Compute the relative URL for a non-pretty-URL asset (images, etc.).
///
/// `relative_url` applies pretty URL formatting (strips extension, adds `/`),
/// which is wrong for binary assets. This function computes a raw relative
/// path preserving the filename and extension.
fn relative_asset_url(from_path: &str, to_path: &str) -> String {
    let from_dir = parent_dir(from_path);
    let from_parts: Vec<&str> = if from_dir.is_empty() {
        vec![]
    } else {
        from_dir.split('/').collect()
    };

    let to_parts: Vec<&str> = if to_path.is_empty() {
        vec![]
    } else {
        to_path.split('/').collect()
    };

    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let ups = from_parts.len() - common;
    let remaining = &to_parts[common..];

    let mut result = String::new();
    for _ in 0..ups {
        result.push_str("../");
    }
    for (i, part) in remaining.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        result.push_str(part);
    }

    if result.is_empty() {
        // Same directory, just the filename
        to_path.rsplit('/').next().unwrap_or(to_path).to_string()
    } else {
        result
    }
}

/// Extract the parent directory from a path.
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(pos) => &path[..pos],
        None => "",
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

            let url = relative_url(from_path, &target_path);
            let anchor = build_anchor(section);
            format!("[{}]({}{})", display_text, url, anchor)
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
    let (file_part, section, _alias) = parse_wikilink_inner(inner);

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
                // Image embed: produce ![alt](url)
                let alt = file_stem(&target_path);
                let url = relative_asset_url(from_path, &target_path);
                format!("![{}]({})", alt, url)
            } else {
                // Markdown embed: produce <!-- moss-embed:path -->
                let anchor = build_anchor(section);
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
        assert_eq!(result.content, "See [guide](../guide/) for details.");
        assert!(result.diagnostics.is_empty());
    }

    // 2. Wikilink with alias
    #[test]
    fn test_wikilink_with_alias() {
        let graph = test_graph();
        let result = resolve_wikilinks("Read [[guide|the guide]].", &graph, "posts/hello.md");
        assert_eq!(result.content, "Read [the guide](../guide/).");
    }

    // 3. Wikilink with heading
    #[test]
    fn test_wikilink_with_heading() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("See [[guide#Getting Started]].", &graph, "posts/hello.md");
        assert_eq!(
            result.content,
            "See [guide > Getting Started](../guide/#getting-started)."
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
            "See [guide > ^setup-step](../guide/#setup-step)."
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
            "See [setup](../guide/#getting-started)."
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
        assert!(result.content.contains("After [guide](../guide/)."));
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
            "See [guide](../guide/) and [disclaimer](../disclaimer/)."
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
        assert!(result.content.contains("After [guide](../guide/)."));
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
        // Empty inner text resolves to from_path (same-file link via relative_url)
        assert_eq!(result.content, "See [](hello/).");
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
        assert_eq!(result.content, "See [guide](../guide/).\n");
    }

    #[test]
    fn test_double_backtick_inline_code() {
        let graph = test_graph();
        let result =
            resolve_wikilinks("Use ``[[guide]]`` syntax.", &graph, "posts/hello.md");
        assert_eq!(result.content, "Use ``[[guide]]`` syntax.");
    }
}
