//! Visitor helpers over the typed AST.
//!
//! Pattern matching is the visitor framework (no `Visit` trait, no
//! `Box<dyn Node>`). These free functions exist for the cases that
//! genuinely need recursive descent across every variant — URL resolution,
//! shortcode-presence queries — and would otherwise be repeated in every
//! consumer.
//!
//! ## When to add a visitor here
//!
//! Add a free function only when the alternative is repeated recursive
//! traversal across multiple call sites. Per the design doc principle P4:
//! one-off transformations belong inline as `match block { ... }`.

use super::document::Document;
use super::node::{Block, Inline};
use super::shortcode::ShortcodeKind;
use super::url::Url;

/// Visit every URL in the document with a callback that may mutate it
/// in place. Walks links, image srcs, and all nested block/inline content.
///
/// Used by the resolve-classification pass: the upstream resolve pipeline
/// has already rewritten markdown sources so URLs come out as one of the
/// shapes documented in [`crate::ast::url::Url`]. The callback inspects the
/// raw string and replaces it with a [`crate::ast::url::Url::Resolved`].
pub fn visit_urls_mut<F>(doc: &mut Document, mut callback: F)
where
    F: FnMut(&mut Url),
{
    for block in &mut doc.blocks {
        visit_urls_in_block(block, &mut callback);
    }
}

fn visit_urls_in_block<F>(block: &mut Block, callback: &mut F)
where
    F: FnMut(&mut Url),
{
    match block {
        Block::Heading { children, .. } => {
            for inline in children {
                visit_urls_in_inline(inline, callback);
            }
        }
        Block::Paragraph(children) => {
            for inline in children {
                visit_urls_in_inline(inline, callback);
            }
        }
        Block::Callout { children, .. } => {
            for nested in children {
                visit_urls_in_block(nested, callback);
            }
        }
        Block::List { items, .. } => {
            for item_blocks in items {
                for nested in item_blocks {
                    visit_urls_in_block(nested, callback);
                }
            }
        }
        Block::Table { header, rows } => {
            for cell in header {
                for inline in cell {
                    visit_urls_in_inline(inline, callback);
                }
            }
            for row in rows {
                for cell in row {
                    for inline in cell {
                        visit_urls_in_inline(inline, callback);
                    }
                }
            }
        }
        Block::BlockQuote(children) => {
            for nested in children {
                visit_urls_in_block(nested, callback);
            }
        }
        Block::Shortcode(_) => {
            // Phase A: Shortcode is empty (uninhabited). Per-variant URL
            // descent (Buttons.items.url, Gallery.items.src, Grid.cells, ...)
            // lands alongside each Phase B migration.
        }
        Block::CodeBlock { .. } | Block::ThematicBreak | Block::Other(_) => {
            // No URLs in these.
        }
    }
}

fn visit_urls_in_inline<F>(inline: &mut Inline, callback: &mut F)
where
    F: FnMut(&mut Url),
{
    match inline {
        Inline::Link { url, children, .. } => {
            callback(url);
            for nested in children {
                visit_urls_in_inline(nested, callback);
            }
        }
        Inline::Image { src, .. } => {
            callback(src);
        }
        Inline::Emphasis(children) | Inline::Strong(children) => {
            for nested in children {
                visit_urls_in_inline(nested, callback);
            }
        }
        Inline::Text(_) | Inline::Code(_) | Inline::LineBreak | Inline::Other(_) => {}
    }
}

/// Visit every block (top-level + nested) with a read-only callback. The
/// callback returns `false` to short-circuit the traversal (any returned
/// `false` makes the whole walk return `false`).
///
/// Used for queries like "does any block contain a `:::subscribe`
/// shortcode?" — the body of `has_shortcode_recursive` below.
pub fn visit_blocks<F>(doc: &Document, mut callback: F) -> bool
where
    F: FnMut(&Block) -> bool,
{
    for block in &doc.blocks {
        if !visit_block(block, &mut callback) {
            return false;
        }
    }
    true
}

