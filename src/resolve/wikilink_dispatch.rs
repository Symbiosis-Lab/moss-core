//! Phase 3: Stage 2 entry point for wikilink embed dispatch.
//!
//! This module is the sole dispatcher for `[[…]]` / `![[…]]` events
//! emitted by pulldown-cmark with `Options::ENABLE_WIKILINKS`. The
//! src-tauri pipeline's `transform_events` (in
//! `src-tauri/src/build/markdown/pipeline.rs`) calls
//! [`dispatch_wikilink_embed_with_registry`] once per WikiLink-typed
//! event, swallows the event range, and substitutes the renderer-
//! produced HTML.
//!
//! # History
//!
//! - **PR1 (`c2fbdd593`)**: this module landed as a dormant API alongside
//!   the dispatch arm shape in `transform_events` (also dormant — gated
//!   by the absence of `ENABLE_WIKILINKS`).
//! - **PR2 (this change)**: enabled `ENABLE_WIKILINKS` at every
//!   `Parser::new_ext` site, wired the dispatcher closure into
//!   `transform_events`, and deleted the prior Stage 1 string-rewriter
//!   (`crates/moss-core/src/resolve/wikilinks.rs`, ~2155 LOC).
//!
//! # What this reuses
//!
//! - Extension routing goes through [`super::embed_renderer::lookup_renderer`]
//!   (the same registry the pre-PR2 Stage 1 resolver used). No parallel
//!   dispatcher.
//! - Anchor / query splitting on `dest_url` mirrors the pre-PR2
//!   `wikilinks::parse_wikilink_inner`'s `#` / `?` priority logic.
//! - Width-token extraction uses [`crate::media::extract_width_from_alias`].
//!
//! # What's new
//!
//! - [`parse_pothole_params`] reads the pothole text (the `bar` in
//!   `[[foo|bar]]`) and classifies it as one of:
//!   * empty — no pothole
//!   * width-token — Obsidian `[[img.jpg|400]]` shorthand
//!   * params — `width=400 align=left` (every-token-K=V rule)
//!   * alias — plain display text
//!
//!   The every-token-K=V rule (locked by arch review) prevents free-text
//!   captions like `alt text=cover` from being mis-parsed as `text=cover`.

use crate::asset_snapshot::AssetSnapshot;
use crate::content_graph::ContentGraph;
use crate::path_ext::path_extension;
use crate::media::{
    extract_width_from_alias, parse_media_attrs, AlignSide, Fit, MediaAttrs, Position,
};

use super::embed_renderer::{
    lookup_renderer, EmbedRenderer, ParsedEmbed, RenderedEmbed, Sizing, IMAGE_EXTENSIONS,
};
use super::fuzzy_path::{relative_asset_path, resolve_reference, ResolvedRef};
use super::title_params::TitleParams;
use super::{Diagnostic, LinkType, OutgoingLink};

/// Classification of pothole text (`|...` in `[[file|...]]`).
#[derive(Debug, Clone, PartialEq)]
pub enum PotholeContent {
    /// No pothole or pothole is whitespace-only.
    Empty,
    /// Obsidian width-token shorthand: `[[img.jpg|400]]`, `[[img.jpg|100%]]`,
    /// `[[img.jpg|200x150]]`. Carries the canonical width string
    /// (one of `body | wide | page | screen` after token-matching) and the
    /// trailing alias remainder (often empty).
    WidthToken {
        width: &'static str,
        rest_alias: String,
    },
    /// Typed params: every whitespace-separated token matched
    /// `^[a-z][a-z0-9_-]*=...`.
    Params(TitleParams),
    /// Plain alias display text (Obsidian default).
    Alias(String),
}

/// Result of splitting a wikilink `dest_url` into its `file`, `section`,
/// `query` components.
///
/// Pulldown-cmark hands us `dest_url` verbatim — `[[foo#bar?baz]]` arrives as
/// `dest_url="foo#bar?baz"`. We still need to split for renderer dispatch
/// (image / markdown / iframe / etc.) and for emitted-href construction.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitDestUrl<'a> {
    pub file: &'a str,
    pub section: Option<&'a str>,
    pub query: Option<&'a str>,
}

/// Output of [`dispatch_wikilink_embed`].
#[derive(Debug, Clone)]
pub struct WikilinkEmit {
    /// Rendered HTML or markdown to splice into the event stream.
    /// For an embed (`![[…]]`) this is the renderer's output. For a plain
    /// wikilink (`[[…]]`) this is a markdown link the caller can let
    /// pulldown-cmark re-parse, or a final HTML fragment.
    pub output: EmitKind,
    /// Outgoing link to register with ContentGraph.
    pub outgoing_link: Option<OutgoingLink>,
    /// Diagnostics (e.g. unresolved reference).
    pub diagnostics: Vec<Diagnostic>,
}

/// The shape of the dispatcher's emitted content. Mirrors
/// [`super::embed_renderer::RenderedEmbed`] for embeds, plus a separate
/// variant for non-embed wikilinks (`[[file]]`).
#[derive(Debug, Clone, PartialEq)]
pub enum EmitKind {
    /// Markdown-level text that downstream CommonMark will re-process.
    /// Example: image renderer returns `![alt](url)`.
    Inline(String),
    /// Final HTML — must NOT be re-parsed by the markdown engine.
    /// Example: iframe renderer.
    Html(String),
    /// A marker comment for a post-pass resolver (notebook, table, plugin).
    Deferred(String),
    /// A standard markdown link string. Used for non-embed wikilinks
    /// (`[[file]]` rather than `![[file]]`).
    Link(String),
    /// A typed AST block to splice in directly (image-embed synth-collapse).
    /// Unlike [`EmitKind::Html`] (which lands as an opaque `Block::Other`
    /// carrying no `BlockMeta`), a typed block placed 1:1 at `blocks[i]`
    /// inherits the source paragraph's `block_meta[i]` — so a lone image
    /// embed rendered as `Block::Figure` keeps its `data-source-line` in
    /// preview/site builds. The image arm uses this; non-image embeds keep
    /// emitting `EmitKind::Html`.
    Block(Box<crate::ast::node::Block>),
}

/// Parse pothole text using the every-token-K=V rule.
///
/// Order of attempts:
/// 1. Empty → [`PotholeContent::Empty`].
/// 2. Obsidian width-token (`400`, `100%`, `200x150`, `full`, etc.) via
///    [`extract_width_from_alias`] → [`PotholeContent::WidthToken`].
/// 3. Every whitespace-separated token matches `^[a-z][a-z0-9_-]*=...`
///    → [`PotholeContent::Params`].
/// 4. Otherwise → [`PotholeContent::Alias`].
///
/// The every-token rule is critical: `[[file|alt text=cover]]` must be
/// recognized as alias text (because `alt` is bare), not as a `text=cover`
/// param. See plan v2 revision notes.
pub fn parse_pothole_params(text: &str) -> PotholeContent {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return PotholeContent::Empty;
    }

    // Step 2: try Obsidian width-token shorthand. This recognizes single
    // tokens like `400`, `100%`, `200x150`, `wide`, `full`, etc. The width
    // matcher only fires on isolated tokens; multi-word free text like
    // "wide angle photo" is not classified as a width token.
    let (width, rest_alias) = extract_width_from_alias(trimmed);
    if let Some(w) = width {
        return PotholeContent::WidthToken {
            width: w,
            rest_alias,
        };
    }

    // Step 3: every-token-K=V rule. Each whitespace-separated token must
    // match `^[a-z][a-z0-9_-]*=`. If ANY token fails the pattern, the
    // entire pothole falls through to alias.
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if !tokens.is_empty() && tokens.iter().all(|t| is_kv_token(t)) {
        let mut params = TitleParams::default();
        for token in &tokens {
            if let Some((k, v)) = token.split_once('=') {
                params.insert(k, v);
            }
        }
        return PotholeContent::Params(params);
    }

    // Step 4: fallback. Preserve original text exactly (caller may want
    // verbatim alias display).
    PotholeContent::Alias(text.to_string())
}

