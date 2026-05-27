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
use common::{file_stem, path_extension_lower};

// Re-export the canonical 4-char attribute escaper so src-tauri synthesizers
// (pdf / iframe / model / audio / video) can share one definition instead of
// inlining private copies that drifted apart (moss-core's was 4 chars; some
// synthesizers via `moss_core::media::html_escape` was 5 chars including
// `'` → `&#39;`). The 4-char form is correct per HTML5: apostrophe is safe
// inside `"…"` attributes.
pub use common::html_escape_attr;

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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// Trailing Pandoc `{.class key=value}` attribute block, if present.
    ///
    /// Per Decision #8 of the unified-image-emission architecture, Pandoc-
    /// style attribute blocks are the canonical author surface for moss-
    /// vocabulary attributes; the pipe-keyword form remains as compat sugar.
    /// When both are present, the attribute block wins on typed-field
    /// conflicts (Decision #11); class lists union+dedupe.
    pub attrs: Option<crate::ast::attrs::AttrBlock>,
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
use crate::media::{is_all_display_keywords, parse_media_attrs};

use super::fuzzy_path::relative_asset_path;
use super::title_params::{emit_title, TitleParams};

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
        RenderedEmbed::Inline(self.render_to_markdown(embed))
    }
}

impl ImageRenderer {
    /// Stage 1 emission: produce a standard CommonMark image
    /// `![alt](url)` or `![alt](url "moss:params")`. pulldown-cmark parses this
    /// natively as `Tag::Image`; Phase 1's Stage 2 dispatcher reads the
    /// `moss:` title and synthesizes the final `<figure>`/`<img>` HTML.
    ///
    /// The wikilink resolver pre-extracts the spec § P9 width token into
    /// `embed.width`, and feeds the remaining alias text into `embed.alias`.
    /// We translate both into title-attribute params here — no HTML is emitted
    /// by moss-core anymore.
    fn render_to_markdown(&self, embed: &ParsedEmbed<'_>) -> String {
        let url = relative_asset_path(embed.from_path, embed.resolved_path);

        let mut params = TitleParams::default();

        // Width comes from the resolver's pre-extracted width segment
        // (canonical value-space term — see `crate::media::match_width_token`).
        if let Some(w) = embed.width {
            params.insert("width", w);
        }

        // Classify alias. An alias is "display keywords" if every
        // space-separated token is either a recognized display keyword
        // (`is_all_display_keywords`) OR a recognized width token
        // (`match_width_token`). This lets authors write the compound form
        // `align-left wide` in a single pipe segment — both classify as
        // structural, not as a caption.
        // Empty alias (`![[file|]]`) is treated as no alias.
        let (display_kw, caption_text) = match embed.alias {
            Some(alias) if alias.is_empty() => (None, None),
            Some(alias) if is_structural_alias(alias) => (Some(alias), None),
            Some(other) => (None, Some(other.to_string())),
            None => (None, None),
        };

        // Track passthrough classes from BOTH the pipe-alias display-keyword
        // stream AND the trailing Pandoc attribute block. They union+dedupe
        // when both forms are present (Decision #11).
        let mut class_names: Vec<String> = Vec::new();
        let mut extra_attrs: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();

        // Fold display keywords + inline width tokens into typed params.
        if let Some(dk) = display_kw {
            // Pull any inline width tokens out of the alias before parsing
            // display keywords (so parse_media_attrs sees a clean stream of
            // align / fit / position tokens).
            let mut kept = Vec::new();
            for tok in dk.split_whitespace() {
                if let Some(canonical) = crate::media::match_width_token(tok) {
                    // Last-wins if multiple width tokens appear (defensive —
                    // production input has at most one).
                    params.insert("width", canonical);
                } else {
                    kept.push(tok);
                }
            }
            let attrs = parse_media_attrs(&kept.join(" "));
            if let Some(side) = attrs.align {
                params.insert("align", align_keyword(side));
            }
            if let Some(fit) = attrs.fit {
                params.insert("fit", fit.to_css_value());
            }
            if let Some(pos) = attrs.position {
                params.insert("position", pos.to_css_value());
            }
            class_names.extend(attrs.class_names.into_iter());
            for (k, v) in attrs.extra_attrs.into_iter() {
                extra_attrs.insert(k, v);
            }
        }

        // Fold the trailing Pandoc attribute block. Attribute block wins on
        // typed-field conflicts (Decision #11). Class lists union+dedupe
        // (handled by `add_class_dedup`). Pandoc `class="..."` longhand is
        // treated as a space-separated class list (Pandoc spec parity); the
        // same recognized-vocabulary extraction applies as for `.class`.
        if let Some(block) = &embed.attrs {
            // `.class` shorthand → typed fields (align) or passthrough.
            for class in &block.classes {
                if let Some(side) = crate::media::AlignSide::from_keyword(class) {
                    params.insert("align", align_keyword(side));
                } else if let Some(f) = crate::media::Fit::from_keyword(class) {
                    params.insert("fit", f.to_css_value());
                } else if let Some(p) = crate::media::Position::from_keyword(class) {
                    params.insert("position", p.to_css_value());
                } else {
                    add_class_dedup(&mut class_names, class);
                }
            }
            // Width flag from attr block.
            if let Some(w) = block.width {
                params.insert("width", w);
            }
            // Pandoc `class="..."` longhand → split + same dispatch.
            // Also consume it from the kv map so it does not flow into
            // `extra_attrs`.
            let mut consumed_class_kv = false;
            for (k, v) in &block.kvs {
                if k == "class" {
                    consumed_class_kv = true;
                    for c in v.split_whitespace() {
                        if let Some(side) = crate::media::AlignSide::from_keyword(c) {
                            params.insert("align", align_keyword(side));
                        } else if let Some(f) = crate::media::Fit::from_keyword(c) {
                            params.insert("fit", f.to_css_value());
                        } else if let Some(p) = crate::media::Position::from_keyword(c) {
                            params.insert("position", p.to_css_value());
                        } else {
                            add_class_dedup(&mut class_names, c);
                        }
                    }
                }
            }
            // Remaining key=value pairs flow through as extra_attrs (and
            // override pipe-alias extras on conflict — attribute block wins).
            for (k, v) in &block.kvs {
                if consumed_class_kv && k == "class" {
                    continue;
                }
                extra_attrs.insert(k.clone(), v.clone());
            }
        }

        if !class_names.is_empty() {
            params.insert("classes", class_names.join(" "));
        }
        for (k, v) in &extra_attrs {
            params.insert(k.clone(), v.clone());
        }

        let alt = caption_text.unwrap_or_default();

        if params.is_empty() {
            format!("![{}]({})", markdown_escape_alt(&alt), url)
        } else {
            let title = emit_title(&params);
            format!(
                r#"![{}]({} "{}")"#,
                markdown_escape_alt(&alt),
                url,
                markdown_escape_title(&title),
            )
        }
    }
}

