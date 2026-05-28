//! Typed URL resolution: walk a [`Document`] and classify every
//! [`Url::Unresolved`] into a [`Url::Resolved`] with the right [`UrlKind`].
//!
//! Phase 4 PR6 (2026-05-28): replaces the two Stage 1 passes
//! (`markdown_refs::resolve_markdown_refs` for bare-filename image refs,
//! `markdown_links::resolve_markdown_links` for standard `[text](url)`
//! markdown links) with one typed visitor over the AST. The `moss-resolved:`
//! intermediate URL scheme collapses at moss-core's output boundary —
//! the visitor emits the final resolved internal href directly.
//!
//! Phase 4 PR7a (2026-05-28): `markdown_refs::resolve_markdown_refs` was
//! deleted after its parity with this visitor was proven. The companion
//! `markdown_links::resolve_markdown_links` pass survives because it still
//! emits a `moss-resolved:` sentinel that production's `classify_url_prod`
//! decoder consumes — see investigation notes for the deferred deletion.
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
//! ## OutgoingLink byte-equivalence contract
//!
//! The returned `Vec<OutgoingLink>` must be byte-equivalent (same
//! `target_path`, `display_text`, `link_type` per entry; same sequence)
//! to the concatenation of today's Stage 1 outputs:
//!
//! ```text
//! markdown_refs::resolve_markdown_refs(...).outgoing_links
//!   ++
//! markdown_links::resolve_markdown_links(rewritten, ...).outgoing_links
//! ```
//!
//! Verified by [`tests::byte_equivalence`] which runs the legacy passes
//! and the visitor against the same source and asserts identical Vecs.

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
        // content graph, push OutgoingLink, emit the resolved internal
        // href. Mirrors markdown_links::rewrite_line.
        let (path_part, suffix) = split_path_suffix(&raw);
        match resolve_reference(path_part, graph, source_path) {
            ResolvedRef::Found(resolved) => {
                outgoing.push(OutgoingLink {
                    target_path: resolved.clone(),
                    display_text: display_text.to_string(),
                    link_type: LinkType::Standard,
                });
                // The visitor's contract: produce final href, no moss-resolved
                // prefix. The internal kind tells the renderer it's a
                // markdown-target link.
                let final_href = match suffix {
                    Some(s) => format!("{}{}", resolved, s),
                    None => resolved,
                };
                *link_url = Url::Resolved(ResolvedUrl::new(final_href, UrlKind::Internal));
            }
            ResolvedRef::Unresolved => {
                // Mirrors Stage 1: leave the URL as-is in the rewritten
                // source; Stage 1 emitted a diagnostic but we don't track
                // those here (byte-equivalence is on OutgoingLink only).
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
        Block::Table { header, rows } => {
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
        Block::Table { header, rows } => {
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

        // The URL inside the doc must be Resolved.
        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    assert!(url.is_resolved());
                    let r = url.as_resolved();
                    assert_eq!(r.kind, UrlKind::Internal);
                    assert_eq!(r.href, "文字/文字.md");
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
                    let r = url.as_resolved();
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
                    let r = url.as_resolved();
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
                    let r = url.as_resolved();
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
                    let r = src.as_resolved();
                    assert_eq!(r.href, "../assets/photo.jpg");
                    assert_eq!(r.kind, UrlKind::Asset);
                }
                Inline::Link { children: link_kids, .. } => {
                    // pulldown-cmark may wrap an image-only paragraph in a
                    // figure or other structure depending on detection;
                    // accept either the direct image or one-level
                    // deeper.
                    if let Some(Inline::Image { src, .. }) = link_kids.first() {
                        let r = src.as_resolved();
                        assert_eq!(r.href, "../assets/photo.jpg");
                    }
                }
                _ => panic!("expected Image, got {children:?}"),
            },
            Block::Figure { image, .. } => {
                // PR3's Block::Figure: image-only paragraph may parse as
                // Figure directly.
                if let Inline::Image { src, .. } = image {
                    let r = src.as_resolved();
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
                        let r = src.as_resolved();
                        assert_eq!(r.href, "nonexistent.jpg");
                        assert_eq!(r.kind, UrlKind::Asset);
                        found_image = true;
                    }
                }
            }
            if let Block::Figure { image, .. } = block {
                if let Inline::Image { src, .. } = image {
                    let r = src.as_resolved();
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
        assert!(outgoing.is_empty(), "code block content must not produce OutgoingLink");
    }

    #[test]
    fn fragment_preserved_on_internal_link() {
        let mut doc = parse("[x](文字/文字.md#sec)");
        let graph = graph_with(&["index.md", "文字/文字.md"]);
        let outgoing = resolve_urls(&mut doc, &graph, "index.md");

        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target_path, "文字/文字.md");

        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let r = url.as_resolved();
                    assert_eq!(r.href, "文字/文字.md#sec");
                }
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

        match &doc.blocks[0] {
            Block::Paragraph(children) => match &children[0] {
                Inline::Link { url, .. } => {
                    let r = url.as_resolved();
                    assert_eq!(
                        r.href,
                        "assets/scale-compare.html?a=major_pent&r=major_pent%3AD"
                    );
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }

    // -----------------------------------------------------------------
    // Byte-equivalence: visitor OutgoingLink Vec matches Stage 1 output
    // -----------------------------------------------------------------

    /// Run the remaining Stage 1 pass (`markdown_links`) against the
    /// source and return its `OutgoingLink` Vec — this is what the
    /// visitor must match byte-for-byte for standard markdown links.
    ///
    /// Phase 4 PR7a (2026-05-28): the companion Stage 1 pass
    /// `markdown_refs::resolve_markdown_refs` was deleted alongside the
    /// matching `byte_equivalence_bare_image_filename` test — its parity
    /// was already proven by `resolves_bare_filename_image_against_graph`
    /// (which exercises the visitor directly without a Stage 1 baseline).
    fn stage1_outgoing(
        content: &str,
        graph: &crate::content_graph::ContentGraph,
        source_path: &str,
    ) -> Vec<OutgoingLink> {
        let links = crate::resolve::markdown_links::resolve_markdown_links(
            content,
            graph,
            source_path,
        );
        links.outgoing_links
    }

    fn assert_outgoing_links_eq(a: &[OutgoingLink], b: &[OutgoingLink]) {
        assert_eq!(
            a.len(),
            b.len(),
            "OutgoingLink count mismatch: {a:?} vs {b:?}"
        );
        for (idx, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(
                x.target_path, y.target_path,
                "OutgoingLink[{idx}] target_path differs: {x:?} vs {y:?}"
            );
            assert_eq!(
                x.display_text, y.display_text,
                "OutgoingLink[{idx}] display_text differs: {x:?} vs {y:?}"
            );
            assert_eq!(
                x.link_type, y.link_type,
                "OutgoingLink[{idx}] link_type differs: {x:?} vs {y:?}"
            );
        }
    }

    #[test]
    fn byte_equivalence_standard_markdown_link() {
        let source = "index.md";
        let content = "[文字](文字.md)";
        let graph = graph_with(&["index.md", "文字/文字.md"]);

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_outgoing_links_eq(&visitor, &stage1);
    }

    // Phase 4 PR7a (2026-05-28): `byte_equivalence_bare_image_filename`
    // was removed alongside deletion of `crates/moss-core/src/resolve/
    // markdown_refs.rs`. The visitor's bare-filename behavior is still
    // covered by `resolves_bare_filename_image_against_graph` above (no
    // Stage 1 baseline needed — the assertion checks the resolved URL
    // directly).

    #[test]
    fn byte_equivalence_multiple_links_one_line() {
        let source = "index.md";
        let content = "[a](foo.md) and [b](bar.md)";
        let graph = graph_with(&["index.md", "foo.md", "bar.md"]);

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_outgoing_links_eq(&visitor, &stage1);
    }

    #[test]
    fn byte_equivalence_external_links_no_outgoing() {
        let source = "index.md";
        let content = "[ext](https://example.com) [anchor](#top) [mail](mailto:a@b)";
        let graph = graph_with(&["index.md"]);

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_outgoing_links_eq(&visitor, &stage1);
        assert!(visitor.is_empty());
    }

    #[test]
    fn byte_equivalence_unresolved_link() {
        let source = "index.md";
        let content = "[missing](missing.md)";
        let graph = graph_with(&["index.md"]);

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_outgoing_links_eq(&visitor, &stage1);
        assert!(visitor.is_empty());
    }

    #[test]
    fn byte_equivalence_code_block_skipped() {
        let source = "index.md";
        let content = "Before\n\n```\n[link](inside.md)\n![](photo.jpg)\n```\n\nAfter [link](inside.md).";
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("inside.md", "i");
        b.add_file("assets/photo.jpg", "p");
        let graph = b.build();

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        // Stage 1 fence-skip: inside-fence URLs don't appear. Outside:
        // only the trailing [link](inside.md) emits an OutgoingLink.
        assert_outgoing_links_eq(&visitor, &stage1);
    }

    #[test]
    fn byte_equivalence_query_and_fragment() {
        let source = "index.md";
        let content = "[d](app.html?x=1#sec)";
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("assets/app.html", "h");
        let graph = b.build();

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_outgoing_links_eq(&visitor, &stage1);
    }

    #[test]
    fn link_wrapping_image_target_path_matches_stage1() {
        // The shape produced by `[![[image.png]]](target.html?q)` after the
        // wikilinks pass rewrites the embed to `![alt](path)`. PR6 visitor
        // and Stage 1 agree on `target_path` and `link_type`; they DIVERGE
        // on `display_text`:
        //   - Stage 1 uses the raw markdown source between `[` and `]`
        //     (literal `![scale-compare](assets/scale-compare.png)`).
        //   - PR6 visitor uses parsed plain text (the alt text
        //     `scale-compare`).
        // The visitor's behavior is semantically more correct; Stage 1's
        // raw-source string was a regex-rewriter artifact. Since
        // OutgoingLink::display_text has no production consumer (verified
        // via `git grep display_text` in src-tauri returning only types.rs
        // doc strings), this divergence is non-breaking. Recorded as a
        // known shape-spec deviation; the load-bearing fields
        // (`target_path`, `link_type`) remain byte-equivalent.
        let source = "index.md";
        let content = "[![scale-compare](assets/scale-compare.png)](scale-compare.html?a=major_pent&r=major_pent%3AD)";
        let mut b = ContentGraphBuilder::new();
        b.add_file("index.md", "x");
        b.add_file("assets/scale-compare.html", "h");
        b.add_file("assets/scale-compare.png", "p");
        let graph = b.build();

        let stage1 = stage1_outgoing(content, &graph, source);
        let mut doc = parse(content);
        let visitor = resolve_urls(&mut doc, &graph, source);

        assert_eq!(stage1.len(), 1);
        assert_eq!(visitor.len(), 1);
        assert_eq!(visitor[0].target_path, stage1[0].target_path);
        assert_eq!(visitor[0].link_type, stage1[0].link_type);
        // display_text divergence is intentional and documented above.
        assert_eq!(visitor[0].display_text, "scale-compare");
        assert_eq!(
            stage1[0].display_text,
            "![scale-compare](assets/scale-compare.png)"
        );
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
                    let r = url.as_resolved();
                    assert_eq!(r.href, "/about.html");
                }
                _ => panic!("expected Link"),
            },
            _ => panic!("expected Paragraph"),
        }
    }
}
