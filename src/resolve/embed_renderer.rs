//! Renderer registry for `![[file]]` embeds.
//!
//! Each renderer maps a file extension (or extension family) to an output
//! format. The caller resolves the embed target via the ContentGraph, then
//! dispatches to the renderer for the target's extension. Unknown extensions
//! fall back to a file link (Obsidian parity) — that fallback lives in the
//! caller, not here.
//!
//! # moss-core ↔ src-tauri boundary
//!
//! moss-core is pure: no filesystem, no network, no async. This constrains
//! what a renderer can do:
//!
//! - **Pure renderers** (image, iframe, audio, video, 3D, table) — return
//!   `RenderedEmbed::Inline(markdown)` or `RenderedEmbed::Html(html)`. No I/O.
//!   The string is spliced directly into the compiled output.
//! - **I/O-bound renderers** (markdown transclusion, notebook, PDF preview) —
//!   return `RenderedEmbed::Deferred { marker }`. src-tauri runs a post-pass
//!   (`resolve_embeds` in `embeds.rs`) that reads the target file and splices
//!   its rendered content into the marker.
//!
//! Plugin-registered renderers (Phase E) must follow the same rule: if they
//! need I/O, they emit a marker and register a corresponding resolver on the
//! src-tauri side.

use std::sync::OnceLock;

mod common;
use common::{build_src, dim_attrs, file_stem, html_escape_attr, path_extension_lower};

// ---------------------------------------------------------------------------
// Reserved classnames (HTML/CSS contract, per moss#508)
// ---------------------------------------------------------------------------

/// Base class applied to all typed-embed output elements.
///
/// Theme authors may target `.moss-embed` to style the wrapper of any embed;
/// renderer-specific classes (e.g. [`CLASS_EMBED_IFRAME`]) extend the base.
/// The CSS that ships with moss is defined in src-tauri (see issue #508 for
/// the HTML/CSS contract).
pub const CLASS_EMBED: &str = "moss-embed";

/// Applied to iframe renderer output (Phase B).
pub const CLASS_EMBED_IFRAME: &str = "moss-embed-iframe";

/// Applied to PDF renderer output (Phase C).
pub const CLASS_EMBED_PDF: &str = "moss-embed-pdf";

/// Applied to audio renderer output (Phase C).
pub const CLASS_EMBED_AUDIO: &str = "moss-embed-audio";

/// Applied to video renderer output (Phase C).
pub const CLASS_EMBED_VIDEO: &str = "moss-embed-video";

/// Applied to notebook renderer output (Phase D).
pub const CLASS_EMBED_NOTEBOOK: &str = "moss-embed-notebook";

/// Applied to 3D model renderer output (Phase D).
pub const CLASS_EMBED_3D: &str = "moss-embed-3d";

/// Applied to tabular-data renderer output (Phase D).
pub const CLASS_EMBED_TABLE: &str = "moss-embed-table";

/// An embed that has been parsed and path-resolved, ready for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEmbed<'a> {
    /// Resolved target path, as returned by the ContentGraph.
    pub resolved_path: &'a str,
    /// The calling file's path (for computing relative asset URLs).
    pub from_path: &'a str,
    /// `?query` from the source wikilink, without the leading `?`.
    pub query: Option<&'a str>,
    /// `#fragment` from the source wikilink, without the leading `#`.
    /// For `.md` renderers this is a heading/block-ref marker (block refs
    /// keep their `^` prefix). For every other renderer this is a URL fragment.
    pub section: Option<&'a str>,
    /// `|pipe-content` from the source wikilink. Image renderer uses this
    /// for display keywords / size; other renderers parse per their convention.
    pub alias: Option<&'a str>,
}

/// Output of a renderer.
///
/// The variant tells the caller what further processing (if any) the string
/// needs. See the module-level doc for the moss-core ↔ src-tauri boundary rule.
#[derive(Debug, PartialEq, Eq)]
pub enum RenderedEmbed {
    /// Markdown-level text that will be processed by CommonMark downstream.
    /// Example: `![alt](url)` from the image renderer.
    Inline(String),
    /// Final HTML to splice into the output — must NOT be re-processed by the
    /// markdown parser. Example: `<iframe …>` from the iframe renderer.
    Html(String),
    /// A marker comment for `resolve_embeds` to resolve in a post-pass with
    /// file I/O. Example: `<!-- moss-embed-ipynb:notebook.ipynb -->` for the
    /// notebook renderer. The marker format is renderer-specific; downstream
    /// resolvers match on the prefix.
    Deferred { marker: String },
}

