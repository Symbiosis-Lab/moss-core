//! Pure extraction of a document's headings (text + slug + level) for the
//! editor's `[[Page#Heading]]` autocomplete. Reuses `parse()` (which runs
//! `assign_heading_id_suffixes`) so the returned slugs are byte-identical
//! to the rendered `<hN id="...">` attributes — the keystone invariant.
//!
//! v1 extracts TOP-LEVEL headings only (the common case). Headings nested
//! inside callouts / blockquotes / lists are not offered for autocomplete;
//! a recursive walk is a follow-up if needed.

use crate::ast::{parse, Block, Inline};

/// A heading discovered in a document, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingInfo {
    /// Plain-text heading title (inline markup flattened).
    pub text: String,
    /// Final deduped slug — matches the rendered `<hN id="...">`.
    pub slug: String,
    /// Heading level 1..=6.
    pub level: u8,
}

/// Flatten an inline slice to its plain-text content (markup removed).
fn inlines_to_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(t),
            Inline::Code(c) => out.push_str(c),
            Inline::Emphasis(children) | Inline::Strong(children) => {
                inlines_to_text(children, out)
            }
            Inline::Link { children, .. } => inlines_to_text(children, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::LineBreak => out.push(' '),
            Inline::Other(_) => {}
        }
    }
}

/// Extract all top-level headings from `markdown` in document order, with
/// final (deduped) slugs identical to the rendered `<hN id>`.
pub fn extract_headings(markdown: &str) -> Vec<HeadingInfo> {
    let doc = parse(markdown);
    let mut out = Vec::new();
    for block in &doc.blocks {
        if let Block::Heading {
            level,
            children,
            id,
        } = block
        {
            let mut text = String::new();
            inlines_to_text(children, &mut text);
            out.push(HeadingInfo {
                text: text.trim().to_string(),
                slug: id.clone().unwrap_or_default(),
                level: *level,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_slug_level() {
        let md = "# Title\n\n## Getting Started\n\ntext\n\n### Sub *em*\n";
        let hs = extract_headings(md);
        assert_eq!(hs.len(), 3);
        assert_eq!(hs[0], HeadingInfo { text: "Title".into(), slug: "title".into(), level: 1 });
        assert_eq!(hs[1], HeadingInfo { text: "Getting Started".into(), slug: "getting-started".into(), level: 2 });
        assert_eq!(hs[2], HeadingInfo { text: "Sub em".into(), slug: "sub-em".into(), level: 3 });
    }

    #[test]
    fn dedups_duplicate_slugs() {
        let md = "## Setup\n\n## Setup\n";
        let hs = extract_headings(md);
        assert_eq!(hs[0].slug, "setup");
        assert_eq!(hs[1].slug, "setup-1");
    }

    #[test]
    fn preserves_cjk() {
        let md = "## 中文标题\n";
        let hs = extract_headings(md);
        assert_eq!(hs[0].slug, "中文标题");
        assert_eq!(hs[0].text, "中文标题");
    }

    #[test]
    fn empty_doc_no_headings() {
        assert!(extract_headings("just a paragraph\n").is_empty());
    }

    #[test]
    fn slug_matches_obsidian_anchor_for_punctuation() {
        // Keystone: slug must equal what obsidian_heading_anchor produces
        // (the same fn the renderer uses for id=). Spot-check a heading with
        // punctuation that the algorithm keeps/strips distinctively.
        let md = "## Step 1: Install\n";
        let hs = extract_headings(md);
        assert_eq!(hs[0].slug, crate::heading_anchor::obsidian_heading_anchor("Step 1: Install"));
    }
}