/// Test if a single token matches the K=V pattern: `^[a-z][a-z0-9_-]*=...`.
///
/// The key must start with a lowercase ASCII letter and continue with
/// lowercase ASCII letters / digits / underscore / hyphen, followed by
/// `=`. The value side is not constrained here.
fn is_kv_token(token: &str) -> bool {
    let Some((key, _value)) = token.split_once('=') else {
        return false;
    };
    if key.is_empty() {
        return false;
    }
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false; // empty key (guarded above, but keep the type-safe form)
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Split a pulldown-cmark wikilink `dest_url` into `file`, `section`, `query`.
///
/// Ported from the pre-Phase-3 `wikilinks::parse_wikilink_inner` (the
/// `before-pipe` half — the `|alias` part is handled by pulldown-cmark
/// via pothole events, so it doesn't appear in `dest_url`).
///
/// Whichever of `#` or `?` appears first in `dest_url` owns its tail; the
/// other is split out of that tail. Matches Obsidian's heading-ref priority
/// (`[[file#section]]`) while accepting URL-style mixes
/// (`[[file.html?x=1#frag]]`).
pub fn split_dest_url(dest_url: &str) -> SplitDestUrl<'_> {
    let hash_pos = dest_url.find('#');
    let query_pos = dest_url.find('?');

    // char-aligned: `h`/`q` are byte indices of ASCII `#`/`?`, each a
    // single-byte UTF-8 char. `h+1`/`q+1` step over the single byte.
    #[allow(clippy::string_slice)]
    match (hash_pos, query_pos) {
        (None, None) => SplitDestUrl {
            file: dest_url,
            section: None,
            query: None,
        },
        (Some(h), None) => SplitDestUrl {
            file: &dest_url[..h],
            section: Some(&dest_url[h + 1..]),
            query: None,
        },
        (None, Some(q)) => SplitDestUrl {
            file: &dest_url[..q],
            section: None,
            query: Some(&dest_url[q + 1..]),
        },
        (Some(h), Some(q)) if h < q => SplitDestUrl {
            file: &dest_url[..h],
            section: Some(&dest_url[h + 1..q]),
            query: Some(&dest_url[q + 1..]),
        },
        (Some(h), Some(q)) => SplitDestUrl {
            file: &dest_url[..q],
            section: Some(&dest_url[h + 1..]),
            query: Some(&dest_url[q + 1..h]),
        },
    }
}

/// Build the anchor fragment (e.g. `#getting-started` or `#block-id`) from a
/// section reference. Mirrors [`super::wikilinks`]'s `build_anchor`.
///
/// # Live-build scope (read before relying on this)
///
/// `build_anchor` is reached ONLY from [`dispatch_wikilink_form`] (the
/// `is_embed: false` branch). That branch is DORMANT in production: the sole
/// runtime caller of this dispatcher — the AST visitor
/// [`crate::ast::dispatch_wikilink_embeds`] — hard-codes `is_embed: true`
/// (it walks only `![[…]]` image embeds). The user-facing
/// `[[Page#Heading]]` text-link fragment slugging is performed instead by
/// `crate::ast::resolve_urls::slug_wikilink_suffix`, which this function
/// mirrors. Keep the two in sync; the resolve_urls tests are the real guards.
fn build_anchor(section: Option<&str>) -> String {
    use crate::heading::anchor::obsidian_heading_anchor;
    match section {
        None => String::new(),
        Some("") => String::new(),
        Some(s) => {
            if let Some(block_id) = s.strip_prefix('^') {
                format!("#{}", block_id)
            } else {
                format!("#{}", obsidian_heading_anchor(s))
            }
        }
    }
}

/// Phase 3 PR1: Stage 2 entry point for wikilink dispatch.
///
/// Reads a parsed wikilink (the `dest_url` and pothole-text fields from
/// pulldown-cmark's `Tag::Link { link_type: LinkType::WikiLink { has_pothole } }`
/// or `Tag::Image { … LinkType::WikiLink … }`) and produces rendered output
/// via the existing [`super::embed_renderer`] registry.
///
/// # Arguments
///
/// * `dest_url` — pulldown-cmark's `dest_url` (everything before `|` in
///   the source; may carry `#section` and/or `?query` fragments).
/// * `pothole` — the pothole text (everything after `|`), or `None` if
///   `has_pothole=false`.
/// * `is_embed` — `true` for `![[…]]` (image-form), `false` for `[[…]]`.
///   Routes embeds through the registry; routes plain wikilinks to a
///   standard markdown link.
/// * `graph` — content graph for path resolution.
/// * `from_path` — calling file's path (for relative URL computation +
///   diagnostics).
///
/// # Status (Phase 3 PR1, dormant)
///
/// This function compiles and is unit-tested, but no caller wires it in
/// at runtime yet. PR2 enables `ENABLE_WIKILINKS` and adds the call from
/// `src-tauri/src/build/markdown/pipeline.rs::transform_events`.
pub fn dispatch_wikilink_embed(
    dest_url: &str,
    pothole: Option<&str>,
    is_embed: bool,
    graph: &ContentGraph,
    from_path: &str,
    assets: &AssetSnapshot,
) -> WikilinkEmit {
    dispatch_wikilink_embed_with_lookup(
        dest_url,
        pothole,
        is_embed,
        graph,
        from_path,
        assets,
        &|ext| lookup_renderer(ext).map(|r| r as &dyn EmbedRenderer),
    )
}

/// Like [`dispatch_wikilink_embed`] but threads a custom registry lookup.
/// Used when the caller has plugin-registered renderers.
pub fn dispatch_wikilink_embed_with_registry(
    dest_url: &str,
    pothole: Option<&str>,
    is_embed: bool,
    graph: &ContentGraph,
    from_path: &str,
    assets: &AssetSnapshot,
    registry: &super::registry::RendererRegistry,
) -> WikilinkEmit {
    dispatch_wikilink_embed_with_lookup(
        dest_url,
        pothole,
        is_embed,
        graph,
        from_path,
        assets,
        &|ext| registry.lookup(ext).map(|r| r as &dyn EmbedRenderer),
    )
}

fn dispatch_wikilink_embed_with_lookup(
    dest_url: &str,
    pothole: Option<&str>,
    is_embed: bool,
    graph: &ContentGraph,
    from_path: &str,
    assets: &AssetSnapshot,
    lookup: &dyn Fn(&str) -> Option<&dyn EmbedRenderer>,
) -> WikilinkEmit {
    let split = split_dest_url(dest_url);
    let pothole_content = match pothole {
        None => PotholeContent::Empty,
        Some(s) => parse_pothole_params(s),
    };

    if is_embed {
        dispatch_embed_form(&split, pothole_content, graph, from_path, assets, lookup)
    } else {
        dispatch_wikilink_form(&split, pothole_content, graph, from_path)
    }
}

/// Reassemble a `SplitDestUrl` back into a single URL string.
///
/// Inverse of `split_dest_url`. Used before external-URL provider detection
/// so the full URL (including `?query` and `#fragment`) is available.
///
/// Note: always emits in canonical `?query#fragment` order regardless of the
/// original source order. For well-formed URLs (query before fragment) this is
/// byte-identical to the input. Degenerate `#fragment?query` inputs are silently
/// reordered — acceptable for external URL embeds where providers only accept
/// canonical query-first URLs.
fn reassemble_url(split: &SplitDestUrl<'_>) -> String {
    let mut url = split.file.to_string();
    if let Some(q) = split.query {
        url.push('?');
        url.push_str(q);
    }
    if let Some(s) = split.section {
        url.push('#');
        url.push_str(s);
    }
    url
}

