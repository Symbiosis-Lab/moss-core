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
pub mod folder_list;
use common::{
    build_src, dim_attrs, file_stem, html_escape_attr, path_extension_lower, width_attr,
};

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

// ---------------------------------------------------------------------------
// Deferred-marker prefixes (contract with src-tauri resolvers)
// ---------------------------------------------------------------------------

/// Marker prefix emitted by [`MarkdownEmbedRenderer`].
///
/// Format: `<!-- moss-embed:PATH[#anchor] -->`. Resolved by src-tauri's
/// `resolve_embeds` (inlines target markdown content).
///
/// No `-<type>` suffix for historical reasons: this was the original embed
/// marker before typed embeds existed. New typed markers use
/// `moss-embed-<type>:` (see [`MARKER_IPYNB`], [`MARKER_TABLE`]).
pub const MARKER_MARKDOWN: &str = "moss-embed";

/// Marker prefix emitted by [`NotebookRenderer`].
///
/// Format: `<!-- moss-embed-ipynb:PATH[?query] -->`. Resolved by src-tauri
/// via nbconvert.
pub const MARKER_IPYNB: &str = "moss-embed-ipynb";

/// Marker prefix emitted by [`TableRenderer`].
///
/// Format: `<!-- moss-embed-table:PATH -->`. src-tauri reads the file and
/// calls [`crate::csv_table::render`] (a pure renderer).
pub const MARKER_TABLE: &str = "moss-embed-table";

// Re-export folder_list marker constants for convenience.
pub use folder_list::{MARKER_FOLDER_LIST, MARKER_END};

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
    /// `|pipe-content` from the source wikilink — with any spec § P9 width
    /// token already split out into [`Self::width`]. Image renderer uses
    /// this for display keywords / size; other renderers parse per their
    /// convention.
    pub alias: Option<&'a str>,
    /// Canonical width value (`body | wide | page | screen`) extracted from
    /// the pipe-alias by the wikilink resolver. `None` means the author
    /// did not include a width token; renderers omit `data-width` in that
    /// case so themes can target the default via `:not([data-width])`.
    ///
    /// `full` is normalised to `screen` upstream — values reaching here
    /// are already in value-space terms (see
    /// [`crate::media::match_width_token`]).
    pub width: Option<&'static str>,
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
    /// A marker comment for a post-pass resolver to expand with file I/O.
    ///
    /// Format convention: `<!-- <prefix>:<target> -->` where `<prefix>`
    /// uniquely identifies the resolver (e.g. `moss-embed-ipynb`,
    /// `moss-embed-table`, `moss-embed-plugin-<plugin-name>`) and
    /// `<target>` is the body the resolver parses (commonly a path,
    /// optionally with `?query#fragment|alias`).
    ///
    /// The resolver lives in src-tauri (where async and I/O are allowed).
    /// Built-in prefixes are exported as pub const: [`MARKER_MARKDOWN`],
    /// [`MARKER_IPYNB`], [`MARKER_TABLE`]. Plugin-registered renderers
    /// emit `moss-embed-plugin-<plugin-name>:` — see
    /// [`super::registry`] for the full two-pass dispatch design.
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

    /// Page-level HTML fragments this renderer needs in `<head>`, injected
    /// once per page that contains at least one embed from this renderer.
    ///
    /// Example: `ModelViewerRenderer` returns a `<script type="module">` tag
    /// so that `<model-viewer>` custom elements work. The build pipeline
    /// collects and deduplicates these across all embeds on a page.
    ///
    /// Default: empty. Renderers with no page-level assets don't override.
    fn head_assets(&self) -> &[&'static str] {
        &[]
    }
}

