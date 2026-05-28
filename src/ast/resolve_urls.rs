//! Typed URL resolution: walk a [`Document`] and classify every
//! [`Url::Unresolved`] into a [`Url::Resolved`] with the right [`UrlKind`].
//!
//! Phase 4 PR6 (2026-05-28): replaces the two Stage 1 passes
//! (`markdown_refs::resolve_markdown_refs` for bare-filename image refs,
//! `markdown_links::resolve_markdown_links` for standard `[text](url)`
//! markdown links) with one typed visitor over the AST.
//!
//! Phase 4 PR7a (2026-05-28): `markdown_refs::resolve_markdown_refs` was
//! deleted after its parity with this visitor was proven.
//!
//! Phase 4 PR7a-stage1b (2026-05-28): `markdown_links::resolve_markdown_links`
//! was deleted in this PR. The visitor's `resolve_link_urls` now emits the
//! same `moss-resolved:<path>` sentinel Stage 1 emitted, leaving the URL
//! as `Url::Unresolved` so src-tauri's `classify_url_prod` decoder can
//! apply `page_map` / `external_url_map` / wikilink-class-aware decoding
//! unchanged. The sentinel IS the moss-core ↔ src-tauri layering seam:
//! moss-core resolves filesystem paths, src-tauri owns the deployed URL
//! space.
//!
//! ## Why one function, not two
//!
//! Stage 1 split bare-image refs and standard-link refs into separate
//! line-level passes because each had its own source-rewriting needs
//! (bare images got a relative path; standard links got a `moss-resolved:`
//! prefix that downstream code decoded). The typed AST distinguishes
//! `Inline::Image::src` (image refs, always asset URLs) from
//! `Inline::Link::url` (standard links, may be markdown targets or assets)
//! structurally — one walk classifies both correctly.
//!
//! ## Fence-awareness is automatic
//!
//! Stage 1 carried 100+ lines of fence-tracking regex per pass to skip
//! code blocks (since both passes scanned raw markdown text). The typed
//! AST handles this structurally — `Block::CodeBlock` and inline
//! `Inline::Code` are not visited by [`visit_urls_mut`]. The visitor
//! never sees a URL inside a code fence.
//!
//! ## OutgoingLink contract
//!
//! The returned `Vec<OutgoingLink>` carries the same load-bearing
//! shape (target_path, link_type, document-order sequence) Stage 1's
//! `markdown_refs::resolve_markdown_refs` + `markdown_links::
//! resolve_markdown_links` produced before deletion. The visitor uses
//! parsed inline text for `display_text`; Stage 1 used the raw source
//! between `[` and `]`. Since `display_text` has no production
//! consumer, this divergence is non-breaking — recorded as a known
//! shape-spec deviation in `link_wrapping_image_target_path`.

use super::document::Document;
use super::node::{Block, Inline};
use super::shortcode::Shortcode;
use super::url::{ResolvedUrl, Url, UrlKind};
use super::visit::visit_urls_mut;
use crate::content_graph::ContentGraph;
use crate::resolve::fuzzy_path::{relative_asset_path, resolve_reference, ResolvedRef};
use crate::resolve::{LinkType, OutgoingLink};

/// Walk every URL in `doc` and classify it into [`Url::Resolved`].
///
/// Returns the list of [`OutgoingLink`] entries discovered during resolution
/// — byte-equivalent to today's Stage 1 `markdown_refs` + `markdown_links`
/// combined output (same shape, same sequence).
///
/// # Arguments
///
/// * `doc` — the typed document. Every [`Url::Unresolved`] is replaced in
///   place with a [`Url::Resolved`]. URLs that are already [`Url::Resolved`]
///   are left untouched (idempotent on a resolved document).
/// * `graph` — the content graph for bare-filename / cross-page lookups.
/// * `source_path` — the file containing the URLs, used by
///   [`resolve_reference`] for relative-path disambiguation and by
///   [`relative_asset_path`] for computing relative asset hrefs.
pub fn resolve_urls(
    doc: &mut Document,
    graph: &ContentGraph,
    source_path: &str,
) -> Vec<OutgoingLink> {
    // Phase 1: walk asset URLs (image refs) and accumulate their
    // OutgoingLink entries. This pass mirrors today's
    // `markdown_refs::resolve_markdown_refs` — it only touches asset URLs
    // and produces OutgoingLink for resolved bare-filename images.
    let mut outgoing: Vec<OutgoingLink> = Vec::new();
    resolve_image_urls(doc, graph, source_path, &mut outgoing);

    // Phase 2: walk link URLs and accumulate their OutgoingLink entries.
    // Mirrors today's `markdown_links::resolve_markdown_links` — only
    // touches link URLs and produces OutgoingLink for resolved cross-page
    // links.
    //
    // Two-pass ordering matches Stage 1's resolve.rs sequence (refs first,
    // then links). The image-URL display_text comes from alt; the link-URL
    // display_text comes from the link text. Each phase appends to the
    // shared `outgoing` Vec in document order.
    resolve_link_urls(doc, graph, source_path, &mut outgoing);

    // Phase 3 (NOT done by default): the renderer's invariant requires
    // every URL be `Url::Resolved` at HTML emission time. For non-graph
    // URLs the visitor left as `Url::Unresolved` (resolver-prefixed,
    // anchors that fell through, edge cases), the caller is responsible
    // for one more classification pass before rendering. Callers that
    // need a complete classification can call
    // [`classify_remaining_urls`] explicitly. The src-tauri host pipeline
    // chains a second `visit_urls_mut` to apply its `classify_url_prod`
    // for page_map-aware decoding of the three sentinel prefixes.

    outgoing
}

