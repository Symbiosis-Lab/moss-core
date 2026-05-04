//! Render hooks: per-node-type interception for HTML emission.
//!
//! Hugo's render-hooks pattern, ported to Rust. The renderer walks the AST
//! and calls `RenderHooks::render_*` methods at interceptable points; each
//! method has a default implementation in [`DefaultHooks`] that produces
//! moss's canonical HTML.
//!
//! Consumers (src-tauri's `PipelineHooks`, future plugins) override one
//! method without touching the renderer or the AST. Defaults handle the
//! ~80% case; overrides handle site-specific concerns (asset path
//! rewriting, classname injection, etc).

use super::shortcode::Shortcode;
use super::url::{ResolvedUrl, UrlKind};

/// Per-node-type HTML emission hooks. All methods have default
/// implementations; consumers override only the ones they need.
///
/// Methods write into the supplied `&mut String` rather than returning
/// fresh allocations. This avoids per-call String construction inside the
/// render loop and lets consumers compose without intermediate copies.
pub trait RenderHooks {
    /// Emit `<a href="...">...</a>` for a link.
    ///
    /// The default impl carries forward moss's existing post-render
    /// conventions: `class="wikilink"` for resolved wikilinks,
    /// `target="_blank" rel="noopener"` for asset-newtab links.
    fn render_link(&self, out: &mut String, url: &ResolvedUrl, content: &str) {
        match url.kind {
            UrlKind::Wikilink => {
                out.push_str(r#"<a class="wikilink" href=""#);
                out.push_str(&escape_attr(&url.href));
                out.push_str(r#"">"#);
            }
            UrlKind::AssetNewtab => {
                out.push_str(r#"<a target="_blank" rel="noopener" href=""#);
                out.push_str(&escape_attr(&url.href));
                out.push_str(r#"">"#);
            }
            _ => {
                out.push_str(r#"<a href=""#);
                out.push_str(&escape_attr(&url.href));
                out.push_str(r#"">"#);
            }
        }
        out.push_str(content);
        out.push_str("</a>");
    }

    /// Emit `<img src="..." alt="...">` for an image.
    fn render_image(&self, out: &mut String, src: &ResolvedUrl, alt: &str, title: Option<&str>) {
        out.push_str(r#"<img src=""#);
        out.push_str(&escape_attr(&src.href));
        out.push_str(r#"" alt=""#);
        out.push_str(&escape_attr(alt));
        out.push('"');
        if let Some(t) = title {
            out.push_str(r#" title=""#);
            out.push_str(&escape_attr(t));
            out.push('"');
        }
        out.push_str(" />");
    }

    /// Emit a shortcode block as HTML.
    ///
    /// The default impl produces a minimal HTML skeleton suitable for the
    /// moss-core test harness; site-specific HTML (subscribe forms, button
    /// styles, gallery grids) lives in src-tauri's `PipelineHooks` impl
    /// because it depends on filesystem context (site_id, lang, asset
    /// paths) that moss-core doesn't have.
    fn render_shortcode(&self, out: &mut String, sc: &Shortcode) {
        match sc {
            Shortcode::Subscribe(args) => {
                // Test-harness skeleton; src-tauri's PipelineHooks renders
                // the production HTML (form action URL, language defaults,
                // status spans). Description prose moved out of the
                // shortcode under the unified grammar.
                let placeholder = args.placeholder.as_deref().unwrap_or("you@example.com");
                out.push_str(r#"<div class="moss-subscribe-form">"#);
                out.push_str(r#"<input type="email" placeholder=""#);
                out.push_str(&escape_attr(placeholder));
                out.push_str(r#"" />"#);
                out.push_str("<button>");
                out.push_str(&escape_text(args.button.as_deref().unwrap_or("Subscribe")));
                out.push_str("</button>");
                out.push_str("</div>");
            }
            Shortcode::Gallery(args) => {
                let mut class_attr = String::from("moss-gallery");
                if !args.classes.is_empty() {
                    class_attr.push(' ');
                    class_attr.push_str(&args.classes);
                }
                let style_attr = match args.columns {
                    Some(n) => format!(r#" style="--gallery-columns: {n}""#),
                    None => String::new(),
                };
                out.push_str(r#"<div class=""#);
                out.push_str(&escape_attr(&class_attr));
                out.push('"');
                out.push_str(&style_attr);
                out.push('>');
                for item in &args.items {
                    out.push_str(r#"<div class="moss-gallery-item"><img src=""#);
                    let src_str = match &item.src {
                        crate::ast::url::Url::Resolved(r) => r.href.clone(),
                        crate::ast::url::Url::Unresolved(s) => {
                            debug_assert!(
                                false,
                                "Url::Unresolved({s:?}) reached render_shortcode \
                                 — visit_urls_mut missing for Gallery"
                            );
                            s.clone()
                        }
                    };
                    // Legacy gallery emits src verbatim (no escape) and no
                    // src trim, mirroring shortcode.rs:1651-1653. Match
                    // byte-for-byte.
                    out.push_str(&src_str);
                    out.push_str(r#"" alt=""#);
                    out.push_str(&item.alt);
                    out.push_str(r#"" loading="lazy""#);
                    let style = crate::media::parse_media_attrs(&item.attrs);
                    if let Some(s) = style.to_inline_style() {
                        out.push_str(r#" style=""#);
                        out.push_str(&s);
                        out.push('"');
                    }
                    out.push_str("></div>");
                }
                out.push_str("</div>");
            }
            Shortcode::Buttons(args) => {
                if args.items.is_empty() {
                    return;
                }
                let mut class_attr = String::from("moss-buttons");
                if !args.classes.is_empty() {
                    class_attr.push(' ');
                    class_attr.push_str(&args.classes);
                }
                out.push_str(r#"<div class=""#);
                out.push_str(&escape_attr(&class_attr));
                out.push_str(r#"">"#);
                for (i, item) in args.items.iter().enumerate() {
                    let primary_class = if i == 0 {
                        "moss-btn moss-btn-primary"
                    } else {
                        "moss-btn moss-btn-secondary"
                    };
                    let track = item
                        .text
                        .to_lowercase()
                        .replace(|c: char| !c.is_alphanumeric(), "-")
                        .trim_matches('-')
                        .to_string();
                    let resolved = match &item.url {
                        crate::ast::url::Url::Resolved(r) => r,
                        crate::ast::url::Url::Unresolved(s) => {
                            debug_assert!(
                                false,
                                "Url::Unresolved({s:?}) reached render_shortcode \
                                 — visit_urls_mut missing for Buttons"
                            );
                            // Release: emit href as-is so we don't crash.
                            out.push_str(r#"<a href=""#);
                            out.push_str(&escape_attr(s));
                            out.push_str(r#"" class=""#);
                            out.push_str(primary_class);
                            out.push_str(r#"" data-track=""#);
                            out.push_str(&escape_attr(&track));
                            out.push_str(r#"">"#);
                            out.push_str(&escape_text(&item.text));
                            out.push_str("</a>");
                            continue;
                        }
                    };
                    out.push_str(r#"<a href=""#);
                    out.push_str(&escape_attr(&resolved.href));
                    out.push_str(r#"" class=""#);
                    out.push_str(primary_class);
                    // Wikilink kind adds class="wikilink" suffix; collapse
                    // both classes into a single class attribute.
                    if matches!(resolved.kind, crate::ast::url::UrlKind::Wikilink) {
                        out.push_str(" wikilink");
                    }
                    out.push_str(r#"" data-track=""#);
                    out.push_str(&escape_attr(&track));
                    out.push_str(r#"""#);
                    if matches!(resolved.kind, crate::ast::url::UrlKind::AssetNewtab) {
                        out.push_str(r#" target="_blank" rel="noopener""#);
                    }
                    out.push('>');
                    out.push_str(&escape_text(&item.text));
                    out.push_str("</a>");
                }
                out.push_str("</div>");
            }
        }
    }

    /// Emit `<h1>...</h1>` (or h2/h3/...) for a heading.
    ///
    /// `id` is the heading anchor id (slug). When `Some`, the rendered
    /// tag carries `id="..."` so anchor links work.
    fn render_heading(&self, out: &mut String, level: u8, id: Option<&str>, content: &str) {
        out.push('<');
        out.push('h');
        out.push((b'0' + level) as char);
        if let Some(id) = id {
            out.push_str(r#" id=""#);
            out.push_str(&escape_attr(id));
            out.push('"');
        }
        out.push('>');
        out.push_str(content);
        out.push_str("</h");
        out.push((b'0' + level) as char);
        out.push('>');
    }
}

/// Default render hooks. moss-core ships this as the base implementation
/// of [`RenderHooks`]; the methods are the trait's default impls.
#[derive(Debug, Default)]
pub struct DefaultHooks;

impl RenderHooks for DefaultHooks {}

// ---------------------------------------------------------------------------
// Internal escape helpers used by both DefaultHooks and the renderer.
// ---------------------------------------------------------------------------

/// Escape `&"<>` for HTML attribute values.
pub(super) fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape `&<>` for HTML text content. Mirrors pulldown-cmark's text
/// escape (it does NOT escape `"` in text — only in attributes).
pub(super) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::url::Url;

    #[test]
    fn escape_attr_handles_all_unsafe_chars() {
        assert_eq!(escape_attr(r#"&"<>"#), "&amp;&quot;&lt;&gt;");
    }

    #[test]
    fn escape_text_does_not_quote_double_quote() {
        assert_eq!(escape_text(r#"hi "there" & y"#), r#"hi "there" &amp; y"#);
    }

    #[test]
    fn default_hooks_render_heading_emits_id() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_heading(&mut out, 2, Some("setup"), "Setup");
        assert_eq!(out, r#"<h2 id="setup">Setup</h2>"#);
    }

    #[test]
    fn default_hooks_render_heading_without_id() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_heading(&mut out, 3, None, "Sub");
        assert_eq!(out, "<h3>Sub</h3>");
    }

    #[test]
    fn default_hooks_render_link_internal_default() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_link(
            &mut out,
            &ResolvedUrl::new("docs/", UrlKind::Internal),
            "Docs",
        );
        assert_eq!(out, r#"<a href="docs/">Docs</a>"#);
    }

    #[test]
    fn default_hooks_render_link_wikilink_kind_injects_class() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_link(
            &mut out,
            &ResolvedUrl::new("../docs/", UrlKind::Wikilink),
            "Docs",
        );
        assert_eq!(out, r#"<a class="wikilink" href="../docs/">Docs</a>"#);
    }

    #[test]
    fn default_hooks_render_link_asset_newtab_injects_target() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_link(
            &mut out,
            &ResolvedUrl::new("file.pdf", UrlKind::AssetNewtab),
            "PDF",
        );
        assert_eq!(
            out,
            r#"<a target="_blank" rel="noopener" href="file.pdf">PDF</a>"#
        );
    }

    #[test]
    fn default_hooks_render_image_with_alt() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_image(
            &mut out,
            &ResolvedUrl::new("cat.jpg", UrlKind::Asset),
            "A cat",
            None,
        );
        assert_eq!(out, r#"<img src="cat.jpg" alt="A cat" />"#);
    }

    #[test]
    fn default_hooks_render_image_with_title() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_image(
            &mut out,
            &ResolvedUrl::new("cat.jpg", UrlKind::Asset),
            "A cat",
            Some("Photo"),
        );
        assert_eq!(
            out,
            r#"<img src="cat.jpg" alt="A cat" title="Photo" />"#
        );
    }

    #[test]
    fn default_hooks_escape_link_href() {
        let hooks = DefaultHooks;
        let mut out = String::new();
        hooks.render_link(
            &mut out,
            &ResolvedUrl::new(r#"q=a&b="c""#, UrlKind::External),
            "x",
        );
        assert!(out.contains(r#"href="q=a&amp;b=&quot;c&quot;""#));
    }

    #[test]
    fn render_link_unused_url_param_compiles() {
        // Ensure `Url` type is reachable from this module (sanity check
        // that imports are correct after refactors).
        let _u = Url::resolved("x", UrlKind::Internal);
    }
}
