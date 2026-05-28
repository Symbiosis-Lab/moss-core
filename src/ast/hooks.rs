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
//!
//! # Architectural prior art
//!
//! `RenderHooks` is moss's port of Hugo's render-hooks pattern
//! ([`markup/goldmark/render_hooks.go`](https://github.com/gohugoio/hugo/blob/master/markup/goldmark/render_hooks.go)).
//! In Hugo, `hookedRenderer` IS Goldmark's `NodeRenderer` — hooks fire
//! during the AST walk, every CommonMark-native attribute reaches the hook
//! (e.g. `linkContext.Title`), and the template owns rendering decisions.
//!
//! Cross-SSG research (2026-05-27) confirms this is the canonical shape:
//! every AST-bearing SSG (Hugo, Markdoc, mdast/remark, Pandoc, comrak,
//! recent mdBook) carries every parser-emitted attribute (including link
//! title) through to the renderer. Dropping fields the parser saw is
//! universally regarded as Gatsby's mistake — lossy AST forces consumers
//! into a plugin ecosystem they wouldn't need if the AST were faithful.
//!
//! See [docs/architecture/typed-ast-cross-ssg-research-2026-05-27.md](../../../../docs/architecture/typed-ast-cross-ssg-research-2026-05-27.md)
//! for the full research synthesis and [typed-body-ast.md](../../../../docs/architecture/typed-body-ast.md)
//! for the design intent + 7 principles.

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
    ///
    /// # Title parameter (PR8 — scheduled)
    ///
    /// This signature is missing the `title: Option<&str>` parameter that
    /// CommonMark links can carry (`[text](href "title")`). Title is
    /// silently dropped today through the AST render path. Invisible
    /// because production HTML still comes from `pulldown_cmark::html::push_html`
    /// (events carry title natively); becomes a regression the moment
    /// PR7a flips production to `render_document`.
    ///
    /// PR8 restores `title: Option<&str>` alongside other `RenderHooks`
    /// signature changes (`ResolvedUrl` private-constructor lockdown).
    /// Every comparable AST-bearing SSG passes title to its render hook
    /// (Hugo's `linkContext.Title`, Markdoc, mdast's `Resource.title`,
    /// comrak, Pandoc) — see
    /// [docs/architecture/typed-ast-cross-ssg-research-2026-05-27.md](../../../../docs/architecture/typed-ast-cross-ssg-research-2026-05-27.md).
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

    /// Optional [`AssetSnapshot`] for the gallery synth path.
    ///
    /// Phase 2E v5 PR3 (2026-05-26): when the impl returns
    /// `Some(snapshot)`, [`render_shortcode`]'s `Gallery` arm routes each
    /// item through [`crate::render::image::synthesize_image_html`] with
    /// [`ImageContext::GalleryThumb`], producing the canonical synth byte
    /// shape (`<picture><source srcset=*.webp>`, dims, LQIP, lazy
    /// loading). When the impl returns `None` (tests, fragment-render
    /// paths, downstream consumers without a snapshot), the legacy
    /// bare-`<img>` emission survives so the regex post-pass picks up
    /// the attributes.
    ///
    /// Default impl: `None`. [`DefaultHooks::with_snapshot`] overrides
    /// to expose the constructor-provided snapshot.
    ///
    /// The constructor-field pattern was chosen over extending
    /// `render_shortcode`'s signature: a typed snapshot field on the
    /// impl preserves the trait's public contract and keeps `RenderHooks`
    /// focused on "what HTML to emit", not "what HTML to emit given
    /// these inputs".
    fn gallery_assets(&self) -> Option<&crate::asset_snapshot::AssetSnapshot> {
        None
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
                if let Some(w) = &args.width {
                    out.push_str(r#" data-width=""#);
                    out.push_str(w);
                    out.push('"');
                }
                out.push('>');
                // Phase 2E v5 PR3 (2026-05-26): when the impl carries an
                // `AssetSnapshot` (production path via
                // `DefaultHooks::with_snapshot`), each gallery item routes
                // through `render::image::synthesize_image_html` with
                // `ImageContext::GalleryThumb` — producing the canonical
                // synth byte shape (`<picture><source srcset=*.webp>`
                // wrap, dims, LQIP, `loading="lazy"`). When `self.assets`
                // is `None` (tests, fragment-render paths), graceful-
                // degrade to the legacy bare-`<img>` emission so the
                // surviving regex post-pass picks up the attributes.
                // The legacy branch matches the pre-PR3 byte shape
                // exactly (no escape on src; alt unescaped).
                let assets_for_synth = self.gallery_assets();
                for item in &args.items {
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
                    out.push_str(r#"<div class="moss-gallery-item">"#);
                    if let Some(assets) = assets_for_synth {
                        // Synth path: full attribute set + optional
                        // <picture> wrap. The per-item inline style from
                        // MediaAttrs (e.g. object-position) flows through
                        // ImageRenderOptions::extra_attrs so the
                        // synthesizer's LQIP / dim emission joins it
                        // correctly (the synth suppresses its own
                        // style= when extra_attrs already carries one,
                        // matching the legacy regex's has_style guard).
                        let media_style = crate::media::parse_media_attrs(&item.attrs)
                            .to_inline_style();
                        let style_frag = media_style
                            .map(|s| format!(r#"style="{}""#, s));
                        let opts = crate::render::image::ImageRenderOptions {
                            eager: false,
                            extra_attrs: style_frag.as_deref(),
                            ..Default::default()
                        };
                        let item_html = crate::render::image::synthesize_image_html(
                            &src_str,
                            &item.alt,
                            assets,
                            crate::render::image::ImageContext::GalleryThumb,
                            &opts,
                        );
                        out.push_str(&item_html);
                    } else {
                        // Legacy bare-<img> path (no snapshot in scope).
                        // The surviving regex pass injects dims / LQIP /
                        // <picture> wrap downstream. Byte shape must match
                        // pre-PR3 emission for test/fragment-render
                        // parity.
                        out.push_str(r#"<img src=""#);
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
                        out.push('>');
                    }
                    out.push_str("</div>");
                }
                out.push_str("</div>");
            }
            Shortcode::Buttons(args) => {
                if args.items.is_empty() {
                    return;
                }
                // v1: the inverted variant is expressed via `data-style="inverted"`.
                // Any other authoring classes flow through as-is on `class=`.
                let mut data_style: Option<&str> = None;
                let mut extra_classes_vec: Vec<&str> = Vec::new();
                for token in args.classes.split_ascii_whitespace() {
                    if token == "inverted" {
                        data_style = Some("inverted");
                    } else {
                        extra_classes_vec.push(token);
                    }
                }
                let mut class_attr = String::from("moss-buttons");
                if !extra_classes_vec.is_empty() {
                    class_attr.push(' ');
                    class_attr.push_str(&extra_classes_vec.join(" "));
                }
                out.push_str(r#"<div class=""#);
                out.push_str(&escape_attr(&class_attr));
                if let Some(style) = data_style {
                    out.push_str(r#"" data-style=""#);
                    out.push_str(style);
                }
                out.push_str(r#"">"#);
                for (i, item) in args.items.iter().enumerate() {
                    let data_role = if i == 0 { "primary" } else { "secondary" };
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
                            out.push_str(r#"" class="moss-btn" data-role=""#);
                            out.push_str(data_role);
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
                    out.push_str(r#"" class="moss-btn"#);
                    // Wikilink kind adds class="wikilink" suffix; collapse
                    // both classes into a single class attribute.
                    if matches!(resolved.kind, crate::ast::url::UrlKind::Wikilink) {
                        out.push_str(" wikilink");
                    }
                    out.push_str(r#"" data-role=""#);
                    out.push_str(data_role);
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
            Shortcode::Hero(args) => {
                // Test-harness skeleton; the production renderer in
                // src-tauri's PipelineHooks routes the image through the
                // resolver, runs media-attrs into a style attribute, and
                // processes the overlay markdown to HTML. This default
                // emits a minimal `<section class="moss-hero">` so unit
                // tests can pattern-match on the wrapper without
                // depending on the full pipeline.
                let mut class_attr = String::from("moss-hero");
                if !args.classes.is_empty() {
                    class_attr.push(' ');
                    class_attr.push_str(&args.classes);
                }
                out.push_str(r#"<section class=""#);
                out.push_str(&escape_attr(&class_attr));
                out.push('"');
                if let Some(w) = &args.width {
                    out.push_str(r#" data-width=""#);
                    out.push_str(w);
                    out.push('"');
                }
                out.push('>');
                if let Some(image) = &args.image {
                    let src = match image {
                        crate::ast::url::Url::Resolved(r) => r.href.clone(),
                        crate::ast::url::Url::Unresolved(s) => {
                            debug_assert!(
                                false,
                                "Url::Unresolved({s:?}) reached render_shortcode \
                                 — visit_urls_mut missing for Hero"
                            );
                            s.clone()
                        }
                    };
                    out.push_str(r#"<img src=""#);
                    out.push_str(&escape_attr(&src));
                    out.push_str(r#"" alt="" />"#);
                }
                if !args.overlay_markdown.is_empty() {
                    out.push_str(r#"<div class="moss-hero-content">"#);
                    out.push_str(&escape_text(&args.overlay_markdown));
                    out.push_str("</div>");
                }
                out.push_str("</section>");
            }
            Shortcode::Grid(args) => {
                // Test-harness skeleton; the production renderer in
                // src-tauri's PipelineHooks runs each cell through the
                // markdown pipeline (including nested typed shortcodes).
                // This default emits a minimal `<div class="moss-grid">`
                // wrapper with each cell's raw text inside a
                // `.moss-grid-card` div so unit tests can pattern-match.
                let mut class_attr = String::from("moss-grid");
                if !args.classes.is_empty() {
                    class_attr.push(' ');
                    class_attr.push_str(&args.classes);
                }
                let style_attr = match &args.ratio {
                    Some(r) => {
                        let cols = r
                            .split(':')
                            .map(|n| format!("{}fr", n.trim()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        format!(r#" style="grid-template-columns:{}""#, cols)
                    }
                    None => String::new(),
                };
                out.push_str(r#"<div class=""#);
                out.push_str(&escape_attr(&class_attr));
                out.push_str(r#"" data-columns=""#);
                out.push_str(&args.columns.to_string());
                out.push_str("\"");
                out.push_str(&style_attr);
                if let Some(w) = &args.width {
                    out.push_str(r#" data-width=""#);
                    out.push_str(w);
                    out.push('"');
                }
                out.push('>');
                for cell in &args.cells {
                    out.push_str(r#"<div class="moss-grid-card">"#);
                    out.push_str(&escape_text(cell));
                    out.push_str("</div>");
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
///
/// Phase 2E v5 PR3 (2026-05-26): refactored from a unit struct to a
/// lifetime-parameterized struct so callers can attach an
/// [`AssetSnapshot`] for the gallery synth path. Use
/// [`DefaultHooks::new`] / `Default::default()` for the no-snapshot
/// behavior (legacy bare-`<img>` emission; surviving regex post-pass
/// fills in attributes); use [`DefaultHooks::with_snapshot`] for the
/// production path (full synth byte shape — `<picture><source srcset>`,
/// dims, LQIP, lazy loading).
///
/// The constructor-field pattern was chosen over extending
/// [`RenderHooks::render_shortcode`]'s signature with an
/// `&AssetSnapshot` arg: a struct field is non-breaking for downstream
/// `RenderHooks` impls and keeps the trait focused on "what HTML to
/// emit" rather than "what HTML to emit given these inputs". See
/// [`gallery_assets`](RenderHooks::gallery_assets) for the override
/// hook the trait exposes.
#[derive(Debug, Default)]
pub struct DefaultHooks<'a> {
    assets: Option<&'a crate::asset_snapshot::AssetSnapshot>,
}

impl<'a> DefaultHooks<'a> {
    /// Construct a no-snapshot `DefaultHooks`. The `Gallery` arm of
    /// [`RenderHooks::render_shortcode`] emits the legacy bare-`<img>`
    /// byte shape so the regex post-pass can fill in attributes. Use
    /// this for tests, fragment-render paths, and downstream consumers
    /// without a primed `AssetSnapshot`.
    pub fn new() -> Self {
        Self { assets: None }
    }

    /// Construct a `DefaultHooks` carrying an [`AssetSnapshot`] for the
    /// gallery synth path. The `Gallery` arm of
    /// [`RenderHooks::render_shortcode`] routes each item through
    /// [`crate::render::image::synthesize_image_html`] with
    /// [`crate::render::image::ImageContext::GalleryThumb`], emitting
    /// the canonical synth byte shape.
    pub fn with_snapshot(assets: &'a crate::asset_snapshot::AssetSnapshot) -> Self {
        Self {
            assets: Some(assets),
        }
    }
}

impl<'a> RenderHooks for DefaultHooks<'a> {
    fn gallery_assets(&self) -> Option<&crate::asset_snapshot::AssetSnapshot> {
        self.assets
    }
}

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
        let hooks = DefaultHooks::new();
        let mut out = String::new();
        hooks.render_heading(&mut out, 2, Some("setup"), "Setup");
        assert_eq!(out, r#"<h2 id="setup">Setup</h2>"#);
    }

    #[test]
    fn default_hooks_render_heading_without_id() {
        let hooks = DefaultHooks::new();
        let mut out = String::new();
        hooks.render_heading(&mut out, 3, None, "Sub");
        assert_eq!(out, "<h3>Sub</h3>");
    }

    #[test]
    fn default_hooks_render_link_internal_default() {
        let hooks = DefaultHooks::new();
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
        let hooks = DefaultHooks::new();
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
        let hooks = DefaultHooks::new();
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
        let hooks = DefaultHooks::new();
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
        let hooks = DefaultHooks::new();
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
        let hooks = DefaultHooks::new();
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

    // ── spec § P9 `data-width` emission ──────────────────────────────
    //
    // The author writes `:::hero {full}` and the parser canonicalizes
    // the flag to `Some("screen")`. The default render hook produces a
    // `<section class="moss-hero" data-width="screen">…` wrapper.
    // Default (no width attr) MUST omit `data-width` entirely so the
    // emitted HTML stays sparse — themes target the absence via
    // `:not([data-width])`.

    use super::super::shortcode::{
        GalleryItem, GalleryShortcode, GridShortcode, HeroShortcode, Shortcode,
    };

    fn render_shortcode_html(sc: &Shortcode) -> String {
        let mut out = String::new();
        DefaultHooks::new().render_shortcode(&mut out, sc);
        out
    }

    #[test]
    fn hero_with_width_screen_emits_data_width() {
        let sc = Shortcode::Hero(HeroShortcode {
            width: Some("screen".to_string()),
            ..Default::default()
        });
        let html = render_shortcode_html(&sc);
        assert!(
            html.contains(r#"data-width="screen""#),
            "expected data-width=screen, got: {html}"
        );
        assert!(html.contains(r#"class="moss-hero""#), "got: {html}");
    }

    #[test]
    fn hero_with_width_wide_emits_data_width() {
        let sc = Shortcode::Hero(HeroShortcode {
            width: Some("wide".to_string()),
            ..Default::default()
        });
        let html = render_shortcode_html(&sc);
        assert!(html.contains(r#"data-width="wide""#), "got: {html}");
    }

    #[test]
    fn hero_default_omits_data_width() {
        // Default: no `data-width` attribute at all. Sparse HTML matters
        // because themes target the absence (`:not([data-width])`); a
        // baked-in `data-width="body"` would shadow that intent.
        let sc = Shortcode::Hero(HeroShortcode::default());
        let html = render_shortcode_html(&sc);
        assert!(
            !html.contains("data-width"),
            "default should omit data-width, got: {html}"
        );
    }

    #[test]
    fn gallery_with_width_page_emits_data_width() {
        let sc = Shortcode::Gallery(GalleryShortcode {
            width: Some("page".to_string()),
            items: vec![GalleryItem {
                src: Url::resolved("a.jpg", UrlKind::Asset),
                alt: String::new(),
                attrs: String::new(),
            }],
            ..Default::default()
        });
        let html = render_shortcode_html(&sc);
        assert!(html.contains(r#"data-width="page""#), "got: {html}");
        assert!(html.contains(r#"class="moss-gallery""#), "got: {html}");
    }

    #[test]
    fn gallery_default_omits_data_width() {
        let sc = Shortcode::Gallery(GalleryShortcode::default());
        let html = render_shortcode_html(&sc);
        assert!(
            !html.contains("data-width"),
            "default should omit data-width, got: {html}"
        );
    }

    #[test]
    fn grid_with_width_wide_emits_data_width() {
        let sc = Shortcode::Grid(GridShortcode {
            columns: 2,
            width: Some("wide".to_string()),
            cells: vec!["a".to_string(), "b".to_string()],
            ..Default::default()
        });
        let html = render_shortcode_html(&sc);
        assert!(html.contains(r#"data-width="wide""#), "got: {html}");
        assert!(html.contains(r#"class="moss-grid""#), "got: {html}");
        // Other attrs still present.
        assert!(html.contains(r#"data-columns="2""#), "got: {html}");
    }

    #[test]
    fn grid_default_omits_data_width() {
        let sc = Shortcode::Grid(GridShortcode {
            columns: 1,
            cells: vec!["solo".to_string()],
            ..Default::default()
        });
        let html = render_shortcode_html(&sc);
        assert!(
            !html.contains("data-width"),
            "default should omit data-width, got: {html}"
        );
    }

    // ── DefaultHooks::with_snapshot — Gallery synth path ─────────────
    //
    // `DefaultHooks::new()` emits the legacy bare-`<img>` gallery item shape
    // (the surviving regex post-pass fills in dims / LQIP / `<picture>` wrap
    // downstream). `DefaultHooks::with_snapshot(snap)` routes each item
    // through `synthesize_image_html` with `ImageContext::GalleryThumb` —
    // producing the canonical synth byte shape directly from the snapshot.
    //
    // Pre-Phase-2E-v5-PR3 (2026-05-26) only the legacy fallback was tested
    // (all 25 unit tests in this file use `DefaultHooks::new()`). The synth
    // path was exercised exclusively through snapshot fixtures — fragile
    // for byte-shape regressions. These tests pin the structural
    // invariants directly.

    use crate::asset_snapshot::{AssetSnapshot, VariantKindSet};
    use std::path::PathBuf;

    fn gallery_with_one_item(src: &str) -> Shortcode {
        Shortcode::Gallery(GalleryShortcode {
            items: vec![GalleryItem {
                src: Url::resolved(src, UrlKind::Asset),
                alt: "photo".to_string(),
                attrs: String::new(),
            }],
            ..Default::default()
        })
    }

    #[test]
    fn default_hooks_new_gallery_emits_bare_img() {
        // Legacy fallback: no snapshot in scope. The surviving regex
        // post-pass injects dims / LQIP / `<picture>` downstream. The byte
        // shape here must match the pre-PR3 emission so fragment-render
        // paths (tests, in-app preview) stay consistent.
        let sc = gallery_with_one_item("photos/cat.jpg");
        let mut out = String::new();
        DefaultHooks::new().render_shortcode(&mut out, &sc);
        assert!(
            out.contains(r#"<img src="photos/cat.jpg""#),
            "expected bare <img>, got: {out}",
        );
        assert!(out.contains(r#"alt="photo""#), "got: {out}");
        assert!(out.contains(r#"loading="lazy""#), "got: {out}");
        assert!(!out.contains("<picture"), "legacy path must NOT emit <picture>, got: {out}");
    }

    #[test]
    fn default_hooks_with_snapshot_gallery_emits_picture_for_registered_webp() {
        // Synth path: snapshot registers a WebP variant. The synthesizer
        // wraps the `<img>` in `<picture><source srcset=*.webp>` (the
        // canonical responsive-image shape for raster originals).
        let src = "photos/cat.jpg";
        let mut snap = AssetSnapshot::new();
        snap.variants.insert(
            PathBuf::from("photos/cat"),
            VariantKindSet { webp: true, avif: false },
        );

        let sc = gallery_with_one_item(src);
        let mut out = String::new();
        DefaultHooks::with_snapshot(&snap).render_shortcode(&mut out, &sc);
        assert!(out.contains("<picture"), "expected <picture> wrap, got: {out}");
        assert!(
            out.contains(r#"srcset="photos/cat.webp""#),
            "expected webp srcset, got: {out}",
        );
        assert!(out.contains(r#"src="photos/cat.jpg""#), "got: {out}");
    }

    #[test]
    fn default_hooks_with_snapshot_gallery_uses_snapshot_dims() {
        // Synth path with known dims: `width=`/`height=` attrs come from
        // the snapshot's `dimensions` map. Pin the values so a future
        // regression that fails to read dimensions from the snapshot
        // (e.g., a `dims_lookup` rewrite that forgets a path-mapping step)
        // gets caught here, not via diff in a snapshot fixture.
        let src = "photos/cat.jpg";
        let mut snap = AssetSnapshot::new();
        snap.dimensions.insert(PathBuf::from(src), (800, 600));

        let sc = gallery_with_one_item(src);
        let mut out = String::new();
        DefaultHooks::with_snapshot(&snap).render_shortcode(&mut out, &sc);
        assert!(out.contains(r#"width="800""#), "expected width=800, got: {out}");
        assert!(out.contains(r#"height="600""#), "expected height=600, got: {out}");
    }
}