/// Dispatch for `![[…]]` (embed form). Mirrors `resolve_embed`'s body.
fn dispatch_embed_form(
    split: &SplitDestUrl<'_>,
    pothole: PotholeContent,
    graph: &ContentGraph,
    from_path: &str,
    assets: &AssetSnapshot,
    lookup: &dyn Fn(&str) -> Option<&dyn EmbedRenderer>,
) -> WikilinkEmit {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // External URL embed: bypass ContentGraph resolution entirely.
    // Any http:// or https:// URL is synthesized as an iframe directly.
    // Provider detection (YouTube/Vimeo/CodePen) happens inside
    // synthesize_url_embed_html; unrecognised URLs get a generic <iframe>.
    if split.file.starts_with("http://") || split.file.starts_with("https://") {
        let full_url = reassemble_url(split);
        let html = crate::render::url_embed::synthesize_url_embed_html(
            &full_url,
            &pothole,
            assets,
        );
        return WikilinkEmit {
            output: EmitKind::Html(html),
            outgoing_link: None,
            diagnostics: vec![],
        };
    }

    // Phase 3 PR2: trailing-slash dispatch is the folder-list embed
    // (`![[/journal/]]`). We must check this BEFORE `resolve_reference`
    // because ContentGraph::resolve_path normalizes trailing slashes
    // away — running it first would always discard the folder-embed
    // signal. The actual listing is rendered by the src-tauri marker
    // resolver (Task 16) which has `all_docs` available; here we just
    // emit a marker carrying the user-written path + the source file
    // path (for relative resolution).
    //
    // Pothole text after `|` becomes the folder-list params string
    // (e.g. `limit:5,more,sort:date`). We parse it back from whatever
    // pothole shape pulldown-cmark gave us.
    if !split.file.is_empty() && split.file.ends_with('/') {
        let pothole_raw = match &pothole {
            PotholeContent::Empty => String::new(),
            PotholeContent::WidthToken { rest_alias, .. } => rest_alias.clone(),
            PotholeContent::Params(_) => String::new(),
            PotholeContent::Alias(s) => s.clone(),
        };
        let params = super::embed_renderer::folder_list::parse_params(&pothole_raw);
        let marker =
            super::embed_renderer::folder_list::emit_marker(split.file, from_path, &params);
        return WikilinkEmit {
            output: EmitKind::Html(marker),
            outgoing_link: Some(OutgoingLink {
                target_path: split.file.to_string(),
                display_text: split.file.to_string(),
                link_type: LinkType::Embed,
            }),
            diagnostics,
        };
    }

    // Resolve. Same logic as resolve_embed: empty file → same file;
    // non-empty → fuzzy resolve.
    let resolved = if split.file.is_empty() {
        ResolvedRef::Found(from_path.to_string())
    } else {
        resolve_reference(split.file, graph, from_path)
    };

    // Derive `alias` and `width` for ParsedEmbed from the pothole.
    // For PotholeContent::Params we surface no alias; the params are
    // carried via TitleParams (consumers in PR4 onward can read them
    // directly without round-tripping through the `moss:` title channel).
    // For PR1 the params get folded into the renderer via the same path
    // Stage 1 uses today: there's no Stage-2 consumer yet, so we forward
    // alias as None when we have pure params.
    let (alias_owned, width): (Option<String>, Option<&'static str>) = match &pothole {
        PotholeContent::Empty => (None, None),
        PotholeContent::WidthToken { width, rest_alias } => (
            if rest_alias.is_empty() {
                None
            } else {
                Some(rest_alias.clone())
            },
            Some(*width),
        ),
        PotholeContent::Params(_) => (None, None),
        PotholeContent::Alias(s) => (Some(s.clone()), None),
    };

    match resolved {
        ResolvedRef::Found(target_path) => {
            let outgoing = OutgoingLink {
                target_path: target_path.clone(),
                display_text: split.file.to_string(),
                link_type: LinkType::Embed,
            };

            let parsed = ParsedEmbed {
                resolved_path: &target_path,
                from_path,
                query: split.query,
                section: split.section,
                alias: alias_owned.as_deref(),
                width,
                attrs: None,
            };

            // Phase 3 PR4.5 (2026-05-27): non-image wikilink embeds
            // (video / pdf / audio / iframe / 3D) route DIRECTLY to the
            // typed-HTML synthesizer here. Previously they emitted
            // `EmitKind::Inline(markdown_link)` with a `moss:kind=…`
            // title that Stage 2 was supposed to read back via
            // `parse_title` — but PR4 deleted `parse_title`, and PR2's
            // markdown round-trip had already been dropping the title.
            // The result was non-image embeds rendering as plain
            // `<a href>` links. The fix is to skip the round-trip
            // entirely: derive `TitleParams` from the pothole content
            // and the resolved URL, then hand them straight to the
            // per-kind synthesizer. Image embeds keep their inline-
            // markdown emission so `<picture>` / `<figure>` wrap stays
            // in the markdown round-trip path that already worked.
            let ext = path_extension(&target_path);
            let url = relative_asset_path(from_path, &target_path);
            if let Some(synth_kind) = ext.as_deref().and_then(synth_kind_for_ext) {
                let params = build_synth_params(synth_kind, &parsed, &pothole);
                let html = match synth_kind {
                    SynthKind::Video => {
                        crate::render::video::synthesize_video_html(&params, &url, assets)
                    }
                    SynthKind::Pdf => {
                        crate::render::pdf::synthesize_pdf_html(&params, &url, assets)
                    }
                    SynthKind::Audio => {
                        crate::render::audio::synthesize_audio_html(&params, &url, assets)
                    }
                    SynthKind::Iframe => {
                        crate::render::iframe::synthesize_iframe_html(&params, &url, assets)
                    }
                    SynthKind::Model => {
                        crate::render::model::synthesize_model_html(&params, &url, assets)
                    }
                };
                return WikilinkEmit {
                    output: EmitKind::Html(html),
                    outgoing_link: Some(outgoing),
                    diagnostics,
                };
            }

            // Image embeds — the unified arm (image-embed synth-collapse).
            //
            // ALL `![[photo.jpg]]` forms route through here to a typed
            // `Block::Figure`, the SAME node the CommonMark `![](url)` path
            // produces. This replaced the prior split (a fit/position
            // "fast-path" that emitted bare `<picture>` + an
            // `ImageRenderer::render_to_markdown` round-trip that dropped
            // width via `let _ = params`). Six embed kinds already went
            // dispatch → synth → Html; image now matches via
            // dispatch → Block::Figure → render_document.
            //
            // Four sources of display params are assembled into the figure:
            //   1. width  ← `embed.width` (canonical WidthToken) → figure `data-width=`
            //   2. caption + alt ← `classify_image_alias` (structural → none,
            //      caption-text → both, empty → none; never `Some("")`)
            //   3. fit/position ← `build_image_media_attrs` → `to_inline_style()`
            //      → inner `<img>` `style=` (NOT the figure)
            //   4. align + class_names ← `build_image_media_attrs` → figure class list
            //
            // Emitting `EmitKind::Block` (not `EmitKind::Html`) keeps the
            // 1:1 `apply_emit` substitution so the figure inherits the source
            // paragraph's `block_meta` → `data-source-line` survives.
            //
            // `find_lone_wikilink_image` guarantees the dispatcher is only
            // reached for a lone embed (within its container), so the figure
            // shape is always correct here.
            if matches!(ext.as_deref(), Some(e) if IMAGE_EXTENSIONS.iter().any(|x| *x == e)) {
                let media = build_image_media_attrs(&pothole, parsed.attrs.as_ref());
                // Recover a content-relative percent (`|55%`) from the alias.
                // A percent isn't a named width token, so `parse_pothole_params`
                // classifies it as `Alias` and it would otherwise leak into the
                // caption. Split it here so the figure carries the width and the
                // caption is the remaining (width-stripped) alias. Recovered here
                // (not in `parse_pothole_params`) so the shared pothole classifier
                // stays width-vocabulary-agnostic.
                // Sync: the no-graph twin lives in ast/parser.rs::try_promote_to_figure
                // (wikilink_pothole arm) — both split width via media::split_alt_width.
                let (alias_no_width, pct_width): (Option<String>, Option<String>) =
                    match parsed.alias {
                        Some(a) => {
                            let (rest, w) = crate::media::split_alt_width(a);
                            (Some(rest), w)
                        }
                        None => (None, None),
                    };
                let alias_class =
                    crate::media::classify_image_alias(alias_no_width.as_deref());
                let alt = alias_class.caption.clone().unwrap_or_default();
                let caption: Option<Vec<crate::ast::node::Inline>> = alias_class
                    .caption
                    .map(|c| vec![crate::ast::node::Inline::Text(c)]);
                // `AlignSide::css_class()` returns the canonical
                // `moss-align-left` / `moss-align-right` class verbatim —
                // the same class the figure renderer appends.
                let align = media.align.map(|side| side.css_class().to_string());
                let img_style = media.to_inline_style();
                // Width source, in priority order:
                //  1. canonical pothole WidthToken (`|wide`) — `width`
                //  2. a width token embedded in a structural alias (`|wide cover`)
                //  3. a content-relative percent anywhere in the pothole (`|55%`)
                let figure_width: Option<String> = width
                    .map(|w| w.to_string())
                    .or_else(|| {
                        alias_class.display_keywords.as_deref().and_then(|kw| {
                            kw.split_whitespace()
                                .find_map(crate::media::match_width_token)
                                .map(|w| w.to_string())
                        })
                    })
                    .or(pct_width);
                let figure = crate::ast::node::Block::Figure {
                    image: crate::ast::node::Inline::Image {
                        // `Asset` is the canonical kind for an `<img src>`
                        // (matches resolve_urls' image-URL classification).
                        src: crate::ast::url::Url::resolved(
                            url.clone(),
                            crate::ast::url::UrlKind::Asset,
                        ),
                        alt,
                        title: None,
                        is_wikilink: true,
                        wikilink_pothole: None,
                    },
                    caption,
                    // Named token OR `"NN%"` percent; the node stores
                    // `Option<String>` (for Deserialize).
                    width: figure_width,
                    align,
                    class_names: media.class_names,
                    img_style,
                };
                return WikilinkEmit {
                    output: EmitKind::Block(Box::new(figure)),
                    outgoing_link: Some(outgoing),
                    diagnostics,
                };
            }

            let emit = match ext.as_deref().and_then(lookup) {
                Some(r) => match r.render(&parsed) {
                    RenderedEmbed::Inline(s) => EmitKind::Inline(s),
                    RenderedEmbed::Html(s) => EmitKind::Html(s),
                    RenderedEmbed::Deferred { marker } => EmitKind::Deferred(marker),
                },
                None => {
                    // Fallback: plain file link (Obsidian parity for
                    // unknown extensions).
                    EmitKind::Inline(format!("[{}]({})", split.file, url))
                }
            };

            WikilinkEmit {
                output: emit,
                outgoing_link: Some(outgoing),
                diagnostics,
            }
        }
        ResolvedRef::Unresolved => {
            diagnostics.push(Diagnostic {
                message: format!("Unresolved embed: ![[{}]]", split.file),
                source_path: from_path.to_string(),
                reference: split.file.to_string(),
            });

            WikilinkEmit {
                output: EmitKind::Inline(format!(
                    "[{}](moss-unresolved:{})",
                    split.file, split.file
                )),
                outgoing_link: Some(OutgoingLink {
                    target_path: split.file.to_string(),
                    display_text: split.file.to_string(),
                    link_type: LinkType::Embed,
                }),
                diagnostics,
            }
        }
    }
}

