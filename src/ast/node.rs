//! Block-level and inline-level AST nodes.
//!
//! Closed enums; pattern matching is the visitor framework. The variants
//! cover what moss emits today (CommonMark + GFM extensions enabled in the
//! pipeline: tables, strikethrough, footnotes — see
//! `src-tauri/src/build/markdown/pipeline.rs`'s `Options` setup).
//!
//! Anything pulldown-cmark emits that the AST hasn't modeled flows through
//! `Block::Other` / `Inline::Other`, which carries the raw HTML so the
//! renderer passes it through unchanged. New variants may be promoted out
//! of `Other` over time as a need is identified.

use serde::{Deserialize, Serialize};

use super::shortcode::Shortcode;
use super::url::Url;

/// A block-level AST node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    /// `# Heading` (level 1) through `###### Heading` (level 6).
    Heading {
        level: u8,
        children: Vec<Inline>,
        /// Heading anchor id (slug). Computed by the parser via
        /// [`crate::heading_anchor::obsidian_heading_anchor`].
        id: Option<String>,
    },
    /// A paragraph of inline content.
    Paragraph(Vec<Inline>),
    /// `> [!type] body` — typed callouts. The `kind` is the literal
    /// callout name as written; downstream code maps to CSS classes.
    Callout {
        kind: String,
        children: Vec<Block>,
    },
    /// `- item` / `1. item`. Each item is a list of blocks (so list items
    /// can carry paragraphs, sub-lists, etc).
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    /// A fenced code block.
    CodeBlock {
        lang: Option<String>,
        value: String,
    },
    /// Markdown table.
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// `> blockquote`
    BlockQuote(Vec<Block>),
    /// A typed shortcode block (`:::name ...args\n body :::`).
    Shortcode(Shortcode),
    /// `<hr>` thematic break.
    ThematicBreak,
    /// Escape hatch: anything pulldown-cmark emits that the AST hasn't
    /// modeled. Carries the raw HTML so the renderer passes it through
    /// unchanged.
    Other(String),
}

/// An inline-level AST node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Inline {
    Text(String),
    /// `[content](url "title")`
    Link {
        url: Url,
        title: Option<String>,
        children: Vec<Inline>,
    },
    /// `![alt](src "title")`
    Image {
        src: Url,
        alt: String,
        title: Option<String>,
    },
    /// `*emphasis*`
    Emphasis(Vec<Inline>),
    /// `**strong**`
    Strong(Vec<Inline>),
    /// `` `code` ``
    Code(String),
    /// Hard line break.
    LineBreak,
    /// Escape hatch for unmodeled inline HTML.
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::super::url::{Url, UrlKind};
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    #[test]
    fn block_heading_constructable() {
        let b = Block::Heading {
            level: 1,
            children: vec![text("Hello")],
            id: Some("hello".to_string()),
        };
        match b {
            Block::Heading { level, children, id } => {
                assert_eq!(level, 1);
                assert_eq!(children.len(), 1);
                assert_eq!(id.as_deref(), Some("hello"));
            }
            _ => panic!("expected Heading"),
        }
    }

    #[test]
    fn block_paragraph_holds_inlines() {
        let b = Block::Paragraph(vec![text("hi"), Inline::LineBreak, text("there")]);
        match b {
            Block::Paragraph(items) => assert_eq!(items.len(), 3),
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn block_list_each_item_is_block_vec() {
        let b = Block::List {
            ordered: false,
            items: vec![
                vec![Block::Paragraph(vec![text("first")])],
                vec![Block::Paragraph(vec![text("second")])],
            ],
        };
        match b {
            Block::List { ordered, items } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2);
            }
            _ => panic!("expected List"),
        }
    }

    #[test]
    fn block_table_two_dim_rows() {
        let b = Block::Table {
            header: vec![vec![text("A")], vec![text("B")]],
            rows: vec![
                vec![vec![text("1")], vec![text("2")]],
                vec![vec![text("3")], vec![text("4")]],
            ],
        };
        match b {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            _ => panic!("expected Table"),
        }
    }

    #[test]
    fn block_other_carries_raw_html() {
        let b = Block::Other("<custom>raw</custom>".to_string());
        match b {
            Block::Other(s) => assert_eq!(s, "<custom>raw</custom>"),
            _ => panic!("expected Other"),
        }
    }

    #[test]
    fn block_thematic_break_is_unit_variant() {
        let b = Block::ThematicBreak;
        assert!(matches!(b, Block::ThematicBreak));
    }

    #[test]
    fn inline_link_carries_url_and_children() {
        let i = Inline::Link {
            url: Url::unresolved("docs/"),
            title: None,
            children: vec![text("Documentation")],
        };
        match i {
            Inline::Link { url, title, children } => {
                assert!(url.is_unresolved());
                assert!(title.is_none());
                assert_eq!(children.len(), 1);
            }
            _ => panic!("expected Link"),
        }
    }

    #[test]
    fn inline_image_uses_url_for_src() {
        // Per R6: Inline::Image carries Url (not a separate Src type).
        // UrlKind::Asset is the relevant variant after resolution.
        let i = Inline::Image {
            src: Url::resolved("img/cat.jpg", UrlKind::Asset),
            alt: "Cat".to_string(),
            title: None,
        };
        match i {
            Inline::Image { src, alt, title: _ } => {
                let r = src.as_resolved();
                assert_eq!(r.kind, UrlKind::Asset);
                assert_eq!(alt, "Cat");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn inline_emphasis_and_strong_nest() {
        let i = Inline::Strong(vec![Inline::Emphasis(vec![text("nested")])]);
        match i {
            Inline::Strong(children) => match &children[0] {
                Inline::Emphasis(inner) => assert_eq!(inner.len(), 1),
                _ => panic!("expected Emphasis"),
            },
            _ => panic!("expected Strong"),
        }
    }

    #[test]
    fn block_round_trips_through_serde() {
        let original = Block::Heading {
            level: 2,
            children: vec![Inline::Text("Setup".to_string())],
            id: Some("setup".to_string()),
        };
        let s = serde_json::to_string(&original).expect("serialize");
        let back: Block = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn inline_link_with_resolved_url_round_trips() {
        let original = Inline::Link {
            url: Url::resolved("../docs/", UrlKind::Wikilink),
            title: Some("Docs".to_string()),
            children: vec![Inline::Text("see".to_string())],
        };
        let s = serde_json::to_string(&original).expect("serialize");
        let back: Inline = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(original, back);
    }
}
