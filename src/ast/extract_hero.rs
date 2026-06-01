//! Pre-render Hero extraction.
//!
//! Walks the top-level [`Document::blocks`] looking for the first
//! `Block::Shortcode(Shortcode::Hero(_))`, removes it from the document,
//! and returns the rendered hero HTML plus the OG-fallback fields the
//! cover/description chains consume.
//!
//! # Why extract-at-caller (Phase 4 PR7a)
//!
//! Production has historically hoisted the first `:::hero` block to the
//! article template's hero slot (the rendered HTML lands in the template
//! header, separate from the body). `apply_typed_shortcodes` intercepted
//! Hero variants in the AST before they reached the HTML renderer.
//!
//! When PR7a flips production to `render_document`, the body renderer
//! walks the full block sequence. Letting it render a Hero shortcode
//! inline would duplicate the slot rendering (hero appears in BOTH the
//! template hero slot AND the body) OR force the hooks to emit nothing
//! for Hero (which the renderer can't distinguish from a real empty
//! emission).
//!
//! Extract-at-caller solves this cleanly:
//! 1. The pipeline calls `extract_hero(&mut doc, &hooks)` BEFORE
//!    `render_document(&doc, &hooks)`.
//! 2. `extract_hero` walks `doc.blocks`, finds the first Hero, calls
//!    the hooks to render it, removes the Hero block from `doc.blocks`,
//!    and returns the rendered HTML + captured OG fields.
//! 3. `render_document` then walks the hero-free block sequence; no
//!    special Hero arm needed in the renderer or hooks.
//!
//! # Why top-level only
//!
//! Per the current SoCiviC + chps fixtures (the 4 client sites at Phase 4
//! cutover), `:::hero` blocks only appear at the document top level
//! (or as the only block in the document). The extractor doesn't descend
//! into shortcode bodies. If a future fixture nests Hero inside Grid
//! cells, this function will not extract it — the renderer's hooks
//! implementation must decide what to do then (probably error or render
//! inline). Keeping the extractor top-level matches today's interception
//! semantics in `apply_typed_shortcodes`.

use super::document::Document;
use super::hooks::RenderHooks;
use super::node::{Block, Inline};
use super::shortcode::Shortcode;
use super::url::Url;

/// Captured Hero data after extraction.
///
/// The pipeline threads these into `ParsedDocument`:
/// - `html` — rendered `<section class="moss-hero">…</section>` lands in
///   the template hero slot.
/// - `image_url` — drives the homepage-hero rung of the cover chain.
/// - `overlay_text` — drives the homepage-hero rung of the description
///   chain (first-paragraph text extraction).
#[derive(Debug, Default, Clone)]
pub struct HeroExtraction {
    pub html: String,
    pub image_url: Option<String>,
    pub overlay_text: Option<String>,
}

