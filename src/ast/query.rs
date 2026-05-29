//! Read-only structural queries over the typed AST.
//!
//! Pure helpers that walk a `Document` to extract structural facts
//! (first image, presence of a particular shortcode kind, etc.). No
//! mutation. Pure function of the AST.
//!
//! # Why this module exists
//!
//! Several pipeline-level decisions historically scanned rendered HTML
//! with regex (`first_body_image` in `cover.rs`, etc.). Phase 4
//! retires those by walking the typed AST instead. Each `find_*` helper
//! is the typed equivalent of one historical regex.
//!
//! See `docs/architecture/typed-body-ast.md` (principle: AST is data).

use super::document::Document;
use super::node::{Block, Inline};
use super::shortcode::Shortcode;

/// Find the first `Inline::Image` reachable from the document's top-level
/// block sequence.
///
/// Search order (depth-first, document order):
/// 1. `Block::Figure { image, .. }` — the image-only paragraph promoted
///    in PR3. Direct match.
/// 2. `Block::Paragraph(inlines)` — look for the first `Inline::Image`
///    appearing in the paragraph (descending into nested
///    Emphasis/Strong/Link children — matches today's
///    `transform_events::body_cover_path` capture behavior, which
///    extracts the first markdown-origin image regardless of inline
///    wrapping).
/// 3. `Block::Shortcode(_)` — descend into shortcode bodies (Grid cells,
///    Hero overlay, Gallery items) in document order. Production's
///    cover chain explicitly considers shortcode-borne images.
/// 4. `Block::List`, `Block::BlockQuote`, `Block::Callout`, `Block::Table`
///    — descend into their nested block sequences.
/// 5. `Block::LinkCard` — descend into children.
///
/// Returns `None` if no image is found anywhere reachable.
///
/// # Phase 4 PR7a (2026-05-28)
///
/// Replaces the `body_cover_path` capture currently in
/// `pipeline.rs::transform_events`. Closes the acceptance criteria of
/// issue #643 (first-body-image AST walker) on top of the typed AST.
pub fn find_first_block_image(doc: &Document) -> Option<&Inline> {
    find_first_image_in_blocks(&doc.blocks)
}

fn find_first_image_in_blocks(blocks: &[Block]) -> Option<&Inline> {
    for block in blocks {
        if let Some(img) = find_first_image_in_block(block) {
            return Some(img);
        }
    }
    None
}

fn find_first_image_in_block(block: &Block) -> Option<&Inline> {
    match block {
        // Direct hit: PR3's typed Figure variant.
        Block::Figure { image, .. } => Some(image),

        // Paragraphs may contain inline images (possibly nested inside
        // emphasis/strong/link children — the inline walker descends).
        Block::Paragraph(inlines) => find_first_image_in_inlines(inlines),

        // List items, blockquotes, callouts, table cells: descend into
        // their nested block sequences. Same depth-first document-order
        // walk as the top-level loop.
        Block::List { items, .. } => {
            for item in items {
                if let Some(img) = find_first_image_in_blocks(item) {
                    return Some(img);
                }
            }
            None
        }
        Block::BlockQuote(children) => find_first_image_in_blocks(children),
        Block::Callout { children, .. } => find_first_image_in_blocks(children),
        Block::Table { header, rows, .. } => {
            // Headers walked first (document order), then rows.
            for cell in header {
                if let Some(img) = find_first_image_in_inlines(cell) {
                    return Some(img);
                }
            }
            for row in rows {
                for cell in row {
                    if let Some(img) = find_first_image_in_inlines(cell) {
                        return Some(img);
                    }
                }
            }
            None
        }

        // Shortcodes: descend by variant. Gallery/Hero/Grid carry images.
        Block::Shortcode(sc) => find_first_image_in_shortcode(sc),

        // Compound-link grid cells (PR4.5 LinkCard) wrap block-level
        // children including images.
        Block::LinkCard { children, .. } => find_first_image_in_blocks(children),

        Block::Heading { children, .. } => find_first_image_in_inlines(children),

        Block::CodeBlock { .. } | Block::ThematicBreak | Block::Other(_) => None,
    }
}

fn find_first_image_in_inlines(inlines: &[Inline]) -> Option<&Inline> {
    for inline in inlines {
        match inline {
            Inline::Image { .. } => return Some(inline),
            Inline::Link { children, .. }
            | Inline::Emphasis(children)
            | Inline::Strong(children) => {
                if let Some(img) = find_first_image_in_inlines(children) {
                    return Some(img);
                }
            }
            Inline::Text(_) | Inline::Code(_) | Inline::LineBreak | Inline::Other(_) => {}
        }
    }
    None
}