/// Dispatch for `[[…]]` (plain wikilink). Mirrors `resolve_wikilink`'s body
/// (the non-embed case).
///
/// DORMANT in the live build: the only production caller
/// ([`crate::ast::dispatch_wikilink_embeds`]) always passes `is_embed: true`,
/// so plain `[[…]]` text links never route here. They reach the typed AST as
/// `Inline::Link { is_wikilink: true }` and are resolved by
/// `crate::ast::resolve_urls` instead. This function remains as a tested
/// helper (and for any future plugin/CLI caller that passes `is_embed:
/// false`).
fn dispatch_wikilink_form(
    split: &SplitDestUrl<'_>,
    pothole: PotholeContent,
    graph: &ContentGraph,
    from_path: &str,
) -> WikilinkEmit {
    let mut diagnostics = Vec::new();

    // For plain wikilinks, only Alias-shaped potholes contribute to
    // display text. Width tokens and params are meaningless on a
    // non-embed wikilink — preserve Stage 1 behavior by ignoring them.
    let alias_display = match &pothole {
        PotholeContent::Alias(s) => Some(s.clone()),
        PotholeContent::WidthToken { rest_alias, .. } if !rest_alias.is_empty() => {
            Some(rest_alias.clone())
        }
        _ => None,
    };

    let display_text = if let Some(a) = alias_display {
        a
    } else if let Some(sec) = split.section {
        if split.file.is_empty() {
            sec.to_string()
        } else {
            format!("{} > {}", split.file, sec)
        }
    } else {
        split.file.to_string()
    };

    let resolved = if split.file.is_empty() {
        ResolvedRef::Found(from_path.to_string())
    } else {
        resolve_reference(split.file, graph, from_path)
    };

    match resolved {
        ResolvedRef::Found(target_path) => {
            let outgoing = OutgoingLink {
                target_path: target_path.clone(),
                display_text: display_text.clone(),
                link_type: LinkType::Wikilink,
            };

            let anchor = build_anchor(split.section);
            let link = if split.file.is_empty() {
                format!("[{}]({})", display_text, anchor)
            } else {
                format!(
                    "[{}](moss-resolved:{}{})",
                    display_text, target_path, anchor
                )
            };

            WikilinkEmit {
                output: EmitKind::Link(link),
                outgoing_link: Some(outgoing),
                diagnostics,
            }
        }
        ResolvedRef::Unresolved => {
            diagnostics.push(Diagnostic {
                message: format!("Unresolved wikilink: [[{}]]", split.file),
                source_path: from_path.to_string(),
                reference: split.file.to_string(),
            });
            WikilinkEmit {
                output: EmitKind::Link(format!(
                    "[{}](moss-unresolved:{})",
                    display_text, split.file
                )),
                outgoing_link: Some(OutgoingLink {
                    target_path: split.file.to_string(),
                    display_text,
                    link_type: LinkType::Wikilink,
                }),
                diagnostics,
            }
        }
    }
}

/// Build [`MediaAttrs`] from a pothole's alias / params.
///
/// Two active sources of display vocabulary for image embeds:
///
/// - Alias form (`![[hero.jpg|cover left]]`) — whitespace-separated
///   display keywords. The pothole arrives as
///   [`PotholeContent::Alias`] or [`PotholeContent::WidthToken::rest_alias`]
///   when a width token preceded the keywords. `parse_media_attrs` decodes
///   them into typed `fit` / `position` / `align` fields.
/// - Params form (`![[hero.jpg|fit=cover position=left]]`) — every token
///   is `key=value`. The pothole arrives as
///   [`PotholeContent::Params`] carrying a `TitleParams` bag; we look up
///   `fit` / `position` / `align` by name and convert their values via the
///   per-enum `from_keyword`. Unknown keys flow through as `extra_attrs`.
///
/// Pandoc attribute blocks (`![[hero.jpg|cover]]{.theme-rounded x="y"}`) are
/// a third potential source, but [`ParsedEmbed::attrs`] is currently
/// hard-coded to `None` at the dispatcher's image branch (see
/// `dispatch_embed_form`). The `attrs` parameter is plumbed through for
/// future wiring; today the function ignores it. Don't grow the merge
/// logic here until a caller actually populates `parsed.attrs`.
fn build_image_media_attrs(
    pothole: &PotholeContent,
    _attrs: Option<&crate::ast::attrs::AttrBlock>,
) -> MediaAttrs {
    let mut media = MediaAttrs::default();

    // Source 1: alias form. Only fold when the entire alias is structural
    // (every token is a display keyword) — non-structural aliases are
    // caption text and don't contribute display params.
    let alias_text = match pothole {
        PotholeContent::Alias(s) => Some(s.as_str()),
        PotholeContent::WidthToken { rest_alias, .. } if !rest_alias.is_empty() => {
            Some(rest_alias.as_str())
        }
        _ => None,
    };
    if let Some(text) = alias_text {
        // Width tokens (`wide`, `screen`, etc.) may appear adjacent to fit /
        // position keywords in space-separated aliases like
        // `![[hero|wide cover]]`. They ride on the figure wrapper via
        // `embed.width`, not the inner `<img>`; strip them here so the
        // remainder ("cover") parses cleanly through `parse_media_attrs`.
        // Without this, `is_all_display_keywords("wide cover")` returns
        // `false` (because "wide" isn't a display keyword) and we'd
        // silently drop the fit/position — the same regression this branch
        // exists to fix.
        let cleaned: Vec<&str> = text
            .split_whitespace()
            .filter(|t| crate::media::match_width_token(t).is_none())
            .collect();
        let cleaned_str = cleaned.join(" ");
        if !cleaned_str.is_empty() && crate::media::is_all_display_keywords(&cleaned_str) {
            let parsed = parse_media_attrs(&cleaned_str);
            media.fit = parsed.fit;
            media.position = parsed.position;
            media.align = parsed.align;
            // `parse_media_attrs` doesn't populate `class_names` or
            // `extra_attrs` today (those come from Pandoc blocks, which
            // aren't wired). The extends here are forward-looking scaffolding
            // — harmless no-ops on current `MediaAttrs` shape.
            media.class_names.extend(parsed.class_names);
            for (k, v) in parsed.extra_attrs {
                media.extra_attrs.insert(k, v);
            }
        }
    }

    // Source 2: Params form (K=V pothole). Recognized keys override; the
    // rest flow through as `extra_attrs`.
    //
    // `style` is filtered OUT here because `synthesize_image_with_media_attrs`
    // builds the `style="…"` attribute from `MediaAttrs::to_inline_style()`;
    // letting an author-typed `style=foo` ALSO flow into `extra_attrs` would
    // emit two `style=` attributes on the same `<img>` and the browser would
    // honor the last one, silently dropping moss's object-fit / object-position.
    if let PotholeContent::Params(params) = pothole {
        for (k, v) in &params.params {
            match k.as_str() {
                "fit" => {
                    if let Some(fit) = Fit::from_keyword(v) {
                        media.fit = Some(fit);
                    }
                }
                "position" => {
                    if let Some(pos) = Position::from_keyword(v) {
                        media.position = Some(pos);
                    }
                }
                "align" => {
                    if let Some(side) = AlignSide::from_keyword(v) {
                        media.align = Some(side);
                    }
                }
                // `width` / `data-width` ride on the figure wrapper, not the
                // inner `<img>` — handled upstream via `embed.width`.
                "width" | "data-width" => {}
                "classes" => {
                    for c in v.split_whitespace() {
                        if !media.class_names.iter().any(|x| x == c) {
                            media.class_names.push(c.to_string());
                        }
                    }
                }
                // Drop `style=` to avoid duplicate-attribute emission;
                // see function-level note above.
                "style" => {}
                _ => {
                    media.extra_attrs.insert(k.clone(), v.clone());
                }
            }
        }
    }

    media
}