/// Find and extract the first top-level Hero shortcode from `doc`.
///
/// Returns `Some(HeroExtraction)` if a Hero was found (and removed from
/// `doc.blocks`); returns `None` if the document has no Hero at the top
/// level.
///
/// The Hero is rendered via `hooks.render_shortcode(&mut out, sc)` — the
/// caller's `RenderHooks` impl decides the exact byte shape (production
/// uses `PipelineHooks::render_shortcode` with the Hero arm calling
/// `render_hero_html_typed`).
pub fn extract_hero(doc: &mut Document, hooks: &dyn RenderHooks) -> Option<HeroExtraction> {
    let hero_idx = doc.blocks.iter().position(|b| {
        matches!(b, Block::Shortcode(Shortcode::Hero(_)))
    })?;

    // Pop the block from the document. Keep `block_meta` in sync — both
    // vecs must remain the same length per the Document invariant
    // asserted in `render_document`.
    let hero_block = doc.blocks.remove(hero_idx);
    if hero_idx < doc.block_meta.len() {
        doc.block_meta.remove(hero_idx);
    }

    // Pattern-match again to access the typed HeroShortcode for OG-fallback
    // field capture.
    let hero_shortcode = match &hero_block {
        Block::Shortcode(sc) => sc,
        _ => return None,
    };
    let hero_args = match hero_shortcode {
        Shortcode::Hero(args) => args,
        _ => return None,
    };

    // OG-fallback fields, read directly from the typed AST (post URL
    // resolution by `resolve_urls`). The plan's Decision 1 calls this out:
    //   "captures `image_url` from `args.image` (Url::Resolved → href);
    //    captures `overlay_text` from the existing `args.overlay_text` field"
    let image_url = match &hero_args.image {
        Some(Url::Resolved(r)) => Some(r.href.clone()),
        Some(Url::Unresolved(s)) => {
            // Defensive: visit_urls_mut / resolve_urls should have
            // classified this; if not, return raw so the cover chain
            // still gets a value (silent None would erase the hero rung).
            debug_assert!(
                false,
                "Url::Unresolved({s:?}) reached extract_hero — \
                 resolve_urls missing for Hero (image)"
            );
            Some(s.clone())
        }
        None => None,
    };

    // overlay_text: walk the typed overlay first; fall back to the
    // captured-at-parse-time markdown source if the typed walk yields
    // empty.
    //
    // Plan Decision 1 notes the existing `overlay_text` field is the
    // one PR4.5 flagged as the TODO(phase4-cleanup) consumed at
    // extract-at-caller. Today we still also walk the typed Vec<Block>
    // (production builds overlay_text alongside the typed overlay, so
    // either source works); when the TODO is closed, only the typed
    // walk remains.
    let walked = first_paragraph_plain_text(&hero_args.overlay);
    let overlay_text = if !walked.trim().is_empty() {
        Some(walked)
    } else if !hero_args.overlay_text.trim().is_empty() {
        Some(hero_args.overlay_text.clone())
    } else {
        None
    };

    // Render via the hooks' Hero arm. Production's `PipelineHooks`
    // dispatches to `render_hero_html_typed` which produces the full
    // section+slot+overlay HTML.
    let mut html = String::new();
    hooks.render_shortcode(&mut html, hero_shortcode, None);

    Some(HeroExtraction {
        html,
        image_url,
        overlay_text,
    })
}

/// Walk a typed block sequence and return the first paragraph's plain
/// text (no markdown formatting). Returns empty string if no paragraph
/// is found.
///
/// Mirrors the intent of `crate::build::page::meta::extract_description`
/// but operates on the typed AST instead of markdown source — the
/// described follow-up at `HeroShortcode::overlay_text` (TODO
/// `phase4-cleanup`).
fn first_paragraph_plain_text(blocks: &[Block]) -> String {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => return inlines_plain_text(inlines),
            // Skip headings and shortcodes; the description chain wants
            // first body prose. Lists and other paragraphs follow if the
            // first hit didn't qualify.
            _ => continue,
        }
    }
    String::new()
}

