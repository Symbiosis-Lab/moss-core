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
use super::node::Block;
use super::parser::{parse_fragment_with_config, ParseConfig};
use super::shortcode::{HeroShortcode, Shortcode};
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
    // Capture source_line before removing meta so the hero template slot
    // can carry data-source-range for click-to-source in the preview.
    let hero_source_line = doc.block_meta.get(hero_idx).and_then(|m| m.source_line);
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
    // Hero is hoisted out of the body to the article template's hero slot.
    // The slot IS in the preview DOM and clickable, so we pass source_line
    // so the rendered section carries data-source-range for click-to-source.
    hooks.render_shortcode(&mut html, hero_shortcode, hero_source_line);

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
            Block::Paragraph(inlines) => {
                return crate::ast::plain_text::inlines_to_plain_text(inlines)
            }
            // Skip headings and shortcodes; the description chain wants
            // first body prose. Lists and other paragraphs follow if the
            // first hit didn't qualify.
            //
            // This walk is deliberately SHALLOW — it never descends into a
            // container, `Block::FootnoteDefinition` included. Endnote prose
            // is not the overlay's opening line, and a footnote body reached
            // by recursion would land in `<meta name="description">`.
            _ => continue,
        }
    }
    String::new()
}

// ── Source parsing: `:::hero` block → HeroShortcode ─────────────────────
//
// Moved here from `shortcode_extract` (2026-08-03): the hero concern gets
// one owner. `shortcode_extract` still routes the `:::hero` name to
// `parse_hero`; everything that decides what a hero's image IS lives here.

/// Parse a `:::hero` block in any of three syntactic forms.
///
/// Image source priority:
/// 1. `image=path` attribute in the `{...}` block (new grammar).
/// 2. **Directive-line path**: `:::hero ./path.jpg` or
///    `:::hero ./path.jpg|attrs` or `:::hero ./path.jpg {.classes}` —
///    moss-releases / client-site backward-compat. The path appears as
///    raw text before any `{...}` attribute block.
/// 3. **Body-image fallback**: scan first non-empty body line for a
///    media reference (`![[path|attrs]]`, `![alt](path|attrs)`, or
///    bare media filename). Step 3 of the grammar migration rewrites
///    these to use the `image=` attribute.
/// 4. None — renderer emits a `<section>` with no `<img>`.
///
/// Returns `(HeroShortcode, bool)` where the bool is `true` when the
/// body-image fallback (Priority 3) fired, signaling the caller to
/// emit a deprecation warning.
pub(super) fn parse_hero(args: &str, body: &str, config: &ParseConfig) -> (HeroShortcode, bool) {
    let trimmed_args = args.trim();

    // Split args on the first `{` to separate the directive-line path
    // (if any) from the attribute block (if any).
    let (positional, attr_block): (&str, &str) = if let Some(pos) = trimmed_args.find('{') {
        // char-aligned: pos points to ASCII '{' from str::find — safe to slice.
        #[allow(clippy::string_slice)]
        (trimmed_args[..pos].trim(), &trimmed_args[pos..])
    } else {
        (trimmed_args, "")
    };

    // Parse the attribute block, if present.
    let parsed = if attr_block.is_empty() {
        Default::default()
    } else {
        crate::ast::attrs::parse_attrs(attr_block).unwrap_or_default()
    };
    let classes = parsed.class_string();
    let width = parsed.width.map(str::to_string);
    let mobile = parsed.get("mobile").map(str::to_string);

    // Priority 1: `image=` attribute.
    if let Some(image_value) = parsed.get("image") {
        let (path, attrs_str) = crate::media::split_pipe(image_value);
        let overlay_text = body.trim().to_string();
        let overlay = parse_overlay_to_blocks(&overlay_text, config);
        return (
            HeroShortcode {
                image: if path.trim().is_empty() {
                    None
                } else {
                    Some(Url::unresolved(path.trim().to_string()))
                },
                extra_images: Vec::new(),
                attrs: attrs_str.to_string(),
                classes,
                overlay,
                overlay_text,
                width,
                mobile,
            },
            false,
        );
    }

    // Priority 2: directive-line path (legacy syntax). When the
    // positional text is non-empty, treat it as the image path with
    // optional `|attrs` pipe suffix. Body becomes pure overlay markdown.
    if !positional.is_empty() {
        let (path, attrs_str) = crate::media::split_pipe(positional);
        let overlay_text = body.trim().to_string();
        let overlay = parse_overlay_to_blocks(&overlay_text, config);
        return (
            HeroShortcode {
                image: if path.trim().is_empty() {
                    None
                } else {
                    Some(Url::unresolved(path.trim().to_string()))
                },
                extra_images: Vec::new(),
                attrs: attrs_str.to_string(),
                classes,
                overlay,
                overlay_text,
                width,
                mobile,
            },
            false,
        );
    }

    // Priority 3: body-image fallback. Every CONSECUTIVE leading media
    // line is a background slide (2026-07-27 multi-image hero) — the
    // first is the primary image, the rest `extra_images`; blank lines
    // between media lines don't end the run. The first non-media,
    // non-empty line starts the overlay.
    let mut overlay_lines: Vec<&str> = Vec::new();
    let mut image_path: Option<String> = None;
    let mut image_attrs = String::new();
    let mut extra_images: Vec<Url> = Vec::new();
    let mut in_media_run = true;
    let mut used_priority_3 = false;
    for line in body.lines() {
        if in_media_run {
            if line.trim().is_empty() {
                continue;
            }
            if let Some((path, attrs_str)) = parse_hero_media_line(line) {
                // A bare filename containing whitespace on a CONTINUATION
                // line is almost certainly prose that happens to end in a
                // media extension ("Photo: alpine-meadow.jpg") — treat it
                // as overlay rather than silently eating a caption. The
                // first line keeps the historical bare-filename grammar.
                let bare = !line.trim_start().starts_with("![");
                if image_path.is_some() && bare && path.contains(char::is_whitespace) {
                    // fall through: the line below ends the media run.
                } else if image_path.is_none() {
                    image_path = Some(path);
                    // Frame-level media attrs (object-fit/position) come
                    // from the primary slide and apply to every slide.
                    image_attrs = attrs_str;
                    used_priority_3 = true;
                    continue;
                } else {
                    extra_images.push(Url::unresolved(path));
                    continue;
                }
            }
            // First non-media, non-empty line — overlay starts here.
            in_media_run = false;
        }
        overlay_lines.push(line);
    }
    let overlay_text = overlay_lines.join("\n").trim().to_string();
    let overlay = parse_overlay_to_blocks(&overlay_text, config);
    (
        HeroShortcode {
            image: image_path.map(Url::unresolved),
            extra_images,
            attrs: image_attrs,
            classes,
            overlay,
            overlay_text,
            width,
            mobile,
        },
        used_priority_3,
    )
}