/// Built-in renderer registry. Initialized lazily on first lookup.
///
/// Each renderer is a unit struct, so the pointer is to a zero-size `'static`
/// — no heap allocation ever. Future renderers (notebook, 3d, table, plugins)
/// get appended here as they ship.
///
/// Extension sets across renderers are currently disjoint. Adding overlap
/// (e.g., if a future renderer claims `.ogg` for video) would require
/// tie-break logic here; first-match-wins is the only implicit rule today.
fn registry() -> &'static [&'static dyn EmbedRenderer] {
    static INIT: OnceLock<Vec<&'static dyn EmbedRenderer>> = OnceLock::new();
    INIT.get_or_init(|| {
        vec![
            &ImageRenderer as &'static dyn EmbedRenderer,
            &MarkdownEmbedRenderer as &'static dyn EmbedRenderer,
            &IframeRenderer as &'static dyn EmbedRenderer,
            &PdfRenderer as &'static dyn EmbedRenderer,
            &AudioRenderer as &'static dyn EmbedRenderer,
            &VideoRenderer as &'static dyn EmbedRenderer,
            &NotebookRenderer as &'static dyn EmbedRenderer,
            &ModelViewerRenderer as &'static dyn EmbedRenderer,
            &TableRenderer as &'static dyn EmbedRenderer,
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
use crate::media::{
    format_img_tag, html_escape, is_all_display_keywords, parse_media_attrs,
};

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

        // The wikilink resolver pre-extracts any spec § P9 width token from
        // the alias into `embed.width` (a canonical value-space term — see
        // `crate::media::match_width_token`). When width is present we emit
        // the full `<figure class="moss-image" data-width="...">...</figure>`
        // shape directly: pulldown-cmark passes that through as an
        // `HtmlBlock` (block-level), so the figure wrapper survives end-to-
        // end without needing the src-tauri standalone-paragraph rule to
        // recognise raw `<img>` HtmlBlocks. Per spec, `data-width` sits on
        // the wrapper, not the inner img.
        //
        // `embed.alias` arrives with the width segment already stripped (the
        // resolver hands the renderer the post-extraction remainder), so the
        // arms below treat it as ordinary caption/display-keyword content.
        let out = match (embed.width, embed.alias) {
            // Width-only alias (`![[photo.jpg|full]]`) — empty alt because
            // width is structural, not a caption.
            (Some(w), None) => format!(
                r#"<figure class="moss-image" data-width="{}"><img src="{}" alt="" /></figure>"#,
                w,
                html_escape(&url),
            ),
            // Width + display keywords (`![[photo.jpg|cover|full]]`) — the
            // display keywords land on the inner img as inline style; the
            // figure carries data-width. Filename stem becomes alt for
            // assistive tech (matches the no-width display-keyword arm).
            (Some(w), Some(rest)) if is_all_display_keywords(rest) => {
                let alt = file_stem(embed.resolved_path);
                let attrs = parse_media_attrs(rest);
                let style_attr = attrs
                    .to_inline_style()
                    .map(|s| format!(" style=\"{}\"", html_escape(&s)))
                    .unwrap_or_default();
                format!(
                    r#"<figure class="moss-image" data-width="{}"><img src="{}" alt="{}"{} /></figure>"#,
                    w,
                    html_escape(&url),
                    html_escape(&alt),
                    style_attr,
                )
            }
            // Width + caption text (`![[photo.jpg|A nice photo|full]]`) —
            // caption goes into a `<figcaption>` inside the figure, matching
            // the byte shape of `image_render::wrap_in_figure(.., Some(text), ..)`
            // in src-tauri. Alt mirrors caption text for a11y parity with
            // implicit-figure rendering.
            (Some(w), Some(rest)) => format!(
                r#"<figure class="moss-image" data-width="{}"><img src="{}" alt="{}" /><figcaption>{}</figcaption></figure>"#,
                w,
                html_escape(&url),
                html_escape(rest),
                html_escape(rest),
            ),
            // No width — fall through to the pre-existing classifier.
            (None, alias) => match alias {
                Some(alias_text) if is_all_display_keywords(alias_text) => {
                    // Display-keyword aliases (`cover`, `left top`, …) describe
                    // layout, not content — keep the filename stem as alt so the
                    // image still has *some* description for screen readers.
                    let alt = file_stem(embed.resolved_path);
                    let attrs = parse_media_attrs(alias_text);
                    format_img_tag(&url, &alt, &attrs)
                }
                Some(alias_text) => format!("![{}]({})", alias_text, url),
                None => {
                    // No author-provided alias → empty alt rather than synthesizing
                    // one from the filename stem. Synthesized alts like
                    // `Pasted image 20260505161028` are not meaningful descriptions
                    // for assistive tech, AND a non-empty alt would trip the
                    // bare-image-paragraph figure rule into producing a `<figure>`
                    // captioned with the filename — visible junk.
                    // See docs/plans/2026-05-05-figure-captions-design.md.
                    format!("![]({})", url)
                }
            },
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
            marker: format!("<!-- {}:{}{} -->", MARKER_MARKDOWN, embed.resolved_path, anchor),
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
        let (dim_width_attr, height_attr) = dim_attrs(embed.alias);
        let title_attr = iframe_title_attr(embed.alias);
        let width_data_attr = width_attr(embed.width);

        let html = format!(
            "<iframe class=\"{}\" data-type=\"iframe\"{} src=\"{}\"{}{}{} loading=\"lazy\"></iframe>",
            CLASS_EMBED,
            width_data_attr,
            html_escape_attr(&src),
            title_attr,
            dim_width_attr,
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

// ---------------------------------------------------------------------------
// PdfRenderer
// ---------------------------------------------------------------------------

/// Renderer for PDF embeds: `![[report.pdf]]` → `<object type="application/pdf">`.
///
/// `<object>` has better keyboard navigation than `<iframe>` for PDFs and
/// supports inline fallback content for browsers that can't render PDFs natively.
#[derive(Debug)]
pub struct PdfRenderer;

impl EmbedRenderer for PdfRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["pdf"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let data_url = build_src(&url, embed.query, embed.section);
        let (dim_width_attr, height_attr) = dim_attrs(embed.alias);
        let name = html_escape_attr(&file_stem(embed.resolved_path));
        let width_data_attr = width_attr(embed.width);

        // <object> with inline download fallback for browsers that can't render PDFs.
        let html = format!(
            "<object class=\"{}\" data-type=\"pdf\"{} type=\"application/pdf\" data=\"{}\"{}{}><a href=\"{}\">Download {}</a></object>",
            CLASS_EMBED,
            width_data_attr,
            html_escape_attr(&data_url),
            dim_width_attr,
            height_attr,
            html_escape_attr(&url),
            name,
        );
        RenderedEmbed::Html(html)
    }
}

// ---------------------------------------------------------------------------
// AudioRenderer
// ---------------------------------------------------------------------------

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "ogg", "flac", "m4a", "opus"];

/// Renderer for audio embeds: `![[song.mp3]]` → `<audio controls>`.
///
/// `preload=metadata` so the browser fetches duration/sample-rate but not the
/// full payload until the user presses play.
///
/// Output form: `<audio><source src="..." type="..."></audio>` (HTML5
/// multi-source). This is safe today because no audio extension rewriter
/// exists in src-tauri — audio files pass through unchanged. If a future
/// converter is introduced (e.g., `.flac→.mp3` for size, `.m4a→.opus` for
/// browser parity, see #504), this renderer must switch to the single
/// `src=` form for the same reason `VideoRenderer` did: the
/// `add_*_placeholder_attributes` regex pattern in
/// `src-tauri/src/build/media/placeholder.rs` matches `<tag\s+[^>]*?src=>`,
/// not nested `<source>` children. See #593 and the docstring on
/// `VideoRenderer` for the full failure mode.
#[derive(Debug)]
pub struct AudioRenderer;

impl EmbedRenderer for AudioRenderer {
    fn extensions(&self) -> &[&'static str] {
        AUDIO_EXTENSIONS
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let ext = path_extension_lower(embed.resolved_path);
        let mime = audio_mime_for_ext(&ext);
        let width_data_attr = width_attr(embed.width);

        let html = format!(
            "<audio class=\"{}\" data-type=\"audio\"{} controls preload=\"metadata\"><source src=\"{}\" type=\"{}\">Your browser does not support the audio tag.</audio>",
            CLASS_EMBED,
            width_data_attr,
            html_escape_attr(&url),
            mime,
        );
        RenderedEmbed::Html(html)
    }
}

fn audio_mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "opus" => "audio/opus",
        _ => {
            // Defensive: registry-gated, so only reachable if AUDIO_EXTENSIONS
            // gains an entry without a matching mime entry here.
            debug_assert!(false, "unmapped audio extension: {}", ext);
            "application/octet-stream"
        }
    }
}

// ---------------------------------------------------------------------------
// VideoRenderer
// ---------------------------------------------------------------------------

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "mov", "m4v"];

/// Renderer for video embeds: `![[clip.mp4]]` → `<video src="..." controls>`.
///
/// `|WxH` becomes width/height attrs. `preload=metadata` so the browser
/// fetches duration/dimensions but not the full payload until play.
///
/// Output form: single `src=` attribute on `<video>` (no `<source>` child),
/// no `type=` attribute. Two coupled reasons:
///
/// 1. **`type=` would go stale.** The downstream rewriter
///    `src-tauri/src/build/media/placeholder.rs::add_video_placeholder_attributes`
///    rewrites the `src` extension from `.mov` to `.mp4` after the renderer
///    runs (moss converts `.mov` source files to `.mp4` during build, so a
///    raw `.mov` reference would 404). Any explicit `type="video/quicktime"`
///    emitted here would survive the rewrite as a lie. Browser sniffing
///    from the rewritten URL extension is more reliable than a stale type
///    hint.
///
/// 2. **The rewriter regex requires single-`src=` form.** It matches
///    `<video\s+[^>]*?src="...">` — `src=` must be on the `<video>` tag
///    itself, not on a nested `<source>` child. With nested `<source>`,
///    the regex no-ops and the `.mov→.mp4` rewrite + `data-placeholder-src`
///    + `poster` + `data-thumb-src` injection all silently drop. This
///    constraint is load-bearing; see #592 for an integration test that
///    pins it across the cross-crate boundary, and #593 for the audio
///    asymmetry. This shape also matches the historical moss output that
///    liu-guo.com still ships.
///
/// Note: `.mov` is codec-dependent at the source. Safari plays QuickTime
/// natively; Chrome/Firefox accept the MIME but decode only if the
/// container's video codec is supported (usually H.264). The `.mov→.mp4`
/// rewriter solves this in practice — served files end as `.mp4`.
#[derive(Debug)]
pub struct VideoRenderer;

impl EmbedRenderer for VideoRenderer {
    fn extensions(&self) -> &[&'static str] {
        VIDEO_EXTENSIONS
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let (dim_width_attr, height_attr) = dim_attrs(embed.alias);
        let width_data_attr = width_attr(embed.width);

        let html = format!(
            "<video class=\"{}\" data-type=\"video\"{} src=\"{}\" controls preload=\"metadata\"{}{}></video>",
            CLASS_EMBED,
            width_data_attr,
            html_escape_attr(&url),
            dim_width_attr,
            height_attr,
        );
        RenderedEmbed::Html(html)
    }
}

// ---------------------------------------------------------------------------
// NotebookRenderer
// ---------------------------------------------------------------------------

/// Renderer for Jupyter notebooks: `![[file.ipynb]]` → deferred marker.
///
/// Emits `<!-- moss-embed-ipynb:PATH -->` (with optional `?query` appended).
/// The real rendering happens in src-tauri via nbconvert or equivalent —
/// src-tauri resolves the marker post-pass.
#[derive(Debug)]
pub struct NotebookRenderer;

impl EmbedRenderer for NotebookRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["ipynb"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        // NOTE: embed.width is intentionally dropped at the marker boundary.
        // Notebook wrappers are emitted in src-tauri post-passes that don't
        // currently read width from the marker target. Track when those
        // switch to data-width emission — file a follow-up issue if needed.
        let target = match embed.query {
            Some(q) => format!("{}?{}", embed.resolved_path, q),
            None => embed.resolved_path.to_string(),
        };
        RenderedEmbed::Deferred {
            marker: format!("<!-- {}:{} -->", MARKER_IPYNB, target),
        }
    }
}