/// True when every whitespace-separated token in `alias` is either a
/// recognized display keyword (fit / position / align) OR a canonical
/// width token (body / wide / page / screen / full).
///
/// This is the structural-vs-caption classifier for image aliases: a
/// fully-structural alias contributes only to title params; anything else
/// becomes alt text. The `is_all_display_keywords` half is unchanged
/// (covers two-word position tokens like `top left`); the width-token
/// half lets authors write `align-left wide` without breaking the pipe.
fn is_structural_alias(alias: &str) -> bool {
    // Fast path: any caption-like text fails `is_all_display_keywords`
    // and would also fail the per-token loop below.
    if is_all_display_keywords(alias) {
        return true;
    }
    let tokens: Vec<&str> = alias.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    // Walk tokens; admit width tokens, otherwise defer to display-keyword
    // recognition (per-token, since position tokens may pair across two).
    let mut i = 0;
    while i < tokens.len() {
        // Width token: single-token, simple admit.
        if crate::media::match_width_token(tokens[i]).is_some() {
            i += 1;
            continue;
        }
        // Two-word position (e.g. `top left`).
        if i + 1 < tokens.len() {
            let combined = format!("{} {}", tokens[i], tokens[i + 1]);
            if crate::media::Position::from_keyword(&combined).is_some() {
                i += 2;
                continue;
            }
        }
        // Single-token display keyword.
        if crate::media::Fit::from_keyword(tokens[i]).is_some()
            || crate::media::Position::from_keyword(tokens[i]).is_some()
            || crate::media::AlignSide::from_keyword(tokens[i]).is_some()
        {
            i += 1;
            continue;
        }
        return false;
    }
    true
}

/// Map an `AlignSide` to its canonical title-param keyword (`"left"` or
/// `"right"`). Stage 2 reverses this via `AlignSide::from_keyword`-style
/// recognition (it accepts both `left` and `align-left`).
fn align_keyword(side: crate::media::AlignSide) -> &'static str {
    match side {
        crate::media::AlignSide::Left => "left",
        crate::media::AlignSide::Right => "right",
    }
}

/// Push `class` into `acc` only if not already present. Used when merging
/// pipe-alias passthrough classes with Pandoc attribute-block classes
/// (Decision #11: class lists union and dedupe).
fn add_class_dedup(acc: &mut Vec<String>, class: &str) {
    if !acc.iter().any(|c| c == class) {
        acc.push(class.to_string());
    }
}

