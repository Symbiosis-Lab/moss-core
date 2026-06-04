//! Render typed AST → HTML via [`RenderHooks`].
//!
//! Walks every variant; calls hooks at interceptable points. Debug-asserts
//! on `Url::Unresolved` reaching the renderer — a missing visitor is a bug.
//!
//! # Phase 4: render_document IS the production rendering path (target)
//!
//! Today (2026-05-27) this function runs as a parallel observer via
//! `observe_typed_ast` in `src-tauri/src/build/markdown/pipeline.rs`;
//! production HTML still comes from `pulldown_cmark::html::push_html` over
//! the event stream. Phase 4 PR7a flips this: `render_document` becomes
//! the production renderer, `html::push_html` is no longer called in the
//! main pipeline, and `transform_events` is reduced to a thin
//! events-to-Document adapter (or deleted).
//!
//! # Why the AST renders (not pulldown-cmark)
//!
//! Cross-SSG research (2026-05-27) — see
//! [docs/architecture/typed-ast-cross-ssg-research-2026-05-27.md](../../../../docs/architecture/typed-ast-cross-ssg-research-2026-05-27.md)
//! — confirms every AST-bearing SSG with secondary consumers (link
//! graphs, editors, validators, multi-target rendering) puts the AST at
//! the rendering source:
//!
//! - **mdBook** (same parser as moss) recently migrated from
//!   `html::push_html` to a typed `Tree<Node>` via `ego_tree`. Same
//!   destination, same motivation.
//! - **Hugo** dispatches NodeRenderer per AST node-kind; render hooks
//!   fire during AST walk.
//! - **Markdoc** ships `AstNode → RenderableTreeNode → HTML/React`.
//! - **Pandoc** has been AST-first since 2006; output is a writer per
//!   target format.
//! - **Quarto 2** is mid-migration from Stage 1 pre-parsers to AST-first
//!   for three reasons: performance, fragility, information loss.
//!
//! Streaming-only SSGs (Zola, markdown-it ecosystem) live without an AST,
//! but pay the cost: structural reshape requires fragile token-window
//! pattern matching; secondary consumers can't ride on event streams.
//! moss has secondary consumers (#599 page threading, editor's
//! `scan_shortcodes`, `has_shortcode_recursive`, future WASM editor,
//! future LSP-style diagnostics) — AST is non-optional.
//!
//! See [docs/architecture/typed-body-ast.md](../../../../docs/architecture/typed-body-ast.md)
//! for the design intent + 7 principles, and
//! [docs/plans/2026-05-27-phase4-typed-ast-completion.md](../../../../docs/plans/2026-05-27-phase4-typed-ast-completion.md)
//! for the Phase 4 execution plan.

use super::document::{BlockMeta, Document};
use super::hooks::{escape_attr, escape_text, RenderHooks};
use super::node::{Block, Fold, Inline};
use super::url::Url;

/// Render a [`Document`] to an HTML string using the given hooks.
///
/// # Panics (debug only)
///
/// If any URL is still `Url::Unresolved` when the renderer reaches it.
/// `visit_urls_mut` must run before this function. In release builds the
/// raw unresolved string is emitted as-is to avoid crashing on a bug.
pub fn render_document<H: RenderHooks>(doc: &Document, hooks: &H) -> String {
    let mut out = String::new();
    // Walk blocks + meta in lockstep. Invariant: block_meta.len() ==
    // blocks.len() (asserted in debug, defensive in release).
    debug_assert_eq!(
        doc.blocks.len(),
        doc.block_meta.len(),
        "Document invariant: blocks.len() == block_meta.len()"
    );
    for (i, block) in doc.blocks.iter().enumerate() {
        let meta = doc.block_meta.get(i).copied().unwrap_or_default();
        render_block(hooks, &mut out, block, &meta);
    }
    out
}

/// Render a sequence of blocks to HTML. Used by [`render_document`]
/// and by src-tauri's `render_hero_html_typed` (Phase 4 PR4.5) to render
/// a `Vec<Block>` that didn't come from a full `Document` (e.g. a hero
/// overlay).
///
/// **Source-line caveat:** this entry point has no per-block meta vec, so
/// every block renders without `data-source-line`. Callers that need
/// source-line annotations must walk meta-block pairs themselves (see
/// [`render_document`]). Today only [`render_document`] consumes meta;
/// nested-block walks (list items, callout bodies, blockquotes) are
/// also meta-free — `data-source-line` is a top-level-block-only
/// concern, matching the legacy `transform_events` emit shape.
///
/// `H: ?Sized` so the function can be called with `&dyn RenderHooks` or
/// with `self: &Self` from inside a trait default method (where `Self`
/// is not statically `Sized`). The hook surface is a thin dispatch
/// boundary; monomorphization across all concrete impls is not required.
pub fn render_blocks<H: RenderHooks + ?Sized>(hooks: &H, out: &mut String, blocks: &[Block]) {
    for block in blocks {
        // Nested blocks render without source-line annotations (the
        // legacy transform_events emitted `data-source-line` on the
        // outer `<ul>`/`<ol>`/`<blockquote>` and inner `<li>` only —
        // top-level + list-item depth. We omit the `<li>` annotation
        // for now; the iframe-bridge consumer picks the outer wrapper
        // when no inner annotation exists.
        render_block(hooks, out, block, &BlockMeta::default());
    }
}