/// Discriminant for the per-kind HTML synthesizer the dispatcher routes to
/// directly (Phase 3 PR4.5). Non-image / non-deferred extensions skip the
/// markdown round-trip and emit `EmitKind::Html` straight from the synth
/// function — see the dispatcher branch in `dispatch_embed_form`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SynthKind {
    Video,
    Pdf,
    Audio,
    Iframe,
    Model,
}

/// Classify a file extension into a [`SynthKind`] when the dispatcher should
/// emit final HTML directly. Returns `None` for image (`png`/`jpg`/...) —
/// which keeps its inline-markdown round-trip — and for deferred kinds
/// (`md`/`ipynb`/`csv`/`tsv`) which still need src-tauri post-passes.
///
/// The extension table now lives in `ext_kind::reference_kind_for_ext` (the
/// single source of truth). The `EmbedRenderer::extensions()` slices in
/// `embed_renderer.rs` still exist and are still used by the renderer
/// registry — do NOT delete them.
fn synth_kind_for_ext(ext: &str) -> Option<SynthKind> {
    use crate::resolve::ext_kind::{reference_kind_for_ext, ExtKind};
    match reference_kind_for_ext(ext) {
        ExtKind::Video => Some(SynthKind::Video),
        ExtKind::Pdf => Some(SynthKind::Pdf),
        ExtKind::Audio => Some(SynthKind::Audio),
        ExtKind::Iframe => Some(SynthKind::Iframe),
        ExtKind::Model => Some(SynthKind::Model),
        ExtKind::Image | ExtKind::Transclusion | ExtKind::Notebook | ExtKind::Table | ExtKind::Other => None,
    }
}