fn find_first_image_in_shortcode(sc: &Shortcode) -> Option<&Inline> {
    match sc {
        Shortcode::Subscribe(_) | Shortcode::Buttons(_) | Shortcode::Recent(_) => None,
        Shortcode::Gallery(args) => {
            // Gallery items carry image refs but not as Inline::Image —
            // they're typed as GalleryItem { src, alt, attrs }. The
            // cover chain currently consumes hero/figure/paragraph
            // images; gallery images are not part of body_cover_path
            // today. Skip.
            //
            // If a future Phase makes Gallery items consumable here,
            // the right move is to add a synthetic Inline::Image; for
            // now matching today's transform_events behavior is the
            // gate.
            let _ = args;
            None
        }
        Shortcode::Hero(args) => {
            // Hero overlay walked first; the hero image itself is not
            // surfaced as an Inline::Image (it's a `Url` on the
            // HeroShortcode). The cover chain reads
            // `hero_image_url` directly from `apply_typed_shortcodes`
            // / extract_hero — that field is the source of truth for
            // hero-rung; body_cover_path explicitly excludes it.
            find_first_image_in_blocks(&args.overlay)
        }
        Shortcode::Grid(args) => {
            // Walk each grid cell's blocks in document order.
            for cell in &args.cells {
                if let Some(img) = find_first_image_in_blocks(cell) {
                    return Some(img);
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::node::Inline;
    use super::super::shortcode::{GridShortcode, HeroShortcode};
    use super::super::url::{Url, UrlKind};
    use super::*;

    fn img(src: &str) -> Inline {
        Inline::Image {
            src: Url::resolved(src, UrlKind::Asset),
            alt: String::new(),
            title: None,
            is_wikilink: false,
            wikilink_pothole: None,
        }
    }

    fn img_block(src: &str) -> Block {
        Block::Figure {
            image: img(src),
            caption: None,
        }
    }

    fn p_with_text(text: &str) -> Block {
        Block::Paragraph(vec![Inline::Text(text.into())])
    }

    #[test]
    fn returns_none_on_empty_doc() {
        let doc = Document::new();
        assert!(find_first_block_image(&doc).is_none());
    }

    #[test]
    fn returns_none_when_no_image_anywhere() {
        let doc = Document::from_blocks(vec![
            p_with_text("plain prose"),
            Block::Heading {
                level: 2,
                children: vec![Inline::Text("Title".into())],
                id: None,
            },
        ]);
        assert!(find_first_block_image(&doc).is_none());
    }

    #[test]
    fn finds_image_inside_figure() {
        let doc = Document::from_blocks(vec![img_block("photo.jpg")]);
        match find_first_block_image(&doc) {
            Some(Inline::Image { src, .. }) => {
                let Url::Resolved(r) = src else {
                    panic!("expected Resolved, got {src:?}")
                };
                assert_eq!(r.href, "photo.jpg");
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn finds_image_inside_paragraph() {
        let doc = Document::from_blocks(vec![Block::Paragraph(vec![
            Inline::Text("see ".into()),
            img("inline.png"),
        ])]);
        match find_first_block_image(&doc) {
            Some(Inline::Image { src, .. }) => {
                let Url::Resolved(r) = src else {
                    panic!("expected Resolved, got {src:?}")
                };
                assert_eq!(r.href, "inline.png");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn finds_image_inside_emphasis_or_link() {
        // Inline images can appear nested inside emphasis/strong/link
        // children. The walker must descend.
        let doc = Document::from_blocks(vec![Block::Paragraph(vec![Inline::Emphasis(vec![img(
            "nested.png",
        )])])]);
        match find_first_block_image(&doc) {
            Some(Inline::Image { src, .. }) => {
                let Url::Resolved(r) = src else {
                    panic!("expected Resolved, got {src:?}")
                };
                assert_eq!(r.href, "nested.png");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn picks_first_image_in_document_order() {
        let doc = Document::from_blocks(vec![
            p_with_text("intro"),
            img_block("first.jpg"),
            img_block("second.jpg"),
        ]);
        match find_first_block_image(&doc) {
            Some(Inline::Image { src, .. }) => {
                let Url::Resolved(r) = src else {
                    panic!("expected Resolved, got {src:?}")
                };
                assert_eq!(r.href, "first.jpg");
            }
            _ => panic!("expected first.jpg"),
        }
    }

    #[test]
    fn finds_image_inside_grid_cell() {
        // PR4.5: Grid cells are typed Vec<Vec<Block>>. The walker must
        // descend into the inner block sequences.
        let cell = vec![img_block("grid.png")];
        let doc = Document::from_blocks(vec![Block::Shortcode(Shortcode::Grid(GridShortcode {
            columns: 1,
            ratio: None,
            classes: String::new(),
            cells: vec![cell],
            width: None,
        }))]);
        match find_first_block_image(&doc) {
            Some(Inline::Image { src, .. }) => {
                let Url::Resolved(r) = src else {
                    panic!("expected Resolved, got {src:?}")
                };
                assert_eq!(r.href, "grid.png");
            }
            _ => panic!("expected grid.png"),
        }
    }

    #[test]
    fn finds_image_inside_blockquote() {
        let doc = Document::from_blocks(vec![Block::BlockQuote(vec![img_block("quoted.jpg")])]);
        assert!(find_first_block_image(&doc).is_some());
    }

    #[test]
    fn finds_image_inside_list_item() {
        let doc = Document::from_blocks(vec![Block::List {
            ordered: false,
            start: None,
            items: vec![vec![img_block("list.png")]],
            item_source_lines: vec![],
        }]);
        assert!(find_first_block_image(&doc).is_some());
    }

    #[test]
    fn hero_overlay_image_is_findable() {
        // Hero overlay may carry images. Until extract_hero runs, the
        // body walker still surfaces them (matches today's body_cover
        // behavior where the hero is part of the body until hoisted).
        let overlay = vec![img_block("overlay.jpg")];
        let doc = Document::from_blocks(vec![Block::Shortcode(Shortcode::Hero(HeroShortcode {
            image: Some(Url::resolved("hero.jpg", UrlKind::Asset)),
            attrs: String::new(),
            classes: String::new(),
            overlay,
            overlay_text: String::new(),
            width: None,
        }))]);
        match find_first_block_image(&doc) {
            Some(Inline::Image { src, .. }) => {
                let Url::Resolved(r) = src else {
                    panic!("expected Resolved, got {src:?}")
                };
                assert_eq!(r.href, "overlay.jpg");
            }
            _ => panic!("expected overlay image"),
        }
    }
}