fn render_block<H: RenderHooks + ?Sized>(
    hooks: &H,
    out: &mut String,
    block: &Block,
    meta: &BlockMeta,
) {
    match block {
        Block::Heading {
            level,
            children,
            id,
        } => {
            let mut content = String::new();
            render_inlines(hooks, &mut content, children);
            hooks.render_heading(out, *level, id.as_deref(), meta.source_line, &content);
            out.push('\n');
        }
        Block::Paragraph(children) => {
            out.push_str("<p");
            push_source_line_attr(out, meta.source_line);
            out.push('>');
            render_inlines(hooks, out, children);
            out.push_str("</p>\n");
        }
        Block::Callout {
            kind,
            fold,
            title,
            children,
        } => {
            // Phase 4 PR4: byte-shape mirrors the (now-deleted) Stage 1
            // `resolve/callouts.rs` output that production HTML still
            // assumes — `<div class="callout" data-type="{slug}"> /
            //   <div class="callout-title">{title}</div> /
            //   <div class="callout-content">…</div>
            // </div>`. The `data-fold` attribute is new in PR4 (Obsidian
            // foldable callouts); absent on non-foldable callouts so
            // existing fixtures remain byte-identical.
            //
            // `data-source-line` injected when meta carries it; matches the
            // legacy `transform_events` shape on the blockquote-promoted
            // callout (the legacy emit was for `<blockquote>` since
            // callouts hadn't moved to a typed `<div>` shape yet at the
            // time; downstream consumer (iframe-bridge) accepts the attr
            // on any wrapper element).
            out.push_str(r#"<div class="callout" data-type=""#);
            out.push_str(kind.as_slug());
            out.push_str(r#"""#);
            push_source_line_attr(out, meta.source_line);
            if let Some(fold_state) = fold {
                let fold_attr = match fold_state {
                    Fold::Open => "open",
                    Fold::Closed => "closed",
                };
                out.push_str(r#" data-fold=""#);
                out.push_str(fold_attr);
                out.push_str(r#"""#);
            }
            out.push_str(">\n");
            // Title slot: prefer the parser-extracted title; fall back
            // to the kind's capitalized default (matches Stage 1).
            let display_title = title
                .as_deref()
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .map(|t| escape_text(t))
                .unwrap_or_else(|| kind.default_title().to_string());
            out.push_str(r#"  <div class="callout-title">"#);
            out.push_str(&display_title);
            out.push_str("</div>\n");
            out.push_str(r#"  <div class="callout-content">"#);
            out.push('\n');
            render_blocks(hooks, out, children);
            out.push_str("</div>\n");
            out.push_str("</div>\n");
        }
        Block::List {
            ordered,
            start,
            items,
            item_source_lines,
        } => {
            // Parallel-vec invariant: when the parser populated per-item
            // source lines, the vector must align 1:1 with `items` so the
            // `idx`-keyed lookup at `item_source_lines.get(idx)` is
            // well-defined. Empty (default) means "parser ran without
            // `emit_source_lines`" — that's the legitimate skip case.
            // Mirrors the document-level `blocks.len() == block_meta.len()`
            // invariant asserted at the top of `render_document`.
            debug_assert!(
                item_source_lines.is_empty() || item_source_lines.len() == items.len(),
                "Block::List invariant: item_source_lines.len() ({}) must equal items.len() ({}) when populated",
                item_source_lines.len(),
                items.len()
            );
            if *ordered {
                out.push_str("<ol");
                // Emit `start="N"` when the parser captured an explicit
                // non-default start number (`3. foo` → `Some(3)`).
                // `None` for the default `1. foo` case keeps the
                // shorter `<ol>` shape. Attribute order mirrors other
                // typed-AST blocks: existing tag attrs first, then
                // `data-source-line`. Phase 4 followup B (2026-05-28).
                if let Some(n) = start {
                    out.push_str(" start=\"");
                    out.push_str(&n.to_string());
                    out.push('"');
                }
                push_source_line_attr(out, meta.source_line);
                out.push_str(">\n");
            } else {
                out.push_str("<ul");
                push_source_line_attr(out, meta.source_line);
                out.push_str(">\n");
            }
            for (idx, item_blocks) in items.iter().enumerate() {
                // Per-`<li>` source line — populated only when the parser
                // ran with `emit_source_lines: true` (otherwise
                // `item_source_lines` is empty). Mirrors the legacy
                // transform_events shape (commit f91aca8fa, 2026-04-01) that
                // emitted `data-source-line` on `<li>` for proportional
                // scroll-sync interpolation between editor and preview.
                out.push_str("<li");
                let item_line = item_source_lines.get(idx).copied().flatten();
                push_source_line_attr(out, item_line);
                out.push('>');
                // Single-paragraph items render their inline content inline
                // (no extra <p>). Mirrors pulldown-cmark's "tight list" output.
                if let [Block::Paragraph(inlines)] = item_blocks.as_slice() {
                    render_inlines(hooks, out, inlines);
                } else {
                    out.push('\n');
                    render_blocks(hooks, out, item_blocks);
                }
                out.push_str("</li>\n");
            }
            if *ordered {
                out.push_str("</ol>\n");
            } else {
                out.push_str("</ul>\n");
            }
        }
        Block::CodeBlock { lang, value } => {
            out.push_str("<pre");
            push_source_line_attr(out, meta.source_line);
            out.push('>');
            match lang {
                Some(l) => {
                    out.push_str(r#"<code class="language-"#);
                    out.push_str(&escape_attr(l));
                    out.push_str(r#"">"#);
                }
                None => out.push_str("<code>"),
            }
            out.push_str(&escape_text(value));
            out.push_str("</code></pre>\n");
        }
        Block::Table {
            header,
            rows,
            header_source_line,
            row_source_lines,
        } => {
            // Parallel-vec invariant: when the parser populated per-row
            // source lines, the vector must align 1:1 with `rows`.
            // Empty (default) means "parser ran without
            // `emit_source_lines`" — that's the legitimate skip case.
            // Mirrors the `Block::List` and document-level invariants.
            debug_assert!(
                row_source_lines.is_empty() || row_source_lines.len() == rows.len(),
                "Block::Table invariant: row_source_lines.len() ({}) must equal rows.len() ({}) when populated",
                row_source_lines.len(),
                rows.len()
            );
            out.push_str("<table");
            push_source_line_attr(out, meta.source_line);
            out.push_str(">\n<thead>\n<tr");
            // Header `<tr>` source line. Same f91aca8fa shape — annotated
            // when the parser tracked lines, omitted otherwise.
            push_source_line_attr(out, *header_source_line);
            out.push('>');
            for cell in header {
                out.push_str("<th>");
                render_inlines(hooks, out, cell);
                out.push_str("</th>");
            }
            out.push_str("</tr>\n</thead>\n");
            if !rows.is_empty() {
                out.push_str("<tbody>\n");
                for (idx, row) in rows.iter().enumerate() {
                    out.push_str("<tr");
                    let row_line = row_source_lines.get(idx).copied().flatten();
                    push_source_line_attr(out, row_line);
                    out.push('>');
                    for cell in row {
                        out.push_str("<td>");
                        render_inlines(hooks, out, cell);
                        out.push_str("</td>");
                    }
                    out.push_str("</tr>\n");
                }
                out.push_str("</tbody>\n");
            }
            out.push_str("</table>\n");
        }
        Block::BlockQuote(children) => {
            out.push_str("<blockquote");
            push_source_line_attr(out, meta.source_line);
            out.push_str(">\n");
            render_blocks(hooks, out, children);
            out.push_str("</blockquote>\n");
        }
        Block::Shortcode(sc) => {
            hooks.render_shortcode(out, sc, meta.source_line);
            out.push('\n');
        }
        Block::ThematicBreak => {
            out.push_str("<hr");
            push_source_line_attr(out, meta.source_line);
            out.push_str(" />\n");
        }
        Block::Figure {
            image,
            caption,
            width,
            align,
            class_names,
            img_style,
        } => {
            // Phase 4 PR3 (2026-05-27): image-only paragraphs promoted at
            // parse time become Block::Figure. The render shape is a
            // `<figure class="moss-image">` wrap around the image hook's
            // output, optionally followed by `<figcaption>{caption}</figcaption>`.
            //
            // The inner image renders via `hooks.render_image` (the same
            // path as Inline::Image — production wires this through
            // `DefaultHooks::with_snapshot` / `PipelineHooks` which uses
            // `ImageContext::MarkdownInline`, producing the bare
            // `<picture><img></picture>` shape). The structural `<figure>`
            // wrapper is the Figure renderer's responsibility — this keeps
            // the byte shape contract with shape-spec § 1: the spec sample
            // shows `<figure>` containing exactly the MarkdownInline inner.
            //
            // Caption omission: `caption: None` means "no figcaption" (the
            // empty-alt case). Empty caption Vec is also treated as no
            // figcaption — defensive, since `caption: Some(vec![])` would
            // otherwise emit `<figcaption></figcaption>`.
            //
            // Figure-level display params (`width`, `align`, `class_names`,
            // `img_style`) are populated only by parameterized wikilink
            // embeds (image-embed synth-collapse). The class list /
            // `data-width=` byte shape matches
            // `render::image::wrap_in_figure_full` so an embed-sourced figure
            // and a CommonMark `![](url)` figure with the same params are
            // byte-identical. For the CommonMark path these are all defaults,
            // so `class="moss-image"` with no `data-width=` — unchanged from
            // before the collapse.
            let mut class_value = String::from("moss-image");
            if let Some(a) = align {
                class_value.push(' ');
                class_value.push_str(a);
            }
            for cn in class_names {
                if cn.is_empty() {
                    continue;
                }
                class_value.push(' ');
                class_value.push_str(cn);
            }
            out.push_str(r#"<figure class=""#);
            out.push_str(&escape_attr(&class_value));
            out.push('"');
            if let Some(w) = width {
                out.push_str(r#" data-width=""#);
                out.push_str(&escape_attr(w));
                out.push('"');
            }
            push_source_line_attr(out, meta.source_line);
            out.push('>');
            // Render the inner image. Pattern-match the constrained shape;
            // any other inline falls back to the standard inline path so
            // the renderer never panics on a malformed Figure.
            match image {
                Inline::Image {
                    src, alt, title, ..
                } => match src {
                    Url::Resolved(r) => {
                        hooks.render_image_styled(out, r, alt, title.as_deref(), img_style.as_deref());
                    }
                    Url::Unresolved(s) => {
                        debug_assert!(
                                false,
                                "Url::Unresolved({s:?}) reached Block::Figure renderer — visit_urls_mut missing or buggy"
                            );
                        out.push_str(r#"<img src=""#);
                        out.push_str(&escape_attr(s));
                        out.push_str(r#"" alt=""#);
                        out.push_str(&escape_attr(alt));
                        out.push_str(r#"" />"#);
                    }
                },
                _ => {
                    // Defensive: a non-Image inline in a Figure violates
                    // the parser-enforced shape, but the renderer must
                    // still emit something rather than crash.
                    render_inline(hooks, out, image);
                }
            }
            if let Some(cap_inlines) = caption {
                if !cap_inlines.is_empty() {
                    out.push_str("<figcaption>");
                    render_inlines(hooks, out, cap_inlines);
                    out.push_str("</figcaption>");
                }
            }
            out.push_str("</figure>\n");
        }
        Block::LinkCard { url, children } => {
            // Phase 4 PR4.5 (2026-05-28): the compound-link grid-cell shape.
            // External URLs render as a link-preview wrapper; internal URLs
            // render as `data-kind="link"` grid-card.
            //
            // Production byte shape matches today's src-tauri
            // `render_compound_link_cell` output (ported here so that
            // shape was deleted from src-tauri in PR4.5). The wrapping
            // `<div class="moss-grid">` chrome lives in the Grid render
            // arm in hooks.rs; LinkCard is the per-cell shape.
            let resolved = match url {
                Url::Resolved(r) => r,
                Url::Unresolved(s) => {
                    debug_assert!(
                        false,
                        "Url::Unresolved({s:?}) reached Block::LinkCard renderer — visit_urls_mut missing or buggy"
                    );
                    out.push_str(r#"<a href=""#);
                    out.push_str(&escape_attr(s));
                    out.push_str(r#"" class="moss-grid-card" data-kind="link">"#);
                    render_blocks(hooks, out, children);
                    out.push_str("</a>");
                    return;
                }
            };
            use super::url::UrlKind;
            let is_external = matches!(resolved.kind, UrlKind::External | UrlKind::AssetNewtab);
            if is_external {
                out.push_str(r#"<a href=""#);
                out.push_str(&escape_attr(&resolved.href));
                out.push_str(
                    r#"" class="moss-grid-card link-preview" target="_blank" rel="noopener">"#,
                );
            } else {
                out.push_str(r#"<a href=""#);
                out.push_str(&escape_attr(&resolved.href));
                out.push_str(r#"" class="moss-grid-card" data-kind="link">"#);
            }
            render_blocks(hooks, out, children);
            out.push_str("</a>");
        }
        Block::Other(html) => {
            out.push_str(html);
        }
    }
}

/// Append ` data-source-line="N"` to `out` when `source_line` is `Some`.
/// No-op otherwise.
///
/// Used at every top-level block's opening tag arm so the preview's
/// `cm-scroll-sync` (in `frontend/bridge/iframe-bridge.ts`) can locate
/// the DOM element that corresponds to a given editor source line.
///
/// Matches the legacy `transform_events` emit byte shape — leading space,
/// double-quoted attribute value, decimal integer — verified against
/// `src-tauri/src/build/ship.rs::apply_strip_removes_data_source_line`
/// which scrubs this exact pattern from the ship-stage output.
fn push_source_line_attr(out: &mut String, source_line: Option<usize>) {
    if let Some(n) = source_line {
        use std::fmt::Write as _;
        // unwrap_or: writing into a String never fails, but the API
        // returns Result. Keep this honest.
        let _ = write!(out, r#" data-source-line="{}""#, n);
    }
}

pub(super) fn render_inlines<H: RenderHooks + ?Sized>(
    hooks: &H,
    out: &mut String,
    inlines: &[Inline],
) {
    for inline in inlines {
        render_inline(hooks, out, inline);
    }
}

fn render_inline<H: RenderHooks + ?Sized>(hooks: &H, out: &mut String, inline: &Inline) {
    match inline {
        Inline::Text(t) => out.push_str(&escape_text(t)),
        Inline::Link {
            url,
            title: _title,
            children,
            is_wikilink,
        } => {
            let resolved = match url {
                Url::Resolved(r) => r,
                Url::Unresolved(s) => {
                    debug_assert!(
                        false,
                        "Url::Unresolved({s:?}) reached renderer — visit_urls_mut missing or buggy"
                    );
                    // In release: emit href as-is so we don't crash, but
                    // the wide-net invariant test will catch the leak.
                    out.push_str(r#"<a href=""#);
                    out.push_str(&escape_attr(s));
                    out.push_str(r#"">"#);
                    render_inlines(hooks, out, children);
                    out.push_str("</a>");
                    return;
                }
            };
            let mut content = String::new();
            render_inlines(hooks, &mut content, children);
            // Phase 4 PR7a-flip-core-A (2026-05-28): pass the
            // `is_wikilink` flag directly to the hook. Pre-flip-core-A,
            // this arm synthesized a wikilink-kinded `ResolvedUrl` to
            // coax the hook's wikilink branch — a lossy workaround that
            // dropped the original `UrlKind` (`AssetNewtab` wikilinks
            // lost their `target="_blank" rel="noopener"`). The hook's
            // new signature carries both concerns orthogonally.
            hooks.render_link(out, resolved, *is_wikilink, &content);
        }
        Inline::Image {
            src, alt, title, ..
        } => {
            let resolved = match src {
                Url::Resolved(r) => r,
                Url::Unresolved(s) => {
                    debug_assert!(
                        false,
                        "Url::Unresolved({s:?}) reached renderer — visit_urls_mut missing or buggy"
                    );
                    out.push_str(r#"<img src=""#);
                    out.push_str(&escape_attr(s));
                    out.push_str(r#"" alt=""#);
                    out.push_str(&escape_attr(alt));
                    out.push_str(r#"" />"#);
                    return;
                }
            };
            hooks.render_image(out, resolved, alt, title.as_deref());
        }
        Inline::Emphasis(children) => {
            out.push_str("<em>");
            render_inlines(hooks, out, children);
            out.push_str("</em>");
        }
        Inline::Strong(children) => {
            out.push_str("<strong>");
            render_inlines(hooks, out, children);
            out.push_str("</strong>");
        }
        Inline::Code(c) => {
            out.push_str("<code>");
            out.push_str(&escape_text(c));
            out.push_str("</code>");
        }
        Inline::LineBreak => out.push_str("<br />\n"),
        Inline::Other(html) => out.push_str(html),
    }
}

#[cfg(test)]
mod tests {
    use super::super::hooks::DefaultHooks;
    use super::super::node::Inline;
    use super::super::url::{Url, UrlKind};
    use super::*;

    fn render(blocks: Vec<Block>) -> String {
        let doc = Document::from_blocks(blocks);
        render_document(&doc, &DefaultHooks::new())
    }

    #[test]
    fn renders_empty_document_to_empty_string() {
        assert_eq!(render(vec![]), "");
    }

    #[test]
    fn renders_paragraph() {
        let html = render(vec![Block::Paragraph(vec![Inline::Text("hi".into())])]);
        assert_eq!(html, "<p>hi</p>\n");
    }

    #[test]
    fn renders_heading_with_id() {
        let html = render(vec![Block::Heading {
            level: 2,
            children: vec![Inline::Text("Setup".into())],
            id: Some("setup".into()),
        }]);
        assert_eq!(html, "<h2 id=\"setup\">Setup<a class=\"moss-heading-anchor\" href=\"#setup\" aria-label=\"Permalink to this section\"><span aria-hidden=\"true\">#</span></a></h2>\n");
    }

    #[test]
    fn renders_resolved_link_internal() {
        let html = render(vec![Block::Paragraph(vec![Inline::Link {
            url: Url::resolved("docs/", UrlKind::Internal),
            title: None,
            children: vec![Inline::Text("Docs".into())],
            is_wikilink: false,
        }])]);
        assert_eq!(html, "<p><a href=\"docs/\">Docs</a></p>\n");
    }

    #[test]
    fn renders_resolved_link_wikilink_carries_class() {
        // PR7a: wikilink class can come from either the resolved URL kind
        // (legacy production path) OR the new is_wikilink AST flag.
        let html = render(vec![Block::Paragraph(vec![Inline::Link {
            url: Url::resolved("../docs/", UrlKind::Wikilink),
            title: None,
            children: vec![Inline::Text("Docs".into())],
            is_wikilink: false,
        }])]);
        assert!(html.contains(r#"class="wikilink""#), "got: {html}");
    }

    #[test]
    fn renders_link_with_is_wikilink_flag_emits_class() {
        // PR7a: is_wikilink: true on a non-wikilink-kind URL still
        // produces the wikilink class. Parser sets this for any
        // pulldown-cmark Tag::Link { link_type: LinkType::WikiLink, .. }.
        let html = render(vec![Block::Paragraph(vec![Inline::Link {
            url: Url::resolved("../docs/", UrlKind::Internal),
            title: None,
            children: vec![Inline::Text("Docs".into())],
            is_wikilink: true,
        }])]);
        assert!(
            html.contains(r#"class="wikilink""#),
            "is_wikilink: true should produce class=\"wikilink\"; got: {html}"
        );
    }

    #[test]
    fn renders_resolved_image() {
        let html = render(vec![Block::Paragraph(vec![Inline::Image {
            src: Url::resolved("cat.jpg", UrlKind::Asset),
            alt: "Cat".into(),
            title: None,
            is_wikilink: false,
            wikilink_pothole: None,
        }])]);
        assert_eq!(html, "<p><img src=\"cat.jpg\" alt=\"Cat\" /></p>\n");
    }

    #[test]
    fn renders_emphasis_and_strong() {
        let html = render(vec![Block::Paragraph(vec![
            Inline::Emphasis(vec![Inline::Text("em".into())]),
            Inline::Text(" ".into()),
            Inline::Strong(vec![Inline::Text("strong".into())]),
        ])]);
        assert_eq!(html, "<p><em>em</em> <strong>strong</strong></p>\n");
    }

    #[test]
    fn renders_inline_code_with_escaping() {
        let html = render(vec![Block::Paragraph(vec![Inline::Code("a<b>c".into())])]);
        assert_eq!(html, "<p><code>a&lt;b&gt;c</code></p>\n");
    }

    #[test]
    fn renders_unordered_list_tight() {
        let html = render(vec![Block::List {
            ordered: false,
            start: None,
            items: vec![
                vec![Block::Paragraph(vec![Inline::Text("one".into())])],
                vec![Block::Paragraph(vec![Inline::Text("two".into())])],
            ],
            item_source_lines: vec![],
        }]);
        assert_eq!(html, "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n");
    }

    #[test]
    fn renders_ordered_list() {
        let html = render(vec![Block::List {
            ordered: true,
            start: None,
            items: vec![vec![Block::Paragraph(vec![Inline::Text("a".into())])]],
            item_source_lines: vec![],
        }]);
        assert!(html.starts_with("<ol>"));
    }

    #[test]
    fn render_ordered_list_emits_start_attribute_when_non_default() {
        // `3. foo` should produce `<ol start="3">…</ol>`. The attribute
        // appears immediately after `<ol`, before any `data-source-line`
        // (Phase 4 followup B contract).
        let html = render(vec![Block::List {
            ordered: true,
            start: Some(3),
            items: vec![vec![Block::Paragraph(vec![Inline::Text("a".into())])]],
            item_source_lines: vec![],
        }]);
        assert!(
            html.starts_with(r#"<ol start="3">"#),
            "expected start attr immediately after <ol, got: {html}"
        );
    }

    #[test]
    fn render_ordered_list_omits_start_when_default_1() {
        // `start: None` is the canonical shape for "default 1." lists.
        // The renderer must NOT emit `start="1"` (semantically
        // identical to omitting the attr, but noisier).
        let html = render(vec![Block::List {
            ordered: true,
            start: None,
            items: vec![vec![Block::Paragraph(vec![Inline::Text("a".into())])]],
            item_source_lines: vec![],
        }]);
        assert!(html.starts_with("<ol>"), "expected bare <ol>, got: {html}");
        assert!(
            !html.contains("start="),
            "ordered list with default start should not emit start attr, got: {html}"
        );
    }

    #[test]
    fn render_unordered_list_emits_no_start() {
        // Even if `start: Some(N)` were somehow set on an unordered
        // list (shouldn't happen via the parser, but defense in depth),
        // `<ul>` must never carry `start=`.
        let html = render(vec![Block::List {
            ordered: false,
            start: Some(5),
            items: vec![vec![Block::Paragraph(vec![Inline::Text("a".into())])]],
            item_source_lines: vec![],
        }]);
        assert!(html.starts_with("<ul>"), "expected bare <ul>, got: {html}");
        assert!(
            !html.contains("start="),
            "unordered list must never carry start attr, got: {html}"
        );
    }

    #[test]
    fn renders_code_block_with_lang() {
        let html = render(vec![Block::CodeBlock {
            lang: Some("rust".into()),
            value: "fn main() {}".into(),
        }]);
        assert_eq!(
            html,
            "<pre><code class=\"language-rust\">fn main() {}</code></pre>\n"
        );
    }

    #[test]
    fn renders_code_block_without_lang() {
        let html = render(vec![Block::CodeBlock {
            lang: None,
            value: "bare".into(),
        }]);
        assert_eq!(html, "<pre><code>bare</code></pre>\n");
    }

    #[test]
    fn renders_thematic_break() {
        let html = render(vec![Block::ThematicBreak]);
        assert_eq!(html, "<hr />\n");
    }

    // -----------------------------------------------------------------
    // Phase 4 PR4: Block::Callout render shape
    // -----------------------------------------------------------------

    use super::super::node::{CalloutKind, Fold};

    #[test]
    fn renders_basic_callout_with_title() {
        let html = render(vec![Block::Callout {
            kind: CalloutKind::Note,
            fold: None,
            title: Some("Heads up".into()),
            children: vec![Block::Paragraph(vec![Inline::Text("Body.".into())])],
        }]);
        assert!(
            html.contains(r#"<div class="callout" data-type="note">"#),
            "expected callout div with data-type, got: {html}"
        );
        assert!(
            html.contains(r#"<div class="callout-title">Heads up</div>"#),
            "expected inline title slot, got: {html}"
        );
        assert!(
            html.contains(r#"<div class="callout-content">"#),
            "expected content slot, got: {html}"
        );
        assert!(html.contains("<p>Body.</p>"), "body must render: {html}");
    }

    #[test]
    fn renders_callout_falls_back_to_default_title() {
        let html = render(vec![Block::Callout {
            kind: CalloutKind::Warning,
            fold: None,
            title: None,
            children: vec![],
        }]);
        assert!(
            html.contains(r#"<div class="callout-title">Warning</div>"#),
            "expected capitalized fallback title, got: {html}"
        );
    }

    #[test]
    fn renders_foldable_callout_with_data_fold_attribute() {
        let html_open = render(vec![Block::Callout {
            kind: CalloutKind::Tip,
            fold: Some(Fold::Open),
            title: Some("Open".into()),
            children: vec![],
        }]);
        assert!(
            html_open.contains(r#"data-type="tip""#) && html_open.contains(r#"data-fold="open""#),
            "expected data-fold='open' attribute, got: {html_open}"
        );

        let html_closed = render(vec![Block::Callout {
            kind: CalloutKind::Tip,
            fold: Some(Fold::Closed),
            title: None,
            children: vec![],
        }]);
        assert!(
            html_closed.contains(r#"data-fold="closed""#),
            "expected data-fold='closed' attribute, got: {html_closed}"
        );
    }

    #[test]
    fn callout_alias_renders_canonical_data_type_slug() {
        // tldr → abstract; ensures the canonicalized slug is what
        // appears in HTML.
        let html = render(vec![Block::Callout {
            kind: CalloutKind::Abstract,
            fold: None,
            title: Some("TL;DR".into()),
            children: vec![],
        }]);
        assert!(
            html.contains(r#"data-type="abstract""#),
            "expected canonical slug 'abstract', got: {html}"
        );
    }

    #[test]
    fn callout_title_is_html_escaped() {
        // Title is rendered through escape_text (the same function the
        // existing renderer uses for text content). escape_text escapes
        // `<`, `>`, `&` but NOT `"` — `"` is only dangerous inside HTML
        // attribute values, and title sits between `<div>` tags as text.
        let html = render(vec![Block::Callout {
            kind: CalloutKind::Warning,
            fold: None,
            title: Some(r#"Use <script> & "quotes""#.into()),
            children: vec![],
        }]);
        assert!(
            html.contains("Use &lt;script&gt; &amp;"),
            "title must escape lt/gt/amp, got: {html}"
        );
        // No raw `<script>` may appear inside the title div.
        assert!(
            !html.contains("<div class=\"callout-title\">Use <script>"),
            "unescaped angle brackets leaked, got: {html}"
        );
    }

    #[test]
    fn renders_blockquote_with_paragraph() {
        let html = render(vec![Block::BlockQuote(vec![Block::Paragraph(vec![
            Inline::Text("q".into()),
        ])])]);
        assert_eq!(html, "<blockquote>\n<p>q</p>\n</blockquote>\n");
    }

    #[test]
    fn renders_table() {
        let html = render(vec![Block::Table {
            header: vec![vec![Inline::Text("A".into())]],
            rows: vec![vec![vec![Inline::Text("1".into())]]],
            header_source_line: None,
            row_source_lines: vec![],
        }]);
        assert!(html.contains("<thead>"));
        assert!(html.contains("<tbody>"));
        assert!(html.contains("<th>A</th>"));
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn renders_other_block_passes_html_through() {
        let html = render(vec![Block::Other("<custom></custom>".into())]);
        assert_eq!(html, "<custom></custom>");
    }

    #[test]
    fn text_escapes_lt_gt_amp() {
        let html = render(vec![Block::Paragraph(vec![Inline::Text("a<b>c&d".into())])]);
        assert_eq!(html, "<p>a&lt;b&gt;c&amp;d</p>\n");
    }

    #[test]
    fn round_trips_parse_to_render_for_canonical_doc() {
        // End-to-end: post-resolve markdown → parse → simulate visit
        // (mark every URL Internal) → render → check shape.
        //
        // Phase 4 PR2: the parser now populates Block::Heading.id with the
        // Obsidian anchor slug, so the rendered <h1> carries id="title".
        let md = "# Title\n\npara with [link](docs/) and *em*.\n";
        let mut doc = super::super::parser::parse(md);
        super::super::visit::visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => *u = Url::resolved(s.clone(), UrlKind::Internal),
            _ => {}
        });
        let html = render_document(&doc, &DefaultHooks::new());
        assert!(html.contains(r##"<h1 id="title">Title<a class="moss-heading-anchor" href="#title" aria-label="Permalink to this section"><span aria-hidden="true">#</span></a></h1>"##), "got: {html}");
        assert!(html.contains(r#"<a href="docs/">link</a>"#));
        assert!(html.contains("<em>em</em>"));
    }

    // -----------------------------------------------------------------
    // Phase 4 PR3 (2026-05-27): Block::Figure render
    // -----------------------------------------------------------------

    #[test]
    fn figure_renders_with_caption() {
        // Canonical shape: <figure class="moss-image">{inner img}{figcaption}</figure>.
        // DefaultHooks::new() has no snapshot, so inner is the bare <img>
        // (test path). Production wires DefaultHooks::with_snapshot which
        // routes inner through synth — same shape, richer attrs.
        let html = render(vec![Block::Figure {
            image: Inline::Image {
                src: Url::resolved("logo.png", UrlKind::Asset),
                alt: "A logo".into(),
                title: None,
                is_wikilink: false,
                wikilink_pothole: None,
            },
            caption: Some(vec![Inline::Text("A logo".into())]),
            width: None,
            align: None,
            class_names: Vec::new(),
            img_style: None,
        }]);
        assert!(
            html.starts_with(r#"<figure class="moss-image">"#),
            "expected figure wrap, got: {html}"
        );
        assert!(html.contains(r#"src="logo.png""#), "got: {html}");
        assert!(html.contains(r#"alt="A logo""#), "got: {html}");
        assert!(
            html.contains("<figcaption>A logo</figcaption>"),
            "got: {html}"
        );
        assert!(html.ends_with("</figure>\n"), "got: {html}");
    }

    #[test]
    fn figure_renders_without_caption_when_none() {
        // Empty-alt case: caption: None → no <figcaption> element.
        let html = render(vec![Block::Figure {
            image: Inline::Image {
                src: Url::resolved("x.png", UrlKind::Asset),
                alt: String::new(),
                title: None,
                is_wikilink: false,
                wikilink_pothole: None,
            },
            caption: None,
            width: None,
            align: None,
            class_names: Vec::new(),
            img_style: None,
        }]);
        assert!(html.contains("<figure"), "got: {html}");
        assert!(
            !html.contains("<figcaption"),
            "expected no figcaption, got: {html}"
        );
        assert!(html.contains("</figure>"), "got: {html}");
    }

    #[test]
    fn figure_renders_no_figcaption_for_empty_caption_vec() {
        // Defensive: caption: Some(vec![]) is treated identically to None.
        let html = render(vec![Block::Figure {
            image: Inline::Image {
                src: Url::resolved("x.png", UrlKind::Asset),
                alt: "x".into(),
                title: None,
                is_wikilink: false,
                wikilink_pothole: None,
            },
            caption: Some(vec![]),
            width: None,
            align: None,
            class_names: Vec::new(),
            img_style: None,
        }]);
        assert!(!html.contains("<figcaption"), "got: {html}");
    }

    #[test]
    fn figure_caption_escapes_html_unsafe_chars() {
        // Caption is a Vec<Inline>; Inline::Text passes through
        // escape_text. The figure renderer must NOT double-escape; the
        // existing inline path is the single source of escaping.
        let html = render(vec![Block::Figure {
            image: Inline::Image {
                src: Url::resolved("p.jpg", UrlKind::Asset),
                alt: "a<b>c".into(),
                title: None,
                is_wikilink: false,
                wikilink_pothole: None,
            },
            caption: Some(vec![Inline::Text("a<b>c".into())]),
            width: None,
            align: None,
            class_names: Vec::new(),
            img_style: None,
        }]);
        assert!(
            html.contains("<figcaption>a&lt;b&gt;c</figcaption>"),
            "got: {html}"
        );
    }

    #[test]
    fn figure_end_to_end_from_parser_to_render() {
        // Parse → visit (resolve URL) → render: covers the full path.
        let md = "![A photo](photo.jpg)\n";
        let mut doc = super::super::parser::parse(md);
        super::super::visit::visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => *u = Url::resolved(s.clone(), UrlKind::Asset),
            _ => {}
        });
        let html = render_document(&doc, &DefaultHooks::new());
        assert!(
            html.contains(r#"<figure class="moss-image">"#),
            "expected figure, got: {html}"
        );
        assert!(html.contains(r#"src="photo.jpg""#), "got: {html}");
        assert!(
            html.contains("<figcaption>A photo</figcaption>"),
            "got: {html}"
        );
    }

    #[test]
    fn paragraph_with_image_and_text_does_not_become_figure() {
        // End-to-end regression guard: ![img](u) caption text MUST stay
        // as a paragraph (not get the figure wrap) so the prose isn't
        // swallowed. Mirrors the parser-side guard `image_with_caption_text_does_not_promote`.
        let md = "![alt](a.jpg) plain text\n";
        let mut doc = super::super::parser::parse(md);
        super::super::visit::visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => *u = Url::resolved(s.clone(), UrlKind::Asset),
            _ => {}
        });
        let html = render_document(&doc, &DefaultHooks::new());
        assert!(
            !html.contains("<figure"),
            "image+text must not be wrapped in figure, got: {html}"
        );
        assert!(html.contains("plain text"), "got: {html}");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "visit_urls_mut missing")]
    fn unresolved_url_in_link_panics_in_debug() {
        // Critical contract: the bypass class is a debug-time crash.
        let _ = render(vec![Block::Paragraph(vec![Inline::Link {
            url: Url::unresolved("docs/"),
            title: None,
            children: vec![],
            is_wikilink: false,
        }])]);
    }

    // -----------------------------------------------------------------
    // 2026-05-28 (Phase 4 source-line wiring): BlockMeta → data-source-line
    // emission.
    // -----------------------------------------------------------------

    /// Render with explicit per-block meta. Helper for the source-line tests.
    fn render_with_meta(blocks: Vec<Block>, meta: Vec<BlockMeta>) -> String {
        let doc = Document::from_blocks_with_meta(blocks, meta);
        render_document(&doc, &DefaultHooks::new())
    }

    #[test]
    fn paragraph_emits_data_source_line_when_meta_set() {
        let html = render_with_meta(
            vec![Block::Paragraph(vec![Inline::Text("hi".into())])],
            vec![BlockMeta {
                source_line: Some(7),
            }],
        );
        assert_eq!(html, "<p data-source-line=\"7\">hi</p>\n");
    }

    #[test]
    fn heading_emits_data_source_line_through_hook() {
        let html = render_with_meta(
            vec![Block::Heading {
                level: 2,
                children: vec![Inline::Text("Setup".into())],
                id: Some("setup".into()),
            }],
            vec![BlockMeta {
                source_line: Some(3),
            }],
        );
        assert!(
            html.contains(r##"<h2 id="setup" data-source-line="3">Setup<a class="moss-heading-anchor" href="#setup" aria-label="Permalink to this section"><span aria-hidden="true">#</span></a></h2>"##),
            "got: {html}"
        );
    }

    #[test]
    fn list_blockquote_codeblock_table_hr_emit_data_source_line() {
        // Each block type that the legacy transform_events annotated
        // must emit `data-source-line` when meta carries it. Single
        // smoke test covering every top-level block kind.
        let blocks = vec![
            Block::BlockQuote(vec![Block::Paragraph(vec![Inline::Text("q".into())])]),
            Block::List {
                ordered: false,
                start: None,
                items: vec![vec![Block::Paragraph(vec![Inline::Text("a".into())])]],
                item_source_lines: vec![],
            },
            Block::List {
                ordered: true,
                start: None,
                items: vec![vec![Block::Paragraph(vec![Inline::Text("b".into())])]],
                item_source_lines: vec![],
            },
            Block::CodeBlock {
                lang: Some("rust".into()),
                value: "x".into(),
            },
            Block::Table {
                header: vec![vec![Inline::Text("H".into())]],
                rows: vec![vec![vec![Inline::Text("c".into())]]],
                header_source_line: None,
                row_source_lines: vec![],
            },
            Block::ThematicBreak,
        ];
        let meta = vec![
            BlockMeta {
                source_line: Some(1),
            },
            BlockMeta {
                source_line: Some(2),
            },
            BlockMeta {
                source_line: Some(3),
            },
            BlockMeta {
                source_line: Some(4),
            },
            BlockMeta {
                source_line: Some(5),
            },
            BlockMeta {
                source_line: Some(6),
            },
        ];
        let html = render_with_meta(blocks, meta);
        assert!(
            html.contains(r#"<blockquote data-source-line="1">"#),
            "blockquote missing: {html}"
        );
        assert!(
            html.contains(r#"<ul data-source-line="2">"#),
            "ul missing: {html}"
        );
        assert!(
            html.contains(r#"<ol data-source-line="3">"#),
            "ol missing: {html}"
        );
        assert!(
            html.contains(r#"<pre data-source-line="4">"#),
            "pre missing: {html}"
        );
        assert!(
            html.contains(r#"<table data-source-line="5">"#),
            "table missing: {html}"
        );
        assert!(
            html.contains(r#"<hr data-source-line="6" />"#),
            "hr missing: {html}"
        );
    }

    #[test]
    fn list_emits_per_li_data_source_line_when_parser_tracks() {
        // 2026-05-28 (Phase 4 source-lines followup): `<li>` carries
        // `data-source-line="N"` when the parser populated
        // `item_source_lines`. Mirrors the legacy transform_events shape
        // (commit f91aca8fa, 2026-04-01) that emitted on `<li>` for
        // proportional scroll-sync interpolation. The outer `<ul>` carries
        // BlockMeta.source_line separately.
        let blocks = vec![Block::List {
            ordered: false,
            start: None,
            items: vec![
                vec![Block::Paragraph(vec![Inline::Text("one".into())])],
                vec![Block::Paragraph(vec![Inline::Text("two".into())])],
                vec![Block::Paragraph(vec![Inline::Text("three".into())])],
            ],
            item_source_lines: vec![Some(10), Some(11), Some(12)],
        }];
        let meta = vec![BlockMeta {
            source_line: Some(10),
        }];
        let html = render_with_meta(blocks, meta);
        assert!(
            html.contains(r#"<ul data-source-line="10">"#),
            "ul opener missing: {html}"
        );
        assert!(
            html.contains(r#"<li data-source-line="10">one</li>"#),
            "li 10 missing: {html}"
        );
        assert!(
            html.contains(r#"<li data-source-line="11">two</li>"#),
            "li 11 missing: {html}"
        );
        assert!(
            html.contains(r#"<li data-source-line="12">three</li>"#),
            "li 12 missing: {html}"
        );
    }

    #[test]
    fn list_omits_li_data_source_line_when_parser_did_not_track() {
        // When `item_source_lines` is empty (default — parser ran with
        // `emit_source_lines: false`), no per-`<li>` attribute is emitted.
        // Locks the publish-build invariant: byte-identical output to the
        // pre-followup renderer.
        let blocks = vec![Block::List {
            ordered: false,
            start: None,
            items: vec![
                vec![Block::Paragraph(vec![Inline::Text("a".into())])],
                vec![Block::Paragraph(vec![Inline::Text("b".into())])],
            ],
            item_source_lines: vec![],
        }];
        let html = render_with_meta(blocks, vec![BlockMeta::default()]);
        assert_eq!(html, "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n");
    }

    #[test]
    fn table_emits_per_tr_data_source_line_when_parser_tracks() {
        // Header `<tr>` carries `header_source_line`; each body `<tr>`
        // carries the matching `row_source_lines[i]`.
        let blocks = vec![Block::Table {
            header: vec![vec![Inline::Text("H".into())]],
            rows: vec![
                vec![vec![Inline::Text("1".into())]],
                vec![vec![Inline::Text("2".into())]],
                vec![vec![Inline::Text("3".into())]],
            ],
            header_source_line: Some(5),
            row_source_lines: vec![Some(7), Some(8), Some(9)],
        }];
        let meta = vec![BlockMeta {
            source_line: Some(5),
        }];
        let html = render_with_meta(blocks, meta);
        assert!(
            html.contains(r#"<table data-source-line="5">"#),
            "table opener missing: {html}"
        );
        assert!(html.contains(r#"<thead>"#), "thead missing: {html}");
        // Header row line — note the header tr is on the marker line
        // because pulldown-cmark anchors the head row to the line of the
        // `| h |` header markdown row.
        assert!(
            html.contains(r#"<tr data-source-line="5"><th>H</th>"#),
            "head tr missing: {html}"
        );
        assert!(
            html.contains(r#"<tr data-source-line="7"><td>1</td>"#),
            "body tr 7 missing: {html}"
        );
        assert!(
            html.contains(r#"<tr data-source-line="8"><td>2</td>"#),
            "body tr 8 missing: {html}"
        );
        assert!(
            html.contains(r#"<tr data-source-line="9"><td>3</td>"#),
            "body tr 9 missing: {html}"
        );
    }

    #[test]
    fn table_omits_tr_data_source_line_when_parser_did_not_track() {
        // Publish-build invariant: byte-identical output to pre-followup.
        let blocks = vec![Block::Table {
            header: vec![vec![Inline::Text("A".into())]],
            rows: vec![vec![vec![Inline::Text("1".into())]]],
            header_source_line: None,
            row_source_lines: vec![],
        }];
        let html = render_with_meta(blocks, vec![BlockMeta::default()]);
        // No `data-source-line` anywhere — the table opener also has
        // BlockMeta::default() (None), so the entire `<table>...</table>`
        // block is annotation-free.
        assert!(
            !html.contains("data-source-line"),
            "no annotation expected: {html}"
        );
        assert!(html.contains("<thead>"));
        assert!(html.contains("<tr><th>A</th></tr>"));
        assert!(html.contains("<tr><td>1</td></tr>"));
    }

    #[test]
    fn figure_emits_data_source_line_on_outer_tag() {
        let blocks = vec![Block::Figure {
            image: Inline::Image {
                src: Url::resolved("p.jpg", UrlKind::Asset),
                alt: "A".into(),
                title: None,
                is_wikilink: false,
                wikilink_pothole: None,
            },
            caption: Some(vec![Inline::Text("A".into())]),
            width: None,
            align: None,
            class_names: Vec::new(),
            img_style: None,
        }];
        let meta = vec![BlockMeta {
            source_line: Some(9),
        }];
        let html = render_with_meta(blocks, meta);
        assert!(
            html.contains(r#"<figure class="moss-image" data-source-line="9">"#),
            "got: {html}"
        );
    }

    #[test]
    fn no_data_source_line_when_meta_none() {
        // Default `Document::from_blocks` creates meta vec of all
        // `BlockMeta::default()`; nothing should leak.
        let html = render(vec![
            Block::Paragraph(vec![Inline::Text("hi".into())]),
            Block::ThematicBreak,
        ]);
        assert!(
            !html.contains("data-source-line"),
            "default render must NOT emit data-source-line, got: {html}"
        );
    }

    #[test]
    fn end_to_end_parse_with_config_emits_data_source_line() {
        // The full path: parse_with_config → visit_urls_mut → render_document.
        let md = "# Title\n\nfirst paragraph\n\n## Sub\n\nsecond paragraph\n";
        let config = super::super::parser::ParseConfig {
            emit_source_lines: true,
            implicit_figure: true,
            source_line_offset: 0,
        };
        let mut doc = super::super::parser::parse_with_config(md, &config);
        super::super::visit::visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => *u = Url::resolved(s.clone(), UrlKind::Internal),
            _ => {}
        });
        let html = render_document(&doc, &DefaultHooks::new());
        assert!(
            html.contains(r##"<h1 id="title" data-source-line="1">Title<a class="moss-heading-anchor" href="#title" aria-label="Permalink to this section"><span aria-hidden="true">#</span></a></h1>"##),
            "H1 should carry data-source-line=1: {html}"
        );
        assert!(
            html.contains(r#"<p data-source-line="3">first paragraph</p>"#),
            "first paragraph should carry data-source-line=3: {html}"
        );
        assert!(
            html.contains(r##"<h2 id="sub" data-source-line="5">Sub<a class="moss-heading-anchor" href="#sub" aria-label="Permalink to this section"><span aria-hidden="true">#</span></a></h2>"##),
            "H2 should carry data-source-line=5: {html}"
        );
        assert!(
            html.contains(r#"<p data-source-line="7">second paragraph</p>"#),
            "second paragraph should carry data-source-line=7: {html}"
        );
    }
}
