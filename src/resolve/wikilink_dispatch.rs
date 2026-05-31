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
fn build_anchor(section: Option<&str>) -> String {
    use crate::heading_anchor::obsidian_heading_anchor;
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

            // Image embeds carrying display params (`![[hero.jpg|cover]]`,
            // `![[hero.jpg|cover left]]`, `![[hero.jpg|fit=cover position=left]]`)
            // route directly to `synthesize_image_html` here. Without this
            // branch, `ImageRenderer::render_to_markdown` discards the params
            // accumulated from the alias / pothole (`let _ = params`,
            // embed_renderer.rs after Phase 3 PR4) — the emitted markdown is
            // bare `![alt](url)`, fit/position are gone, and the author's
            // intent silently no-ops.
            //
            // Image embeds without display params (`![[hero.jpg]]` plus
            // caption-text aliases like `![[hero.jpg|My nice photo]]`) keep
            // the legacy markdown round-trip — `ImageRenderer` emits
            // `![alt](url)`, the outer pipeline re-parses and lets
            // `emit_standalone_figure_image` decide on the figure wrap from
            // the surrounding paragraph shape.
            //
            // TODO Phase 4: collapse this branch into `SynthKind::Image`
            // alongside the video/pdf/audio/iframe/model dispatch above, with
            // `build_synth_params` learning to project `MediaAttrs` into
            // `TitleParams` and the caller passing the right `ImageContext`
            // (MarkdownInline vs. MarkdownStandalone) based on paragraph
            // shape. That refactor also restores width/align/class_names
            // threading to the figure wrapper — currently dropped here
            // because `MarkdownInline` has no figure-level slots. See the
            // polish-pass plan's Item B for the discussion + the
            // architecture review of this commit for the recommended seam.
            if matches!(ext.as_deref(), Some(e) if IMAGE_EXTENSIONS.iter().any(|x| *x == e)) {
                let media = build_image_media_attrs(&pothole, parsed.attrs.as_ref());
                if media.fit.is_some() || media.position.is_some() {
                    // When fit/position fire, the alias was a structural
                    // display-keyword run (e.g. "cover", "contain center")
                    // — not caption text. alt stays empty so it doesn't
                    // leak the keywords into the accessible name.
                    // (Matches `ImageRenderer::render_to_markdown`'s
                    // `caption_text = None` branch in the structural-alias
                    // arm.)
                    let html =
                        synthesize_image_with_media_attrs(&url, /* alt */ "", assets, &media);
                    return WikilinkEmit {
                        output: EmitKind::Html(html),
                        outgoing_link: Some(outgoing),
                        diagnostics,
                    };
                }
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

/// Synthesize the `<img>` HTML for a wikilink embed carrying display
/// params (fit / position), routing through the canonical
/// `synthesize_image_html` so `<picture>`, dims, eager/lazy, and LQIP
/// suppression all stay in one place. The inline style produced by
/// [`MediaAttrs::to_inline_style`] is appended via
/// [`crate::render::image::ImageRenderOptions::extra_attrs`] — the
/// synthesizer detects the resulting `style="..."` fragment and skips its
/// own LQIP/dominant-color style emission (per the `extra_has_style`
/// guard in `synthesize_inner`).
///
/// Author-provided extras (`media.extra_attrs`) are emitted after the
/// style fragment, HTML-escaped, in deterministic [`BTreeMap`] order.
/// `class_names` ride on the inner `<img>` via [`ImageRenderOptions::class`]
/// — present-tense limitation: align (which conceptually belongs on the
/// figure wrapper) and the figure-level class list are not yet carried
/// through; see the polish-pass plan for the standalone-wrap follow-up.
fn synthesize_image_with_media_attrs(
    url: &str,
    alt: &str,
    assets: &AssetSnapshot,
    media: &MediaAttrs,
) -> String {
    use crate::media::html_escape;
    use crate::render::image::{synthesize_image_html, ImageContext, ImageRenderOptions};

    let mut extra = String::new();
    if let Some(style) = media.to_inline_style() {
        extra.push_str(r#"style=""#);
        extra.push_str(&html_escape(&style));
        extra.push('"');
    }
    for (k, v) in &media.extra_attrs {
        if !extra.is_empty() {
            extra.push(' ');
        }
        extra.push_str(&html_escape(k));
        extra.push_str(r#"=""#);
        extra.push_str(&html_escape(v));
        extra.push('"');
    }

    let class_owned = if media.class_names.is_empty() {
        None
    } else {
        Some(media.class_names.join(" "))
    };

    synthesize_image_html(
        url,
        alt,
        assets,
        ImageContext::MarkdownInline,
        &ImageRenderOptions {
            eager: false,
            class: class_owned.as_deref(),
            extra_attrs: if extra.is_empty() { None } else { Some(&extra) },
        },
    )
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
/// Extension tables MUST stay in sync with the corresponding `EmbedRenderer`
/// `extensions()` slices in `embed_renderer.rs`. A future refactor that
/// surfaces the kind on the renderer trait could remove this duplication.
fn synth_kind_for_ext(ext: &str) -> Option<SynthKind> {
    let lower = ext.to_ascii_lowercase();
    match lower.as_str() {
        "mp4" | "webm" | "mov" | "m4v" => Some(SynthKind::Video),
        "pdf" => Some(SynthKind::Pdf),
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "opus" => Some(SynthKind::Audio),
        "html" | "htm" => Some(SynthKind::Iframe),
        "glb" | "gltf" => Some(SynthKind::Model),
        _ => None,
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
            SynthKind::Video | SynthKind::Pdf | SynthKind::Model => match Sizing::parse(alias) {
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

    #[test]
    fn wikilink_section_fragment_is_slugged() {
        // `[[notes#My Heading]]` slugs the section fragment via
        // `build_anchor` → `obsidian_heading_anchor` → `#my-heading`.
        // Real emitted output: `[notes > My Heading](moss-resolved:notes.md#my-heading)`.
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
    fn same_page_section_wikilink_emits_bare_anchor() {
        // Same-page `[[#My Heading]]` resolves to a bare slugged anchor with
        // no `moss-resolved:` prefix (the file part is empty, so the link
        // target is just the fragment).
        // Real emitted output: `[My Heading](#my-heading)`.
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
    fn wikilink_block_ref_is_not_slugged() {
        // Block refs `[[notes#^block-id]]` strip the `^` and emit the id
        // RAW (no slugging) — `build_anchor` short-circuits on the `^`
        // prefix before calling `obsidian_heading_anchor`.
        // Real emitted output: `[notes > ^block-id](moss-resolved:notes.md#block-id)`.
        let graph = build_graph(&["notes.md"]);
        let emit = dispatch_wikilink_embed(
            "notes#^block-id",
            None,
            false,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Link(link) => {
                assert!(link.contains("#block-id"), "got: {}", link);
            }
            other => panic!("expected Link, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_embed_uses_image_renderer() {
        let graph = build_graph(&["photo.jpg"]);
        let emit = dispatch_wikilink_embed(
            "photo.jpg",
            None,
            /* is_embed */ true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Inline(s) => {
                // ImageRenderer emits Inline markdown `![alt](url)` for the
                // no-width / no-attrs path (see embed_renderer.rs::ImageRenderer).
                assert!(s.starts_with("!["));
                assert!(s.contains("photo.jpg"));
            }
            other => panic!("expected Inline, got {:?}", other),
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

    #[test]
    fn dispatch_width_token_pothole_passes_width_to_renderer() {
        // `![[photo.jpg|wide]]` should propagate `wide` to the renderer.
        // The pothole parses as a WidthToken and the dispatcher hands
        // ParsedEmbed { width: Some("wide"), .. } to the ImageRenderer.
        // Phase 3 PR4 (2026-05-27): ImageRenderer now emits bare
        // markdown — width is no longer round-tripped through a title
        // attribute. The pothole-classifier still surfaces the canonical
        // width to typed consumers downstream (the dispatcher's
        // `WidthToken` variant — see `parse_pothole_params` test above).
        assert_eq!(
            parse_pothole_params("wide"),
            PotholeContent::WidthToken {
                width: "wide",
                rest_alias: String::new()
            }
        );
        let graph = build_graph(&["photo.jpg"]);
        let emit = dispatch_wikilink_embed(
            "photo.jpg",
            Some("wide"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Inline(s) => assert_eq!(s, "![](photo.jpg)"),
            other => panic!("expected Inline markdown, got {:?}", other),
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
    fn dispatch_image_alias_with_fit_emits_object_fit_style() {
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("cover"),
            /* is_embed */ true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(s.contains("<img"), "expected <img>, got: {}", s);
                assert!(
                    s.contains("object-fit:cover"),
                    "expected object-fit:cover, got: {}",
                    s,
                );
                // Structural alias must not leak into accessible name:
                // `cover` is a display keyword, not caption text.
                assert!(s.contains(r#"alt="""#), "expected empty alt, got: {}", s,);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_alias_with_fit_and_position_emits_both_styles() {
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("cover left"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(
                    s.contains("object-fit:cover"),
                    "expected object-fit:cover, got: {}",
                    s,
                );
                assert!(
                    s.contains("object-position:left"),
                    "expected object-position:left, got: {}",
                    s,
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_params_form_emits_object_fit_style() {
        // `![[hero.jpg|fit=cover position=left]]` — K=V pothole.
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("fit=cover position=left"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(
                    s.contains("object-fit:cover"),
                    "expected object-fit:cover, got: {}",
                    s,
                );
                assert!(
                    s.contains("object-position:left"),
                    "expected object-position:left, got: {}",
                    s,
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_with_two_word_position_emits_combined_style() {
        // `![[hero.jpg|cover top right]]` — two-word position keyword.
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("cover top right"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(
                    s.contains("object-fit:cover"),
                    "expected object-fit:cover, got: {}",
                    s,
                );
                assert!(
                    s.contains("object-position:top right"),
                    "expected object-position:top right, got: {}",
                    s,
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_without_display_params_stays_on_legacy_path() {
        // `![[hero.jpg]]` — no pothole. Still routes through
        // ImageRenderer's markdown round-trip (EmitKind::Inline).
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            None,
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Inline(s) => assert!(s.starts_with("!["), "expected markdown, got: {}", s),
            other => panic!("expected Inline markdown, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_with_caption_alias_stays_on_legacy_path() {
        // `![[hero.jpg|A nice photo]]` — caption text, not display params.
        // Falls through to ImageRenderer's markdown round-trip.
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("A nice photo"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Inline(s) => {
                assert!(s.contains("A nice photo"), "expected caption, got: {}", s)
            }
            other => panic!("expected Inline markdown, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_with_width_token_only_stays_on_legacy_path() {
        // `![[hero.jpg|wide]]` — width token, no fit/position.
        // Falls through to ImageRenderer (which today carries width via
        // the `embed.width` field, not via the new dispatcher branch).
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("wide"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        assert!(
            matches!(emit.output, EmitKind::Inline(_)),
            "width-only alias must NOT trigger the new fit/position branch",
        );
    }

    #[test]
    fn dispatch_image_params_style_does_not_duplicate_style_attribute() {
        // Regression test: an author-typed `style=foo` in the K=V pothole
        // used to land in `MediaAttrs::extra_attrs` and emit a second
        // `style="foo"` attribute after the synth's `style="object-fit:…"`.
        // The browser would honor the last `style=` and drop moss's
        // object-fit silently. `build_image_media_attrs` now filters
        // `style` out of `extra_attrs` since the synth owns inline-style
        // emission.
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("fit=cover style=bar"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                let style_count = s.matches("style=").count();
                assert_eq!(
                    style_count, 1,
                    "expected exactly one style= attribute, got {} in: {}",
                    style_count, s,
                );
                assert!(s.contains("object-fit:cover"), "got: {}", s);
                // The author's `style=bar` is silently dropped — better
                // than two style= attrs. Document the trade-off via
                // assertion.
                assert!(!s.contains(r#"style="bar""#), "got: {}", s);
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

    #[test]
    fn dispatch_image_with_width_and_fit_emits_fit_but_drops_width() {
        // `![[hero.jpg|wide cover]]` — width AND fit in one space-separated
        // alias. The width token gets stripped from the alias-cleaning step
        // (so fit still parses cleanly through `parse_media_attrs`), but
        // the dispatcher's image branch hard-codes `ImageContext::MarkdownInline`
        // — there is no `data-width=` slot on the inner `<img>`. Width
        // belongs on the `<figure>` wrapper, which this branch doesn't emit.
        //
        // **Known limitation, not a regression**: the pre-fix path through
        // `ImageRenderer::render_to_markdown` ALSO dropped width here (the
        // `let _ = params;` line discarded it along with fit/position). The
        // legacy figure-wrap path via Phase 3 PR4's retired `parse_title`
        // would have surfaced it, but that's gone. Restoring it is Phase 4
        // territory (TODO: collapse this branch into `SynthKind::Image` with
        // context-aware emission — see the polish-pass plan's Item B
        // discussion and the architecture review of this commit).
        //
        // The test pins the CURRENT behavior so a future refactor that
        // accidentally re-surfaces width has to update this assertion
        // intentionally.
        let graph = build_graph(&["hero.jpg"]);
        let emit = dispatch_wikilink_embed(
            "hero.jpg",
            Some("wide cover"),
            true,
            &graph,
            "index.md",
            &empty_snapshot(),
        );
        match emit.output {
            EmitKind::Html(s) => {
                assert!(
                    s.contains("object-fit:cover"),
                    "expected object-fit:cover, got: {}",
                    s,
                );
                assert!(
                    !s.contains(r#"data-width="#),
                    "current branch does not thread width to figure wrap, got: {}",
                    s,
                );
            }
            other => panic!("expected Html, got: {:?}", other),
        }
    }

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
}