fn visit_block<F>(block: &Block, callback: &mut F) -> bool
where
    F: FnMut(&Block) -> bool,
{
    if !callback(block) {
        return false;
    }
    match block {
        Block::Callout { children, .. } | Block::BlockQuote(children) => {
            for nested in children {
                if !visit_block(nested, callback) {
                    return false;
                }
            }
        }
        Block::List { items, .. } => {
            for item_blocks in items {
                for nested in item_blocks {
                    if !visit_block(nested, callback) {
                        return false;
                    }
                }
            }
        }
        // Headings, paragraphs, code blocks, tables, shortcodes (Phase A
        // empty), thematic breaks, raw HTML — terminal at the block level.
        // Inline children of headings/paragraphs are visited by inline
        // visitors, not block visitors.
        _ => {}
    }
    true
}

/// True if any block in the document is a shortcode of the given kind
/// (recursive — descends into callouts, blockquotes, list items).
///
/// Replaces the `project_has_inline_subscribe` filesystem scan once
/// shortcodes migrate to typed AST in Phase B.
pub fn has_shortcode_recursive(doc: &Document, kind: ShortcodeKind) -> bool {
    let mut found = false;
    visit_blocks(doc, |block| {
        if let Block::Shortcode(sc) = block {
            if sc.kind() == kind {
                found = true;
                return false; // short-circuit
            }
        }
        true
    });
    found
}

#[cfg(test)]
mod tests {
    use super::super::node::Inline;
    use super::super::url::{Url, UrlKind};
    use super::*;

    fn paragraph_with_link(url: &str) -> Block {
        Block::Paragraph(vec![Inline::Link {
            url: Url::unresolved(url),
            title: None,
            children: vec![Inline::Text("t".into())],
        }])
    }