/// Shared attribute-block fold for non-image renderers (iframe, pdf, audio,
/// video, 3D). Mirrors the ImageRenderer logic at a smaller scope: extract
/// recognized vocabulary (align) into typed params; pass through everything
/// else as `classes` + extra key=value attrs.
///
/// Lives here so all `render_link_markdown` consumers stay in lockstep when
/// Decision #11 (attribute-block-wins, class lists union+dedupe) evolves.
///
/// `pub(super)` for sibling-module use within `resolve/`. Originally exposed
/// so the Stage 1 native-markdown sweep (`wikilinks::stage1_sweep`) could
/// fold trailing `{...}` attribute blocks into native-image rewrites;
/// that sweep retired in Phase 3 PR2, but the visibility stays the same
/// shape so plugin-side callers can still reach it.
pub(super) fn fold_attrs_into_params(
    block: &crate::ast::attrs::AttrBlock,
    params: &mut TitleParams,
) {
    let mut classes: Vec<String> = Vec::new();
    let mut consumed_class_kv = false;

    for class in &block.classes {
        if let Some(side) = crate::media::AlignSide::from_keyword(class) {
            params.insert("align", align_keyword(side));
        } else {
            add_class_dedup(&mut classes, class);
        }
    }

    if let Some(w) = block.width {
        // Non-image renderers expose width on the wrapper as `data-width`
        // (see `render_link_markdown`), matching the pipe-alias path.
        params.insert("data-width", w);
    }

    for (k, v) in &block.kvs {
        if k == "class" {
            consumed_class_kv = true;
            for c in v.split_whitespace() {
                if let Some(side) = crate::media::AlignSide::from_keyword(c) {
                    params.insert("align", align_keyword(side));
                } else {
                    add_class_dedup(&mut classes, c);
                }
            }
        }
    }
    if !classes.is_empty() {
        params.insert("classes", classes.join(" "));
    }
    for (k, v) in &block.kvs {
        if consumed_class_kv && k == "class" {
            continue;
        }
        params.insert(k.clone(), v.clone());
    }
}