/// A single dimension with a unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dim {
    Px(u32),
    Percent(f32),
    Vh(f32),
}

impl Dim {
    /// Render this dimension as a CSS length string.
    pub fn to_css(self) -> String {
        match self {
            Dim::Px(n) => format!("{}px", n),
            Dim::Percent(v) => {
                if v.fract() == 0.0 {
                    format!("{}%", v as i64)
                } else {
                    format!("{}%", v)
                }
            }
            Dim::Vh(v) => {
                if v.fract() == 0.0 {
                    format!("{}vh", v as i64)
                } else {
                    format!("{}vh", v)
                }
            }
        }
    }

    /// Parse one dimension. Accepts: `200`, `200px`, `100%`, `80vh`.
    /// Returns None on any parse failure.
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let Some(rest) = s.strip_suffix('%') {
            return rest.trim().parse::<f32>().ok().map(Dim::Percent);
        }
        if let Some(rest) = s.strip_suffix("vh") {
            return rest.trim().parse::<f32>().ok().map(Dim::Vh);
        }
        if let Some(rest) = s.strip_suffix("px") {
            return rest.trim().parse::<u32>().ok().map(Dim::Px);
        }
        s.parse::<u32>().ok().map(Dim::Px)
    }
}

/// Parsed `|WxH` sizing hint from a wikilink pipe segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sizing {
    /// `|200` or `|100%` — width only.
    Width(Dim),
    /// `|200x150` or `|100%x600` — width × height.
    Box(Dim, Dim),
}

impl Sizing {
    /// Parse a pipe segment. Returns None if the string does not look like a
    /// sizing hint — callers can then fall through to their own parser
    /// (e.g. image display keywords).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let Some((w, h)) = s.split_once('x') {
            let wd = Dim::parse(w)?;
            let hd = Dim::parse(h)?;
            return Some(Sizing::Box(wd, hd));
        }
        Dim::parse(s).map(Sizing::Width)
    }
}

/// A renderer converts a `ParsedEmbed` into its rendered form.
pub trait EmbedRenderer: std::fmt::Debug + Send + Sync {
    /// Extensions this renderer claims (lowercase, without leading dot).
    fn extensions(&self) -> &[&'static str];

    /// Render the embed. Must be pure; moss-core is I/O-free.
    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed;
}

/// Built-in renderer registry. Initialized lazily on first lookup.
///
/// Each renderer is a unit struct, so the pointer is to a zero-size `'static`
/// — no heap allocation ever. Future renderers (iframe, pdf, audio, etc.)
/// get appended here as they ship.
fn registry() -> &'static [&'static dyn EmbedRenderer] {
    static INIT: OnceLock<Vec<&'static dyn EmbedRenderer>> = OnceLock::new();
    INIT.get_or_init(|| {
        vec![
            &ImageRenderer as &'static dyn EmbedRenderer,
            &MarkdownEmbedRenderer as &'static dyn EmbedRenderer,
            &IframeRenderer as &'static dyn EmbedRenderer,
        ]
    })
}

/// Look up a renderer by file extension (case-insensitive, no leading dot).
pub fn lookup_renderer(ext: &str) -> Option<&'static dyn EmbedRenderer> {
    if ext.is_empty() {
        return None;
    }
    registry()
        .iter()
        .copied()
        .find(|r| r.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

// ---------------------------------------------------------------------------
// ImageRenderer
// ---------------------------------------------------------------------------

use crate::heading_anchor::obsidian_heading_anchor;
use crate::media::{format_img_tag, is_all_display_keywords, parse_media_attrs};

use super::fuzzy_path::relative_asset_path;

/// Image file extensions recognized by `ImageRenderer`.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// Renderer for image embeds: `![[photo.jpg]]` → `<img>` or `![alt](url)`.
#[derive(Debug)]
pub struct ImageRenderer;

impl EmbedRenderer for ImageRenderer {
    fn extensions(&self) -> &[&'static str] {
        IMAGE_EXTENSIONS
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let out = match embed.alias {
            Some(alias_text) if is_all_display_keywords(alias_text) => {
                let alt = file_stem(embed.resolved_path);
                let attrs = parse_media_attrs(alias_text);
                format_img_tag(&url, &alt, &attrs)
            }
            Some(alias_text) => format!("![{}]({})", alias_text, url),
            None => {
                let alt = file_stem(embed.resolved_path);
                format!("![{}]({})", alt, url)
            }
        };
        RenderedEmbed::Inline(out)
    }
}

// file_stem now lives in common.rs — imported via common::file_stem below.

// ---------------------------------------------------------------------------
// MarkdownEmbedRenderer
// ---------------------------------------------------------------------------

/// Renderer for markdown transclusion: `![[file.md]]` → `<!-- moss-embed:path -->`.
///
/// The marker comment is resolved later by src-tauri's embed resolver, which
/// reads the target file's content and splices it inline. This renderer does
/// not perform I/O.
#[derive(Debug)]
pub struct MarkdownEmbedRenderer;

impl EmbedRenderer for MarkdownEmbedRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["md"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let anchor = build_embed_anchor(embed.section);
        RenderedEmbed::Deferred {
            marker: format!("<!-- moss-embed:{}{} -->", embed.resolved_path, anchor),
        }
    }
}

