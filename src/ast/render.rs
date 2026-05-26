//! Render typed AST → HTML via [`RenderHooks`].
//!
//! Walks every variant; calls hooks at interceptable points. Debug-asserts
//! on `Url::Unresolved` reaching the renderer — a missing visitor is a bug.

use super::document::Document;
use super::hooks::{escape_attr, escape_text, RenderHooks};
use super::node::{Block, Inline};
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

pub(super) fn render_blocks<H: RenderHooks>(hooks: &H, out: &mut String, blocks: &[Block]) {
    for block in blocks {
        render_block(hooks, out, block);
    }
}

fn render_block<H: RenderHooks>(hooks: &H, out: &mut String, block: &Block) {
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
        Block::Callout { kind, children } => {
            out.push_str(r#"<div class="moss-callout callout" data-type=""#);
            out.push_str(&escape_attr(kind));
            out.push_str(r#"">"#);
            out.push('\n');
            render_blocks(hooks, out, children);
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
        Block::Other(html) => {
            out.push_str(html);
        }
    }
}

pub(super) fn render_inlines<H: RenderHooks>(hooks: &H, out: &mut String, inlines: &[Inline]) {
    for inline in inlines {
        render_inline(hooks, out, inline);
    }
}

fn render_inline<H: RenderHooks>(hooks: &H, out: &mut String, inline: &Inline) {
    match inline {
        Inline::Text(t) => out.push_str(&escape_text(t)),
        Inline::Link {
            url,
            title: _title,
            children,
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
            hooks.render_link(out, resolved, &content);
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
    use super::super::url::{ResolvedUrl, Url, UrlKind};
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
        }])]);
        assert_eq!(html, "<p><a href=\"docs/\">Docs</a></p>\n");
    }

    #[test]
    fn renders_resolved_link_wikilink_carries_class() {
        let html = render(vec![Block::Paragraph(vec![Inline::Link {
            url: Url::resolved("../docs/", UrlKind::Wikilink),
            title: None,
            children: vec![Inline::Text("Docs".into())],
        }])]);
        assert!(html.contains(r#"class="wikilink""#), "got: {html}");
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
        let md = "# Title\n\npara with [link](docs/) and *em*.\n";
        let mut doc = super::super::parser::parse(md);
        super::super::visit::visit_urls_mut(&mut doc, |u| match u {
            Url::Unresolved(s) => *u = Url::resolved(s.clone(), UrlKind::Internal),
            _ => {}
        });
        let html = render_document(&doc, &DefaultHooks::new());
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains(r#"<a href="docs/">link</a>"#));
        assert!(html.contains("<em>em</em>"));
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
        }])]);
    }
}