// ---------------------------------------------------------------------------
// ModelViewerRenderer (3D)
// ---------------------------------------------------------------------------

/// Page-level script import needed for `<model-viewer>` to work.
///
/// Loaded from Google's CDN. Pinned to a major version for stability.
/// If this URL becomes unavailable, self-host and update this constant.
const MODEL_VIEWER_SCRIPT: &str = "<script type=\"module\" src=\"https://ajax.googleapis.com/ajax/libs/model-viewer/3.4.0/model-viewer.min.js\"></script>";

/// Renderer for 3D model embeds: `![[model.glb|400x400]]` → `<model-viewer>`.
///
/// Requires the `<model-viewer>` custom element script, injected via
/// `head_assets` once per page that contains any `.glb`/`.gltf` embed.
#[derive(Debug)]
pub struct ModelViewerRenderer;

impl EmbedRenderer for ModelViewerRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["glb", "gltf"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);
        let style = model_viewer_style(embed.alias);
        let width_data_attr = width_attr(embed.width);

        let html = format!(
            "<model-viewer class=\"{}\" data-type=\"3d\"{} src=\"{}\" camera-controls auto-rotate touch-action=\"pan-y\" loading=\"lazy\"{}></model-viewer>",
            CLASS_EMBED,
            width_data_attr,
            html_escape_attr(&url),
            style,
        );
        RenderedEmbed::Html(html)
    }

    fn head_assets(&self) -> &[&'static str] {
        &[MODEL_VIEWER_SCRIPT]
    }
}

