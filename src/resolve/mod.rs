//! Link resolution types and the `resolve_content()` orchestrator.
//!
//! This module provides shared types for the resolve phase of the
//! compilation pipeline, a fuzzy path resolver that wraps
//! [`ContentGraph::resolve_path`](crate::content_graph::ContentGraph::resolve_path),
//! and the top-level [`resolve_content`] function that ties all phases together.

use crate::content_graph::ContentGraph;

pub mod block_refs;
pub mod callouts;
pub mod embeds;
pub mod fuzzy_path;
pub mod wikilinks;

/// A link going out from a document.
#[derive(Debug, Clone)]
pub struct OutgoingLink {
    pub target_path: String,
    pub display_text: String,
    pub link_type: LinkType,
}

/// The kind of link syntax used.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkType {
    /// `[[target]]` or `[[target|display]]`
    Wikilink,
    /// `![[target]]` — an embedded/transcluded reference
    Embed,
    /// Standard markdown `[text](url)`
    Standard,
}

/// A diagnostic message from the resolve phase.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub source_path: String,
    pub reference: String,
}

/// Result of resolving all Obsidian syntax in a markdown file.
#[derive(Debug)]
pub struct ResolveResult {
    /// Clean markdown with all Obsidian syntax resolved.
    pub content_markdown: String,
    /// All outgoing links from this document.
    pub outgoing_links: Vec<OutgoingLink>,
    /// Warnings and errors encountered during resolution.
    pub diagnostics: Vec<Diagnostic>,
    /// Block IDs extracted from this document.
    pub block_ids: Vec<String>,
    /// (target_path, source_path) pairs for embed dependency tracking.
    pub embed_deps: Vec<(String, String)>,
}

/// Resolve all Obsidian syntax in a markdown file, producing clean standard markdown.
///
/// Pipeline order:
/// 1. Separate frontmatter from body
/// 2. Resolve wikilinks (first pass) -- standard `[[…]]` and `![[…]]` to markdown links / embed markers
/// 3. Resolve embed placeholders -- inline `<!-- moss-embed:… -->` markers with file content
/// 4. Resolve wikilinks (second pass) -- catch wikilinks introduced by embedded content
/// 5. Transform block references -- `^id` markers to HTML anchors
/// 6. Transform callouts -- `> [!type]` to HTML divs
/// 7. Rejoin frontmatter + resolved body
pub fn resolve_content(
    source_path: &str,
    raw_markdown: &str,
    graph: &ContentGraph,
    file_reader: &dyn Fn(&str) -> Option<String>,
) -> ResolveResult {
    // Step 1: Separate frontmatter from body.
    let (frontmatter, body) = split_frontmatter(raw_markdown);

    // Step 2: First wikilink pass.
    let wikilink_pass1 = wikilinks::resolve_wikilinks(body, graph, source_path);
    let mut outgoing_links = wikilink_pass1.outgoing_links;
    let mut diagnostics = wikilink_pass1.diagnostics;

    // Step 3: Resolve embeds.
    let embed_result = embeds::resolve_embeds(&wikilink_pass1.content, source_path, file_reader);
    diagnostics.extend(embed_result.diagnostics);
    let embed_deps = embed_result.embed_deps;

    // Step 4: Second wikilink pass (for wikilinks inside embedded content).
    let wikilink_pass2 = wikilinks::resolve_wikilinks(&embed_result.content, graph, source_path);
    outgoing_links.extend(wikilink_pass2.outgoing_links);
    diagnostics.extend(wikilink_pass2.diagnostics);

    // Step 5: Transform block references.
    let (block_result, block_ids) = block_refs::transform_block_refs(&wikilink_pass2.content);

    // Step 6: Transform callouts.
    let callout_result = callouts::transform_callouts(&block_result);

    // Step 7: Rejoin frontmatter + resolved body.
    let content_markdown = match frontmatter {
        Some(fm) => format!("{}{}", fm, callout_result),
        None => callout_result,
    };

    ResolveResult {
        content_markdown,
        outgoing_links,
        diagnostics,
        block_ids,
        embed_deps,
    }
}

/// Split content into (frontmatter_including_delimiters, body).
///
/// If content starts with `---\n`, finds the closing `---\n` and splits there.
/// The frontmatter portion includes both delimiters and the trailing newline
/// after the closing `---`. Returns `(None, full_content)` if no frontmatter.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    if !content.starts_with("---") {
        return (None, content);
    }

    // Find end of the opening `---` line.
    let after_opening = match content.find('\n') {
        Some(pos) => pos + 1,
        None => return (None, content),
    };

    // Search for a closing `---` line in the remainder.
    let rest = &content[after_opening..];
    let mut offset = 0;
    for line in rest.lines() {
        if line.trim() == "---" {
            // Found closing delimiter. Include through the end of this line.
            let close_abs = after_opening + offset + line.len();
            // Include the newline after the closing `---` if present.
            let split_pos = if close_abs < content.len() && content.as_bytes()[close_abs] == b'\n'
            {
                close_abs + 1
            } else {
                close_abs
            };
            return (Some(&content[..split_pos]), &content[split_pos..]);
        }
        offset += line.len() + 1; // +1 for '\n'
    }

    // No closing delimiter found — treat entire content as body.
    (None, content)
}