    #[test]
    fn visits_url_in_paragraph_link() {
        let mut doc = Document::from_blocks(vec![paragraph_with_link("docs/")]);
        let mut seen: Vec<String> = Vec::new();
        visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => seen.push(s.clone()),
            _ => {}
        });
        assert_eq!(seen, vec!["docs/".to_string()]);
    }

    #[test]
    fn visits_url_in_image_src() {
        let mut doc = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Image {
            src: Url::unresolved("img.png"),
            alt: "x".into(),
            title: None,
        }])]);
        let mut seen: Vec<String> = Vec::new();
        visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => seen.push(s.clone()),
            _ => {}
        });
        assert_eq!(seen, vec!["img.png".to_string()]);
    }

    #[test]
    fn callback_can_mutate_url_to_resolved() {
        // Critical contract: a single visit transitions Unresolved → Resolved.
        let mut doc = Document::from_blocks(vec![paragraph_with_link("docs/")]);
        visit_urls_mut(&mut doc, |u| {
            *u = Url::resolved("../docs/", UrlKind::Wikilink);
        });
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    assert!(url.is_resolved());
                    let r = url.as_resolved();
                    assert_eq!(r.href, "../docs/");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn visits_url_inside_heading() {
        let mut doc = Document::from_blocks(vec![Block::Heading {
            level: 2,
            children: vec![Inline::Link {
                url: Url::unresolved("x"),
                title: None,
                children: vec![Inline::Text("t".into())],
            }],
            id: None,
        }]);
        let mut count = 0;
        visit_urls_mut(&mut doc, |_| count += 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn visits_url_inside_emphasis_and_strong() {
        let mut doc = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Strong(vec![
            Inline::Emphasis(vec![Inline::Link {
                url: Url::unresolved("nested"),
                title: None,
                children: vec![],
            }]),
        ])])]);
        let mut count = 0;
        visit_urls_mut(&mut doc, |_| count += 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn visits_url_inside_link_children() {
        // Nested links can't appear in CommonMark, but link children can
        // contain images (e.g. `[![alt](img)](href)`). Both URLs visited.
        let mut doc = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Link {
            url: Url::unresolved("outer"),
            title: None,
            children: vec![Inline::Image {
                src: Url::unresolved("inner.png"),
                alt: "".into(),
                title: None,
            }],
        }])]);
        let mut seen: Vec<String> = Vec::new();
        visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => seen.push(s.clone()),
            _ => {}
        });
        assert_eq!(seen, vec!["outer".to_string(), "inner.png".to_string()]);
    }

    #[test]
    fn visits_urls_inside_list_items() {
        let mut doc = Document::from_blocks(vec![Block::List {
            ordered: false,
            items: vec![
                vec![paragraph_with_link("a")],
                vec![paragraph_with_link("b")],
            ],
        }]);
        let mut seen: Vec<String> = Vec::new();
        visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => seen.push(s.clone()),
            _ => {}
        });
        assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn visits_urls_inside_blockquote() {
        let mut doc = Document::from_blocks(vec![Block::BlockQuote(vec![paragraph_with_link("q")])]);
        let mut count = 0;
        visit_urls_mut(&mut doc, |_| count += 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn visits_urls_inside_table_header_and_rows() {
        let mut doc = Document::from_blocks(vec![Block::Table {
            header: vec![vec![Inline::Link {
                url: Url::unresolved("h"),
                title: None,
                children: vec![],
            }]],
            rows: vec![vec![vec![Inline::Link {
                url: Url::unresolved("r"),
                title: None,
                children: vec![],
            }]]],
        }]);
        let mut seen: Vec<String> = Vec::new();
        visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => seen.push(s.clone()),
            _ => {}
        });
        assert_eq!(seen, vec!["h".to_string(), "r".to_string()]);
    }

    #[test]
    fn visits_urls_inside_callout() {
        let mut doc = Document::from_blocks(vec![Block::Callout {
            kind: "note".into(),
            children: vec![paragraph_with_link("inside")],
        }]);
        let mut count = 0;
        visit_urls_mut(&mut doc, |_| count += 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn does_not_visit_text_or_code() {
        // Text/Code/LineBreak are leaves with no URL field; the visitor
        // must not synthesize visits.
        let mut doc = Document::from_blocks(vec![
            Block::Paragraph(vec![Inline::Text("plain".into()), Inline::Code("c".into())]),
            Block::CodeBlock {
                lang: None,
                value: "x".into(),
            },
            Block::ThematicBreak,
            Block::Other("<raw>".into()),
        ]);
        let mut count = 0;
        visit_urls_mut(&mut doc, |_| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn empty_document_visits_nothing() {
        let mut doc = Document::new();
        let mut count = 0;
        visit_urls_mut(&mut doc, |_| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn visit_blocks_walks_top_level() {
        let doc = Document::from_blocks(vec![Block::ThematicBreak, Block::Paragraph(vec![])]);
        let mut count = 0;
        visit_blocks(&doc, |_| {
            count += 1;
            true
        });
        assert_eq!(count, 2);
    }

    #[test]
    fn visit_blocks_descends_into_blockquote() {
        let doc = Document::from_blocks(vec![Block::BlockQuote(vec![Block::ThematicBreak])]);
        let mut count = 0;
        visit_blocks(&doc, |_| {
            count += 1;
            true
        });
        assert_eq!(count, 2); // BlockQuote + nested ThematicBreak
    }

    #[test]
    fn visit_blocks_descends_into_list_items() {
        let doc = Document::from_blocks(vec![Block::List {
            ordered: false,
            items: vec![vec![Block::ThematicBreak], vec![Block::ThematicBreak]],
        }]);
        let mut count = 0;
        visit_blocks(&doc, |_| {
            count += 1;
            true
        });
        assert_eq!(count, 3); // List + 2 ThematicBreaks
    }

    #[test]
    fn visit_blocks_short_circuits_when_callback_returns_false() {
        let doc = Document::from_blocks(vec![
            Block::ThematicBreak,
            Block::ThematicBreak,
            Block::ThematicBreak,
        ]);
        let mut count = 0;
        let result = visit_blocks(&doc, |_| {
            count += 1;
            count < 2 // stop after 2 visits
        });
        assert!(!result);
        assert_eq!(count, 2);
    }

    #[test]
    fn has_shortcode_recursive_returns_false_on_empty_doc() {
        // Phase A: Shortcode enum is empty. Recursive query returns false
        // for any kind. Per-kind positive-case tests land alongside
        // each Phase B migration (when a Shortcode variant exists).
        let doc = Document::new();
        assert!(!has_shortcode_recursive(&doc, ShortcodeKind::Subscribe));
        assert!(!has_shortcode_recursive(&doc, ShortcodeKind::Buttons));
    }
}
