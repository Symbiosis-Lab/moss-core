//! Embed (transclusion) resolution.
//!
//! Resolves `<!-- moss-embed:TARGET -->` placeholders produced by the wikilink
//! resolver into inlined file content.  Supports full-file and heading-scoped
//! embeds, recursive embedding with cycle detection, and frontmatter stripping.
//!
//! This module is pure Rust with zero I/O — the caller supplies a `file_reader`
//! closure that maps a relative path to file content.

use std::collections::HashSet;

use crate::heading_anchor::obsidian_heading_anchor;

use super::Diagnostic;

/// Maximum recursion depth for nested embeds.
const MAX_EMBED_DEPTH: usize = 10;

/// Prefix for resolved embed markers.
const EMBED_PREFIX: &str = "<!-- moss-embed:";
/// Prefix for unresolved embed markers (left as-is).
const EMBED_UNRESOLVED_PREFIX: &str = "<!-- moss-embed-unresolved:";
/// Suffix for all embed markers.
const EMBED_SUFFIX: &str = " -->";

/// The result of resolving embed placeholders in a document.
pub struct EmbedResult {
    /// The content with embed placeholders replaced by inlined file content.
    pub content: String,
    /// Diagnostics produced during resolution (missing files, cycles, etc.).
    pub diagnostics: Vec<Diagnostic>,
    /// `(target_path, source_path)` pairs for watch-mode invalidation.
    pub embed_deps: Vec<(String, String)>,
}

/// Resolve embed placeholders by inlining target file content.
///
/// `file_reader` is a closure that reads a file by its relative path.
/// This keeps moss-core I/O-free — the caller provides the reader.
///
/// `visited` tracks paths in the current embed chain for cycle detection.
pub fn resolve_embeds(
    content: &str,
    from_path: &str,
    file_reader: &dyn Fn(&str) -> Option<String>,
    visited: &mut HashSet<String>,
) -> EmbedResult {
    resolve_embeds_inner(content, from_path, file_reader, visited, 0)
}