/// Escape alt text for markdown `![...](url)` syntax.
///
/// Brackets MUST be escaped; HTML entities are NOT needed (pulldown-cmark
/// handles `<` `>` `&` per CommonMark rules when alt text is rendered).
fn markdown_escape_alt(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Escape title text for markdown `(url "title")` syntax.
///
/// CommonMark allows three quoting styles; we always emit `"..."` so we
/// only need to escape literal `"` and `\` inside the title body.
fn markdown_escape_title(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Shared Stage 1 emitter for the five non-image renderers (iframe, pdf,
/// audio, video, 3D). Produces a CommonMark link:
///
/// ```text
/// [filename](url "moss:kind=<kind> <params>")
/// ```
///
/// pulldown-cmark parses this as `Tag::Link`; Phase 1's Stage 2 dispatcher
/// keys off `moss:kind=` to choose the right HTML synthesizer.
///
/// `kind` is the canonical kind name (`iframe`, `pdf`, `audio`, `video`,
/// `3d`). `extra` is invoked to inject renderer-specific params; the helper
/// pre-fills `kind=` so callers only handle their own grammar.
fn render_link_markdown(
    embed: &ParsedEmbed<'_>,
    kind: &'static str,
    extra: impl FnOnce(&ParsedEmbed<'_>, &mut TitleParams),
) -> String {
    let url = relative_asset_path(embed.from_path, embed.resolved_path);
    let mut params = TitleParams::default();
    params.insert("kind", kind);
    if let Some(w) = embed.width {
        params.insert("data-width", w);
    }
    extra(embed, &mut params);
    // Fold the trailing Pandoc attribute block last so it wins on typed-field
    // conflicts (Decision #11) — matches the ImageRenderer ordering.
    if let Some(block) = &embed.attrs {
        fold_attrs_into_params(block, &mut params);
    }
    let title = emit_title(&params);
    let name = file_stem(embed.resolved_path);
    format!(
        r#"[{}]({} "{}")"#,
        markdown_escape_alt(&name),
        url,
        markdown_escape_title(&title),
    )
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
        RenderedEmbed::Inline(render_link_markdown(embed, "iframe", iframe_extra_params))
    }
}

/// Iframe-specific param extraction:
///
/// - `?query` and `#fragment` from the wikilink fold into a `src` param so
///   Stage 2 can reconstruct the URL it serves on the iframe element. They
///   are NOT re-inserted into the markdown URL slot because pulldown-cmark
///   would percent-encode them, which would break iframe `src` semantics
///   downstream.
/// - `|WxH` sizing in alias becomes `width=`/`height=` params.
/// - Non-sizing alias text becomes the `title=` param (used today as
///   accessible name / tooltip on the iframe).
fn iframe_extra_params(embed: &ParsedEmbed<'_>, params: &mut TitleParams) {
    if let Some(q) = embed.query {
        params.insert("query", q);
    }
    if let Some(f) = embed.section {
        params.insert("fragment", f);
    }
    let Some(alias) = embed.alias else {
        return;
    };
    match Sizing::parse(alias) {
        Some(Sizing::Width(w)) => {
            params.insert("width", w.to_css());
        }
        Some(Sizing::Box(w, h)) => {
            params.insert("width", w.to_css());
            params.insert("height", h.to_css());
        }
        None => {
            // Non-sizing text alias → iframe title.
            params.insert("title", alias);
        }
    }
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
        RenderedEmbed::Inline(render_link_markdown(embed, "pdf", pdf_extra_params))
    }
}

/// PDF-specific params: viewer fragment (`#page=5`), sizing.
fn pdf_extra_params(embed: &ParsedEmbed<'_>, params: &mut TitleParams) {
    if let Some(q) = embed.query {
        params.insert("query", q);
    }
    if let Some(f) = embed.section {
        params.insert("fragment", f);
    }
    if let Some(alias) = embed.alias {
        match Sizing::parse(alias) {
            Some(Sizing::Width(w)) => {
                params.insert("width", w.to_css());
            }
            Some(Sizing::Box(w, h)) => {
                params.insert("width", w.to_css());
                params.insert("height", h.to_css());
            }
            None => {}
        }
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
        RenderedEmbed::Inline(render_link_markdown(embed, "audio", audio_extra_params))
    }
}

/// Audio-specific params: source extension (for downstream MIME selection).
/// The historical author grammar exposed no per-embed audio flags, so today
/// the only extra param is the file extension. Future flags (`controls`,
/// `loop`, `autoplay`, `muted`) extend here.
fn audio_extra_params(embed: &ParsedEmbed<'_>, params: &mut TitleParams) {
    let ext = path_extension_lower(embed.resolved_path);
    if !ext.is_empty() {
        params.insert("ext", ext);
    }
}

// MIME-type selection for audio embeds now lives downstream (Phase 1
// Stage 2 picks the MIME from the `ext=` title param when synthesizing the
// `<audio><source>` HTML). moss-core's Stage 1 emits the file extension
// directly via `audio_extra_params`; the legacy `audio_mime_for_ext` helper
// is no longer needed here.

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
        RenderedEmbed::Inline(render_link_markdown(embed, "video", video_extra_params))
    }
}

/// Video-specific params: sizing from alias. Author flags (controls, loop,
/// autoplay, muted, poster) extend here when wired up.
fn video_extra_params(embed: &ParsedEmbed<'_>, params: &mut TitleParams) {
    if let Some(alias) = embed.alias {
        match Sizing::parse(alias) {
            Some(Sizing::Width(w)) => {
                params.insert("width", w.to_css());
            }
            Some(Sizing::Box(w, h)) => {
                params.insert("width", w.to_css());
                params.insert("height", h.to_css());
            }
            None => {}
        }
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
        RenderedEmbed::Inline(render_link_markdown(embed, "3d", model_viewer_extra_params))
    }

    fn head_assets(&self) -> &[&'static str] {
        &[MODEL_VIEWER_SCRIPT]
    }
}