/// Build the [`TitleParams`] handed to a per-kind synthesizer.
///
/// Mirrors the `*_extra_params` helpers in `embed_renderer.rs` (which fed
/// the legacy `moss:title` round-trip) — they are the canonical reference
/// for which params each synth function reads. Notable shape:
///
/// - **`data-width`** carries the canonical wrapper width (`body | wide |
///   page | screen`) when the pothole was an Obsidian width-token. Synth
///   functions emit it as the `data-width=` attribute on the wrapping
///   element.
/// - **`width` / `height`** come from `|WxH` sizing aliases parsed via
///   [`Sizing`]. Pixel/percent/vh values are CSS-formatted.
/// - **`title`** (iframe only) carries non-sizing alias text as the
///   iframe's accessible name (legacy behaviour: `[[widget.html|My Widget]]`).
/// - **`query` / `fragment`** (iframe/pdf only) reconstruct the served URL
///   from the split dest-url — pulldown-cmark percent-encodes `?` and `#`
///   if they stay in the URL slot, so the dispatcher hands them out-of-band.
/// - **Pothole `Params`** are folded last so author-typed `width=400` etc.
///   override the alias-derived values (every-token-K=V rule wins).
fn build_synth_params(
    kind: SynthKind,
    embed: &ParsedEmbed<'_>,
    pothole: &PotholeContent,
) -> TitleParams {
    let mut params = TitleParams::default();
    if let Some(w) = embed.width {
        params.insert("data-width", w);
    }

    // iframe / pdf carry ?query and #fragment out-of-band on the synth side.
    if matches!(kind, SynthKind::Iframe | SynthKind::Pdf) {
        if let Some(q) = embed.query {
            params.insert("query", q);
        }
        if let Some(f) = embed.section {
            params.insert("fragment", f);
        }
    }

    // Per-kind alias handling. `embed.alias` is the pothole's alias-shaped
    // remainder (already excludes width tokens) — for non-image kinds it
    // overwhelmingly looks like a `|WxH` sizing hint, but iframe also
    // supports free-text titles.
    if let Some(alias) = embed.alias {
        match kind {
            SynthKind::Video => {
                // Video alias supports a bare `loop` keyword (case-insensitive)
                // plus an optional `WxH` sizing hint in any order.
                // `![[clip.mp4|loop]]` → params["loop"]="1", no sizing.
                // `![[clip.mp4|640x360 loop]]` → sizing AND loop.
                // `![[clip.mp4|640x360]]` → sizing only (backward-compat).
                //
                // Strategy: tokenise on whitespace, extract the `loop` token,
                // then run Sizing::parse on the remaining tokens joined by a
                // space so multi-token sizing like "640x360" keeps working.
                let mut tokens: Vec<&str> = alias.split_whitespace().collect();
                let loop_pos = tokens
                    .iter()
                    .position(|t| t.eq_ignore_ascii_case("loop"));
                if let Some(pos) = loop_pos {
                    tokens.remove(pos);
                    params.insert("loop", "1");
                }
                let remainder = tokens.join(" ");
                if !remainder.is_empty() {
                    match Sizing::parse(&remainder) {
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
            SynthKind::Pdf | SynthKind::Model => match Sizing::parse(alias) {
                Some(Sizing::Width(w)) => {
                    params.insert("width", w.to_css());
                }
                Some(Sizing::Box(w, h)) => {
                    params.insert("width", w.to_css());
                    params.insert("height", h.to_css());
                }
                None => {}
            },
            SynthKind::Iframe => match Sizing::parse(alias) {
                Some(Sizing::Width(w)) => {
                    params.insert("width", w.to_css());
                }
                Some(Sizing::Box(w, h)) => {
                    params.insert("width", w.to_css());
                    params.insert("height", h.to_css());
                }
                None => {
                    // Non-sizing alias text → iframe accessible name.
                    params.insert("title", alias);
                }
            },
            SynthKind::Audio => {
                // Audio synthesizer reads no alias-derived params today
                // (controls / preload defaults are unconditional). Leave
                // params untouched.
            }
        }
    }

    // Author-typed K=V params win over alias-derived values (every-token
    // rule already validated by `parse_pothole_params`).
    if let PotholeContent::Params(p) = pothole {
        for (k, v) in &p.params {
            params.insert(k.clone(), v.clone());
        }
    }

    params
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::{ContentGraph, ContentGraphBuilder};

    // --- parse_pothole_params edge cases ----------------------------------

    #[test]
    fn pothole_empty_string_is_empty() {
        assert_eq!(parse_pothole_params(""), PotholeContent::Empty);
        assert_eq!(parse_pothole_params("   "), PotholeContent::Empty);
    }

    #[test]
    fn pothole_pure_digit_is_alias_not_width_token() {
        // `[[img.jpg|400]]` — `400` is NOT a spec § P9 width keyword
        // (only `body|wide|page|screen|full` match). Pure-pixel widths
        // are handled downstream by the relevant renderer's `Sizing::parse`
        // on the alias. parse_pothole_params therefore classifies `400`
        // as a plain alias here; the image / video renderer's existing
        // alias-based sizing logic (carry through to ParsedEmbed.alias)
        // does the rest.
        match parse_pothole_params("400") {
            PotholeContent::Alias(s) => assert_eq!(s, "400"),
            other => panic!("expected Alias, got {:?}", other),
        }
    }

    #[test]
    fn pothole_plain_alias() {
        // `[[file|My alias]]`
        match parse_pothole_params("My alias") {
            PotholeContent::Alias(s) => assert_eq!(s, "My alias"),
            other => panic!("expected Alias, got {:?}", other),
        }
    }

    #[test]
    fn pothole_kv_pair_is_params() {
        // `[[file|width=400 align=left]]`
        match parse_pothole_params("width=400 align=left") {
            PotholeContent::Params(p) => {
                assert_eq!(p.get("width"), Some("400"));
                assert_eq!(p.get("align"), Some("left"));
            }
            other => panic!("expected Params, got {:?}", other),
        }
    }

    #[test]
    fn pothole_single_kv_is_params() {
        // `[[file|width=400]]`
        match parse_pothole_params("width=400") {
            PotholeContent::Params(p) => {
                assert_eq!(p.get("width"), Some("400"));
            }
            other => panic!("expected Params, got {:?}", other),
        }
    }

    #[test]
    fn pothole_bare_alt_blocks_kv_parse() {
        // CRITICAL: `[[file|alt text=cover]]` — `alt` is bare (no `=`),
        // so the whole thing must be classified as alias text, NOT as
        // a `text=cover` param.
        match parse_pothole_params("alt text=cover") {
            PotholeContent::Alias(s) => assert_eq!(s, "alt text=cover"),
            other => panic!("expected Alias, got {:?}", other),
        }
    }

    #[test]
    fn pothole_uppercase_key_blocks_kv_parse() {
        // `[[file|My Notes=Important]]` — `My` doesn't start with
        // lowercase letter; whole thing falls through to alias.
        match parse_pothole_params("My Notes=Important") {
            PotholeContent::Alias(s) => assert_eq!(s, "My Notes=Important"),
            other => panic!("expected Alias, got {:?}", other),
        }
    }

    #[test]
    fn pothole_no_equals_is_alias() {
        // `[[file|width 400]]` — no `=` on `width` token; alias.
        match parse_pothole_params("width 400") {
            PotholeContent::Alias(s) => assert_eq!(s, "width 400"),
            other => panic!("expected Alias, got {:?}", other),
        }
    }

    #[test]
    fn pothole_partial_kv_falls_through_to_alias() {
        // `[[file|width=400 caption text]]` — first token is K=V but
        // `caption` and `text` aren't. Every-token rule fails → alias.
        match parse_pothole_params("width=400 caption text") {
            PotholeContent::Alias(s) => assert_eq!(s, "width=400 caption text"),
            other => panic!("expected Alias, got {:?}", other),
        }
    }

    #[test]
    fn pothole_kv_with_hyphenated_key() {
        // Hyphen and underscore allowed in keys.
        match parse_pothole_params("aria-label=primary data_id=42") {
            PotholeContent::Params(p) => {
                assert_eq!(p.get("aria-label"), Some("primary"));
                assert_eq!(p.get("data_id"), Some("42"));
            }
            other => panic!("expected Params, got {:?}", other),
        }
    }

    #[test]
    fn pothole_obsidian_width_keyword() {
        // `[[img.jpg|wide]]` — `wide` is a known width keyword.
        match parse_pothole_params("wide") {
            PotholeContent::WidthToken { width, rest_alias } => {
                assert_eq!(width, "wide");
                assert!(rest_alias.is_empty());
            }
            other => panic!("expected WidthToken, got {:?}", other),
        }
    }

    // --- split_dest_url cases --------------------------------------------

    #[test]
    fn split_dest_url_plain_file() {
        let s = split_dest_url("notes");
        assert_eq!(s.file, "notes");
        assert_eq!(s.section, None);
        assert_eq!(s.query, None);
    }

    #[test]
    fn split_dest_url_with_anchor() {
        let s = split_dest_url("notes#section");
        assert_eq!(s.file, "notes");
        assert_eq!(s.section, Some("section"));
        assert_eq!(s.query, None);
    }

    #[test]
    fn split_dest_url_with_query() {
        let s = split_dest_url("page.html?x=1");
        assert_eq!(s.file, "page.html");
        assert_eq!(s.query, Some("x=1"));
    }

    #[test]
    fn split_dest_url_anchor_then_query() {
        let s = split_dest_url("page.html#frag?x=1");
        assert_eq!(s.file, "page.html");
        assert_eq!(s.section, Some("frag"));
        assert_eq!(s.query, Some("x=1"));
    }

    #[test]
    fn split_dest_url_query_then_anchor() {
        // Both '?' and '#' present, '?' first — query owns its tail; '#' splits out.
        let s = split_dest_url("page.html?x=1#frag");
        assert_eq!(s.file, "page.html");
        // query is [q+1..h] => "x=1"
        assert_eq!(s.query, Some("x=1"));
        // section is [h+1..] => "frag"
        assert_eq!(s.section, Some("frag"));
    }

    // --- dispatch_wikilink_embed integration -----------------------------
    //
    // Use a minimal ContentGraph that registers a few paths. We rely on
    // ContentGraph::resolve_path() to map bare names back to filesystem-
    // looking paths (the same surface Stage 1 uses).

    fn build_graph(paths: &[&str]) -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        for p in paths {
            // Derive a simple slug from the filename stem; the slug is
            // only relevant for slug-based resolution which our tests
            // don't exercise (they use bare filenames matching `path`).
            let slug = std::path::Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(p);
            b.add_file(p, slug);
        }
        b.build()
    }

    /// Helper: empty AssetSnapshot. Phase 3 PR4.5 (2026-05-27) added the
    /// `assets` parameter to dispatch_wikilink_embed so non-image embed
    /// kinds can route directly to their HTML synthesizers.
    fn empty_snapshot() -> AssetSnapshot {
        AssetSnapshot::new()
    }

    #[test]
    fn dispatch_bare_wikilink_is_link() {
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "notes",
            None,
            /* is_embed */ false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.contains("notes"));
                assert!(link.contains("moss-resolved:"));
            }
            other => panic!("expected Link, got {:?}", other),
        }
        assert!(emit.outgoing_link.is_some());
        assert!(emit.diagnostics.is_empty());
    }

    #[test]
    fn dispatch_wikilink_with_alias_uses_alias_text() {
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "notes",
            Some("My alias"),
            false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.starts_with("[My alias]"));
            }
            other => panic!("expected Link, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_unresolved_wikilink_emits_diagnostic() {
        let graph = build_graph(&[]);
        let emit = dispatch_wikilink_embed(
            "missing",
            None,
            false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        assert_eq!(emit.diagnostics.len(), 1);
        match emit.output {
            EmitKind::Link(link) => assert!(link.contains("moss-unresolved:")),
            other => panic!("expected Link, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_anchor_wikilink_preserves_section_in_href() {
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "notes#section",
            None,
            false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.contains("moss-resolved:"));
                // Anchor preserved (Obsidian-style heading-anchor slug).
                assert!(link.contains("#section"), "got: {}", link);
            }
            other => panic!("expected Link, got {:?}", other),
        }
    }

    // --- build_anchor / dispatch_wikilink_form (`[[…]]` text-link) -------
    //
    // SCOPE WARNING — read before trusting these as link-path coverage.
    //
    // The three tests below drive `dispatch_wikilink_embed(..., is_embed:
    // false, ..)`, i.e. the `dispatch_wikilink_form` branch and its
    // `build_anchor` helper. That branch is the ONLY caller of `build_anchor`,
    // and in the LIVE build it is DORMANT: the sole production caller of this
    // dispatcher — the AST visitor `crate::ast::dispatch_wikilink_embeds`
    // (`ast/dispatch_wikilink_embeds.rs`) — hard-codes `is_embed: true`
    // (it only walks `![[…]]` image-embed `Inline::Image` nodes). Plain
    // `[[Page#Heading]]` TEXT links never reach this function in production;
    // they arrive as `Inline::Link { is_wikilink: true }` and are resolved
    // by `crate::ast::resolve_urls`, whose `slug_wikilink_suffix` performs
    // the user-facing `#Heading → #heading` slugging.
    //
    // ==> The REAL guards for `[[Page#Heading]]` text-link slugging live in
    //     `crates/moss-core/src/ast/resolve_urls.rs`:
    //       - wikilink_cross_page_fragment_is_slugged
    //       - wikilink_same_page_fragment_is_slugged
    //       - markdown_link_fragment_stays_raw_not_slugged
    //       - wikilink_block_ref_keeps_id_raw
    //       - wikilink_cjk_fragment_preserved
    //       - slug_wikilink_suffix_preserves_query
    //
    // These three tests are kept because `build_anchor` is real code worth
    // locking (it mirrors `slug_wikilink_suffix`, and a plugin/CLI caller
    // could pass `is_embed: false`), NOT because they cover the live link
    // path. Their names are deliberately `build_anchor_*` so a future reader
    // is not misled into thinking text-link resolution is guarded here.

    #[test]
    fn build_anchor_slugs_section_fragment() {
        // Helper-path test (NOT the live `[[…]]` link path — see SCOPE
        // WARNING above; the live guard is
        // `resolve_urls::wikilink_cross_page_fragment_is_slugged`).
        //
        // `dispatch_wikilink_form("notes#My Heading")` slugs the section
        // fragment via `build_anchor` → `obsidian_heading_anchor` →
        // `#my-heading`.
        // Emitted output: `[notes > My Heading](moss-resolved:notes.md#my-heading)`.
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "notes#My Heading",
            None,
            false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.contains("#my-heading"), "got: {}", link);
                assert!(link.contains("moss-resolved:"), "got: {}", link);
            }
            other => panic!("expected Link, got {:?}", other),
        }
    }

    #[test]
    fn build_anchor_same_page_emits_bare_anchor() {
        // Helper-path test (NOT the live `[[…]]` link path — see SCOPE
        // WARNING above; the live guard is
        // `resolve_urls::wikilink_same_page_fragment_is_slugged`).
        //
        // `dispatch_wikilink_form("#My Heading")` (empty file part) resolves
        // to a bare slugged anchor with no `moss-resolved:` prefix.
        // Emitted output: `[My Heading](#my-heading)`.
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "#My Heading",
            None,
            false,
            &graph,
            "notes.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.contains("(#my-heading)"), "got: {}", link);
                assert!(!link.contains("moss-resolved:"), "got: {}", link);
            }
            other => panic!("expected Link, got {:?}", other),
        }
    }

    #[test]
    fn build_anchor_block_ref_is_not_slugged() {
        // Helper-path test (NOT the live `[[…]]` link path — see SCOPE
        // WARNING above; the live guard is
        // `resolve_urls::wikilink_block_ref_keeps_id_raw`).
        //
        // Block refs (^id) are emitted RAW — NOT run through
        // obsidian_heading_anchor. Use a block-id with a space + uppercase
        // so slugging (which would yield "#block-id") is observably
        // different from the raw form ("#Block Id"). This fails loudly if
        // the `^` short-circuit in build_anchor regresses.
        // Emitted output: `[notes > ^Block Id](moss-resolved:notes.md#Block Id)`.
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "notes#^Block Id",
            None,
            false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.contains("#Block Id"), "expected raw block-ref, got: {}", link);
                assert!(!link.contains("#block-id"), "block-ref was slugged: {}", link);
            }
            other => panic!("expected Link, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_video_extension_routes_to_synth() {
        // Phase 3 PR4.5 (2026-05-27): non-image wikilinks now route
        // DIRECTLY to the per-kind synthesizer — the markdown round-trip
        // is gone (it was dropping the `moss:kind=…` title since PR2 and
        // entirely silent after PR4 deleted `parse_title`). The dispatcher
        // returns `EmitKind::Html` carrying the `<video>` byte shape; we
        // pin only the structural identity (element + src) so byte-shape
        // changes are owned by the synth tests in `render/video.rs`.
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<video"), "expected <video>, got: {}", s);
                assert!(s.contains(r#"src="clip.mp4""#), "expected src=, got: {}", s);
                assert!(s.contains("moss-embed-video"), "expected class, got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_pdf_extension_routes_to_synth() {
        // See `dispatch_video_extension_routes_to_synth` for the PR4.5
        // routing rationale. PdfRenderer emits an `<object type="application/pdf">`.
        let graph = build_graph(&["report.pdf"]);
        let emit = dispatch_wikilink_embed(
            "report.pdf",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<object"), "expected <object>, got: {}", s);
                assert!(
                    s.contains(r#"data="report.pdf""#),
                    "expected data=, got: {}",
                    s
                );
                assert!(
                    s.contains(r#"type="application/pdf""#),
                    "expected type=, got: {}",
                    s
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_audio_extension_routes_to_synth() {
        let graph = build_graph(&["song.mp3"]);
        let emit = dispatch_wikilink_embed(
            "song.mp3",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<audio"), "expected <audio>, got: {}", s);
                assert!(s.contains(r#"src="song.mp3""#), "expected src=, got: {}", s);
                assert!(
                    s.contains(r#"type="audio/mpeg""#),
                    "expected MIME, got: {}",
                    s
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_iframe_extension_routes_to_synth() {
        let graph = build_graph(&["widget.html"]);
        let emit = dispatch_wikilink_embed(
            "widget.html",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<iframe"), "expected <iframe>, got: {}", s);
                assert!(
                    s.contains(r#"src="widget.html""#),
                    "expected src=, got: {}",
                    s
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_model_extension_routes_to_synth() {
        let graph = build_graph(&["scene.glb"]);
        let emit = dispatch_wikilink_embed(
            "scene.glb",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(
                    s.contains("<model-viewer"),
                    "expected <model-viewer>, got: {}",
                    s
                );
                assert!(
                    s.contains(r#"src="scene.glb""#),
                    "expected src=, got: {}",
                    s
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_iframe_alias_carries_title() {
        // `![[widget.html|Embedded Widget]]` — non-sizing alias text
        // surfaces on the iframe as the `title=` accessible name. The
        // synth function reads `params.get("title")`; `build_synth_params`
        // routes the alias there for iframe-kind.
        let graph = build_graph(&["widget.html"]);
        let emit = dispatch_wikilink_embed(
            "widget.html",
            Some("Embedded Widget"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(r#"title="Embedded Widget""#), "got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_video_sizing_alias_propagates_dims() {
        // `![[clip.mp4|640x360]]` — sizing alias becomes width/height
        // CSS-formatted on the <video>.
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            Some("640x360"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(r#"width="640px""#), "got: {}", s);
                assert!(s.contains(r#"height="360px""#), "got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    // --- Image display-attr dispatch (fit / position threading) ----------
    //
    // The polish-pass plan (docs/plans/2026-05-27-polish-passes-followups.md
    // Item B) flagged that `![[hero.jpg|cover]]` and
    // `![[hero.jpg|fit=cover position=left]]` were silently dropping
    // fit/position. `ImageRenderer::render_to_markdown` builds `TitleParams`
    // from the alias / pothole, then explicitly discards them with
    // `let _ = params;` — the emitted markdown is bare `![](url)`. The
    // dispatcher now intercepts these cases ahead of the renderer registry
    // and emits a final `<img>` with the appropriate `style=`.

    #[test]
    fn dispatch_only_fires_for_wikilink_caller() {
        // This test documents the safety rule from v2 revision notes:
        // dispatch_wikilink_embed is the ONLY public entry point for
        // wikilink-form events. There is no parallel function for
        // LinkType::Inline. Plain `[link](file.pdf)` events stay as
        // markdown links via pulldown-cmark's default emission.
        //
        // We can't directly test what the caller does (that's in pipeline.rs),
        // but we can pin the invariant by asserting the API surface:
        // the public function takes `is_embed: bool` for `![[…]]` vs
        // `[[…]]`, not a `LinkType` enum that could be confused with Inline.

        // No assertion needed — the type signature itself is the check.
    }
    // ---- Image embed dispatch (image-embed synth-collapse) ---------------
    //
    // ALL `![[photo.jpg]]` forms now emit `EmitKind::Block(Block::Figure)`
    // with full param threading (width / caption / fit / position / align).
    // The OLD tests asserted `EmitKind::Inline("![](url)")` round-trips and
    // a fit/position fast-path that DROPPED width — they encoded the bug
    // this change fixes and were removed.

    fn figure_of(emit: &WikilinkEmit) -> &crate::ast::node::Block {
        match &emit.output {
            EmitKind::Block(b) => b.as_ref(),
            other => panic!("expected EmitKind::Block(Figure), got {other:?}"),
        }
    }

    fn render_figure(emit: &WikilinkEmit) -> String {
        let block = match emit.output.clone() {
            EmitKind::Block(b) => *b,
            other => panic!("expected EmitKind::Block(Figure), got {other:?}"),
        };
        let doc = crate::ast::Document::from_blocks(vec![block]);
        crate::ast::render_document(&doc, &crate::ast::DefaultHooks::new())
    }

    fn dispatch_img(alias: Option<&str>) -> WikilinkEmit {
        let graph = build_graph(&["photo.jpg", "hero.jpg"]);
        dispatch_wikilink_embed("photo.jpg", alias, true, &graph, "index.md", &empty_snapshot())
    }

    #[test]
    fn dispatch_image_plain_emits_figure_block() {
        use crate::ast::node::{Block, Inline};
        let emit = dispatch_img(None);
        match figure_of(&emit) {
            Block::Figure { image, caption, width, align, class_names, img_style } => {
                assert!(caption.is_none(), "plain embed: no caption");
                assert!(width.is_none());
                assert!(align.is_none());
                assert!(class_names.is_empty());
                assert!(img_style.is_none());
                match image {
                    Inline::Image { src, alt, is_wikilink, .. } => {
                        assert!(src.is_resolved());
                        assert_eq!(alt, "");
                        assert!(*is_wikilink);
                    }
                    other => panic!("expected Image, got {other:?}"),
                }
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_image_caption_text_sets_alt_and_figcaption() {
        use crate::ast::node::{Block, Inline};
        let emit = dispatch_img(Some("My caption"));
        match figure_of(&emit) {
            Block::Figure { image, caption, .. } => {
                let cap = caption.as_ref().expect("caption present");
                assert_eq!(cap.len(), 1);
                match &cap[0] {
                    Inline::Text(t) => assert_eq!(t, "My caption"),
                    other => panic!("expected caption Text, got {other:?}"),
                }
                match image {
                    Inline::Image { alt, .. } => assert_eq!(alt, "My caption"),
                    other => panic!("expected Image, got {other:?}"),
                }
            }
            other => panic!("expected Figure, got {other:?}"),
        }
        let html = render_figure(&emit);
        assert!(html.contains(r#"alt="My caption""#), "got: {html}");
        assert!(html.contains("<figcaption>My caption</figcaption>"), "got: {html}");
    }

    #[test]
    fn dispatch_image_width_token_preserved_as_data_width() {
        use crate::ast::node::Block;
        // FIX: width is no longer dropped — it lands as figure data-width=.
        let emit = dispatch_img(Some("wide"));
        match figure_of(&emit) {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("wide"));
                assert!(caption.is_none(), "width token is not a caption");
            }
            other => panic!("expected Figure, got {other:?}"),
        }
        let html = render_figure(&emit);
        assert!(html.contains(r#"data-width="wide""#), "got: {html}");
    }

    #[test]
    fn dispatch_image_cover_emits_object_fit_on_inner_img() {
        use crate::ast::node::Block;
        let emit = dispatch_img(Some("cover"));
        match figure_of(&emit) {
            Block::Figure { img_style, caption, .. } => {
                assert_eq!(img_style.as_deref(), Some("object-fit:cover"));
                assert!(caption.is_none(), "structural alias is not a caption");
            }
            other => panic!("expected Figure, got {other:?}"),
        }
        let html = render_figure(&emit);
        assert!(html.contains("object-fit:cover"), "got: {html}");
        assert!(html.contains(r#"<figure class="moss-image""#), "got: {html}");
    }

    #[test]
    fn dispatch_image_cover_left_emits_fit_and_position() {
        use crate::ast::node::Block;
        let emit = dispatch_img(Some("cover left"));
        match figure_of(&emit) {
            Block::Figure { img_style, .. } => {
                let style = img_style.as_deref().expect("style present");
                assert!(style.contains("object-fit:cover"), "got: {style}");
                assert!(style.contains("object-position:left"), "got: {style}");
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_image_params_form_emits_object_fit() {
        use crate::ast::node::Block;
        let emit = dispatch_img(Some("fit=cover"));
        match figure_of(&emit) {
            Block::Figure { img_style, .. } => {
                assert_eq!(img_style.as_deref(), Some("object-fit:cover"));
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_image_two_word_position_combines() {
        use crate::ast::node::Block;
        let emit = dispatch_img(Some("cover top left"));
        match figure_of(&emit) {
            Block::Figure { img_style, .. } => {
                let style = img_style.as_deref().expect("style present");
                assert!(style.contains("object-fit:cover"), "got: {style}");
                assert!(style.contains("object-position:top left"), "got: {style}");
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_image_wide_cover_combines_width_and_fit() {
        use crate::ast::node::Block;
        // width → figure data-width; fit → inner <img> style. Both survive
        // (the pre-collapse fast-path DROPPED width when fit was present).
        let emit = dispatch_img(Some("wide cover"));
        match figure_of(&emit) {
            Block::Figure { width, img_style, .. } => {
                assert_eq!(width.as_deref(), Some("wide"));
                assert_eq!(img_style.as_deref(), Some("object-fit:cover"));
            }
            other => panic!("expected Figure, got {other:?}"),
        }
        let html = render_figure(&emit);
        assert!(html.contains(r#"data-width="wide""#), "got: {html}");
        assert!(html.contains("object-fit:cover"), "got: {html}");
    }

    #[test]
    fn dispatch_image_inner_img_has_single_style_attr() {
        // Inner <img> carries exactly one style= (object-fit), no LQIP dup
        // (no snapshot here).
        let emit = dispatch_img(Some("fit=cover"));
        let html = render_figure(&emit);
        let n = html.matches("style=").count();
        assert_eq!(n, 1, "exactly one style= attr, got {n}: {html}");
    }

    // Editor Image UX (2026-06-04): wikilink `|NN%` percent width carries
    // into Block::Figure.width instead of leaking into the caption.
    // -------------------------------------------------------------------

    #[test]
    fn wikilink_image_percent_carries_width() {
        use crate::ast::node::Block;
        // ![[pic.jpg|55%]] → Figure { width: Some("55%") }, no bogus caption
        let emit = dispatch_img(Some("55%"));
        match figure_of(&emit) {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("55%"));
                assert!(caption.is_none(), "percent must not become a caption");
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    #[test]
    fn wikilink_image_percent_with_caption() {
        use crate::ast::node::{Block, Inline};
        // ![[pic.jpg|My cap|55%]] → width Some("55%"), caption "My cap"
        let emit = dispatch_img(Some("My cap|55%"));
        match figure_of(&emit) {
            Block::Figure { width, caption, .. } => {
                assert_eq!(width.as_deref(), Some("55%"));
                let cap = caption.as_ref().expect("caption present");
                assert!(
                    matches!(cap.as_slice(), [Inline::Text(t)] if t == "My cap"),
                    "caption should be the non-width segment, got {cap:?}"
                );
            }
            other => panic!("expected Figure, got {other:?}"),
        }
    }

    // --- External URL dispatch ---

    #[test]
    fn dispatch_external_url_youtube_emits_html() {
        let graph = build_graph(&[]);
        let emit = dispatch_wikilink_embed(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<iframe"), "got: {s}");
                assert!(s.contains("youtube.com/embed"), "got: {s}");
                assert!(s.contains(r#"data-provider="youtube""#), "got: {s}");
            }
            other => panic!("expected Html, got: {other:?}"),
        }
        assert!(emit.outgoing_link.is_none(), "external URLs must not register in ContentGraph");
        assert!(emit.diagnostics.is_empty());
    }

    #[test]
    fn dispatch_external_url_generic_emits_html() {
        let graph = build_graph(&[]);
        let emit = dispatch_wikilink_embed(
            "https://example.com/embed",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<iframe"), "got: {s}");
                assert!(s.contains(r#"src="https://example.com/embed""#), "got: {s}");
                assert!(!s.contains("data-provider="), "generic must not have data-provider, got: {s}");
            }
            other => panic!("expected Html, got: {other:?}"),
        }
        assert!(emit.outgoing_link.is_none());
    }

    #[test]
    fn dispatch_external_url_http_also_works() {
        let graph = build_graph(&[]);
        let emit = dispatch_wikilink_embed(
            "http://example.com/embed",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => assert!(s.contains("<iframe"), "got: {s}"),
            other => panic!("expected Html, got: {other:?}"),
        }
    }

    #[test]
    fn dispatch_external_url_with_width_pothole() {
        let graph = build_graph(&[]);
        let emit = dispatch_wikilink_embed(
            "https://vimeo.com/123456789",
            Some("wide"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => assert!(s.contains(r#"data-width="wide""#), "got: {s}"),
            other => panic!("expected Html, got: {other:?}"),
        }
    }

    // --- |loop ambient-video parser arm (spec §3.3a) ----------------------

    #[test]
    fn dispatch_video_loop_alias_emits_ambient_set() {
        // `![[clip.mp4|loop]]` — bare `loop` token sets the ambient playback set
        // and suppresses controls.
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            Some("loop"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(" autoplay"), "missing autoplay, got: {}", s);
                assert!(s.contains(" muted"), "missing muted, got: {}", s);
                assert!(s.contains(" loop"), "missing loop, got: {}", s);
                assert!(s.contains(" playsinline"), "missing playsinline, got: {}", s);
                assert!(!s.contains(" controls"), "controls must be absent on loop branch, got: {}", s);
                assert!(s.contains(" data-loop"), "missing data-loop, got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_video_loop_alias_case_insensitive() {
        // `![[clip.mp4|LOOP]]` — loop keyword must be case-insensitive.
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            Some("LOOP"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(" autoplay"), "missing autoplay on LOOP alias, got: {}", s);
                assert!(!s.contains(" controls"), "controls must be absent on LOOP alias, got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_video_size_and_loop_alias_propagates_all() {
        // `![[clip.mp4|640x360 loop]]` — sizing AND loop must both be set;
        // order within the alias is irrelevant to the output.
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            Some("640x360 loop"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(r#"width="640px""#), "missing width, got: {}", s);
                assert!(s.contains(r#"height="360px""#), "missing height, got: {}", s);
                assert!(s.contains(" autoplay"), "missing autoplay, got: {}", s);
                assert!(s.contains(" data-loop"), "missing data-loop, got: {}", s);
                assert!(!s.contains(" controls"), "controls must be absent, got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_video_loop_first_then_size() {
        // `![[clip.mp4|loop 640x360]]` — loop before size is also valid
        // (order-independent within the alias).
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            Some("loop 640x360"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(r#"width="640px""#), "missing width, got: {}", s);
                assert!(s.contains(r#"height="360px""#), "missing height, got: {}", s);
                assert!(s.contains(" autoplay"), "missing autoplay, got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_video_sizing_alias_still_works_without_loop() {
        // `![[clip.mp4|640x360]]` — sizing without loop must NOT emit autoplay
        // (backward-compat; non-loop path unchanged).
        let graph = build_graph(&["clip.mp4"]);
        let emit = dispatch_wikilink_embed(
            "clip.mp4",
            Some("640x360"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains(r#"width="640px""#), "missing width, got: {}", s);
                assert!(!s.contains(" autoplay"), "autoplay must NOT be emitted without loop, got: {}", s);
                assert!(s.contains(" controls"), "controls must be emitted on non-loop path, got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

}
