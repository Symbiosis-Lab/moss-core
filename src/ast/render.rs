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

use super::document::Document;
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
    render_blocks(hooks, &mut out, &doc.blocks);
    out
}

/// Render a sequence of blocks to HTML. Used by [`render_document`]
/// and by src-tauri's `render_hero_html_typed` (Phase 4 PR4.5) to render
/// a `Vec<Block>` that didn't come from a full `Document` (e.g. a hero
/// overlay).
///
/// `H: ?Sized` so the function can be called with `&dyn RenderHooks` or
/// with `self: &Self` from inside a trait default method (where `Self`
/// is not statically `Sized`). The hook surface is a thin dispatch
/// boundary; monomorphization across all concrete impls is not required.
pub fn render_blocks<H: RenderHooks + ?Sized>(hooks: &H, out: &mut String, blocks: &[Block]) {
    for block in blocks {
        render_block(hooks, out, block);
    }
}

fn render_block<H: RenderHooks + ?Sized>(hooks: &H, out: &mut String, block: &Block) {
    match block {
        Block::Heading {
            level,
            children,
            id,
        } => {
            let mut content = String::new();
            render_inlines(hooks, &mut content, children);
            hooks.render_heading(out, *level, id.as_deref(), &content);
            out.push('\n');
        }
        Block::Paragraph(children) => {
            out.push_str("<p>");
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
            out.push_str(r#"<div class="callout" data-type=""#);
            out.push_str(kind.as_slug());
            out.push_str(r#"""#);
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
        Block::List { ordered, items } => {
            if *ordered {
                out.push_str("<ol>\n");
            } else {
                out.push_str("<ul>\n");
            }
            for item_blocks in items {
                out.push_str("<li>");
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
            match lang {
                Some(l) => {
                    out.push_str(r#"<pre><code class="language-"#);
                    out.push_str(&escape_attr(l));
                    out.push_str(r#"">"#);
                }
                None => out.push_str("<pre><code>"),
            }
            out.push_str(&escape_text(value));
            out.push_str("</code></pre>\n");
        }
        Block::Table { header, rows } => {
            out.push_str("<table>\n<thead>\n<tr>");
            for cell in header {
                out.push_str("<th>");
                render_inlines(hooks, out, cell);
                out.push_str("</th>");
            }
            out.push_str("</tr>\n</thead>\n");
            if !rows.is_empty() {
                out.push_str("<tbody>\n");
                for row in rows {
                    out.push_str("<tr>");
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
            out.push_str("<blockquote>\n");
            render_blocks(hooks, out, children);
            out.push_str("</blockquote>\n");
        }
        Block::Shortcode(sc) => {
            hooks.render_shortcode(out, sc);
            out.push('\n');
        }
        Block::ThematicBreak => out.push_str("<hr />\n"),
        Block::Figure { image, caption } => {
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
            out.push_str(r#"<figure class="moss-image">"#);
            // Render the inner image. Pattern-match the constrained shape;
            // any other inline falls back to the standard inline path so
            // the renderer never panics on a malformed Figure.
            match image {
                Inline::Image { src, alt, title } => {
                    match src {
                        Url::Resolved(r) => {
                            hooks.render_image(out, r, alt, title.as_deref());
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
                    }
                }
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
            let is_external = matches!(
                resolved.kind,
                UrlKind::External | UrlKind::AssetNewtab
            );
            if is_external {
                out.push_str(r#"<a href=""#);
                out.push_str(&escape_attr(&resolved.href));
                out.push_str(r#"" class="moss-grid-card link-preview" target="_blank" rel="noopener">"#);
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

pub(super) fn render_inlines<H: RenderHooks + ?Sized>(hooks: &H, out: &mut String, inlines: &[Inline]) {
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
        Inline::Image { src, alt, title } => {
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
        assert_eq!(html, "<h2 id=\"setup\">Setup</h2>\n");
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
            items: vec![
                vec![Block::Paragraph(vec![Inline::Text("one".into())])],
                vec![Block::Paragraph(vec![Inline::Text("two".into())])],
            ],
        }]);
        assert_eq!(html, "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n");
    }

    #[test]
    fn renders_ordered_list() {
        let html = render(vec![Block::List {
            ordered: true,
            items: vec![vec![Block::Paragraph(vec![Inline::Text("a".into())])]],
        }]);
        assert!(html.starts_with("<ol>"));
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
        assert_eq!(
            html,
            "<blockquote>\n<p>q</p>\n</blockquote>\n"
        );
    }

    #[test]
    fn renders_table() {
        let html = render(vec![Block::Table {
            header: vec![vec![Inline::Text("A".into())]],
            rows: vec![vec![vec![Inline::Text("1".into())]]],
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
        let html = render(vec![Block::Paragraph(vec![Inline::Text(
            "a<b>c&d".into(),
        )])]);
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
        assert!(html.contains(r#"<h1 id="title">Title</h1>"#), "got: {html}");
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
            },
            caption: Some(vec![Inline::Text("A logo".into())]),
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
            },
            caption: None,
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
            },
            caption: Some(vec![]),
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
            },
            caption: Some(vec![Inline::Text("a<b>c".into())]),
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
        assert!(html.contains("<figcaption>A photo</figcaption>"), "got: {html}");
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
}