/// Inner recursive implementation with depth tracking.
fn resolve_embeds_inner(
    content: &str,
    from_path: &str,
    file_reader: &dyn Fn(&str) -> Option<String>,
    visited: &mut HashSet<String>,
    depth: usize,
) -> EmbedResult {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut embed_deps: Vec<(String, String)> = Vec::new();
    let mut output = String::with_capacity(content.len());

    for line in content.lines() {
        let trimmed = line.trim();

        // Leave unresolved markers as-is.
        if trimmed.starts_with(EMBED_UNRESOLVED_PREFIX) {
            output.push_str(line);
            output.push('\n');
            continue;
        }

        // Check for a resolved embed marker.
        if let Some(target) = parse_embed_marker(trimmed) {
            let (file_path, heading_anchor) = split_target(target);

            // Record the dependency regardless of whether we can resolve it.
            embed_deps.push((file_path.to_string(), from_path.to_string()));

            // Check depth limit.
            if depth >= MAX_EMBED_DEPTH {
                diagnostics.push(Diagnostic {
                    message: format!(
                        "Embed depth limit ({MAX_EMBED_DEPTH}) exceeded for '{file_path}'"
                    ),
                    source_path: from_path.to_string(),
                    reference: target.to_string(),
                });
                output.push_str(line);
                output.push('\n');
                continue;
            }

            // Check for cycles.
            if visited.contains(file_path) {
                diagnostics.push(Diagnostic {
                    message: format!("Circular embed detected: '{file_path}'"),
                    source_path: from_path.to_string(),
                    reference: target.to_string(),
                });
                output.push_str(line);
                output.push('\n');
                continue;
            }

            // Try to read the file.
            match file_reader(file_path) {
                None => {
                    diagnostics.push(Diagnostic {
                        message: format!("Embed target not found: '{file_path}'"),
                        source_path: from_path.to_string(),
                        reference: target.to_string(),
                    });
                    output.push_str(line);
                    output.push('\n');
                }
                Some(file_content) => {
                    let body = strip_frontmatter(&file_content);

                    let section = if let Some(anchor) = heading_anchor {
                        match extract_heading_section(body, anchor) {
                            Some(section) => section,
                            None => {
                                diagnostics.push(Diagnostic {
                                    message: format!(
                                        "Heading '#{anchor}' not found in '{file_path}'"
                                    ),
                                    source_path: from_path.to_string(),
                                    reference: target.to_string(),
                                });
                                body.to_string()
                            }
                        }
                    } else {
                        body.to_string()
                    };

                    // Recurse into the inlined content.
                    visited.insert(file_path.to_string());
                    let nested = resolve_embeds_inner(
                        &section,
                        file_path,
                        file_reader,
                        visited,
                        depth + 1,
                    );
                    visited.remove(file_path);

                    diagnostics.extend(nested.diagnostics);
                    embed_deps.extend(nested.embed_deps);

                    // Append the resolved content. Ensure it ends with a newline
                    // so subsequent lines are not joined.
                    output.push_str(&nested.content);
                    if !nested.content.ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    // If the original content did not end with a newline, remove the trailing
    // one we added.
    if !content.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }

    EmbedResult {
        content: output,
        diagnostics,
        embed_deps,
    }
}

/// Parse an embed marker line and return the target string.
///
/// `<!-- moss-embed:path/to/file.md -->` => `Some("path/to/file.md")`
/// `<!-- moss-embed:path/to/file.md#heading -->` => `Some("path/to/file.md#heading")`
fn parse_embed_marker(line: &str) -> Option<&str> {
    let rest = line.strip_prefix(EMBED_PREFIX)?;
    let target = rest.strip_suffix(EMBED_SUFFIX)?;
    if target.is_empty() {
        return None;
    }
    Some(target)
}

/// Split a target string into (file_path, optional heading_anchor).
///
/// `"guide.md#getting-started"` => `("guide.md", Some("getting-started"))`
/// `"guide.md"` => `("guide.md", None)`
fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.find('#') {
        Some(pos) => {
            let file_path = &target[..pos];
            let anchor = &target[pos + 1..];
            if anchor.is_empty() {
                (file_path, None)
            } else {
                (file_path, Some(anchor))
            }
        }
        None => (target, None),
    }
}

/// Strip YAML frontmatter from file content.
///
/// Frontmatter is delimited by `---` at the very start of the file and a
/// subsequent `---` line.  Everything between (and including) the delimiters
/// is removed.
fn strip_frontmatter(content: &str) -> &str {
    // Must start with `---` on the first line.
    if !content.starts_with("---") {
        return content;
    }

    // Find the end of the first line (the opening `---`).
    let after_opening = match content.find('\n') {
        Some(pos) => pos + 1,
        None => return content, // Only "---" with no closing delimiter.
    };

    // Find the closing `---` line.
    if let Some(close_pos) = find_closing_frontmatter(&content[after_opening..]) {
        let absolute_close = after_opening + close_pos;
        // Skip past the closing `---\n`.
        let after_close = &content[absolute_close..];
        let after_delimiter = match after_close.find('\n') {
            Some(pos) => &after_close[pos + 1..],
            None => "", // File ends right at the closing `---`.
        };
        after_delimiter
    } else {
        // No closing delimiter found — treat entire content as body.
        content
    }
}

/// Find the position of the closing `---` line within a string (relative offset).
fn find_closing_frontmatter(s: &str) -> Option<usize> {
    let mut offset = 0;
    for line in s.lines() {
        if line.trim() == "---" {
            return Some(offset);
        }
        offset += line.len() + 1; // +1 for the '\n'
    }
    None
}

/// Extract the section under a specific heading, identified by its anchor.
///
/// Returns everything from the heading line through the next heading of equal
/// or higher level (or end of file).
fn extract_heading_section(body: &str, target_anchor: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    let mut start_idx = None;
    let mut heading_level = 0;

    // Find the heading whose anchor matches.
    for (i, line) in lines.iter().enumerate() {
        if let Some((level, text)) = parse_heading(line) {
            let anchor = obsidian_heading_anchor(text);
            if anchor == target_anchor {
                start_idx = Some(i);
                heading_level = level;
                break;
            }
        }
    }

    let start = start_idx?;

    // Find where the section ends: next heading of equal or higher level.
    let mut end_idx = lines.len();
    for i in (start + 1)..lines.len() {
        if let Some((level, _)) = parse_heading(lines[i]) {
            if level <= heading_level {
                end_idx = i;
                break;
            }
        }
    }

    let section = lines[start..end_idx].join("\n");
    Some(section)
}

/// Parse a markdown heading line into (level, text).
///
/// `"## Getting Started"` => `Some((2, "Getting Started"))`
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let level = trimmed.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }

    // Must be followed by a space (or be just hashes at end of line).
    let rest = &trimmed[level..];
    if rest.is_empty() {
        return Some((level, ""));
    }
    if !rest.starts_with(' ') {
        return None;
    }

    Some((level, rest[1..].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mock_reader(files: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |path: &str| files.get(path).cloned()
    }

    #[test]
    fn test_basic_embed() {
        let mut files = HashMap::new();
        files.insert(
            "note.md".to_string(),
            "---\ntitle: Note\n---\nHello from note.".to_string(),
        );

        let content = "Before.\n<!-- moss-embed:note.md -->\nAfter.";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert_eq!(result.content, "Before.\nHello from note.\nAfter.");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_heading_scoped_embed() {
        let mut files = HashMap::new();
        files.insert(
            "guide.md".to_string(),
            "---\ntitle: Guide\n---\n# Intro\nIntro text.\n## Getting Started\nStart here.\n## Advanced\nAdvanced stuff."
                .to_string(),
        );

        let content = "<!-- moss-embed:guide.md#getting-started -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert!(result.content.contains("## Getting Started"));
        assert!(result.content.contains("Start here."));
        assert!(!result.content.contains("Advanced stuff."));
        assert!(!result.content.contains("Intro text."));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_heading_not_found() {
        let mut files = HashMap::new();
        files.insert(
            "guide.md".to_string(),
            "---\ntitle: Guide\n---\n# Intro\nIntro text.".to_string(),
        );

        let content = "<!-- moss-embed:guide.md#nonexistent -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        // Full body inlined when heading not found.
        assert!(result.content.contains("Intro text."));
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("not found"));
    }

    #[test]
    fn test_circular_embed_detection() {
        let mut files = HashMap::new();
        files.insert(
            "a.md".to_string(),
            "A content.\n<!-- moss-embed:b.md -->".to_string(),
        );
        files.insert(
            "b.md".to_string(),
            "B content.\n<!-- moss-embed:a.md -->".to_string(),
        );

        let content = "<!-- moss-embed:a.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        // A's content should be inlined (including B's content), but the
        // circular reference back to A should produce a diagnostic.
        assert!(result.content.contains("A content."));
        assert!(result.content.contains("B content."));
        let cycle_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("Circular"));
        assert!(cycle_diag.is_some(), "Expected a circular embed diagnostic");
    }

    #[test]
    fn test_max_depth_protection() {
        // Build a chain: file0 embeds file1, file1 embeds file2, ..., up to 12.
        let mut files = HashMap::new();
        for i in 0..12 {
            let next = i + 1;
            files.insert(
                format!("file{i}.md"),
                format!("Content {i}.\n<!-- moss-embed:file{next}.md -->"),
            );
        }
        files.insert("file12.md".to_string(), "End.".to_string());

        let content = "<!-- moss-embed:file0.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        // Should have a depth-exceeded diagnostic.
        let depth_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("depth limit"));
        assert!(
            depth_diag.is_some(),
            "Expected a depth limit diagnostic, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_file_not_found() {
        let files = HashMap::new();

        let content = "<!-- moss-embed:missing.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("not found"));
        // The marker should be preserved when the file is not found.
        assert!(result.content.contains("<!-- moss-embed:missing.md -->"));
    }

    #[test]
    fn test_unresolved_marker_preserved() {
        let files = HashMap::new();

        let content = "Before.\n<!-- moss-embed-unresolved:some-ref -->\nAfter.";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert!(result
            .content
            .contains("<!-- moss-embed-unresolved:some-ref -->"));
        assert!(result.diagnostics.is_empty());
        assert_eq!(
            result.content,
            "Before.\n<!-- moss-embed-unresolved:some-ref -->\nAfter."
        );
    }

    #[test]
    fn test_recursive_embed() {
        let mut files = HashMap::new();
        files.insert(
            "a.md".to_string(),
            "A content.\n<!-- moss-embed:b.md -->".to_string(),
        );
        files.insert("b.md".to_string(), "B content.".to_string());

        let content = "<!-- moss-embed:a.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert!(result.content.contains("A content."));
        assert!(result.content.contains("B content."));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_embed_deps_tracked() {
        let mut files = HashMap::new();
        files.insert(
            "a.md".to_string(),
            "A content.\n<!-- moss-embed:b.md -->".to_string(),
        );
        files.insert("b.md".to_string(), "B content.".to_string());

        let content = "<!-- moss-embed:a.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        // index.md -> a.md, a.md -> b.md
        assert!(
            result.embed_deps.contains(&("a.md".to_string(), "index.md".to_string())),
            "Missing dep: (a.md, index.md). Got: {:?}",
            result.embed_deps
        );
        assert!(
            result.embed_deps.contains(&("b.md".to_string(), "a.md".to_string())),
            "Missing dep: (b.md, a.md). Got: {:?}",
            result.embed_deps
        );
    }

    #[test]
    fn test_no_frontmatter() {
        let mut files = HashMap::new();
        files.insert(
            "plain.md".to_string(),
            "Just plain content.\nSecond line.".to_string(),
        );

        let content = "<!-- moss-embed:plain.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert_eq!(result.content, "Just plain content.\nSecond line.");
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_multiple_embeds() {
        let mut files = HashMap::new();
        files.insert("one.md".to_string(), "Content one.".to_string());
        files.insert("two.md".to_string(), "Content two.".to_string());

        let content = "<!-- moss-embed:one.md -->\n<!-- moss-embed:two.md -->";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert!(result.content.contains("Content one."));
        assert!(result.content.contains("Content two."));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn test_content_around_embed_preserved() {
        let mut files = HashMap::new();
        files.insert("note.md".to_string(), "Note content.".to_string());

        let content = "Paragraph before.\n\n<!-- moss-embed:note.md -->\n\nParagraph after.";
        let mut visited = HashSet::new();
        let result = resolve_embeds(content, "index.md", &mock_reader(&files), &mut visited);

        assert!(result.content.contains("Paragraph before."));
        assert!(result.content.contains("Note content."));
        assert!(result.content.contains("Paragraph after."));
        assert!(result.diagnostics.is_empty());
    }

    // ----- Helper unit tests -----

    #[test]
    fn test_parse_embed_marker() {
        assert_eq!(
            parse_embed_marker("<!-- moss-embed:path/to/file.md -->"),
            Some("path/to/file.md")
        );
        assert_eq!(
            parse_embed_marker("<!-- moss-embed:file.md#heading -->"),
            Some("file.md#heading")
        );
        assert_eq!(parse_embed_marker("<!-- moss-embed: -->"), None);
        assert_eq!(parse_embed_marker("not an embed marker"), None);
        assert_eq!(
            parse_embed_marker("<!-- moss-embed-unresolved:ref -->"),
            None
        );
    }

    #[test]
    fn test_split_target() {
        assert_eq!(split_target("file.md"), ("file.md", None));
        assert_eq!(
            split_target("file.md#heading"),
            ("file.md", Some("heading"))
        );
        assert_eq!(split_target("file.md#"), ("file.md", None));
        assert_eq!(
            split_target("path/to/file.md#deep-heading"),
            ("path/to/file.md", Some("deep-heading"))
        );
    }

    #[test]
    fn test_strip_frontmatter_basic() {
        let input = "---\ntitle: Test\n---\nBody content.";
        assert_eq!(strip_frontmatter(input), "Body content.");
    }

    #[test]
    fn test_strip_frontmatter_none() {
        let input = "No frontmatter here.\nJust content.";
        assert_eq!(strip_frontmatter(input), input);
    }

    #[test]
    fn test_strip_frontmatter_no_closing() {
        let input = "---\ntitle: Test\nNo closing delimiter.";
        // No closing `---`, so treat as no frontmatter.
        assert_eq!(strip_frontmatter(input), input);
    }

    #[test]
    fn test_extract_heading_section_basic() {
        let body = "# Intro\nIntro text.\n## Getting Started\nStart here.\n## Advanced\nAdvanced.";
        let section = extract_heading_section(body, "getting-started");
        assert!(section.is_some());
        let s = section.unwrap();
        assert!(s.contains("## Getting Started"));
        assert!(s.contains("Start here."));
        assert!(!s.contains("Advanced."));
        assert!(!s.contains("Intro text."));
    }

    #[test]
    fn test_extract_heading_section_last() {
        let body = "# Intro\nIntro text.\n## Last Section\nLast content.";
        let section = extract_heading_section(body, "last-section");
        assert!(section.is_some());
        let s = section.unwrap();
        assert!(s.contains("## Last Section"));
        assert!(s.contains("Last content."));
    }

    #[test]
    fn test_extract_heading_section_not_found() {
        let body = "# Intro\nIntro text.";
        assert!(extract_heading_section(body, "nonexistent").is_none());
    }

    #[test]
    fn test_parse_heading() {
        assert_eq!(parse_heading("# Title"), Some((1, "Title")));
        assert_eq!(parse_heading("## Sub Title"), Some((2, "Sub Title")));
        assert_eq!(parse_heading("###### Deep"), Some((6, "Deep")));
        assert_eq!(parse_heading("Not a heading"), None);
        assert_eq!(parse_heading("#NoSpace"), None);
        assert_eq!(parse_heading(""), None);
    }
}