/// 3D-viewer-specific params: sizing from alias. Author flags (`auto-rotate`,
/// `camera-controls`, `ar`) extend here when surfaced in the wikilink grammar.
fn model_viewer_extra_params(embed: &ParsedEmbed<'_>, params: &mut TitleParams) {
    if let Some(alias) = embed.alias {
        match Sizing::parse(alias) {
            Some(Sizing::Width(w)) => {
                params.insert("width", w.to_css());
            }
            Some(Sizing::Box(w, h)) => {
                params.insert("width", w.to_css());
                params.insert("height", h.to_css());
            }
            None => {}
        }
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
            attrs: None,
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
            attrs: None,
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
            attrs: None,
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
            attrs: None,
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
    fn test_image_renderer_no_alias_emits_bare_markdown() {
        // Stage 1: no params, no caption → bare CommonMark image with empty alt.
        // pulldown-cmark's default Tag::Image handler converts this to <img>
        // with no figure wrapper at all; the bare-image-paragraph rule (in
        // src-tauri post-pass) decides figure-wrap separately.
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "assets/photo.jpg",
            from_path: "hello.md", // sibling — no `../` prefix in output
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![](assets/photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_empty_alias_treated_as_no_alias() {
        // `![[file|]]` (literal empty pipe) goes through `Some("")`. Stage 1
        // treats an empty alias as "no alias": no params, empty alt.
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some(""),
            width: None,
            attrs: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![](photo.jpg)".to_string())
        );
    }

    #[test]
    fn test_image_renderer_alias_plain_text() {
        // Stage 1: plain caption text becomes the alt slot (no params).
        let r = ImageRenderer;
        let embed = ParsedEmbed {
            resolved_path: "photo.jpg",
            from_path: "hello.md",
            query: None,
            section: None,
            alias: Some("A lovely cat"),
            width: None,
            attrs: None,
        };
        assert_eq!(
            r.render(&embed),
            RenderedEmbed::Inline("![A lovely cat](photo.jpg)".to_string())
        );
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

    // -- ImageRenderer Stage 1: markdown emission with `moss:` title -----

    /// Pre-extract width from alias (mirrors the wikilink resolver's pre-pass)
    /// then run the renderer. Returns the Inline string — Stage 1 markdown.
    fn image_inline(alias: Option<&str>) -> String {
        let r = ImageRenderer;
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
            from_path: "hello.md", // sibling — keeps URL == resolved_path
            query: None,
            section: None,
            alias: alias_for_renderer,
            width,
            attrs: None,
        };
        match r.render(&embed) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("image renderer should return Inline"),
        }
    }

    #[test]
    fn stage1_image_no_params_is_bare_markdown() {
        let out = image_inline(None);
        assert_eq!(out, "![](assets/photo.jpg)");
    }

    #[test]
    fn stage1_image_with_align_emits_moss_title() {
        let out = image_inline(Some("align-left"));
        assert_eq!(out, r#"![](assets/photo.jpg "moss:align=left")"#);
    }

    #[test]
    fn stage1_image_with_width_emits_moss_title() {
        // `wide` is a canonical width token; no other params.
        let out = image_inline(Some("wide"));
        assert_eq!(out, r#"![](assets/photo.jpg "moss:width=wide")"#);
    }

    #[test]
    fn stage1_image_full_aliases_to_screen() {
        // `full` is the author-facing alias; canonical value is `screen`.
        let out = image_inline(Some("full"));
        assert_eq!(out, r#"![](assets/photo.jpg "moss:width=screen")"#);
    }

    #[test]
    fn stage1_image_width_body_page_screen_round_trip() {
        for (token, canonical) in [("body", "body"), ("page", "page"), ("screen", "screen")] {
            let out = image_inline(Some(token));
            assert_eq!(
                out,
                format!(r#"![](assets/photo.jpg "moss:width={}")"#, canonical),
                "token={}",
                token
            );
        }
    }

    #[test]
    fn stage1_image_width_caption_moss_title_with_caption_as_alt() {
        // Caption text + width: caption becomes alt, width goes into the
        // title. Multi-pipe form `caption|full` exercises extract_width.
        let out = image_inline(Some("A nice photo|full"));
        assert_eq!(out, r#"![A nice photo](assets/photo.jpg "moss:width=screen")"#);
    }

    #[test]
    fn stage1_image_align_and_width_compose() {
        // BTreeMap canonicalises params alphabetically: align < width.
        let out = image_inline(Some("align-left wide"));
        assert!(
            out.contains(r#""moss:align=left width=wide""#),
            "got: {}",
            out
        );
    }

    #[test]
    fn stage1_image_align_with_width_pipe_composes() {
        // Pipe-separated form: `align-right|wide` — align segment first,
        // width pipe second. Result is the same composition.
        let out = image_inline(Some("align-right|wide"));
        assert!(
            out.contains(r#""moss:align=right width=wide""#),
            "got: {}",
            out
        );
    }

    #[test]
    fn stage1_image_display_keywords_with_width_compose() {
        // `cover` (object-fit) composes with `full` (width).
        let out = image_inline(Some("cover|full"));
        assert!(
            out.contains(r#""moss:fit=cover width=screen""#),
            "got: {}",
            out
        );
    }

    #[test]
    fn stage1_image_align_cover_width_compose() {
        // Three-way composition: align (class), cover (fit), full (width).
        let out = image_inline(Some("align-left cover|full"));
        assert!(
            out.contains(r#""moss:align=left fit=cover width=screen""#),
            "got: {}",
            out
        );
    }

    #[test]
    fn stage1_image_caption_with_width_word_not_shadowed() {
        // The word `wide` inside a longer caption must NOT trigger width
        // classification — only an exact-match alias segment counts.
        // Caption survives in the alt slot.
        let out = image_inline(Some("a wide angle shot"));
        assert!(!out.contains("moss:"), "no params expected, got: {}", out);
        assert_eq!(out, "![a wide angle shot](assets/photo.jpg)");
    }

    #[test]
    fn stage1_image_html_escapes_caption_for_alt() {
        // Caption goes into alt. CommonMark alt allows " and & inline;
        // only [ and ] need escaping. Title escaping handles the param side.
        let out = image_inline(Some(r#"Q&A "best"|wide"#));
        assert!(
            out.starts_with(r#"![Q&A "best"]("#),
            "alt should preserve & and \" verbatim: {}",
            out
        );
        assert!(
            out.ends_with(r#""moss:width=wide")"#),
            "title param expected: {}",
            out
        );
    }

    #[test]
    fn stage1_image_alt_brackets_escaped() {
        // Brackets in caption text must be backslash-escaped to keep the
        // CommonMark parser from terminating the alt span early.
        let out = image_inline(Some("a [bracketed] caption"));
        assert!(
            out.contains(r"![a \[bracketed\] caption]("),
            "got: {}",
            out
        );
    }

    // -- markdown escape helpers (covered above; spec from plan §D1) -----

    #[test]
    fn markdown_escape_alt_brackets() {
        assert_eq!(markdown_escape_alt("plain"), "plain");
        assert_eq!(markdown_escape_alt("has [brackets]"), r"has \[brackets\]");
        assert_eq!(markdown_escape_alt(r"with \ backslash"), r"with \\ backslash");
    }

    #[test]
    fn markdown_escape_title_quotes() {
        assert_eq!(markdown_escape_title("plain"), "plain");
        assert_eq!(markdown_escape_title(r#"has "quotes""#), r#"has \"quotes\""#);
        assert_eq!(markdown_escape_title(r"\backslash"), r"\\backslash");
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

    /// Helper: render iframe embed to Stage 1 markdown (Inline string).
    fn iframe_md(e: &ParsedEmbed) -> String {
        match IframeRenderer.render(e) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("expected Inline (Stage 1 markdown)"),
        }
    }

    #[test]
    fn stage1_iframe_basic_is_link_with_kind() {
        // Stage 1 emits `[filename](url "moss:kind=iframe")`.
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert_eq!(out, r#"[widget](widget.html "moss:kind=iframe")"#);
    }

    #[test]
    fn stage1_iframe_with_query_param() {
        // `?query` is preserved as a title-attribute param (not on the URL
        // slot) so pulldown-cmark doesn't percent-encode reserved chars.
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "scale.html",
            from_path: "post.md",
            query: Some("a=major,minor&r=D"),
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert!(out.contains(r#"kind=iframe"#), "got: {}", out);
        assert!(out.contains(r#"query="a=major,minor"#) || out.contains("query=a=major,minor"), "got: {}", out);
    }

    #[test]
    fn stage1_iframe_with_sizing_alias_emits_width_height_params() {
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("100%x600"),
            width: None,
            attrs: None,
        });
        assert!(out.contains("height=600px"), "got: {}", out);
        // 100% contains no whitespace, emitted unquoted.
        assert!(out.contains("width=100%"), "got: {}", out);
        assert!(out.contains("kind=iframe"), "got: {}", out);
        // No `title=` for sizing alias.
        assert!(!out.contains("title="), "got: {}", out);
    }

    #[test]
    fn stage1_iframe_text_alias_becomes_title_param() {
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("My cool widget"),
            width: None,
            attrs: None,
        });
        // Whitespace forces quoting; emit_title wraps in `"..."`. Inside the
        // markdown title we then escape the inner quotes.
        assert!(out.contains(r#"title=\"My cool widget\""#), "got: {}", out);
        // Filename slot is the file stem (no extension).
        assert!(out.starts_with("[widget]("), "got: {}", out);
    }

    #[test]
    fn stage1_iframe_with_fragment_emits_fragment_param() {
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "doc.html",
            from_path: "post.md",
            query: Some("x=1"),
            section: Some("section2"),
            alias: None,
            width: None,
            attrs: None,
        });
        assert!(out.contains("fragment=section2"), "got: {}", out);
        assert!(out.contains("query=x=1"), "got: {}", out);
    }

    #[test]
    fn stage1_iframe_with_canonical_width_emits_data_width_param() {
        // `embed.width` is a canonical spec § P9 value, preserved as
        // `data-width=` so Stage 2 can route it to the wrapper attribute.
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: Some("wide"),
            attrs: None,
        });
        assert!(out.contains("data-width=wide"), "got: {}", out);
    }

    // --- Sizing malformed-input coverage ---

    #[test]
    fn test_sizing_parse_malformed_box_is_none() {
        assert_eq!(Sizing::parse("100xbad"), None);
        assert_eq!(Sizing::parse("100x"), None);
        assert_eq!(Sizing::parse("-100"), None);
    }

    #[test]
    fn stage1_iframe_malformed_sizing_falls_through_to_title() {
        // Malformed sizing isn't recognised by Sizing::parse; the alias
        // becomes a title param instead (matches the legacy iframe behaviour).
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("100xbad"),
            width: None,
            attrs: None,
        });
        assert!(out.contains("title=100xbad"), "got: {}", out);
        assert!(!out.contains("height="), "got: {}", out);
    }

    // --- PdfRenderer ---

    fn pdf_md(e: &ParsedEmbed) -> String {
        match PdfRenderer.render(e) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("expected Inline (Stage 1 markdown)"),
        }
    }

    #[test]
    fn test_pdf_renderer_extensions() {
        assert_eq!(PdfRenderer.extensions(), &["pdf"]);
    }

    #[test]
    fn stage1_pdf_basic_is_link_with_kind() {
        let out = pdf_md(&ParsedEmbed {
            resolved_path: "report.pdf",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert_eq!(out, r#"[report](report.pdf "moss:kind=pdf")"#);
    }

    #[test]
    fn stage1_pdf_with_page_fragment_emits_fragment_param() {
        // PDF viewer fragments like `#page=5` round-trip through `fragment=`.
        let out = pdf_md(&ParsedEmbed {
            resolved_path: "doc.pdf",
            from_path: "post.md",
            query: None,
            section: Some("page=5"),
            alias: None,
            width: None,
            attrs: None,
        });
        assert!(out.contains("fragment=page=5"), "got: {}", out);
        assert!(out.contains("kind=pdf"), "got: {}", out);
    }

    #[test]
    fn stage1_pdf_with_sizing_emits_width_height_params() {
        let out = pdf_md(&ParsedEmbed {
            resolved_path: "doc.pdf",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("100%x800"),
            width: None,
            attrs: None,
        });
        assert!(out.contains("width=100%"), "got: {}", out);
        assert!(out.contains("height=800px"), "got: {}", out);
    }

    // --- AudioRenderer ---

    fn audio_md(e: &ParsedEmbed) -> String {
        match AudioRenderer.render(e) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("expected Inline (Stage 1 markdown)"),
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
    fn stage1_audio_basic_is_link_with_kind_and_ext() {
        let out = audio_md(&ParsedEmbed {
            resolved_path: "song.mp3",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        // BTreeMap alphabetises params: ext < kind.
        assert_eq!(out, r#"[song](song.mp3 "moss:ext=mp3 kind=audio")"#);
    }

    #[test]
    fn stage1_audio_emits_ext_param_for_each_extension() {
        // Stage 2 reads `ext=` to derive the `<source type=>` MIME type;
        // the renderer's job is just to preserve the extension.
        for ext in ["mp3", "wav", "ogg", "flac", "m4a", "opus"] {
            let path = format!("a.{}", ext);
            let out = audio_md(&ParsedEmbed {
                resolved_path: &path,
                from_path: "post.md",
                query: None,
                section: None,
                alias: None,
                width: None,
                attrs: None,
            });
            assert!(
                out.contains(&format!("ext={}", ext)),
                "ext={}: got {}",
                ext,
                out
            );
            assert!(out.contains("kind=audio"), "got: {}", out);
        }
    }

    // --- VideoRenderer ---

    fn video_md(e: &ParsedEmbed) -> String {
        match VideoRenderer.render(e) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("expected Inline (Stage 1 markdown)"),
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
    fn stage1_video_basic_is_link_with_kind() {
        let out = video_md(&ParsedEmbed {
            resolved_path: "trailer.mp4",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert_eq!(out, r#"[trailer](trailer.mp4 "moss:kind=video")"#);
    }

    #[test]
    fn stage1_video_emits_original_extension_in_url() {
        // The URL slot carries the original extension (e.g. `.mov`) so the
        // downstream `add_video_placeholder_attributes` rewriter can perform
        // `.mov→.mp4` swap on the served URL. The renderer never modifies it.
        for ext in ["mp4", "webm", "mov", "m4v"] {
            let path = format!("clip.{}", ext);
            let out = video_md(&ParsedEmbed {
                resolved_path: &path,
                from_path: "post.md",
                query: None,
                section: None,
                alias: None,
                width: None,
                attrs: None,
            });
            assert!(
                out.contains(&format!("({}.", "clip"))
                    && out.contains(&format!("({}.{}", "clip", ext)),
                "{}: url slot must reflect original extension, got {}",
                ext,
                out
            );
        }
    }

    #[test]
    fn stage1_video_with_sizing_emits_width_height_params() {
        let out = video_md(&ParsedEmbed {
            resolved_path: "clip.mp4",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("640x360"),
            width: None,
            attrs: None,
        });
        assert!(out.contains("width=640px"), "got: {}", out);
        assert!(out.contains("height=360px"), "got: {}", out);
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
            attrs: None,
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
            attrs: None,
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

    fn mv_md(e: &ParsedEmbed) -> String {
        match ModelViewerRenderer.render(e) {
            RenderedEmbed::Inline(s) => s,
            _ => panic!("expected Inline (Stage 1 markdown)"),
        }
    }

    #[test]
    fn test_model_viewer_extensions() {
        let exts = ModelViewerRenderer.extensions();
        assert!(exts.iter().any(|&x| x == "glb"));
        assert!(exts.iter().any(|&x| x == "gltf"));
    }

    #[test]
    fn stage1_model_viewer_basic_is_link_with_3d_kind() {
        let out = mv_md(&ParsedEmbed {
            resolved_path: "teapot.glb",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert_eq!(out, r#"[teapot](teapot.glb "moss:kind=3d")"#);
    }

    #[test]
    fn stage1_model_viewer_with_sizing_emits_width_height_params() {
        let out = mv_md(&ParsedEmbed {
            resolved_path: "m.glb",
            from_path: "post.md",
            query: None,
            section: None,
            alias: Some("400x400"),
            width: None,
            attrs: None,
        });
        assert!(out.contains("width=400px"), "got: {}", out);
        assert!(out.contains("height=400px"), "got: {}", out);
        assert!(out.contains("kind=3d"), "got: {}", out);
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
            attrs: None,
        };
        match TableRenderer.render(&embed) {
            RenderedEmbed::Deferred { marker } => {
                assert_eq!(marker, "<!-- moss-embed-table:data/stars.csv -->")
            }
            _ => panic!("expected Deferred"),
        }
    }

    // -- spec § P9 width: data-width title-param round-trip ---------------

    /// Build a width-only `ParsedEmbed` mirroring the wikilink resolver's
    /// pre-pass output for `![[file|full]]`-style aliases.
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
            attrs: None,
        }
    }

    #[test]
    fn stage1_iframe_width_emits_data_width_title_param() {
        let out = iframe_md(&embed_with_width("widget.html", "screen"));
        assert!(out.contains("data-width=screen"), "got: {}", out);
        assert!(out.contains("kind=iframe"), "got: {}", out);
    }

    #[test]
    fn stage1_iframe_no_width_omits_data_width_param() {
        let out = iframe_md(&ParsedEmbed {
            resolved_path: "widget.html",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert!(!out.contains("data-width="), "got: {}", out);
    }

    #[test]
    fn stage1_pdf_width_emits_data_width_title_param() {
        let out = pdf_md(&embed_with_width("doc.pdf", "wide"));
        assert!(out.contains("data-width=wide"), "got: {}", out);
        assert!(out.contains("kind=pdf"), "got: {}", out);
    }

    #[test]
    fn stage1_audio_width_emits_data_width_title_param() {
        let out = audio_md(&embed_with_width("song.mp3", "body"));
        assert!(out.contains("data-width=body"), "got: {}", out);
        assert!(out.contains("kind=audio"), "got: {}", out);
    }

    #[test]
    fn stage1_video_width_emits_data_width_title_param() {
        let out = video_md(&embed_with_width("clip.mp4", "screen"));
        assert!(out.contains("data-width=screen"), "got: {}", out);
        assert!(out.contains("kind=video"), "got: {}", out);
        // URL slot still carries the original extension — the rewriter
        // contract is preserved at the markdown level.
        assert!(out.contains("](clip.mp4"), "got: {}", out);
    }

    #[test]
    fn stage1_video_no_width_omits_data_width_param() {
        let out = video_md(&ParsedEmbed {
            resolved_path: "clip.mp4",
            from_path: "post.md",
            query: None,
            section: None,
            alias: None,
            width: None,
            attrs: None,
        });
        assert!(!out.contains("data-width="), "got: {}", out);
    }

    #[test]
    fn stage1_model_viewer_width_emits_data_width_title_param() {
        let out = mv_md(&embed_with_width("model.glb", "page"));
        assert!(out.contains("data-width=page"), "got: {}", out);
        assert!(out.contains("kind=3d"), "got: {}", out);
    }
}