/// Extract the parent directory from a `/`-separated path.
///
/// `"posts/hello.md"` -> `"posts"`, `"hello.md"` -> `""`.
pub(crate) fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(pos) => &path[..pos],
        None => "",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;
    use std::collections::HashMap;

    fn test_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("guide.md", "guide");
        b.add_file("note.md", "note");
        b.add_file("disclaimer.md", "disclaimer");
        b.add_file("assets/photo.jpg", "photo");
        b.add_headings(
            "guide.md",
            vec![("Setup".into(), "setup".into())],
        );
        b.add_blocks("guide.md", vec!["key-point".into()]);
        b.build()
    }

    fn test_files() -> HashMap<String, String> {
        let mut files = HashMap::new();
        files.insert(
            "disclaimer.md".into(),
            "---\ntitle: Disclaimer\n---\nThis is the disclaimer.\n\nSee [[guide]] for details."
                .into(),
        );
        files
    }

    fn mock_reader(files: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |path: &str| files.get(path).cloned()
    }

    // ----- split_frontmatter unit tests -----

    #[test]
    fn test_split_fm_present() {
        let input = "---\ntitle: Hello\n---\nBody here.";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm, Some("---\ntitle: Hello\n---\n"));
        assert_eq!(body, "Body here.");
    }

    #[test]
    fn test_split_fm_absent() {
        let input = "Just body content.";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn test_split_fm_no_closing() {
        let input = "---\ntitle: Hello\nno closing delimiter";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    // ----- Integration tests for resolve_content -----

    #[test]
    fn test_full_resolve_pipeline() {
        let graph = test_graph();
        let files = test_files();

        let input = "---\ntitle: Test\n---\nSee [[guide#Setup]] for help.\n\nImportant point. ^my-block\n\n> [!warning] Watch Out\n> Be careful here.";

        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // Frontmatter preserved
        assert!(result.content_markdown.starts_with("---\ntitle: Test\n---\n"));

        // Wikilink resolved
        assert!(result.content_markdown.contains("[guide > Setup](../guide/#setup)"));

        // Block ref transformed
        assert!(result.content_markdown.contains("<span id=\"my-block\"></span>"));
        assert_eq!(result.block_ids, vec!["my-block"]);

        // Callout transformed
        assert!(result.content_markdown.contains("callout-warning"));
        assert!(result.content_markdown.contains("Watch Out"));
    }

    #[test]
    fn test_frontmatter_preserved() {
        let graph = test_graph();
        let files = HashMap::new();

        let input = "---\ntitle: My Page\ntags:\n  - rust\n  - wasm\n---\nPlain body.";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        assert!(result.content_markdown.starts_with("---\ntitle: My Page\ntags:\n  - rust\n  - wasm\n---\n"));
        assert!(result.content_markdown.ends_with("Plain body."));
    }

    #[test]
    fn test_no_obsidian_syntax() {
        let graph = test_graph();
        let files = HashMap::new();

        let input = "---\ntitle: Plain\n---\nJust a plain paragraph.\n\nAnother paragraph.";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        assert_eq!(result.content_markdown, input);
        assert!(result.outgoing_links.is_empty());
        assert!(result.diagnostics.is_empty());
        assert!(result.block_ids.is_empty());
        assert!(result.embed_deps.is_empty());
    }

    #[test]
    fn test_embedded_wikilinks_resolved() {
        let graph = test_graph();
        let files = test_files();

        // disclaimer.md body contains `See [[guide]] for details.`
        // After embed, that wikilink needs to be resolved in the second pass.
        let input = "![[disclaimer]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // The embedded content's wikilink [[guide]] should be resolved
        assert!(
            result.content_markdown.contains("[guide](../guide/)"),
            "Expected resolved wikilink from embedded content, got: {}",
            result.content_markdown
        );
        // The disclaimer body text should be present
        assert!(result.content_markdown.contains("This is the disclaimer."));
    }

    #[test]
    fn test_diagnostics_merged() {
        let graph = test_graph();
        let files = HashMap::new();

        // Two unresolved references: one wikilink, one embed (file not found)
        let input = "[[nonexistent]] and ![[missing]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // Should have diagnostics from wikilinks (unresolved) and possibly embeds
        assert!(
            result.diagnostics.len() >= 2,
            "Expected at least 2 diagnostics, got {}: {:?}",
            result.diagnostics.len(),
            result.diagnostics
        );
    }

    #[test]
    fn test_outgoing_links_tracked() {
        let graph = test_graph();
        let files = test_files();

        // disclaimer.md body contains [[guide]], so after embedding and second pass,
        // we should have links from both passes.
        // Embeds must be on their own line for the embed resolver to process them.
        let input = "[[guide]]\n\n![[disclaimer]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        // First pass: [[guide]] (Wikilink) + ![[disclaimer]] (Embed)
        // Second pass: [[guide]] from embedded content (Wikilink)
        let wikilinks: Vec<_> = result
            .outgoing_links
            .iter()
            .filter(|l| l.link_type == LinkType::Wikilink)
            .collect();
        let embeds: Vec<_> = result
            .outgoing_links
            .iter()
            .filter(|l| l.link_type == LinkType::Embed)
            .collect();

        assert!(
            wikilinks.len() >= 2,
            "Expected at least 2 wikilink outgoing links (from both passes), got {}: {:?}",
            wikilinks.len(),
            wikilinks
        );
        assert!(
            !embeds.is_empty(),
            "Expected at least 1 embed outgoing link"
        );
    }

    #[test]
    fn test_embed_deps_tracked() {
        let graph = test_graph();
        let files = test_files();

        let input = "![[disclaimer]]";
        let result = resolve_content("note.md", input, &graph, &mock_reader(&files));

        assert!(
            result
                .embed_deps
                .contains(&("disclaimer.md".to_string(), "note.md".to_string())),
            "Expected embed dep (disclaimer.md, note.md), got: {:?}",
            result.embed_deps
        );
    }
}
