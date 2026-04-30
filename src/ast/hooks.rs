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
    /// Phase A: uninhabited — `match *sc {}`. Phase B variants override
    /// this on `PipelineHooks` to produce the site-specific HTML each
    /// shortcode emits today.
    fn render_shortcode(&self, _out: &mut String, sc: &Shortcode) {
        match *sc {}
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