/// Build the anchor fragment for a markdown embed marker.
///
/// Preserves the `^` prefix on block references so the downstream embed
/// resolver can distinguish them from headings.
fn build_embed_anchor(section: Option<&str>) -> String {
    match section {
        None => String::new(),
        Some(s) if s.is_empty() => String::new(),
        Some(s) => {
            if s.starts_with('^') {
                format!("#{}", s)
            } else {
                format!("#{}", obsidian_heading_anchor(s))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IframeRenderer
// ---------------------------------------------------------------------------

/// Renderer for local HTML embeds: `![[file.html?query#frag|WxH]]` → `<iframe>`.
///
/// - `?query` is appended to the iframe `src` as URL query.
/// - `#fragment` is appended as URL fragment (order: path?query#frag).
/// - `|W` or `|WxH` becomes iframe width/height attributes via [`Sizing`].
/// - No sandbox attribute is set by default — noted as a follow-up.
#[derive(Debug)]
pub struct IframeRenderer;

impl EmbedRenderer for IframeRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["html", "htm"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let src = build_src(&url, embed.query, embed.section);
        let (width_attr, height_attr) = dim_attrs(embed.alias);
        let classes = format!("{} {}", CLASS_EMBED, CLASS_EMBED_IFRAME);
        let title_attr = iframe_title_attr(embed.alias);

        let html = format!(
            "<iframe class=\"{}\" src=\"{}\"{}{}{} loading=\"lazy\"></iframe>",
            classes,
            html_escape_attr(&src),
            title_attr,
            width_attr,
            height_attr,
        );
        RenderedEmbed::Html(html)
    }
}

/// Derive the `title=` attribute from alias.
///
/// - `|400x300` (sizing): no title (browser shows no tooltip).
/// - `|My cool widget` (plain text): use as title (accessible name + tooltip).
/// - No alias: no title (iframe's own `<title>` provides accessible name).
fn iframe_title_attr(alias: Option<&str>) -> String {
    let Some(a) = alias else {
        return String::new();
    };
    if Sizing::parse(a).is_some() {
        return String::new();
    }
    format!(" title=\"{}\"", html_escape_attr(a))
}

// build_src, dim_attrs, html_escape_attr now live in common.rs — imported above.

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct DummyRenderer;
    impl EmbedRenderer for DummyRenderer {
        fn extensions(&self) -> &[&'static str] {
            &["xyz"]
        }
        fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
            RenderedEmbed::Inline(format!("<dummy src={}>", embed.resolved_path))
        }
    }

    #[test]
    fn test_dummy_renderer_trait_surface() {
        let r = DummyRenderer;
        assert_eq!(r.extensions(), &["xyz"]);
        let embed = ParsedEmbed {
            resolved_path: "a.xyz",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("<dummy src=a.xyz>".to_string())
        );
    }

    // --- MarkdownEmbedRenderer ---

    #[test]
    fn test_markdown_embed_renderer_no_section() {
        let r = MarkdownEmbedRenderer;
        let embed = ParsedEmbed {
            resolved_path: "posts/intro.md",
            from_path: "index.md",
            query: None,
            section: None,
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Deferred {
                marker: "<!-- moss-embed:posts/intro.md -->".to_string()
            }
        );
    }

    #[test]
    fn test_markdown_embed_renderer_heading_section() {
        let r = MarkdownEmbedRenderer;
        let embed = ParsedEmbed {
            resolved_path: "guide.md",
            from_path: "index.md",
            query: None,
            section: Some("Getting Started"),
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Deferred {
                marker: "<!-- moss-embed:guide.md#getting-started -->".to_string()
            }
        );
    }

    #[test]
    fn test_markdown_embed_renderer_block_ref_section() {
        let r = MarkdownEmbedRenderer;
        let embed = ParsedEmbed {
            resolved_path: "guide.md",
            from_path: "index.md",
            query: None,
            section: Some("^block-xyz"),
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Deferred {
                marker: "<!-- moss-embed:guide.md#^block-xyz -->".to_string()
            }
        );
    }

    #[test]
    fn test_markdown_embed_renderer_extensions() {
        assert_eq!(MarkdownEmbedRenderer.extensions(), &["md"]);
    }

    // --- ImageRenderer ---

    #[test]
    fn test_image_renderer_no_alias() {
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "assets/photo.jpg",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![photo](../assets/photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_alias_plain_text() {
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some("A lovely cat"),
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![A lovely cat](photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_display_keywords() {
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some("contain"),
        };
        let out = match r.render(&embed) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("image renderer should return Inline"),
        };
        assert!(out.starts_with("<img "), "expected <img tag, got: {}", out);
        assert!(out.contains("src=\"photo.jpg\""), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_extensions_cover_all_formats() {
        let r = ImageRenderer;
        let exts: Vec<&&str> = r.extensions().iter().collect();
        for e in &["png", "jpg", "jpeg", "gif", "svg", "webp"] {
            assert!(
                exts.iter().any(|&&x| x == *e),
                "missing ext: {} in {:?}",
                e,
                exts
            );
        }
    }

    // --- Dim parser ---

    #[test]
    fn test_dim_css_px() {
        assert_eq!(Dim::Px(200).to_css(), "200px");
    }

    #[test]
    fn test_dim_css_percent() {
        assert_eq!(Dim::Percent(100.0).to_css(), "100%");
        assert_eq!(Dim::Percent(50.5).to_css(), "50.5%");
    }

    #[test]
    fn test_dim_css_vh() {
        assert_eq!(Dim::Vh(100.0).to_css(), "100vh");
    }

    // --- Sizing parser ---

    #[test]
    fn test_sizing_parse_width_only_px() {
        assert_eq!(Sizing::parse("200"), Some(Sizing::Width(Dim::Px(200))));
    }

    #[test]
    fn test_sizing_parse_width_only_percent() {
        assert_eq!(Sizing::parse("100%"), Some(Sizing::Width(Dim::Percent(100.0))));
    }

    #[test]
    fn test_sizing_parse_box_px() {
        assert_eq!(
            Sizing::parse("200x150"),
            Some(Sizing::Box(Dim::Px(200), Dim::Px(150)))
        );
    }

    #[test]
    fn test_sizing_parse_box_percent_by_px() {
        assert_eq!(
            Sizing::parse("100%x600"),
            Some(Sizing::Box(Dim::Percent(100.0), Dim::Px(600)))
        );
    }

    #[test]
    fn test_sizing_parse_box_vh_height() {
        assert_eq!(
            Sizing::parse("100%x100vh"),
            Some(Sizing::Box(Dim::Percent(100.0), Dim::Vh(100.0)))
        );
    }

    #[test]
    fn test_sizing_parse_rejects_display_keywords() {
        assert_eq!(Sizing::parse("contain"), None);
        assert_eq!(Sizing::parse("left top"), None);
    }

    #[test]
    fn test_sizing_parse_empty_returns_none() {
        assert_eq!(Sizing::parse(""), None);
        assert_eq!(Sizing::parse("   "), None);
    }

    // --- Reserved classnames ---

    #[test]
    fn test_embed_class_constants_stable() {
        // These strings are part of moss's HTML/CSS contract (#508).
        // Changing them is a breaking change for theme authors; this test
        // exists to force an explicit decision if anyone tries.
        assert_eq!(CLASS_EMBED, "moss-embed");
        assert_eq!(CLASS_EMBED_IFRAME, "moss-embed-iframe");
        assert_eq!(CLASS_EMBED_PDF, "moss-embed-pdf");
        assert_eq!(CLASS_EMBED_NOTEBOOK, "moss-embed-notebook");
    }

    // --- RenderedEmbed variants ---

    #[test]
    fn test_rendered_embed_html_variant() {
        let h = RenderedEmbed::Html("<iframe src=\"x\"></iframe>".to_string());
        match h {
            RenderedEmbed::Html(s) => assert!(s.contains("iframe")),
            _ => panic!("expected Html variant"),
        }
    }

    #[test]
    fn test_rendered_embed_deferred_variant() {
        let d = RenderedEmbed::Deferred {
            marker: "<!-- moss-embed-ipynb:nb.ipynb -->".to_string(),
        };
        match d {
            RenderedEmbed::Deferred { marker } => assert!(marker.contains("ipynb")),
            _ => panic!("expected Deferred variant"),
        }
    }

    // --- Registry lookup ---

    #[test]
    fn test_lookup_renderer_by_extension() {
        assert!(lookup_renderer("jpg").is_some());
        assert!(lookup_renderer("JPG").is_some()); // case-insensitive
        assert!(lookup_renderer("md").is_some());
        assert!(lookup_renderer("xyz").is_none());
        assert!(lookup_renderer("").is_none());
    }

    // --- IframeRenderer ---

    #[test]
    fn test_iframe_renderer_extensions() {
        let r = IframeRenderer;
        let exts: Vec<&&str> = r.extensions().iter().collect();
        assert!(exts.iter().any(|&&x| x == "html"));
        assert!(exts.iter().any(|&&x| x == "htm"));
    }

    fn iframe_html(e: &ParsedEmbed) -> String {
        match IframeRenderer.render(e) {
            RenderedEmbed::Html(s) => s,
            _ => panic!("expected Html variant"),
        }
    }

    #[test]
    fn test_iframe_renderer_basic() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
        });
        assert!(out.contains("<iframe "), "got: {}", out);
        assert!(
            out.contains("class=\"moss-embed moss-embed-iframe\""),
            "got: {}",
            out
        );
        assert!(out.contains("src=\"widget.html\""), "got: {}", out);
        assert!(out.contains("loading=\"lazy\""), "got: {}", out);
    }

    #[test]
    fn test_iframe_renderer_with_query() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "scale.html",
            from_path: "post.md",
            query: Some("a=major,minor&r=D"),
            section: None,
            alias: None,
        });
        assert!(
            out.contains("src=\"scale.html?a=major,minor&amp;r=D\""),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_iframe_renderer_with_query_and_fragment() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "doc.html",
            from_path: "post.md",
            query: Some("x=1"),
            section: Some("section2"),
            alias: None,
        });
        assert!(
            out.contains("src=\"doc.html?x=1#section2\""),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_iframe_renderer_with_width_only() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("400"),
        });
        assert!(out.contains("width=\"400px\""), "got: {}", out);
        assert!(!out.contains("height="), "got: {}", out);
    }

    #[test]
    fn test_iframe_renderer_with_width_and_height() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("100%x600"),
        });
        assert!(out.contains("width=\"100%\""), "got: {}", out);
        assert!(out.contains("height=\"600px\""), "got: {}", out);
    }

    #[test]
    fn test_iframe_renderer_scale_tree_example() {
        // Real-world case from test-sites/刘果/交互/音阶对比/音阶对比.md
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "scale-family-tree.html",
            from_path: "post.md",
            query: Some("a=major_pent,major_blues,in&r=major_pent:D,major_blues:D"),
            section: None,
            alias: Some("100%x600"),
        });
        assert!(
            out.contains("src=\"scale-family-tree.html?a=major_pent,major_blues,in&amp;r=major_pent:D,major_blues:D\""),
            "got: {}",
            out
        );
        assert!(out.contains("width=\"100%\""), "got: {}", out);
        assert!(out.contains("height=\"600px\""), "got: {}", out);
        assert!(
            out.contains("class=\"moss-embed moss-embed-iframe\""),
            "got: {}",
            out
        );
        // Sizing alias → no title attr (avoid filename leakage as tooltip).
        assert!(!out.contains("title="), "got: {}", out);
    }

    #[test]
    fn test_iframe_renderer_no_alias_emits_no_title() {
        // No alias: iframe's own <title> provides accessible name; no outer tooltip.
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
        });
        assert!(!out.contains("title="), "got: {}", out);
    }

    #[test]
    fn test_iframe_renderer_text_alias_becomes_title() {
        // Non-sizing alias text is used as title (accessible name + tooltip).
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("My cool widget"),
        });
        assert!(
            out.contains("title=\"My cool widget\""),
            "got: {}",
            out
        );
        // And no dim attrs, since alias isn't a sizing hint.
        assert!(!out.contains("width="), "got: {}", out);
    }

    // --- Sizing malformed-input coverage ---

    #[test]
    fn test_sizing_parse_malformed_box_is_none() {
        assert_eq!(Sizing::parse("100xbad"), None);
        assert_eq!(Sizing::parse("100x"), None);
        assert_eq!(Sizing::parse("-100"), None);
    }

    #[test]
    fn test_iframe_renderer_malformed_sizing_drops_dim_attrs() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("100xbad"),
        });
        assert!(!out.contains("width="), "got: {}", out);
        assert!(!out.contains("height="), "got: {}", out);
        // Malformed sizing isn't recognized by Sizing::parse, so it falls through
        // to the title-attr path and becomes a title.
        assert!(out.contains("title=\"100xbad\""), "got: {}", out);
    }
}