/// Derive the inline `style="width:..;height:.."` for `<model-viewer>`.
///
/// Unlike iframe/video, the `<model-viewer>` element uses CSS length values
/// not HTML `width=`/`height=` attributes — hence inline style.
fn model_viewer_style(alias: Option<&str>) -> String {
    let Some(a) = alias else {
        return String::new();
    };
    match Sizing::parse(a) {
        Some(Sizing::Width(w)) => format!(" style=\"width:{}\"", w.to_css()),
        Some(Sizing::Box(w, h)) => format!(
            " style=\"width:{};height:{}\"",
            w.to_css(),
            h.to_css()
        ),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// TableRenderer
// ---------------------------------------------------------------------------

/// Renderer for tabular data: `![[data.csv]]` → deferred marker.
///
/// Emits `<!-- moss-embed-table:PATH -->`. src-tauri reads the CSV/TSV file
/// and calls `moss_core::csv_table::render` (a pure renderer) in a post-pass.
#[derive(Debug)]
pub struct TableRenderer;

impl EmbedRenderer for TableRenderer {
    fn extensions(&self) -> &[&'static str] {
        &["csv", "tsv"]
    }

    fn render(&self, embed: &ParsedEmbed<'_>) -> RenderedEmbed {
        // NOTE: embed.width is intentionally dropped at the marker boundary.
        // Table wrappers are emitted in src-tauri post-passes (csv_table) that
        // don't currently read width from the marker target. Track when those
        // switch to data-width emission — file a follow-up issue if needed.
        RenderedEmbed::Deferred {
            marker: format!("<!-- {}:{} -->", MARKER_TABLE, embed.resolved_path),
        }
    }
}

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
            width: None,
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
            width: None,
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
            width: None,
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
            width: None,
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
    fn test_image_renderer_no_alias_emits_empty_alt() {
        // No author-provided alias → empty alt. Synthesizing one from the
        // filename stem (`photo`, or worse: `Pasted image 20260505161028`)
        // is not a real description for assistive tech, AND a non-empty alt
        // would trip the bare-image-paragraph figure rule into producing a
        // `<figure>` captioned with the filename. Empty alt is the right
        // boundary: image still renders, no spurious caption.
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "assets/photo.jpg",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![](../assets/photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_empty_alias_treated_as_no_alias() {
        // `![[file|]]` (literal empty pipe) goes through `Some("")`. The
        // first match arm checks `is_all_display_keywords("")` which returns
        // false (no tokens), so it falls into the plain-alias branch with
        // empty string — same outcome as `None`: empty alt, no figure
        // wrapping by the bare-image-paragraph rule.
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some(""),
            width: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![](photo.jpg)".to_string())
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
            width: None,
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
            width: None,
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

    // -- ImageRenderer: width pipe-alias (spec § P9, follow-up to PR-1c) --

    fn image_inline(alias: Option<&str>) -> String {
        let r = ImageRenderer;
        // Pre-extract width from alias to mirror the wikilink resolver's
        // pre-pass (so renderer tests exercise the same input shape that
        // production renderers see).
        let (width, alias_owned) = match alias {
            Some(a) => crate::media::extract_width_from_alias(a),
            None => (None, String::new()),
        };
        let alias_for_renderer: Option<&str> = if width.is_some() {
            if alias_owned.is_empty() {
                None
            } else {
                Some(alias_owned.as_str())
            }
        } else {
            alias
        };
        let embed = ParsedEmbed {
            resolved_path: "assets/photo.jpg",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: alias_for_renderer,
            width,
        };
        match r.render(&embed) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("image renderer should return Inline"),
        }
    }

    #[test]
    fn test_image_renderer_width_full_aliases_to_screen() {
        // `full` is the author-facing alias; emitted value is `screen`.
        let out = image_inline(Some("full"));
        assert!(
            out.contains(r#"data-width="screen""#),
            "expected data-width=screen, got: {}",
            out
        );
        // Full figure-wrapped HTML: spec § P9 puts data-width on the
        // wrapper, not the inner img.
        assert!(
            out.starts_with(r#"<figure class="moss-image" data-width="screen">"#),
            "got: {}",
            out
        );
        assert!(out.ends_with("</figure>"), "got: {}", out);
        // Empty alt — width is structural, not a caption.
        assert!(out.contains(r#"alt="""#), "got: {}", out);
        // No figcaption when there's no caption text.
        assert!(!out.contains("<figcaption>"), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_width_wide() {
        let out = image_inline(Some("wide"));
        assert!(out.contains(r#"data-width="wide""#), "got: {}", out);
        assert!(
            out.starts_with(r#"<figure class="moss-image" data-width="wide">"#),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_image_renderer_width_body_page_screen() {
        for (token, canonical) in [("body", "body"), ("page", "page"), ("screen", "screen")] {
            let out = image_inline(Some(token));
            assert!(
                out.contains(&format!("data-width=\"{}\"", canonical)),
                "{}: got {}",
                token,
                out
            );
        }
    }

    #[test]
    fn test_image_renderer_no_alias_omits_data_width() {
        let out = image_inline(None);
        assert!(!out.contains("data-width="), "got: {}", out);
        // No alias → bare markdown form (no figure wrap from this renderer).
        assert!(out.starts_with("!["), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_caption_alias_omits_data_width() {
        // Plain caption text is not a width token; data-width must be absent
        // so themes can use the `:not([data-width])` default selector.
        let out = image_inline(Some("A beautiful sunset"));
        assert!(!out.contains("data-width="), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_caption_with_width_word_not_shadowed() {
        // The word `wide` inside a longer caption must NOT trigger width
        // recognition — only an exact-match alias segment counts.
        let out = image_inline(Some("a wide angle shot"));
        assert!(!out.contains("data-width="), "got: {}", out);
        // Caption survives as alt text via the existing markdown fallback.
        assert!(out.contains("a wide angle shot"), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_caption_then_width_pipe() {
        // Multi-pipe form: caption text followed by a width token.
        let out = image_inline(Some("A nice photo|full"));
        assert!(out.contains(r#"data-width="screen""#), "got: {}", out);
        // Caption survives as alt text AND as figcaption (mirrors implicit-
        // figure shape from src-tauri's transform_events).
        assert!(out.contains(r#"alt="A nice photo""#), "got: {}", out);
        assert!(
            out.contains("<figcaption>A nice photo</figcaption>"),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_image_renderer_display_keywords_with_width() {
        // Display keywords (`cover`) compose with width (`full`).
        let out = image_inline(Some("cover|full"));
        assert!(out.contains(r#"data-width="screen""#), "got: {}", out);
        assert!(out.contains("object-fit:cover"), "got: {}", out);
        // Filename stem used as alt (matches the display-keyword arm).
        assert!(out.contains(r#"alt="photo""#), "got: {}", out);
        // No figcaption — display keywords are layout, not caption text.
        assert!(!out.contains("<figcaption>"), "got: {}", out);
    }

    #[test]
    fn test_image_renderer_width_html_escapes_caption() {
        // Caption text with HTML-unsafe chars must be escaped in the alt
        // attribute AND in the figcaption text.
        let out = image_inline(Some(r#"Q&A "best"|full"#));
        assert!(out.contains(r#"data-width="screen""#), "got: {}", out);
        assert!(
            out.contains(r#"alt="Q&amp;A &quot;best&quot;""#),
            "got: {}",
            out
        );
        assert!(
            out.contains("<figcaption>Q&amp;A &quot;best&quot;</figcaption>"),
            "got: {}",
            out
        );
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
        assert_eq!(CLASS_EMBED_AUDIO, "moss-embed-audio");
        assert_eq!(CLASS_EMBED_VIDEO, "moss-embed-video");
        assert_eq!(CLASS_EMBED_NOTEBOOK, "moss-embed-notebook");
        assert_eq!(CLASS_EMBED_3D, "moss-embed-3d");
        assert_eq!(CLASS_EMBED_TABLE, "moss-embed-table");
    }

    #[test]
    fn test_embed_marker_prefixes_stable() {
        // Marker prefixes are a contract between moss-core (emit) and
        // src-tauri (resolve). Changing them breaks the resolver.
        assert_eq!(MARKER_MARKDOWN, "moss-embed");
        assert_eq!(MARKER_IPYNB, "moss-embed-ipynb");
        assert_eq!(MARKER_TABLE, "moss-embed-table");
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
            width: None,
        });
        assert!(out.contains("<iframe "), "got: {}", out);
        assert!(
            out.contains("class=\"moss-embed\" data-type=\"iframe\""),
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
            width: None,
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
            width: None,
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
            width: None,
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
            width: None,
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
            width: None,
        });
        assert!(
            out.contains("src=\"scale-family-tree.html?a=major_pent,major_blues,in&amp;r=major_pent:D,major_blues:D\""),
            "got: {}",
            out
        );
        assert!(out.contains("width=\"100%\""), "got: {}", out);
        assert!(out.contains("height=\"600px\""), "got: {}", out);
        assert!(
            out.contains("class=\"moss-embed\" data-type=\"iframe\""),
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
            width: None,
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
            width: None,
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
            width: None,
        });
        assert!(!out.contains("width="), "got: {}", out);
        assert!(!out.contains("height="), "got: {}", out);
        // Malformed sizing isn't recognized by Sizing::parse, so it falls through
        // to the title-attr path and becomes a title.
        assert!(out.contains("title=\"100xbad\""), "got: {}", out);
    }

    // --- PdfRenderer ---

    fn pdf_html(e: &ParsedEmbed) -> String {
        match PdfRenderer.render(e) {
            RenderedEmbed::Html(s) => s,
            _ => panic!("expected Html variant"),
        }
    }

    #[test]
    fn test_pdf_renderer_extensions() {
        assert_eq!(PdfRenderer.extensions(), &["pdf"]);
    }

    #[test]
    fn test_pdf_renderer_basic() {
        let out = pdf_html(&ParsedEmbed {
            resolved_path: "assets/report.pdf",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(out.contains("<object "), "got: {}", out);
        assert!(
            out.contains("type=\"application/pdf\""),
            "got: {}",
            out
        );
        assert!(
            out.contains("data=\"../assets/report.pdf\""),
            "got: {}",
            out
        );
        assert!(
            out.contains("class=\"moss-embed\" data-type=\"pdf\""),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_pdf_renderer_with_page_fragment() {
        // #page=5 is the standard PDF viewer fragment.
        let out = pdf_html(&ParsedEmbed {
            resolved_path: "doc.pdf",
            from_path: "post.md",
            query: None,
            section: Some("page=5"),
            alias: None,
            width: None,
        });
        assert!(
            out.contains("data=\"doc.pdf#page=5\""),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_pdf_renderer_with_sizing() {
        let out = pdf_html(&ParsedEmbed {
            resolved_path: "doc.pdf",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("100%x800"),
            width: None,
        });
        assert!(out.contains("width=\"100%\""), "got: {}", out);
        assert!(out.contains("height=\"800px\""), "got: {}", out);
    }

    #[test]
    fn test_pdf_renderer_fallback_link() {
        // <object> must contain fallback content for browsers that can't render PDFs.
        let out = pdf_html(&ParsedEmbed {
            resolved_path: "doc.pdf",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(
            out.contains("href=\"doc.pdf\""),
            "fallback link missing: {}",
            out
        );
        assert!(out.contains("Download doc"), "got: {}", out);
    }

    // --- AudioRenderer ---

    fn audio_html(e: &ParsedEmbed) -> String {
        match AudioRenderer.render(e) {
            RenderedEmbed::Html(s) => s,
            _ => panic!("expected Html"),
        }
    }

    #[test]
    fn test_audio_renderer_extensions() {
        let r = AudioRenderer;
        let exts: Vec<&&str> = r.extensions().iter().collect();
        for e in &["mp3", "wav", "ogg", "flac", "m4a", "opus"] {
            assert!(exts.iter().any(|&&x| x == *e), "missing: {}", e);
        }
    }

    #[test]
    fn test_audio_renderer_basic() {
        let out = audio_html(&ParsedEmbed {
            resolved_path: "assets/song.mp3",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(out.contains("<audio "), "got: {}", out);
        assert!(out.contains("controls"), "got: {}", out);
        assert!(
            out.contains("class=\"moss-embed\" data-type=\"audio\""),
            "got: {}",
            out
        );
        assert!(
            out.contains("<source src=\"../assets/song.mp3\""),
            "got: {}",
            out
        );
        assert!(out.contains("type=\"audio/mpeg\""), "got: {}", out);
    }

    #[test]
    fn test_audio_renderer_mime_types() {
        let cases: &[(&str, &str)] = &[
            ("a.mp3", "audio/mpeg"),
            ("a.wav", "audio/wav"),
            ("a.ogg", "audio/ogg"),
            ("a.flac", "audio/flac"),
            ("a.m4a", "audio/mp4"),
            ("a.opus", "audio/opus"),
        ];
        for (file, mime) in cases {
            let out = audio_html(&ParsedEmbed {
                resolved_path: file,
                from_path: "post.md",
                query: None,
                section: None,
                alias: None,
            width: None,
            });
            assert!(
                out.contains(&format!("type=\"{}\"", mime)),
                "{}: got {}",
                file,
                out
            );
        }
    }

    #[test]
    fn test_audio_renderer_preload_metadata() {
        let out = audio_html(&ParsedEmbed {
            resolved_path: "song.mp3",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(out.contains("preload=\"metadata\""), "got: {}", out);
    }

    // --- VideoRenderer ---

    fn video_html(e: &ParsedEmbed) -> String {
        match VideoRenderer.render(e) {
            RenderedEmbed::Html(s) => s,
            _ => panic!("expected Html"),
        }
    }

    #[test]
    fn test_video_renderer_extensions() {
        let r = VideoRenderer;
        let exts: Vec<&&str> = r.extensions().iter().collect();
        for e in &["mp4", "webm", "mov", "m4v"] {
            assert!(exts.iter().any(|&&x| x == *e), "missing: {}", e);
        }
    }

    #[test]
    fn test_video_renderer_basic() {
        let out = video_html(&ParsedEmbed {
            resolved_path: "assets/trailer.mp4",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(out.contains("<video "), "got: {}", out);
        assert!(out.contains("controls"), "got: {}", out);
        assert!(
            out.contains("class=\"moss-embed\" data-type=\"video\""),
            "got: {}",
            out
        );
        // Single src= attribute on <video>, not nested <source>. Required so
        // the downstream add_video_placeholder_attributes rewriter can match
        // the element and perform .mov→.mp4 conversion + placeholder injection.
        assert!(
            out.contains(" src=\"../assets/trailer.mp4\""),
            "expected single src attribute, got: {}",
            out
        );
        assert!(
            !out.contains("<source"),
            "must not emit <source> child (rewriter only matches <video src=>), got: {}",
            out
        );
        assert!(
            out.ends_with("</video>"),
            "must end with closing </video>, got: {}",
            out
        );
    }

    #[test]
    fn test_video_renderer_with_sizing() {
        let out = video_html(&ParsedEmbed {
            resolved_path: "clip.mp4",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("640x360"),
            width: None,
        });
        assert!(out.contains("width=\"640px\""), "got: {}", out);
        assert!(out.contains("height=\"360px\""), "got: {}", out);
    }

    /// `.mov` source files are valid input — moss converts them to `.mp4`
    /// during build via `add_video_placeholder_attributes`. The renderer
    /// emits the path verbatim with `.mov`; the rewriter swaps the extension
    /// and stores the original in `data-placeholder-src`. This test covers
    /// only the renderer's contribution: it must emit the original extension
    /// without trying to be clever about MIME types or rewriting.
    #[test]
    fn test_video_renderer_emits_original_extension() {
        for ext in ["mp4", "webm", "mov", "m4v"] {
            let path = format!("clip.{}", ext);
            let out = video_html(&ParsedEmbed {
                resolved_path: &path,
                from_path: "post.md",
                query: None,
                section: None,
                alias: None,
            width: None,
            });
            assert!(
                out.contains(&format!(" src=\"clip.{}\"", ext)),
                "{}: src must reflect original extension, got {}",
                ext,
                out
            );
            // No type= attribute: the browser sniffs MIME from the URL after
            // the rewriter has finalized the path. Carrying a stale type=
            // (e.g. video/quicktime for .mov that has been rewritten to .mp4)
            // would either be ignored or worse, mislead the decoder.
            // Bare ` type="...` only — `data-type="..."` from v1 vocab is fine.
            assert!(
                !out.contains(" type=\""),
                "{}: must not emit bare type= (rewriter changes extension later), got {}",
                ext,
                out
            );
        }
    }

    #[test]
    fn test_video_renderer_preload_metadata() {
        let out = video_html(&ParsedEmbed {
            resolved_path: "clip.mp4",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(out.contains("preload=\"metadata\""), "got: {}", out);
    }

    // --- NotebookRenderer ---

    #[test]
    fn test_notebook_renderer_extensions() {
        assert_eq!(NotebookRenderer.extensions(), &["ipynb"]);
    }

    #[test]
    fn test_notebook_renderer_basic() {
        let embed = ParsedEmbed {
            resolved_path: "resources/habitable-zone.ipynb",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        };
        match NotebookRenderer.render(&embed) {
            RenderedEmbed::Deferred { marker } => assert_eq!(
                marker,
                "<!-- moss-embed-ipynb:resources/habitable-zone.ipynb -->"
            ),
            _ => panic!("expected Deferred"),
        }
    }

    #[test]
    fn test_notebook_renderer_with_query() {
        let embed = ParsedEmbed {
            resolved_path: "nb.ipynb",
            from_path: "post.md",
            query: Some("cells=1-5"),
            section: None,
            alias: None,
            width: None,
        };
        match NotebookRenderer.render(&embed) {
            RenderedEmbed::Deferred { marker } => {
                assert!(marker.contains("nb.ipynb?cells=1-5"), "got: {}", marker)
            }
            _ => panic!("expected Deferred"),
        }
    }

    #[test]
    fn test_notebook_renderer_no_head_assets() {
        // nbconvert embeds its own styles inline; no page-level assets needed.
        assert!(NotebookRenderer.head_assets().is_empty());
    }

    // --- ModelViewerRenderer ---

    fn mv_html(e: &ParsedEmbed) -> String {
        match ModelViewerRenderer.render(e) {
            RenderedEmbed::Html(s) => s,
            _ => panic!("expected Html"),
        }
    }

    #[test]
    fn test_model_viewer_extensions() {
        let exts = ModelViewerRenderer.extensions();
        assert!(exts.iter().any(|&x| x == "glb"));
        assert!(exts.iter().any(|&x| x == "gltf"));
    }

    #[test]
    fn test_model_viewer_basic() {
        let out = mv_html(&ParsedEmbed {
            resolved_path: "models/teapot.glb",
            from_path: "posts/hello.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(out.contains("<model-viewer "), "got: {}", out);
        assert!(
            out.contains("src=\"../models/teapot.glb\""),
            "got: {}",
            out
        );
        assert!(
            out.contains("class=\"moss-embed\" data-type=\"3d\""),
            "got: {}",
            out
        );
        assert!(out.contains("camera-controls"), "got: {}", out);
        assert!(out.contains("auto-rotate"), "got: {}", out);
    }

    #[test]
    fn test_model_viewer_with_sizing() {
        let out = mv_html(&ParsedEmbed {
            resolved_path: "m.glb",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("400x400"),
            width: None,
        });
        assert!(
            out.contains("style=\"width:400px;height:400px\""),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_model_viewer_head_assets() {
        let assets = ModelViewerRenderer.head_assets();
        assert_eq!(assets.len(), 1);
        assert!(
            assets[0].contains("model-viewer"),
            "got: {}",
            assets[0]
        );
        assert!(
            assets[0].contains("<script"),
            "got: {}",
            assets[0]
        );
    }

    // --- TableRenderer ---

    #[test]
    fn test_table_renderer_extensions() {
        let exts = TableRenderer.extensions();
        assert!(exts.iter().any(|&x| x == "csv"));
        assert!(exts.iter().any(|&x| x == "tsv"));
    }

    #[test]
    fn test_table_renderer_emits_deferred() {
        let embed = ParsedEmbed {
            resolved_path: "data/stars.csv",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        };
        match TableRenderer.render(&embed) {
            RenderedEmbed::Deferred { marker } => {
                assert_eq!(marker, "<!-- moss-embed-table:data/stars.csv -->")
            }
            _ => panic!("expected Deferred"),
        }
    }

    // -- spec § P9 width: data-width on embed wrappers --------------------

    /// Helper that builds a width-only ParsedEmbed (the typical
    /// `![[file|full]]` shape after the wikilink resolver's pre-pass).
    fn embed_with_width<'a>(
        resolved_path: &'a str,
        width: &'static str,
    ) -> ParsedEmbed<'a> {
        ParsedEmbed {
            resolved_path,
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: Some(width),
        }
    }

    #[test]
    fn test_iframe_width_emits_data_width_attribute() {
        let out = iframe_html(&embed_with_width("widget.html", "screen"));
        assert!(out.contains(r#"data-width="screen""#), "got: {}", out);
        // class="moss-embed" is the wrapper class — data-width sits next to it.
        assert!(out.contains(r#"class="moss-embed""#), "got: {}", out);
    }

    #[test]
    fn test_iframe_no_width_omits_data_width() {
        let out = iframe_html(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(!out.contains("data-width="), "got: {}", out);
    }

    #[test]
    fn test_pdf_width_emits_data_width_attribute() {
        let out = pdf_html(&embed_with_width("doc.pdf", "wide"));
        assert!(out.contains(r#"data-width="wide""#), "got: {}", out);
        assert!(out.contains(r#"class="moss-embed""#), "got: {}", out);
    }

    #[test]
    fn test_audio_width_emits_data_width_attribute() {
        let out = audio_html(&embed_with_width("song.mp3", "body"));
        assert!(out.contains(r#"data-width="body""#), "got: {}", out);
        assert!(out.contains(r#"class="moss-embed""#), "got: {}", out);
    }

    #[test]
    fn test_video_width_emits_data_width_attribute() {
        // `![[clip.mp4|full]]` — width on the <video> wrapper element.
        let out = video_html(&embed_with_width("clip.mp4", "screen"));
        assert!(out.contains(r#"data-width="screen""#), "got: {}", out);
        assert!(out.contains(r#"class="moss-embed""#), "got: {}", out);
        // single src= shape must survive — this is the rewriter contract.
        assert!(out.contains(r#" src="clip.mp4""#), "got: {}", out);
    }

    #[test]
    fn test_video_no_width_omits_data_width() {
        let out = video_html(&ParsedEmbed {
            resolved_path: "clip.mp4",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
        });
        assert!(!out.contains("data-width="), "got: {}", out);
    }

    #[test]
    fn test_model_viewer_width_emits_data_width_attribute() {
        let out = mv_html(&embed_with_width("model.glb", "page"));
        assert!(out.contains(r#"data-width="page""#), "got: {}", out);
        assert!(out.contains(r#"class="moss-embed""#), "got: {}", out);
    }
}