/// Concatenate inline text/code/link-children content into a flat string.
fn inlines_plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(t),
            Inline::Code(c) => out.push_str(c),
            Inline::Link { children, .. }
            | Inline::Emphasis(children)
            | Inline::Strong(children) => {
                out.push_str(&inlines_plain_text(children));
            }
            Inline::LineBreak => out.push(' '),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::Other(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::hooks::DefaultHooks;
    use super::super::node::Inline;
    use super::super::shortcode::HeroShortcode;
    use super::super::url::{Url, UrlKind};
    use super::*;

    fn hero_block(image_url: Option<&str>, overlay: Vec<Block>) -> Block {
        Block::Shortcode(Shortcode::Hero(HeroShortcode {
            image: image_url.map(|u| Url::resolved(u, UrlKind::Asset)),
            attrs: String::new(),
            classes: String::new(),
            overlay,
            overlay_text: String::new(),
            width: None,
            mobile: None,
        }))
    }

    #[test]
    fn extract_hero_returns_none_when_no_hero_present() {
        let mut doc = Document::from_blocks(vec![
            Block::Paragraph(vec![Inline::Text("plain".into())]),
        ]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks);
        assert!(extraction.is_none());
        assert_eq!(doc.blocks.len(), 1, "doc unchanged when no hero");
    }

    #[test]
    fn extract_hero_removes_top_level_hero_from_doc() {
        let mut doc = Document::from_blocks(vec![
            hero_block(Some("hero.jpg"), vec![]),
            Block::Paragraph(vec![Inline::Text("body".into())]),
        ]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks).expect("hero found");
        assert!(!extraction.html.is_empty(), "hero rendered");
        assert_eq!(extraction.image_url.as_deref(), Some("hero.jpg"));
        assert_eq!(doc.blocks.len(), 1, "hero removed from doc");
        assert!(matches!(&doc.blocks[0], Block::Paragraph(_)));
    }

    #[test]
    fn extract_hero_captures_image_url_from_resolved_url() {
        let mut doc = Document::from_blocks(vec![hero_block(Some("path/to/hero.webp"), vec![])]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks).expect("hero found");
        assert_eq!(extraction.image_url.as_deref(), Some("path/to/hero.webp"));
    }

    #[test]
    fn extract_hero_image_url_none_when_no_hero_image() {
        let mut doc = Document::from_blocks(vec![hero_block(None, vec![])]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks).expect("hero found");
        assert!(extraction.image_url.is_none());
    }

    #[test]
    fn extract_hero_overlay_text_walks_typed_blocks() {
        // overlay carries typed paragraphs; first paragraph plain text
        // is the description-chain feed.
        let overlay = vec![Block::Paragraph(vec![
            Inline::Text("Hello, ".into()),
            Inline::Strong(vec![Inline::Text("world".into())]),
        ])];
        let mut doc = Document::from_blocks(vec![hero_block(None, overlay)]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks).expect("hero found");
        assert_eq!(extraction.overlay_text.as_deref(), Some("Hello, world"));
    }

    #[test]
    fn extract_hero_overlay_text_skips_leading_heading() {
        // A heading is not paragraph prose — the extractor advances to
        // the first paragraph block.
        let overlay = vec![
            Block::Heading {
                level: 2,
                children: vec![Inline::Text("Title".into())],
                id: None,
            },
            Block::Paragraph(vec![Inline::Text("Description".into())]),
        ];
        let mut doc = Document::from_blocks(vec![hero_block(None, overlay)]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks).expect("hero found");
        assert_eq!(extraction.overlay_text.as_deref(), Some("Description"));
    }

    #[test]
    fn extract_hero_finds_only_first_hero_when_multiple_present() {
        // Hero-hoisting semantics: the FIRST hero wins; subsequent
        // hero blocks stay in the body (rare authoring shape; the body
        // renderer will then have to decide what to do with them).
        let mut doc = Document::from_blocks(vec![
            hero_block(Some("a.jpg"), vec![]),
            Block::Paragraph(vec![Inline::Text("middle".into())]),
            hero_block(Some("b.jpg"), vec![]),
        ]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks).expect("hero found");
        assert_eq!(extraction.image_url.as_deref(), Some("a.jpg"));
        // After extraction: paragraph + remaining second hero.
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn extract_hero_with_no_top_level_hero_in_nested_block() {
        // Top-level only: a hero inside a blockquote is NOT extracted.
        // (Realistically authors never write this, but the contract
        // matches today's interception semantics.)
        let mut doc = Document::from_blocks(vec![Block::BlockQuote(vec![hero_block(
            Some("nested.jpg"),
            vec![],
        )])]);
        let hooks = DefaultHooks::new();
        let extraction = extract_hero(&mut doc, &hooks);
        assert!(extraction.is_none(), "nested hero NOT extracted");
        assert_eq!(doc.blocks.len(), 1);
    }
}