// ---------------------------------------------------------------------------
// Phase 1: image (Inline::Image::src) URL resolution
// ---------------------------------------------------------------------------

/// Walk every `Inline::Image::src` URL and resolve bare-filename references.
///
/// Mirrors `markdown_refs::resolve_markdown_refs`:
/// - Only touches Inline::Image::src URLs (not Link URLs).
/// - Bare filename + has-extension + no-path-separators → graph lookup.
/// - On `Found`: rewrite to relative asset path; push OutgoingLink.
/// - On `Unresolved`: leave URL as author-input (mark resolved-as-asset so
///   the renderer accepts it).
/// - Pipe-bearing URLs pass through unchanged (Phase 3 PR3 contract).
/// - External / data / mailto / anchor / explicit-relative pass through.
fn resolve_image_urls(
    doc: &mut Document,
    graph: &ContentGraph,
    source_path: &str,
    outgoing: &mut Vec<OutgoingLink>,
) {
    walk_inline_images_mut(doc, &mut |inline| {
        let (src, alt) = match inline {
            Inline::Image { src, alt, .. } => (src, alt.clone()),
            _ => return,
        };

        let raw = match src {
            Url::Unresolved(s) => s.clone(),
            Url::Resolved(_) => return,
        };

        // Pipe-bearing URLs pass through unchanged (Phase 3 PR3): authors
        // use `![[file.jpg|attrs]]` for typed params; pipe in standard
        // markdown URL is literal and intentionally 404s.
        if raw.contains('|') {
            *src = Url::Resolved(ResolvedUrl::new(raw, UrlKind::Asset));
            return;
        }

        if !is_bare_filename(&raw) {
            // Pass through unchanged — external, anchor, data, relative-prefix,
            // path-with-separator. Mark as Asset kind for image URLs.
            *src = Url::Resolved(ResolvedUrl::new(raw, UrlKind::Asset));
            return;
        }

        match resolve_reference(&raw, graph, source_path) {
            ResolvedRef::Found(target_path) => {
                let resolved_url = relative_asset_path(source_path, &target_path);
                outgoing.push(OutgoingLink {
                    target_path,
                    display_text: alt,
                    link_type: LinkType::Standard,
                });
                *src = Url::Resolved(ResolvedUrl::new(resolved_url, UrlKind::Asset));
            }
            ResolvedRef::Unresolved => {
                // Leave as-is (matches Stage 1 behavior: pass through, no
                // diagnostic). Mark Asset so render invariant holds.
                *src = Url::Resolved(ResolvedUrl::new(raw, UrlKind::Asset));
            }
        }
    });
}

