//! Top-level parsed document.

use serde::{Deserialize, Serialize};

use super::node::Block;

/// A parsed markdown document body.
///
/// Wraps `Vec<Block>` with document-level flags. The frontmatter sits in
/// [`crate::frontmatter::ParsedDocument`]; this struct is body-only.
/// Higher-level code combines the two as needed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Document {
    /// Body content as a flat list of block-level nodes.
    pub blocks: Vec<Block>,
    /// True if this document is a slot file (e.g. `footer.md`) — its
    /// content fills a layout slot rather than being rendered as an
    /// article. The renderer suppresses auto-injected article chrome
    /// (`<h1 class="moss-article-title">`) when this is set.
    ///
    /// Replaces the `heading: false` YAML synthesis hack at
    /// `src-tauri/src/build/footer.rs:160`. Lands in Phase A.5 of the
    /// typed-AST migration; in Phase A it's available but not yet
    /// consumed by src-tauri.
    pub slot_only: bool,
}

impl Document {
    /// Construct an empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a document from a list of blocks.
    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        Self {
            blocks,
            slot_only: false,
        }
    }

    /// True if any top-level block is a shortcode of the given kind.
    ///
    /// Shallow check: does NOT descend into nested blocks (e.g. a
    /// `:::subscribe` inside a `:::grid` cell would not be found by this
    /// query alone). Phase A models the shallow case; deeper
    /// `has_shortcode_recursive` lands when nested-shortcode AST queries
    /// are first needed.
    pub fn has_shortcode(&self, kind: super::shortcode::ShortcodeKind) -> bool {
        self.blocks.iter().any(|b| match b {
            Block::Shortcode(sc) => sc.kind() == kind,
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::node::Inline;
    use super::*;

    #[test]
    fn empty_document_has_no_blocks() {
        let d = Document::new();
        assert!(d.blocks.is_empty());
        assert!(!d.slot_only);
    }

    #[test]
    fn from_blocks_constructs_with_blocks() {
        let d = Document::from_blocks(vec![Block::ThematicBreak]);
        assert_eq!(d.blocks.len(), 1);
        assert!(!d.slot_only);
    }

    #[test]
    fn slot_only_default_is_false() {
        // Crucial: an article doc must NOT silently become a slot.
        let d = Document::default();
        assert!(!d.slot_only);
    }

    #[test]
    fn slot_only_settable() {
        let mut d = Document::new();
        d.slot_only = true;
        assert!(d.slot_only);
    }

    #[test]
    fn document_round_trips_through_serde() {
        let mut original = Document::new();
        original.blocks.push(Block::Paragraph(vec![Inline::Text(
            "hello".to_string(),
        )]));
        original.slot_only = true;
        let s = serde_json::to_string(&original).expect("serialize");
        let back: Document = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn has_shortcode_returns_false_when_empty() {
        // The Shortcode enum is empty in Phase A; this test exercises the
        // query path on the empty case (which is the only constructable
        // case until Phase B). Extended per-kind tests land alongside
        // each shortcode migration.
        let d = Document::new();
        assert!(!d.has_shortcode(super::super::shortcode::ShortcodeKind::Subscribe));
    }
}