/// Parse a hero overlay's raw markdown source into `Vec<Block>`.
///
/// Phase 4 PR4.5 (2026-05-28): mirrors `parse_cell_to_blocks` for the
/// grid-cell path but without compound-link detection (an overlay is not
/// a compound-link surface; the SoCiviC pattern is grid-cell-specific).
/// Returns an empty vec when the overlay is empty.
fn parse_overlay_to_blocks(raw: &str, config: &ParseConfig) -> Vec<Block> {
    if raw.is_empty() {
        return Vec::new();
    }
    let doc = parse_fragment_with_config(raw, config);
    doc.blocks
}

/// File extensions recognized as media for hero body-image fallback.
const HERO_MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "avif", "svg", "mp4", "webm", "mov",
];

fn is_bare_hero_media(s: &str) -> bool {
    let (path_part, _) = crate::media::split_pipe(s);
    let path = path_part.trim();
    path.rfind('.')
        .map(|dot| {
            // char-aligned: dot points to ASCII '.' from str::rfind — `dot + 1`
            // lands on the byte after '.', which is also a char boundary.
            #[allow(clippy::string_slice)]
            let ext = &path[dot + 1..];
            HERO_MEDIA_EXTENSIONS
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

/// Parse a line as a media reference. Returns `(path, attrs_str)`.
fn parse_hero_media_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();

    // Wikilink embed: ![[path|attrs]]
    if let Some(inner) = trimmed
        .strip_prefix("![[")
        .and_then(|s| s.strip_suffix("]]"))
    {
        let (path, attrs_str) = crate::media::split_pipe(inner);
        return Some((path.trim().to_string(), attrs_str.to_string()));
    }

    // Standard markdown image: ![alt](path|attrs)
    if trimmed.starts_with("![") {
        if let Some(paren_open) = trimmed.find("](") {
            if trimmed.ends_with(')') {
                // char-aligned: paren_open points to ASCII "](" from str::find
                // (paren_open + 2 lands on first byte after `](`, char boundary);
                // `trimmed.len() - 1` is the byte before the trailing ASCII ')'.
                #[allow(clippy::string_slice)]
                let inner = &trimmed[paren_open + 2..trimmed.len() - 1];
                let (path, attrs_str) = crate::media::split_pipe(inner);
                return Some((path.trim().to_string(), attrs_str.to_string()));
            }
        }
    }

    // Bare media filename: photo.jpg or photo.jpg|contain
    if is_bare_hero_media(trimmed) {
        let (path, attrs_str) = crate::media::split_pipe(trimmed);
        return Some((path.trim().to_string(), attrs_str.to_string()));
    }

    None
}
#[cfg(test)]
#[path = "extract_hero_tests.rs"]
mod tests;