/// Bare-filename detection — mirrors `markdown_refs::is_bare_filename`.
///
/// A URL is a "bare filename" when it has no path separator, no protocol,
/// no fragment-only `#`, no relative `./` `../` prefix, and carries a
/// file extension (one or more chars after the rightmost `.`).
fn is_bare_filename(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    if url.starts_with('#') {
        return false;
    }
    if url.starts_with("./") || url.starts_with("../") {
        return false;
    }
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("//")
        || url.starts_with("data:")
        || url.starts_with("mailto:")
    {
        return false;
    }
    if url.contains('/') || url.contains('\\') {
        return false;
    }
    if let Some(dot_pos) = url.rfind('.') {
        dot_pos > 0 && dot_pos < url.len() - 1
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Phase 2: link (Inline::Link::url + Block::LinkCard::url) URL resolution
// ---------------------------------------------------------------------------

/// Walk every link URL (Inline::Link::url, Block::LinkCard::url) and
/// resolve markdown / asset targets via the content graph.
///
/// Mirrors `markdown_links::resolve_markdown_links`:
/// - Only touches Link URLs (image URLs were handled in phase 1).
/// - Resolvable targets (not external / not anchor / not protocol /
///   not absolute-path / not already-prefixed) → graph lookup.
/// - On `Found`: classify into Internal (markdown) / Asset (binary) and
///   push OutgoingLink with target_path = resolved path.
/// - On `Unresolved`: leave URL author-input; Stage 1 emitted a diagnostic
///   here, but PR6 mirrors the byte-equivalence contract (no diagnostic in
///   the OutgoingLink Vec since Diagnostic is a separate stream).
/// - Anchor / mailto / tel / external pass through with the matching
///   UrlKind so the renderer attaches the right attributes.
fn resolve_link_urls(
    doc: &mut Document,
    graph: &ContentGraph,
    source_path: &str,
    outgoing: &mut Vec<OutgoingLink>,
) {
    walk_links_mut(doc, &mut |link_url, display_text| {
        let raw = match link_url {
            Url::Unresolved(s) => s.clone(),
            Url::Resolved(_) => return,
        };

        // Author-facing short-circuits: classify and stop.
        if let Some(rest) = raw.strip_prefix("mailto:") {
            *link_url = Url::Resolved(ResolvedUrl::new(format!("mailto:{rest}"), UrlKind::Mailto));
            return;
        }
        if let Some(rest) = raw.strip_prefix("tel:") {
            *link_url = Url::Resolved(ResolvedUrl::new(format!("tel:{rest}"), UrlKind::Tel));
            return;
        }
        if raw.starts_with('#') {
            *link_url = Url::Resolved(ResolvedUrl::new(raw, UrlKind::Anchor));
            return;
        }
        if raw.starts_with("http://")
            || raw.starts_with("https://")
            || raw.starts_with("//")
            || raw.starts_with("data:")
        {
            *link_url = Url::Resolved(ResolvedUrl::new(raw, UrlKind::External));
            return;
        }

        // Stage 1 carry-over: URLs already prefixed with a resolver
        // sentinel (`moss-resolved:`, `moss-newtab:`, `wikilink:`) carry
        // Stage 1 / upstream state the visitor cannot decode in isolation
        // — the final pretty URL depends on the host's `page_map`, which
        // lives in src-tauri's pipeline context. Leave these as
        // `Url::Unresolved` so the host's per-URL classifier
        // (`classify_url_prod` in src-tauri's pipeline) can apply the
        // page_map-aware decoding. This preserves the byte-equivalence
        // contract (no OutgoingLink emitted for already-resolved targets
        // — Stage 1 already counted them) while letting the host close
        // the prefix-decoding loop.
        if raw.starts_with("moss-resolved:")
            || raw.starts_with("moss-newtab:")
            || raw.starts_with("wikilink:")
        {
            // Leave Unresolved; host pass classifies.
            return;
        }

        // Absolute filesystem path — treat as opaque. Mirrors
        // markdown_links: `if url.starts_with('/') { return false; }`.
        if raw.starts_with('/') {
            *link_url = Url::Resolved(ResolvedUrl::new(raw, UrlKind::Internal));
            return;
        }

        // Resolvable: split query/fragment, look up the path against the
        // content graph, push OutgoingLink, emit the `moss-resolved:`
        // sentinel for the host classifier. Mirrors
        // markdown_links::rewrite_line byte-for-byte: same sentinel shape
        // (`moss-resolved:<path>[<suffix>]`), same suffix concatenation.
        //
        // Phase 4 PR7a-stage1b (2026-05-28): moss-core resolves the
        // filesystem path; src-tauri's `classify_url_prod` decodes the
        // sentinel into the final pretty / external / asset URL using
        // `page_map`, `external_url_map`, and the wikilink-class signal.
        // The sentinel IS the moss-core ↔ src-tauri layering seam — the
        // visitor must NOT collapse it to a final `Url::Resolved` or
        // page_map decoding silently breaks.
        let (path_part, suffix) = split_path_suffix(&raw);
        match resolve_reference(path_part, graph, source_path) {
            ResolvedRef::Found(resolved) => {
                outgoing.push(OutgoingLink {
                    target_path: resolved.clone(),
                    display_text: display_text.to_string(),
                    link_type: LinkType::Standard,
                });
                let sentinel = match suffix {
                    Some(s) => format!("moss-resolved:{}{}", resolved, s),
                    None => format!("moss-resolved:{}", resolved),
                };
                *link_url = Url::Unresolved(sentinel);
            }
            ResolvedRef::Unresolved => {
                // Mirrors Stage 1: leave the URL as-is in the rewritten
                // source — no `moss-resolved:` prefix, no diagnostic in
                // the OutgoingLink Vec. Mark Internal so the renderer's
                // `Url::Resolved` invariant holds.
                *link_url = Url::Resolved(ResolvedUrl::new(raw, UrlKind::Internal));
            }
        }
    });
}

/// Split a URL into (path, suffix) where `suffix` is `?query` and/or
/// `#fragment` in source order. Mirrors
/// `markdown_links::split_path_suffix`.
fn split_path_suffix(url: &str) -> (&str, Option<&str>) {
    let q = url.find('?');
    let h = url.find('#');
    let cut = match (q, h) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    match cut {
        #[allow(clippy::string_slice)]
        Some(pos) => (&url[..pos], Some(&url[pos..])),
        None => (url, None),
    }
}

// ---------------------------------------------------------------------------
// Phase 3: ensure no Url::Unresolved survives
// ---------------------------------------------------------------------------

/// Classify any URL left as `Url::Unresolved` after phases 1 + 2 into a
/// best-effort `Url::Resolved`. The renderer's invariant requires no
/// `Url::Unresolved` reaches HTML emission; this is the safety net that
/// catches URLs the per-kind phases didn't visit (e.g., a future
/// `Inline::Link` variant added before its phase-2 arm is wired).
///
/// Callers that follow [`resolve_urls`] with their own per-URL classifier
/// (e.g., src-tauri's pipeline calling `classify_url_prod` for
/// resolver-prefix decoding) should NOT call this — let the secondary
/// classifier handle the remaining URLs. Callers that have no secondary
/// pass should call this to maintain the render invariant.
pub fn classify_remaining_urls(doc: &mut Document) {
    visit_urls_mut(doc, |url| {
        if let Url::Unresolved(raw) = url {
            // Conservative fallback: treat as External (renders verbatim,
            // no class/target attrs, no graph lookup). External is the
            // least-special UrlKind — safer than guessing Internal.
            let kind = classify_unresolved_kind(raw);
            let raw_owned = std::mem::take(raw);
            *url = Url::Resolved(ResolvedUrl::new(raw_owned, kind));
        }
    });
}

/// Best-effort kind classification for an Unresolved URL that escaped the
/// per-kind phases. Mirrors the prefix-based detection in
/// `pipeline::classify_url_prod` for consistency.
fn classify_unresolved_kind(raw: &str) -> UrlKind {
    if raw.starts_with("mailto:") {
        UrlKind::Mailto
    } else if raw.starts_with("tel:") {
        UrlKind::Tel
    } else if raw.starts_with('#') {
        UrlKind::Anchor
    } else if raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("//")
        || raw.starts_with("data:")
    {
        UrlKind::External
    } else {
        UrlKind::Internal
    }
}

// ---------------------------------------------------------------------------
// Per-kind walkers (image-only / link-only)
// ---------------------------------------------------------------------------

/// Walk every `Inline::Image` in the document and invoke `f` with a `&mut`
/// reference to the inline. Used by phase 1 — separates image src
/// classification from link URL classification.
fn walk_inline_images_mut<F>(doc: &mut Document, f: &mut F)
where
    F: FnMut(&mut Inline),
{
    for block in &mut doc.blocks {
        walk_images_in_block(block, f);
    }
}

fn walk_images_in_block<F>(block: &mut Block, f: &mut F)
where
    F: FnMut(&mut Inline),
{
    match block {
        Block::Heading { children, .. } | Block::Paragraph(children) => {
            for inline in children {
                walk_images_in_inline(inline, f);
            }
        }
        Block::Callout { children, .. } | Block::BlockQuote(children) => {
            for nested in children {
                walk_images_in_block(nested, f);
            }
        }
        Block::List { items, .. } => {
            for item_blocks in items {
                for nested in item_blocks {
                    walk_images_in_block(nested, f);
                }
            }
        }
        Block::Table { header, rows, .. } => {
            for cell in header {
                for inline in cell {
                    walk_images_in_inline(inline, f);
                }
            }
            for row in rows {
                for cell in row {
                    for inline in cell {
                        walk_images_in_inline(inline, f);
                    }
                }
            }
        }
        Block::Shortcode(sc) => {
            walk_images_in_shortcode(sc, f);
        }
        Block::Figure { image, caption } => {
            walk_images_in_inline(image, f);
            if let Some(cap) = caption {
                for inline in cap {
                    walk_images_in_inline(inline, f);
                }
            }
        }
        Block::LinkCard { children, .. } => {
            for nested in children {
                walk_images_in_block(nested, f);
            }
        }
        Block::CodeBlock { .. } | Block::ThematicBreak | Block::Other(_) => {}
    }
}

fn walk_images_in_shortcode<F>(sc: &mut Shortcode, f: &mut F)
where
    F: FnMut(&mut Inline),
{
    match sc {
        Shortcode::Subscribe(_) | Shortcode::Buttons(_) | Shortcode::Recent(_) => {}
        Shortcode::Gallery(args) => {
            // Gallery items are not Inline::Image (they carry Url directly
            // in GalleryItem::src). Skip — gallery URLs are handled by
            // the generic classify_remaining_urls phase. Today's Stage 1
            // didn't process gallery srcs either.
            let _ = args;
        }
        Shortcode::Hero(args) => {
            // Hero's image: a Url::Unresolved wrapped directly, not in an
            // Inline::Image. Skip — handled by classify_remaining_urls.
            // The overlay blocks may contain Inline::Images; descend.
            let _ = &args.image;
            for block in &mut args.overlay {
                walk_images_in_block(block, f);
            }
        }
        Shortcode::Grid(args) => {
            for cell_blocks in &mut args.cells {
                for block in cell_blocks {
                    walk_images_in_block(block, f);
                }
            }
        }
    }
}

fn walk_images_in_inline<F>(inline: &mut Inline, f: &mut F)
where
    F: FnMut(&mut Inline),
{
    match inline {
        Inline::Image { .. } => {
            f(inline);
        }
        Inline::Link { children, .. } => {
            for nested in children {
                walk_images_in_inline(nested, f);
            }
        }
        Inline::Emphasis(children) | Inline::Strong(children) => {
            for nested in children {
                walk_images_in_inline(nested, f);
            }
        }
        Inline::Text(_) | Inline::Code(_) | Inline::LineBreak | Inline::Other(_) => {}
    }
}

/// Walk every link URL in the document (Inline::Link::url +
/// Block::LinkCard::url) and invoke `f` with `(&mut Url, display_text)`.
///
/// The `display_text` is the link text (concatenated from the Link's
/// children) — needed for the OutgoingLink::display_text contract.
fn walk_links_mut<F>(doc: &mut Document, f: &mut F)
where
    F: FnMut(&mut Url, &str),
{
    for block in &mut doc.blocks {
        walk_links_in_block(block, f);
    }
}

fn walk_links_in_block<F>(block: &mut Block, f: &mut F)
where
    F: FnMut(&mut Url, &str),
{
    match block {
        Block::Heading { children, .. } | Block::Paragraph(children) => {
            for inline in children {
                walk_links_in_inline(inline, f);
            }
        }
        Block::Callout { children, .. } | Block::BlockQuote(children) => {
            for nested in children {
                walk_links_in_block(nested, f);
            }
        }
        Block::List { items, .. } => {
            for item_blocks in items {
                for nested in item_blocks {
                    walk_links_in_block(nested, f);
                }
            }
        }
        Block::Table { header, rows, .. } => {
            for cell in header {
                for inline in cell {
                    walk_links_in_inline(inline, f);
                }
            }
            for row in rows {
                for cell in row {
                    for inline in cell {
                        walk_links_in_inline(inline, f);
                    }
                }
            }
        }
        Block::Shortcode(sc) => {
            walk_links_in_shortcode(sc, f);
        }
        Block::Figure { caption, .. } => {
            if let Some(cap) = caption {
                for inline in cap {
                    walk_links_in_inline(inline, f);
                }
            }
        }
        Block::LinkCard { url, children } => {
            // Compound-link card: the wrapping href is a link URL. Use
            // the inner text content as display_text by recursively
            // gathering it from the children (best-effort — empty string
            // if no text is found).
            let display = gather_text_blocks(children);
            f(url, &display);
            for nested in children {
                walk_links_in_block(nested, f);
            }
        }
        Block::CodeBlock { .. } | Block::ThematicBreak | Block::Other(_) => {}
    }
}

fn walk_links_in_shortcode<F>(sc: &mut Shortcode, f: &mut F)
where
    F: FnMut(&mut Url, &str),
{
    match sc {
        Shortcode::Subscribe(_) | Shortcode::Recent(_) => {}
        Shortcode::Buttons(args) => {
            for item in &mut args.items {
                // ButtonItem display text comes from item.text per the
                // shortcode shape (crates/moss-core/src/ast/shortcode.rs).
                let text = item.text.clone();
                f(&mut item.url, &text);
            }
        }
        Shortcode::Gallery(_) => {
            // Gallery items use src URLs (image-kind), not link URLs.
            // No link-walk action.
        }
        Shortcode::Hero(args) => {
            for block in &mut args.overlay {
                walk_links_in_block(block, f);
            }
        }
        Shortcode::Grid(args) => {
            for cell_blocks in &mut args.cells {
                for block in cell_blocks {
                    walk_links_in_block(block, f);
                }
            }
        }
    }
}

fn walk_links_in_inline<F>(inline: &mut Inline, f: &mut F)
where
    F: FnMut(&mut Url, &str),
{
    match inline {
        Inline::Link { url, children, .. } => {
            // display_text = concatenated plain text of the children.
            // Matches markdown_links::rewrite_line, which uses the raw
            // text between `[` and `]` (no rendering, just the literal).
            let display = gather_text_inlines(children);
            f(url, &display);
            // Descend so nested Links (rare in CommonMark but possible
            // via parser quirks) get visited too.
            for nested in children {
                walk_links_in_inline(nested, f);
            }
        }
        Inline::Image { .. } => {
            // Image src is a Url but it's image-kind — handled by phase 1.
        }
        Inline::Emphasis(children) | Inline::Strong(children) => {
            for nested in children {
                walk_links_in_inline(nested, f);
            }
        }
        Inline::Text(_) | Inline::Code(_) | Inline::LineBreak | Inline::Other(_) => {}
    }
}

/// Concatenate the plain-text content of a list of inlines, mirroring
/// pulldown-cmark's behavior of treating link text as a verbatim string.
/// Used to populate `OutgoingLink::display_text`.
fn gather_text_inlines(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for inline in inlines {
        gather_text_inline(inline, &mut s);
    }
    s
}

fn gather_text_inline(inline: &Inline, out: &mut String) {
    match inline {
        Inline::Text(t) => out.push_str(t),
        Inline::Code(c) => out.push_str(c),
        Inline::Emphasis(children) | Inline::Strong(children) => {
            for nested in children {
                gather_text_inline(nested, out);
            }
        }
        Inline::Link { children, .. } => {
            for nested in children {
                gather_text_inline(nested, out);
            }
        }
        Inline::Image { alt, .. } => out.push_str(alt),
        Inline::LineBreak => out.push('\n'),
        Inline::Other(_) => {}
    }
}

/// Concatenate the plain-text content of a list of blocks. Used by
/// Block::LinkCard arm to populate the OutgoingLink::display_text.
fn gather_text_blocks(blocks: &[Block]) -> String {
    let mut s = String::new();
    for block in blocks {
        gather_text_block(block, &mut s);
    }
    s
}

fn gather_text_block(block: &Block, out: &mut String) {
    match block {
        Block::Heading { children, .. } | Block::Paragraph(children) => {
            for inline in children {
                gather_text_inline(inline, out);
            }
        }
        Block::Figure { image, caption } => {
            if let Inline::Image { alt, .. } = image {
                out.push_str(alt);
            }
            if let Some(cap) = caption {
                for inline in cap {
                    gather_text_inline(inline, out);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::parser::parse;
    use crate::content_graph::ContentGraphBuilder;

    fn graph_with(paths: &[&str]) -> crate::content_graph::ContentGraph {
        let mut b = ContentGraphBuilder::new();
        for p in paths {
            b.add_file(p, p);
        }
        b.build()
    }

    // -----------------------------------------------------------------
    // Single-shot resolve_urls behavior
    // -----------------------------------------------------------------

    #[test]
    fn resolves_standard_markdown_link_to_internal() {
        let mut doc = parse("[文字](文字.md)");
        let graph = graph_with(&["index.md", "文字/文字.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");

        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_path, "文字/文字.md");
        assert_eq!(outgoing[0].display_text, "文字");
        assert_eq!(outgoing[0].link_type, LinkType::Standard);

        // Phase 4 PR7a-stage1b (2026-05-28): the visitor emits a
        // `moss-resolved:` sentinel for internal links (Url::Unresolved)
        // so src-tauri's host classifier can decode it via page_map.
        // The renderer doesn't see this state — the host's
        // `classify_url_prod` pass replaces Unresolved before render.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    assert!(url.is_unresolved(), "expected sentinel, got: {url:?}");
                    match url {
                        Url::Unresolved(s) => assert_eq!(s, "moss-resolved:文字/文字.md"),
                        Url::Resolved(_) => unreachable!(),
                    }
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn passes_through_external_link() {
        let mut doc = parse("[ex](https://example.com)");
        let graph = graph_with(&["index.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");

        assert!(outgoing.is_empty());
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let Url::Resolved(r) = url else {
                        panic!("expected Resolved, got {url:?}")
                    };
                    assert_eq!(r.kind, UrlKind::External);
                    assert_eq!(r.href, "https://example.com");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn classifies_anchor_link() {
        let mut doc = parse("[top](#top)");
        let graph = graph_with(&["index.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");
        assert!(outgoing.is_empty());
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let Url::Resolved(r) = url else {
                        panic!("expected Resolved, got {url:?}")
                    };
                    assert_eq!(r.kind, UrlKind::Anchor);
                    assert_eq!(r.href, "#top");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn classifies_mailto() {
        let mut doc = parse("[Mail](mailto:test@example.com)");
        let graph = graph_with(&["index.md"]);
        let _ = resolve_urls(&mut doc, &graph, "index.md");
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let Url::Resolved(r) = url else {
                        panic!("expected Resolved, got {url:?}")
                    };
                    assert_eq!(r.kind, UrlKind::Mailto);
                    assert_eq!(r.href, "mailto:test@example.com");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn resolves_bare_filename_image_against_graph() {
        let mut doc = parse("![My Photo](photo.jpg)");
        let mut b = ContentGraphBuilder::new();
        b.add_file("assets/photo.jpg", "photo");
        let graph = b.build();
        let outgoing = resolve_urls(&mut doc, &graph, "articles/post.md");

        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_path, "assets/photo.jpg");
        assert_eq!(outgoing[0].display_text, "My Photo");
        assert_eq!(outgoing[0].link_type, LinkType::Standard);

        // Image src rewritten to relative asset path.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Image { src, .. } => {
                    let Url::Resolved(r) = src else {
                        panic!("expected Resolved, got {src:?}")
                    };
                    assert_eq!(r.href, "../assets/photo.jpg");
                    assert_eq!(r.kind, UrlKind::Asset);
                }
                Inline::Link {
                    children: link_kids,
                    ..
                } => {
                    // pulldown-cmark may wrap an image-only paragraph in a
                    // figure or other structure depending on detection;
                    // accept either the direct image or one-level
                    // deeper.
                    if let Some(Inline::Image { src, .. }) = link_kids.first() {
                        let Url::Resolved(r) = src else {
                            panic!("expected Resolved, got {src:?}")
                        };
                        assert_eq!(r.href, "../assets/photo.jpg");
                    }
                }
                _ => panic!("expected Image, got {children:?}"),
            },
            Block::Figure { image, .. } => {
                // PR3's Block::Figure: image-only paragraph may parse as
                // Figure directly.
                if let Inline::Image { src, .. } = image {
                    let Url::Resolved(r) = src else {
                        panic!("expected Resolved, got {src:?}")
                    };
                    assert_eq!(r.href, "../assets/photo.jpg");
                }
            }
            _ => panic!("expected Paragraph or Figure, got {:?}", doc.blocks[0]),
        }
    }

    #[test]
    fn unresolved_bare_filename_passes_through() {
        let mut doc = parse("![](nonexistent.jpg)");
        let graph = graph_with(&["index.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "articles/post.md");

        assert!(outgoing.is_empty());
        // URL stays as raw "nonexistent.jpg" but becomes Resolved (Asset
        // kind) so the renderer's invariant holds.
        let mut found_image = false;
        for block in &doc.blocks {
            if let Block::Paragraph(children) = block {
                for inline in children {
                    if let Inline::Image { src, .. } = inline {
                        let Url::Resolved(r) = src else {
                            panic!("expected Resolved, got {src:?}")
                        };
                        assert_eq!(r.href, "nonexistent.jpg");
                        assert_eq!(r.kind, UrlKind::Asset);
                        found_image = true;
                    }
                }
            }
            if let Block::Figure { image, .. } = block {
                if let Inline::Image { src, .. } = image {
                    let Url::Resolved(r) = src else {
                        panic!("expected Resolved, got {src:?}")
                    };
                    assert_eq!(r.href, "nonexistent.jpg");
                    found_image = true;
                }
            }
        }
        assert!(found_image, "expected an Inline::Image in the parsed doc");
    }

    #[test]
    fn does_not_resolve_url_inside_code_block() {
        // visit_urls_mut never descends into Block::CodeBlock, so the
        // visitor never sees URLs in code fences. This matches Stage 1's
        // fence-aware behavior structurally.
        let mut doc = parse("```\n[link](inside.md)\n```\n");
        let graph = graph_with(&["index.md", "inside.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");
        assert!(
            outgoing.is_empty(),
            "code block content must not produce OutgoingLink"
        );
    }

    #[test]
    fn fragment_preserved_on_internal_link() {
        let mut doc = parse("[x](文字/文字.md#sec)");
        let graph = graph_with(&["index.md", "文字/文字.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");

        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_path, "文字/文字.md");

        // Sentinel emit: suffix concatenated verbatim after the resolved path.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => match url {
                    Url::Unresolved(s) => assert_eq!(s, "moss-resolved:文字/文字.md#sec"),
                    Url::Resolved(r) => panic!("expected sentinel, got Resolved({r:?})"),
                },
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn query_string_preserved_on_internal_link() {
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("assets/scale-compare.html", "h");
        let graph = b.build();

        let mut doc = parse("[demo](scale-compare.html?a=major_pent&r=major_pent%3AD)");
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");

        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_path, "assets/scale-compare.html");

        // Sentinel emit: suffix concatenated verbatim after the resolved path.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => match url {
                    Url::Unresolved(s) => assert_eq!(
                        s,
                        "moss-resolved:assets/scale-compare.html?a=major_pent&r=major_pent%3AD"
                    ),
                    Url::Resolved(r) => panic!("expected sentinel, got Resolved({r:?})"),
                },
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    // -----------------------------------------------------------------
    // OutgoingLink + sentinel-shape coverage
    // -----------------------------------------------------------------
    //
    // Phase 4 PR7a-stage1b (2026-05-28): the Stage 1 pass
    // `markdown_links::resolve_markdown_links` was deleted in this PR
    // alongside the matching `byte_equivalence_*` baseline helpers. The
    // visitor now emits the same `moss-resolved:<path>` sentinel Stage 1
    // emitted, byte-for-byte — proven by the per-test sentinel
    // assertions below. The companion Stage 1 pass
    // `markdown_refs::resolve_markdown_refs` was already deleted in the
    // prior PR; its parity is covered by
    // `resolves_bare_filename_image_against_graph` above.

    #[test]
    fn standard_markdown_link_emits_sentinel() {
        let source = "index.md";
        let content = "[文字](文字.md)";
        let graph = graph_with(&["index.md", "文字/文字.md"]);

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_eq!(visitor.len(), 1);
        assert_eq!(visitor[0].target_path, "文字/文字.md");
        assert_eq!(visitor[0].display_text, "文字");
        assert_eq!(visitor[0].link_type, LinkType::Standard);
        // The sentinel shape is what `classify_url_prod` in src-tauri
        // expects to decode via `page_map` / `external_url_map`.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link {
                    url: Url::Unresolved(s),
                    ..
                } => {
                    assert_eq!(s, "moss-resolved:文字/文字.md");
                }
                _ => panic!("expected Url::Unresolved sentinel, got {:?}", children[0]),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn multiple_links_one_line_emit_sentinels() {
        let source = "index.md";
        let content = "[a](foo.md) and [b](bar.md)";
        let graph = graph_with(&["index.md", "foo.md", "bar.md"]);

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_eq!(visitor.len(), 2);
        assert_eq!(visitor[0].target_path, "foo.md");
        assert_eq!(visitor[1].target_path, "bar.md");
    }

    #[test]
    fn external_links_no_outgoing() {
        let source = "index.md";
        let content = "[ext](https://example.com) [anchor](#top) [mail](mailto:a@b)";
        let graph = graph_with(&["index.md"]);

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert!(visitor.is_empty());
    }

    #[test]
    fn unresolved_link_no_outgoing() {
        let source = "index.md";
        let content = "[missing](missing.md)";
        let graph = graph_with(&["index.md"]);

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert!(visitor.is_empty());
        // The unresolved URL stays as-is (no sentinel) but is marked
        // Url::Resolved so the renderer's invariant holds.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let Url::Resolved(r) = url else {
                        panic!("expected Resolved, got {url:?}")
                    };
                    assert_eq!(r.href, "missing.md");
                    assert_eq!(r.kind, UrlKind::Internal);
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn code_block_urls_not_visited() {
        let source = "index.md";
        let content =
            "Before\n\n```\n[link](inside.md)\n![](photo.jpg)\n```\n\nAfter [link](inside.md).";
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("inside.md", "i");
        b.add_file("assets/photo.jpg", "p");
        let graph = b.build();

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        // Only the trailing `[link](inside.md)` (outside the fence) emits
        // an OutgoingLink. URLs inside `Block::CodeBlock` are not visited.
        assert_eq!(visitor.len(), 1);
        assert_eq!(visitor[0].target_path, "inside.md");
    }

    #[test]
    fn query_and_fragment_sentinel_shape() {
        let source = "index.md";
        let content = "[d](app.html?x=1#sec)";
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("assets/app.html", "h");
        let graph = b.build();

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_eq!(visitor.len(), 1);
        assert_eq!(visitor[0].target_path, "assets/app.html");
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link {
                    url: Url::Unresolved(s),
                    ..
                } => {
                    assert_eq!(s, "moss-resolved:assets/app.html?x=1#sec");
                }
                _ => panic!("expected sentinel, got {:?}", children[0]),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    #[test]
    fn link_wrapping_image_target_path() {
        // Shape produced by `[![[image.png]]](target.html?q)` after the
        // wikilinks pass rewrites the embed to `![alt](path)`.
        // Pre-PR7a-stage1b this test compared to a Stage 1 baseline that
        // used the raw markdown source between `[` and `]` for
        // display_text; the visitor uses parsed plain text (alt text).
        // That divergence was non-breaking (display_text has no
        // production consumer). With Stage 1 deleted we assert on the
        // visitor's behavior directly: load-bearing fields (target_path,
        // link_type) plus the documented display_text.
        let source = "index.md";
        let content = "[![scale-compare](assets/scale-compare.png)](scale-compare.html?a=major_pent&r=major_pent%3AD)";
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("assets/scale-compare.html", "h");
        b.add_file("assets/scale-compare.png", "p");
        let graph = b.build();

        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_eq!(visitor.len(), 1);
        assert_eq!(visitor[0].target_path, "assets/scale-compare.html");
        assert_eq!(visitor[0].link_type, LinkType::Standard);
        assert_eq!(visitor[0].display_text, "scale-compare");
    }

    // -----------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------

    #[test]
    fn pipe_bearing_image_url_unchanged() {
        let mut doc = parse("![alt](photo.jpg|contain)");
        let mut b = ContentGraphBuilder::new();
        b.add_file("assets/photo.jpg", "p");
        let graph = b.build();
        let outgoing = resolve_urls(&mut doc, &graph, "articles/post.md");

        // Phase 3 PR3 contract: pipe-bearing URLs pass through verbatim,
        // no OutgoingLink emitted.
        assert!(outgoing.is_empty());
    }

    #[test]
    fn idempotent_on_already_resolved_url() {
        // If the document already carries Resolved URLs (e.g. a previous
        // pass ran), the visitor should not double-process. Single
        // invocation should produce the SAME state.
        let mut doc = parse("[文字](文字.md)");
        let graph = graph_with(&["index.md", "文字/文字.md"]);
        let outgoing1 = resolve_urls(&mut doc, &graph, "index.md");

        let outgoing2 = resolve_urls(&mut doc, &graph, "index.md");
        // After the first pass everything is Resolved; the second pass
        // produces no new OutgoingLink entries.
        assert!(
            outgoing2.is_empty(),
            "idempotency violated: {:?}",
            outgoing2
        );
        assert_eq!(outgoing1.len(), 1);
    }

    #[test]
    fn absolute_path_passes_through() {
        let mut doc = parse("[abs](/about.html)");
        let graph = graph_with(&["index.md", "about.html"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");
        // Absolute paths bypass the graph (mirrors markdown_links).
        assert!(outgoing.is_empty());
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let Url::Resolved(r) = url else {
                        panic!("expected Resolved, got {url:?}")
                    };
                    assert_eq!(r.href, "/about.html");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }
}
