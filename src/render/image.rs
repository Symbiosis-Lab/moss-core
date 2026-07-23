//! Image HTML synthesizer — the single entry point for emitting `<img>` /
//! `<picture>` markup in moss output.
//!
//! See [`docs/architecture/structural-html-emission.md`](../../../../docs/architecture/structural-html-emission.md)
//! for the architectural principle: structural HTML decisions are made at the
//! typed-data layer (pulldown-cmark events, shortcode AST, typed component
//! props), with all three call sites converging on the function in this
//! module. Regex post-passes are reserved for non-markdown-origin attribute
//! injection only.
//!
//! # Migration state (post-Step-7, 2026-05-16)
//!
//! Steps 1-7 of the structural-html-emission migration are complete:
//! - Step 1: extracted `synthesize_image_html`
//! - Step 2: routed markdown `Tag::Image` events
//! - Step 3: routed `:::hero` shortcode image
//! - Step 4: routed link-preview favicon
//! - Step 6: routed every cover image path (folder cards, child summary
//!   cards, folder index hero, photo/video gallery thumbnails) through
//!   `render_cover_html`
//! - Step 7: retired `wrap_img_in_picture` (the structural part of the
//!   legacy regex post-pass). The synthesizer now owns every `<picture>`
//!   wrap in moss output. `add_image_placeholder_attributes` survives as
//!   the attribute-injection seam for the documented carve-outs (see
//!   below).
//!
//! The byte-shape contract is captured by snapshot tests at the bottom of
//! this file. They are the line of defense against accidental output
//! drift; any future change to attribute order, quoting, or whitespace
//! must update them deliberately.
//!
//! Later steps will:
//! - Step 8: switch `MarkdownStandalone` to a `<figure class="moss-image">`
//!   wrapper (breaking change for user themes — staged separately)
//! - Add AVIF `<source>` lines once the image pipeline produces AVIF
//! - Drop the inline LQIP `style=` in favor of a wrapper CSS custom prop
//!
//! # Step-8 contract: synthesizer owns the outer `<figure>` (planned)
//!
//! `transform_events` currently wraps the synthesizer's `MarkdownStandalone`
//! output in its own `<figure>` for the three caption-pattern branches
//! (image+emphasis, separate-emphasis, implicit-figure). After Step 8 the
//! synthesizer emits `<figure class="moss-image">` itself; if `transform_events`
//! still wraps, the output will be `<figure><figure class="moss-image">...</figure>
//! <figcaption>...</figcaption></figure>` — invalid double-wrap.
//!
//! The Step-8 contract: caption flows into the synthesizer via
//! `MarkdownStandalone { caption: Option<&str> }` (or a richer
//! `CaptionMarkdown` for emphasis-in-caption support), and the three
//! caption-pattern branches collapse into a single `Event::Html(
//! synthesize_image_html(..., MarkdownStandalone { caption }))` emission.
//! The `<figcaption>` becomes the synthesizer's responsibility, NOT
//! `transform_events`. Captures the spec at
//! `docs/architecture/structural-html-emission.md#output-shape`.
//!
//! # Carve-outs: bare `<img>` emitters not routed through the synthesizer
//!
//! Four emission paths land bare `<img>` HTML in the output stream that
//! does NOT flow through `synthesize_image_html`. They rely on the regex
//! post-pass (`build/media/placeholder.rs::add_image_placeholder_attributes`)
//! for attribute injection (dims/loading/decoding/LQIP). None of them
//! are bugs — each has a documented architectural reason to stay outside
//! the synthesizer:
//!
//! - **Site logo** (`build/components/nav.rs::render_logo`) — themed UI
//!   affordance, not content. The logo has its own CSS sizing
//!   (`.site-logo { height: 1.8em }`) and does not participate in
//!   LQIP/dims/WebP-variant rendering.
//! - **RSS read-tracking pixel** (`build/feeds/rss.rs`) — 1×1 `<img>` not
//!   rendered visibly; the synthesizer's dims fallback (800×600) and LQIP
//!   would be wrong for this case.
//! - **Email body images** (`infra/newsletter.rs`) — email clients (Gmail,
//!   Outlook, Apple Mail) do not consistently support `<picture>` or
//!   `data-placeholder-src`-driven hydration. Keep flat for cross-client
//!   degradation.
//! - **Raw HTML `<img>` in markdown source** — author-written
//!   `<img src="...">` literally embedded in `.md` files. pulldown-cmark
//!   emits these as `Event::Html` opaque pass-through, so they never reach
//!   `Tag::Image` and are not a moss-controlled emitter. Treated as user
//!   input, the markdown HTML is opaque to the synthesizer and gets only
//!   the additive attribute injection pass.
//!
//! Photography/video gallery thumbnails — previously a carve-out — were
//! folded into the synthesizer in Step 7's commit. The **review colophon
//! cover** (`build/features/review.rs::render_colophon`) — also previously
//! a carve-out — was folded in 2026-05-16. Both use
//! `ImageContext::FolderCardCover` (container-bounded thumbnail semantics).
//!
//! These four remaining carve-outs are flagged here so future maintenance
//! does not drop their attribute injection. Step 7 retired the structural
//! part of the regex (`wrap_img_in_picture`); the surviving
//! `add_image_placeholder_attributes` provides additive attrs only for
//! these bare-img paths.

use crate::asset_paths::{
    deployed_width, is_ladder_source_ext, is_webp_source_ext, ladder_rungs, to_webp, to_webp_rung,
};
use crate::asset_snapshot::{AssetSnapshot, FALLBACK_HEIGHT, FALLBACK_WIDTH};
use crate::contract::sizes as ctx_sizes;
// Same XML-safe escaping used everywhere else in moss for attribute values.
use crate::media::html_escape;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Where the image lives in the document, which determines the wrapper
/// element and attribute set.
///
/// Step-1 implementation supports only `MarkdownInline` (the default emission
/// shape from pulldown-cmark's serializer plus the legacy regex pair's added
/// attributes). Other variants are scaffolded so the call sites in Steps 3-6
/// can pass them without breaking the byte-shape contract; the synthesizer
/// produces the same output for all variants until the wrapper-change step.
///
/// Step 8 (2026-05-17) made these contexts diverge structurally:
/// - `MarkdownStandalone { caption }` → `<figure class="moss-image">…[<figcaption>]…</figure>` wrapper
/// - `MarkdownInline` → bare `<img>` (or `<picture><img></picture>`)
/// - `Hero` → bare `<img>` (the hero shortcode wraps with `<header>`)
/// - `FolderCardCover` → bare `<img>` (`.moss-card-cover > ` wraps)
/// - `LinkPreview` → bare `<img>` (link-preview anchor wraps)
/// - `Favicon` → bare 16×16 `<img>` with no `<picture>`, no LQIP
///
/// Not `Copy` (the embedded `&str` caption would force a lifetime on
/// every consumer); cloning is cheap (borrow) and the call sites pass by
/// value through the synthesizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageContext<'a> {
    /// Image-only paragraph in markdown — emits `<figure class="moss-image">`
    /// around the synthesized `<picture>`/`<img>` output. Used by
    /// `transform_events`'s three caption-pattern branches:
    ///
    /// - **image+emphasis**: `![alt](src) *caption*` — `caption = Some(emphasis_text)`
    /// - **separate-emphasis**: image-only paragraph followed by emphasis-
    ///   only paragraph — `caption = Some(emphasis_text)`
    /// - **implicit figure**: image-only paragraph with non-empty alt and
    ///   `[site].implicit_figure = true` — `caption = Some(alt_text)`
    ///
    /// `caption = None` means "figure wrap but no `<figcaption>`" (reserved
    /// for future callers that want the wrapper structurally without prose).
    ///
    /// `width = Some("body|wide|page|screen")` emits `data-width="..."` on
    /// the outer `<figure>` element per spec § P9. `None` omits the
    /// attribute entirely so themes can target the absence via
    /// `:not([data-width])`. `full` aliases to `screen` upstream — values
    /// reaching this struct are already in canonical value-space.
    MarkdownStandalone {
        caption: Option<&'a str>,
        width: Option<&'a str>,
        /// Editorial runaround alignment, surfaced as a CSS class on the
        /// outer `<figure>`. Phase 1 C1 (2026-05-25): Stage 2 dispatcher
        /// passes `Some("moss-align-left")` / `Some("moss-align-right")`
        /// when the markdown image's title carries `moss:align=left|right`.
        /// `None` omits the class.
        align: Option<&'a str>,
        /// Arbitrary CSS class names from `moss:classes="foo bar"` title
        /// params (Phase 1 C1). Each entry is appended to the figure's
        /// `class="moss-image …"` attribute, space-separated. Empty slice
        /// leaves the class list at its default (`moss-image`).
        class_names: &'a [String],
        /// Arbitrary `key="value"` HTML attributes from leftover
        /// `moss:` title params (Phase 1 C1). The dispatcher passes
        /// every param that isn't a known field (kind/width/align/classes)
        /// through here so future params propagate without code changes.
        /// Keys are emitted in BTreeMap order for stable byte shape.
        /// Empty map emits no extra attributes.
        extra_attrs: &'a BTreeMap<String, String>,
    },
    /// Image inside prose, a list, a table cell, or a callout body.
    /// Always emits bare `<img>` or `<picture><img></picture>`.
    MarkdownInline,
    /// `:::hero` shortcode body image. The hero wrapper handles layout.
    Hero,
    /// Folder-card cover or child-summary cover image.
    FolderCardCover,
    /// External-link preview thumbnail image.
    LinkPreview,
    /// Favicon for a link-preview card. Bare 16×16 `<img>`, no `<picture>`,
    /// no LQIP, no responsive variants.
    Favicon,

    /// Phase 2 scaffold (filled by Phase 2B agent): site-header / nav logo.
    /// Bare `<img>` with `class="site-logo"`, no LQIP, no `<picture>`, no
    /// `loading="lazy"` (logo is above-the-fold). CSS handles sizing
    /// (`.site-logo { height: 1.8em }`).
    SiteLogo,
    /// Phase 2 scaffold (filled by Phase 2C agent): RSS read-tracking pixel.
    /// 1×1 invisible `<img>`. No `loading="lazy"` (the pixel must fire on
    /// read for tracking). No LQIP, no `<picture>`.
    TrackingPixel,
    /// Phase 2 scaffold (filled by Phase 2D agent): newsletter email body
    /// image. Email-client-safe HTML subset: no `<picture>`, no `data-*`,
    /// inline `style="display:block;max-width:100%;height:auto"` for
    /// responsive email layouts. Explicit dims when known (callers pass
    /// `Option<u32>` for width/height; missing → omit per email-client
    /// tolerance).
    EmailBody {
        width: Option<u32>,
        height: Option<u32>,
    },

    /// Phase 2E v5 PR3 (2026-05-26): `:::gallery` body image. Below-the-
    /// fold thumbnail (`loading="lazy"`); same `<picture>`/dims/LQIP byte
    /// shape as `MarkdownInline`. The outer `.moss-gallery-item` wrapper
    /// is owned by the gallery shortcode's `DefaultHooks` impl in
    /// `crates/moss-core/src/ast/hooks.rs`; this variant emits just the
    /// inner `<picture><img></picture>` / `<img>` shape so the wrapper
    /// can sit around it.
    ///
    /// Distinct from `MarkdownInline` so the gallery's per-item style
    /// passthrough (object-position from `MediaAttrs`) has a typed home
    /// to evolve into; today both contexts produce the same inner byte
    /// shape via `synthesize_inner`.
    GalleryThumb,

    /// Phase 2E PR2 (2026-05-26): bare `<img>` emission with no `<picture>`
    /// wrap, no `<source>`, no LQIP, no width/height, no `loading` attr.
    /// Used by the hero typed-renderer fallback path
    /// (`typed_renderers.rs::render_hero_html_typed`) when no manifest
    /// (`MediaDimensionLookup`) is in scope — i.e. test / fragment-render
    /// paths where `AssetRegistry::set_pending` has NOT been called for
    /// the source's `.webp` companion.
    ///
    /// The asset-publish invariant (see `.claude/CLAUDE.md` § "Asset
    /// publish invariant") requires us to NEVER emit a
    /// `<source srcset="*.webp">` for an unregistered variant — the
    /// preview server cannot return placeholder bytes for an URL that
    /// AssetRegistry doesn't know about, and `<picture>` does not recover
    /// from a chosen-source 404. `HeroBare` is the explicit opt-out for
    /// code paths that run before set_pending.
    ///
    /// The byte shape mirrors the pre-Phase-2E fallback exactly:
    /// `<img src="X" alt="Y"[ STYLE] />` where `STYLE` is the inline
    /// style fragment (already escaped) threaded through
    /// [`ImageRenderOptions::extra_attrs`] by the caller. The `class` and
    /// `eager` options are ignored — the hero `<header>` wraps and CSS
    /// handles loading priority.
    HeroBare,
}

/// Optional rendering attributes a caller may pass.
///
/// The defaults preserve the current regex-pass byte shape:
/// - `loading="lazy"` unless `eager=true` (which switches to `loading="eager"
///   fetchpriority="high"`)
/// - No extra inline style
/// - No extra CSS classes on the inner `<img>`
#[derive(Debug, Default, Clone)]
pub struct ImageRenderOptions<'a> {
    /// Above-the-fold loading hint. When true, emits
    /// `loading="eager" fetchpriority="high"` instead of `loading="lazy"`.
    pub eager: bool,
    /// CSS classes to add to the inner `<img>`. Used by link-preview
    /// favicons (`link-preview-favicon`) and similar UI affordances.
    pub class: Option<&'a str>,
    /// Raw extra HTML attribute fragment appended after the standard
    /// attribute block (e.g., `style="object-fit:cover;object-position:50% 50%"`
    /// for `:::hero` covers carrying `MediaAttrs`). The caller is responsible
    /// for HTML-escaping values inside this fragment.
    pub extra_attrs: Option<&'a str>,
}

/// Synthesize the HTML for an image reference.
///
/// `src` is the resolved URL as it should appear in `<img src=>` (already
/// passed through the link resolver and dir_overrides for CJK sites).
///
/// `alt` is the accessible name. Empty string is permitted for decorative
/// images; callers in WCAG-sensitive contexts should pass meaningful text.
///
/// `assets` is the [`AssetSnapshot`] holding pre-fetched per-path dimensions,
/// LQIP data URIs, dominant colors, and registered variant kinds (WebP/AVIF).
/// Phase 1 of the unified-image-emission migration (2026-05-25) replaced the
/// prior `Option<&MediaDimensionLookup>` parameter with this typed contract —
/// `MediaDimensionLookup` still populates the snapshot in `pipeline.rs`'s
/// `build_asset_snapshot` boundary, but the synthesizer no longer probes it
/// directly. Callers that don't have a populated snapshot (test/fragment-
/// render paths) pass `&AssetSnapshot::new()`; the synthesizer then emits
/// fallback dims (800×600) and no LQIP/color style.
///
/// `context` and `options` describe the call site. `Favicon` short-circuits
/// to a 16×16 bare `<img>` (no manifest, no LQIP, no `<picture>`).
///
/// Byte-shape contract (preserved through Phase 1's data-source switch):
///
/// - With no `<picture>` wrap (non-raster):
///   `<img src="X" width="W" height="H" loading="lazy" style="…" alt="Y" />`
/// - Raster originals (png/jpg/jpeg):
///   `<picture><source srcset="X.webp" type="image/webp"><img src="X" width="W" height="H" loading="lazy" style="…" alt="Y" /></picture>`
/// - Inline style is `background-image:url(LQIP);background-size:cover` when
///   the snapshot has LQIP; `background-color:#RRGGBB` when only dominant
///   color is available; absent when neither.
/// - For `eager: true`: `loading="eager" fetchpriority="high"` replaces
///   `loading="lazy"`.
pub fn synthesize_image_html(
    src: &str,
    alt: &str,
    assets: &AssetSnapshot,
    context: ImageContext<'_>,
    options: &ImageRenderOptions<'_>,
) -> String {
    // Favicon short-circuit: hardcoded 16×16, no snapshot lookup, no <picture>.
    // Matches the current emission shape in
    // `build/markdown/typed_renderers.rs::render_link_preview`. `assets` is
    // intentionally unused — favicons are UI affordances that never
    // participate in the variant manifest.
    if matches!(context, ImageContext::Favicon) {
        let class_attr = options
            .class
            .map(|c| format!(r#" class="{}""#, html_escape(c)))
            .unwrap_or_default();
        return format!(
            r#"<img{} src="{}" width="16" height="16" alt="{}">"#,
            class_attr,
            html_escape(src),
            html_escape(alt),
        );
    }

    // Phase 2 scaffold (filled by Phase 2 carve-out agents): the three former
    // bare-<img> carve-outs become first-class synthesizer contexts. Each
    // short-circuits before synthesize_inner (which assumes the standard
    // <picture>/LQIP/dims pipeline that's wrong for these elements).
    // Site-logo short-circuit (Phase 2B carve-out): bare `<img>` with
    // `class="site-logo"`, no `<picture>`, no LQIP, no `loading="lazy"` —
    // the logo is above-the-fold; CSS handles sizing
    // (`.site-logo { height: 1.8em }`). Attribute order
    // (`class`, `src`, `alt`, `aria-hidden`) preserves the pre-Phase-2
    // byte shape emitted by `nav.rs::generate_navigation`.
    if matches!(context, ImageContext::SiteLogo) {
        return format!(
            r#"<img class="site-logo" src="{}" alt="{}" aria-hidden="true">"#,
            html_escape(src),
            html_escape(alt),
        );
    }
    if matches!(context, ImageContext::TrackingPixel) {
        // Phase 2C: RSS read-tracking pixel. 1×1 invisible <img>.
        // NO loading="lazy" — the pixel must fire on read for tracking.
        // NO LQIP, NO <picture>, NO alt text (empty alt for invisible
        // decoration). Self-closing form because this commonly lands in
        // RSS feed XML (CDATA-wrapped <description>).
        return format!(
            r#"<img src="{}" alt="" width="1" height="1" />"#,
            html_escape(src),
        );
    }
    if matches!(context, ImageContext::HeroBare) {
        // Phase 2E PR2 (2026-05-26): no-snapshot hero fallback. Byte
        // shape matches the pre-PR2 emission at
        // `typed_renderers.rs::render_hero_html_typed` lines 554-557:
        // `<img src="X" alt="Y"[ STYLE] />`. `assets` is intentionally
        // unused (the caller passes an empty snapshot via the no-
        // manifest branch); the snapshot is part of the signature only
        // for symmetry with the other contexts. `options.class` and
        // `options.eager` are ignored — hero chrome (CSS / `<header>`)
        // handles styling and loading priority.
        //
        // The inline `style=` fragment that the legacy fallback baked
        // directly into `format!` is now threaded through
        // `options.extra_attrs` (the caller pre-escapes the value and
        // omits the leading space, matching the existing extra_attrs
        // contract — the synthesizer prepends a single space).
        let extra = options
            .extra_attrs
            .map(|s| format!(" {}", s))
            .unwrap_or_default();
        return format!(
            r#"<img src="{}" alt="{}"{} />"#,
            html_escape(src),
            html_escape(alt),
            extra,
        );
    }
    if let ImageContext::EmailBody { width, height } = context {
        // Phase 2D (2026-05-25): email-client-safe <img> for newsletter body
        // images. Email clients do NOT support <picture>, do NOT support
        // data-* attributes, and frequently strip <style> blocks — inline
        // `style=` on <img> plus explicit width/height attrs is the
        // cross-client minimum for responsive layouts. width/height are
        // Option<u32>: omitted when unknown (e.g. remote URLs that aren't in
        // AssetSnapshot). Email clients tolerate missing dims at the cost of
        // a small layout shift.
        let mut dims = String::new();
        if let Some(w) = width {
            dims.push_str(&format!(r#" width="{}""#, w));
        }
        if let Some(h) = height {
            dims.push_str(&format!(r#" height="{}""#, h));
        }
        return format!(
            r#"<img src="{}" alt="{}"{} style="display:block;max-width:100%;height:auto;" />"#,
            html_escape(src),
            html_escape(alt),
            dims,
        );
    }

    // Step 8: the inner `<img>` / `<picture>` is shape-equivalent across
    // every non-favicon context. The wrapping `<figure class="moss-image">`
    // and optional `<figcaption>` are the only context-dependent
    // structure. Compute the inner first, then wrap if requested.
    //
    // The context also decides the `sizes=` value for the srcset ladder
    // (responsive-image-variants Task 3): full-bleed surfaces (hero,
    // `data-width="screen|full"` figures) span the viewport; cards/gallery
    // thumbs occupy grid cells; everything else renders in the content
    // column. Only emitted when the ladder is non-empty — see
    // synthesize_inner.
    //
    // wide/page intentionally map to SIZES_BODY: today's site.css has NO
    // width rule for data-width (ADR-021 follow-up), so wide/page figures
    // render at the content column and 100vw would over-fetch. When ADR-021
    // gives data-width real widths, update this mapping (blurry-risk
    // otherwise). Same note in contract/sizes.rs.
    let sizes_value: &str = match &context {
        ImageContext::Hero => ctx_sizes::SIZES_FULL_BLEED,
        ImageContext::MarkdownStandalone { width: Some(w), .. }
            if matches!(*w, "screen" | "full") =>
        {
            ctx_sizes::SIZES_FULL_BLEED
        }
        ImageContext::FolderCardCover | ImageContext::LinkPreview => ctx_sizes::SIZES_CARD,
        ImageContext::GalleryThumb => ctx_sizes::SIZES_GALLERY,
        _ => ctx_sizes::SIZES_BODY,
    };
    let inner = synthesize_inner(src, alt, assets, options, sizes_value);

    match context {
        ImageContext::MarkdownStandalone {
            caption,
            width,
            align,
            class_names,
            extra_attrs,
        } => wrap_in_figure_full(&inner, caption, width, align, class_names, extra_attrs),
        // All other variants are "no outer wrapper" — caller-owned chrome
        // (hero `<header>`, folder card container, link preview anchor)
        // surrounds the bare img/picture output.
        _ => inner,
    }
}

/// Wrap an already-shaped image fragment in the standalone-image
/// figure container.
///
/// `inner_html` is either:
/// - The output of `synthesize_inner` (markdown `Tag::Image` flowing
///   through `synthesize_image_html`), or
/// - A resolve-phase `<img …>` / `<video …>` HTML string emitted by
///   moss-core's wikilink-lowering (`![[file|display-params]]`).
///
/// Output shape:
///
/// - `caption = Some(text)`, `width = None`:
///   ```html
///   <figure class="moss-image"><picture>…<img …></picture>
///   <figcaption>text</figcaption></figure>
///   ```
/// - `caption = None`, `width = Some("screen")`:
///   ```html
///   <figure class="moss-image" data-width="screen"><picture>…<img …></picture></figure>
///   ```
/// - `width = None` omits the attribute entirely so themes can target
///   the absence via `:not([data-width])`. Per spec § P9, `data-width`
///   sits on the wrapper element (here, `<figure>`) rather than the
///   inner `<img>`.
///
/// Caption text is HTML-escaped at the boundary. Future work: allow
/// markdown formatting inside the caption via an explicit
/// `CaptionMarkdown` variant on `ImageContext`.
///
/// `pub(super)` so `build/markdown/pipeline.rs` can call this directly
/// for the raw-HTML media branch of `emit_standalone_figure_image`
/// without duplicating the wrapper byte shape. The synthesizer is the
/// single source of truth for `<figure class="moss-image">` — when the
/// wrapper class evolves (e.g. `moss-image moss-image--auto` per the
/// Step-8 spec), only this function changes.
pub fn wrap_in_figure(
    inner_html: &str,
    caption: Option<&str>,
    width: Option<&str>,
) -> String {
    // 3-arg shorthand kept for the raw-HTML media branch in
    // pipeline.rs::emit_standalone_figure_image (wikilink display-keyword
    // images that don't carry moss: title params — no align / extra
    // classes / extra attrs). Delegates to the canonical wrapper so the
    // byte shape stays defined in exactly one place.
    let empty_classes: &[String] = &[];
    let empty_attrs: BTreeMap<String, String> = BTreeMap::new();
    wrap_in_figure_full(inner_html, caption, width, None, empty_classes, &empty_attrs)
}

/// Canonical `<figure>`-wrapping function consumed by both `synthesize_image_html`
/// for `MarkdownStandalone` and the 3-arg compatibility shim `wrap_in_figure`.
///
/// Class list assembly: `class="moss-image{ align_class?}{ class_names…}"`.
/// Extra attrs render as `key="escaped_value"` in BTreeMap order, after
/// `data-width=` and before the inner content.
pub(super) fn wrap_in_figure_full(
    inner_html: &str,
    caption: Option<&str>,
    width: Option<&str>,
    align: Option<&str>,
    class_names: &[String],
    extra_attrs: &BTreeMap<String, String>,
) -> String {
    // `width` here is a closed-set &'static str from `match_width_token`
    // ("body" | "wide" | "page" | "screen"). The `html_escape` call is
    // defensive belt-and-braces — it never actually substitutes — and is
    // kept for symmetry with the embed-renderer side's `html_escape_attr`.
    let width_attr = width
        .map(|w| format!(r#" data-width="{}""#, html_escape(w)))
        .unwrap_or_default();

    // Compose the class attribute: `moss-image` first (the structural
    // hook), then the optional align class, then any author-supplied
    // class names. Single space separator keeps the byte shape stable
    // across the empty / align-only / class-only / both permutations.
    let mut class_value = String::from("moss-image");
    if let Some(a) = align {
        class_value.push(' ');
        class_value.push_str(a);
    }
    for cn in class_names {
        if cn.is_empty() {
            continue;
        }
        class_value.push(' ');
        class_value.push_str(cn);
    }
    let class_attr = format!(r#" class="{}""#, html_escape(&class_value));

    // Extra attrs are emitted in BTreeMap order (deterministic byte shape).
    let mut extra = String::new();
    for (k, v) in extra_attrs {
        extra.push(' ');
        extra.push_str(k);
        extra.push_str(r#"=""#);
        extra.push_str(&html_escape(v));
        extra.push('"');
    }

    match caption {
        Some(text) => format!(
            r#"<figure{class}{w}{extra}>{inner}<figcaption>{cap}</figcaption></figure>"#,
            class = class_attr,
            w = width_attr,
            extra = extra,
            inner = inner_html,
            cap = html_escape(text),
        ),
        None => format!(
            r#"<figure{class}{w}{extra}>{inner}</figure>"#,
            class = class_attr,
            w = width_attr,
            extra = extra,
            inner = inner_html,
        ),
    }
}

/// Synthesize the inner `<img>` (or `<picture><img></picture>`) without
/// the standalone-figure wrapper. Shared by every non-favicon context.
///
/// `sizes_value` is the context-resolved `sizes=` attribute value
/// (`contract::sizes`); it is only emitted when the source is wide enough
/// to have ladder rungs — narrow/unknown-dims sources keep the legacy
/// single-URL `<source>` byte shape.
fn synthesize_inner(
    src: &str,
    alt: &str,
    assets: &AssetSnapshot,
    options: &ImageRenderOptions<'_>,
    sizes_value: &str,
) -> String {
    // Phase B (Task 12): a webp SOURCE is already webp — to_webp(src) == src, so
    // a `<picture><source srcset=to_webp(src)>` would emit a `<source>` byte-
    // identical to the inner `<img>` (pointless). Instead, emit the responsive
    // ladder DIRECTLY on the `<img>` via `srcset`+`sizes`. This branch MUST run
    // BEFORE `is_raster_original` — which now also matches webp (webp joined
    // `is_ladder_source_ext` in Phase B) — so webp never falls into the
    // `<picture>` conversion path below.
    //
    // Animated webp gets NO ladder: `assets.is_animated(src)` (scan-derived,
    // Task 9/10) → empty ladder → the base `<img>` is byte-identical to today's
    // bare webp emission. Small / unknown-dims webp is likewise byte-identical.
    // This is the ONLY census site that passes a non-`false` animated flag; the
    // pipeline sites keep `false` (canonical rationale + the EXIF-orientation
    // caveat live on `asset_paths::ladder_rungs`). base_url == src: the served
    // base webp IS the source (`to_webp(src) == src`).
    if is_webp_source(src) {
        return match resolve_ladder(assets, src, lookup_animated(assets, src)) {
            None => render_img_tag(src, alt, assets, options, None),
            Some((rungs, base_w)) => {
                let srcset = build_srcset(src, src, rungs, base_w);
                render_img_tag(src, alt, assets, options, Some((&srcset, sizes_value)))
            }
        };
    }

    let img_tag = render_img_tag(src, alt, assets, options, None);

    // For raster originals, always emit <picture><source srcset=X.webp>.
    // This markup is MODE-INDEPENDENT — the on-disk HTML is identical in
    // preview and publish. The webp is encoded in the BACKGROUND for ALL modes
    // (blocking.rs registers the variant Pending; a BackgroundHandle worker
    // runs the encode). Two mechanisms keep the webp URL live without a 404:
    //   • Preview: the server serves the FULL ORIGINAL source bytes (source
    //     passthrough, preview/server/router.rs) for the not-yet-encoded
    //     variant URL, so the first paint is sharp; a Failed encode instead
    //     surfaces a warning SVG (preview/server/placeholder.rs).
    //   • Publish: the seal/persist task AWAITS the background drain barrier
    //     before sealing, so the sealed/deployed generation always contains the
    //     encoded .webp on disk (ADR-013 by construction).
    // So the URL is always live in both modes.
    //
    // We must never emit a <source> that might 404 because a chosen <source>
    // 404 is non-recoverable inside <picture> per HTML spec §
    // "update-the-source-set" + § "update-the-image-data": browser commits to
    // the chosen URL, fetch fails, image state goes to broken, error fires —
    // browser does NOT walk back to the inner <img>.
    //
    // For non-raster sources (svg, favicons via Favicon context), no variant
    // exists; emit the bare <img>.
    //
    // Pattern: explicit promise model. See
    // docs/plans/2026-05-20-image-variant-honest-mirror.md (Layer 3).
    if is_raster_original(src) {
        // to_webp(src) inherits the dir_overrides + relative-prefix already
        // applied to `src` by the upstream renderer. Swapping the extension
        // on `src` keeps the emitted URL aligned with the AssetRegistry's
        // registered key (blocking.rs's set_pending uses the same to_webp
        // derivation). It is the `<picture>` base descriptor URL here.
        let srcset_path = to_webp(src);
        // `false`: png/jpg/jpeg are never animated through this path (animated
        // gif/webp never reach `is_raster_original`; APNG is flattened by the
        // base+rung encodes alike). Canonical agreement rationale + the
        // EXIF-orientation caveat: `asset_paths::ladder_rungs` census doc.
        // `resolve_ladder` is `Some` only when rungs exist; unknown dims →
        // `None` → the legacy single-URL `<source>` shape.
        match resolve_ladder(assets, src, false) {
            None => format!(
                r#"<picture><source srcset="{}" type="image/webp">{}</picture>"#,
                html_escape(&srcset_path),
                img_tag,
            ),
            Some((rungs, base_w)) => {
                let srcset = build_srcset(src, &srcset_path, rungs, base_w);
                format!(
                    r#"<picture><source srcset="{}" type="image/webp" sizes="{}">{}</picture>"#,
                    html_escape(&srcset),
                    html_escape(sizes_value),
                    img_tag,
                )
            }
        }
    } else {
        img_tag
    }
}

/// Returns true when `src` is a raster original that always gets a webp
/// variant from moss's image pipeline. EMISSION side of the shared ladder
/// gate: extracts `src`'s extension and delegates membership to
/// [`is_ladder_source_ext`] — the ONE predicate the pipeline census sites
/// (registration/encode/sweep/heal) also consume.
///
/// After Phase B (Task 12) this also matches webp, so `synthesize_inner`
/// checks [`is_webp_source`] FIRST and routes webp to the `<img srcset>`
/// branch — a webp reaching THIS predicate's `<picture>` branch would emit a
/// useless `<source>` identical to the inner `<img>`. Reaching here therefore
/// means png/jpg/jpeg in practice (the webp branch already returned).
///
/// Note: this check is extension-only. `collect_images_for_conversion` applies
/// additional content-based filters (e.g. `SkipReason::NotAnImage` for files
/// whose magic bytes don't match the declared format). A file that passes
/// `is_raster_original` here but is filtered by `NotAnImage` will NOT have
/// `set_pending` called for it — the synthesizer will emit a `<picture>` but
/// the registry will serve the LQIP placeholder until the build completes
/// without a webp. In practice this only occurs for genuinely corrupt files
/// (e.g. an HTML 404 page saved as .png) that are not referenced from content.
fn is_raster_original(src: &str) -> bool {
    src.rsplit_once('.')
        .is_some_and(|(_, ext)| is_ladder_source_ext(ext))
}

/// Returns true when `src` is a webp SOURCE (extension `.webp`, case-
/// insensitive). EMISSION side of the webp-vs-conversion split: a webp source
/// carries the responsive ladder directly on `<img srcset>` (no `<picture>`),
/// so `synthesize_inner` tests this before [`is_raster_original`]. Delegates
/// to [`is_webp_source_ext`] — the single extension gate in `asset_paths`.
fn is_webp_source(src: &str) -> bool {
    src.rsplit_once('.')
        .is_some_and(|(_, ext)| is_webp_source_ext(ext))
}

/// Try several path normalizations against `AssetSnapshot.dimensions` so the
/// synthesizer matches the same set of input forms the prior
/// `MediaDimensionLookup::get` handled. `src` may arrive as the resolved URL
/// with a leading `/` (review colophon covers, cover.rs absolute paths) or
/// with a `./` / `../` relative prefix (CJK dir_overrides); scan stores keys
/// in plain relative form. The probe order mirrors the lookup's:
///
/// 1. exact match
/// 2. leading-`/` stripped (absolute-to-relative)
/// 3. leading `./` / `../` stripped (relative normalization)
///
/// Returns `None` when none of the variants is in the snapshot. The caller
/// supplies the fallback (800×600 for dims, no style for LQIP / color).
fn lookup_dims(assets: &AssetSnapshot, src: &str) -> Option<(u32, u32)> {
    probe_paths(src, |p| assets.dims(&p))
}

/// Whether `src` is a scan-flagged animated source. Probes the SAME path
/// normalizations as [`lookup_dims`] against `AssetSnapshot.animated` (keyed
/// identically to `dimensions`, both populated per-source in
/// `build_asset_snapshot`), so a webp found for dims is found for animation
/// too. Missing everywhere → `false` (test/fragment-render paths with an empty
/// snapshot treat sources as non-animated). Only the webp ladder branch
/// consults this — png/jpg/jpeg are never animated through the `<picture>`
/// path (see `asset_paths::ladder_rungs` census doc).
fn lookup_animated(assets: &AssetSnapshot, src: &str) -> bool {
    probe_paths(src, |p| assets.animated.get(&p).copied()).unwrap_or(false)
}

/// Resolve the responsive ladder for `src`: the rung widths (strictly below the
/// deployed base) and the deployed base WIDTH — or `None` when dims are unknown
/// (snapshot miss) or the ladder is empty (small/animated source). Shared by
/// BOTH emission paths (webp `<img srcset>` and png/jpg `<picture><source>`) so
/// the `lookup_dims → ladder_rungs → is_empty → deployed_width` derivation
/// CANNOT drift between them — and it must agree with registration/encode,
/// which call the identical `ladder_rungs`/`deployed_width` (see the
/// deterministic-agreement contract on [`crate::asset_paths::ladder_rungs`],
/// including its EXIF-orientation caveat).
fn resolve_ladder(
    assets: &AssetSnapshot,
    src: &str,
    is_animated: bool,
) -> Option<(&'static [u32], u32)> {
    lookup_dims(assets, src).and_then(|(w, h)| {
        let rungs = ladder_rungs(w, h, is_animated);
        if rungs.is_empty() {
            None
        } else {
            Some((rungs, deployed_width(w, h)))
        }
    })
}

/// Assemble a `srcset` value: one `to_webp_rung(src, w) {w}w` candidate per
/// rung, then the base descriptor `{base_url} {base_w}w`. Shared by BOTH
/// emission paths so the rung-URL derivation and descriptor shape cannot drift
/// between them (and must agree with what registration/encode name). The ONLY
/// difference is `base_url`: a webp SOURCE passes `src` itself (the served base
/// IS the source — `to_webp(src) == src`); a png/jpg/jpeg source passes
/// `to_webp(src)` (the converted `<picture>` base). Rung URLs derive from `src`
/// in both cases. Caller HTML-escapes the returned value.
fn build_srcset(src: &str, base_url: &str, rungs: &[u32], base_w: u32) -> String {
    let mut parts: Vec<String> = rungs
        .iter()
        .map(|w| format!("{} {}w", to_webp_rung(src, *w), w))
        .collect();
    parts.push(format!("{} {}w", base_url, base_w));
    parts.join(", ")
}

fn lookup_lqip<'a>(assets: &'a AssetSnapshot, src: &str) -> Option<&'a str> {
    probe_paths(src, |p| assets.lqip(&p))
}

fn lookup_color<'a>(assets: &'a AssetSnapshot, src: &str) -> Option<&'a String> {
    probe_paths(src, |p| assets.dominant_color.get(&p))
}

fn probe_paths<T>(src: &str, mut probe: impl FnMut(PathBuf) -> Option<T>) -> Option<T> {
    if let Some(v) = probe_normalized(src, &mut probe) {
        return Some(v);
    }
    // BUG 6.2 (belt-and-suspenders): body/wikilink images arrive percent-encoded
    // (`Europe%20-%20A%20Prophecy`), but snapshot keys are the RAW source path.
    // Decode `%XX` and re-probe so the encoded URL reverses to the source key.
    // Pure + zero-I/O; on invalid/lone `%` `percent_decode` returns the input
    // unchanged, so we only re-probe when decoding actually changed something.
    let decoded = percent_decode(src);
    if decoded != src {
        if let Some(v) = probe_normalized(&decoded, &mut probe) {
            return Some(v);
        }
    }
    None
}

/// Probe `src` plus its leading-`/` and leading-`./`/`../`-stripped forms.
fn probe_normalized<T>(src: &str, probe: &mut impl FnMut(PathBuf) -> Option<T>) -> Option<T> {
    if let Some(v) = probe(PathBuf::from(src)) {
        return Some(v);
    }
    let stripped = src.strip_prefix('/').unwrap_or(src);
    if stripped != src {
        if let Some(v) = probe(PathBuf::from(stripped)) {
            return Some(v);
        }
    }
    let mut s: &str = src;
    while let Some(rest) = s.strip_prefix("./").or_else(|| s.strip_prefix("../")) {
        s = rest;
    }
    if s != src {
        if let Some(v) = probe(PathBuf::from(s)) {
            return Some(v);
        }
    }
    None
}

/// Percent-decode `%XX` byte sequences in a URL path (pure, zero-I/O).
/// Mirrors `html_post::percent_decode_path` semantics: a lone/invalid `%` is
/// passed through verbatim, and non-UTF-8 decode results fall back to the raw
/// input. Returns the input unchanged when there is nothing to decode.
fn percent_decode(path: &str) -> String {
    if !path.contains('%') {
        return path.to_string();
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| path.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Emit just the `<img>` tag with all attributes. Internal helper for
/// `synthesize_image_html` — exposed as `pub(crate)` only for snapshot tests
/// that want to assert against the bare img output without the optional
/// `<picture>` wrapper.
///
/// `srcset_sizes` is `Some((srcset, sizes))` ONLY for the Phase-B webp ladder
/// (Task 12), which carries the responsive candidates on the `<img>` itself
/// rather than a `<source>`. When `Some`, ` srcset="…" sizes="…"` is emitted
/// immediately after `src=` (both values HTML-escaped, matching the
/// `<picture>` path's escaping). When `None` — every other caller and every
/// non-laddered webp — the output is BYTE-IDENTICAL to the pre-Task-12 shape.
pub(crate) fn render_img_tag(
    src: &str,
    alt: &str,
    assets: &AssetSnapshot,
    options: &ImageRenderOptions<'_>,
    srcset_sizes: Option<(&str, &str)>,
) -> String {
    // AssetSnapshot's `dims` is keyed by PathBuf; the src arrives as the
    // resolved URL the upstream renderer baked (potentially absolute, e.g.
    // `/image/cover.jpg`). Scan stores relative keys (`image/cover.jpg`),
    // so the synthesizer probes both forms via `lookup_dims`. Snapshot
    // lookups absent → fall back to the legacy 800×600 (matches the prior
    // `MediaDimensionLookup::get` semantics before the Phase 1 B1 migration).
    // Stem fallback (extension-mismatch — e.g. `.mov` vs `.mp4`) was
    // previously handled in MediaDimensionLookup::get; since AssetSnapshot
    // exposes only exact-path access, that case will need follow-up if
    // production sites rely on it (likely only for video posters, which
    // are out of this Tag::Image path).
    let (width, height) = lookup_dims(assets, src).unwrap_or((FALLBACK_WIDTH, FALLBACK_HEIGHT));

    let class_attr = options
        .class
        .map(|c| format!(r#" class="{}""#, html_escape(c)))
        .unwrap_or_default();

    let (loading_attr, fetchpriority_attr) = if options.eager {
        (r#" loading="eager""#, r#" fetchpriority="high""#)
    } else {
        (r#" loading="lazy""#, "")
    };

    // Suppress LQIP / dominant-color style when extra_attrs already carries
    // a style= attribute (e.g., `:::hero {attrs="cover-fit=contain"}` passes
    // `style="object-fit:contain"` through extra_attrs). The browser would
    // honor the LAST style= it sees and drop the LQIP, so emitting both
    // produces malformed HTML and loses the placeholder. The legacy regex
    // pass (`placeholder.rs:413-422`) had the same has_style guard; this
    // preserves parity. Future work: merge the two declarations into a
    // single style= via a typed `ImageRenderOptions::media_attrs` field so
    // the synthesizer owns escaping end-to-end (impl-review item 9).
    let extra_has_style = options
        .extra_attrs
        .map(|s| s.contains("style="))
        .unwrap_or(false);

    let style_attr = if extra_has_style {
        String::new()
    } else if let Some(lqip) = lookup_lqip(assets, src) {
        format!(
            r#" style="background-image:url({});background-size:cover""#,
            lqip
        )
    } else if let Some(color) = lookup_color(assets, src) {
        format!(r#" style="background-color:{}""#, color)
    } else {
        String::new()
    };

    let extra = options
        .extra_attrs
        .map(|s| format!(" {}", s))
        .unwrap_or_default();

    // Phase B webp ladder (Task 12): responsive candidates ride the `<img>`
    // itself. Emitted right after `src=` and before `width=`. Empty for every
    // other caller, keeping the byte shape identical to the pre-Task-12 tag.
    let srcset_attr = match srcset_sizes {
        Some((srcset, sizes)) => format!(
            r#" srcset="{}" sizes="{}""#,
            html_escape(srcset),
            html_escape(sizes),
        ),
        None => String::new(),
    };

    // `data-placeholder-src` removed 2026-05-20: the iframe-bridge handler
    // now matches by URL substring against `src` / `srcset` (see
    // frontend/bridge/iframe-bridge.ts, moss-asset-ready branch). The
    // AssetRegistry's promise model + the preview server's URL-keyed lookup
    // make the attribute redundant. See
    // docs/plans/2026-05-20-image-variant-honest-mirror.md (Layer 3).
    //
    // Inline LQIP via `background-image: url(data:image/jpeg;base64,…)` is
    // kept — legitimate production technique (cf. Vercel `blurDataURL`,
    // nextjs.org/docs/app/api-reference/components/image). Shows a blurred
    // preview instantly while the actual bytes are being decoded.
    format!(
        r#"<img{class_attr} src="{src_esc}"{srcset} width="{w}" height="{h}"{loading}{fetch}{style} alt="{alt}"{extra} />"#,
        class_attr = class_attr,
        src_esc = html_escape(src),
        srcset = srcset_attr,
        w = width,
        h = height,
        loading = loading_attr,
        fetch = fetchpriority_attr,
        style = style_attr,
        alt = html_escape(alt),
        extra = extra,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_snapshot::VariantKindSet;

    /// Build an AssetSnapshot with a single entry — the per-test fixture
    /// shape after the Phase 1 B1 migration (2026-05-25). Replaces the prior
    /// `MediaDimensionLookup`-based `lookup(vec![img_meta(...)])` builder.
    /// The snapshot keys are `PathBuf` per `AssetSnapshot`'s contract; the
    /// stem-derived `variants` entry mirrors what
    /// `AssetRegistry::iter_registered_variants` would populate when a WebP
    /// variant is registered.
    fn snapshot_with(
        path: &str,
        dims: Option<(u32, u32)>,
        color: Option<&str>,
        lqip: Option<&str>,
        webp: bool,
    ) -> AssetSnapshot {
        let mut s = AssetSnapshot::new();
        let key = PathBuf::from(path);
        if let Some(d) = dims {
            s.dimensions.insert(key.clone(), d);
        }
        if let Some(c) = color {
            s.dominant_color.insert(key.clone(), c.to_string());
        }
        if let Some(l) = lqip {
            s.lqip.insert(key.clone(), l.to_string());
        }
        if webp {
            let stem = crate::asset_snapshot::path_strip_extension(&key);
            s.variants.insert(
                stem,
                VariantKindSet {
                    webp: true,
                    avif: false,
                },
            );
        }
        s
    }

    fn snapshot_dims(path: &str, w: u32, h: u32) -> AssetSnapshot {
        snapshot_with(path, Some((w, h)), None, None, false)
    }

    // --- BUG 6: output-URL-form lookups must hit real dims, not 800x600 ---

    /// A body/wikilink image arrives percent-encoded (`Europe%20-%20A%20Prophecy`).
    /// `probe_paths` must percent-decode and reverse `../` so it hits the RAW
    /// source dims key, instead of missing → 800x600 fallback.
    #[test]
    fn probe_paths_percent_decodes_src() {
        let snap = snapshot_dims("assets/Europe - A Prophecy/e-006.jpg", 4515, 6158);
        assert_eq!(
            lookup_dims(&snap, "../../assets/Europe%20-%20A%20Prophecy/e-006.jpg"),
            Some((4515, 6158))
        );
    }

    /// A cover arrives as a slugified output URL (`/assets/europe-a-prophecy/...`).
    /// With the snapshot additively indexed under the slug key (Bug6.1), the
    /// synthesized `<img>` must carry the real portrait dims, NOT the fallback.
    #[test]
    fn cover_slug_url_emits_real_dimensions_not_fallback() {
        let mut snap = snapshot_dims("assets/Europe - A Prophecy/e-006.jpg", 4515, 6158);
        snap.dimensions.insert(
            PathBuf::from("assets/europe-a-prophecy/e-006.jpg"),
            (4515, 6158),
        );
        let html = synthesize_image_html(
            "/assets/europe-a-prophecy/e-006.jpg",
            "cover",
            &snap,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"width="4515" height="6158""#),
            "expected real dims, got: {html}"
        );
        assert!(
            !html.contains(r#"width="800" height="600""#),
            "800x600 fallback fired: {html}"
        );
    }

    // --- <picture>-wrapped shape (raster originals always wrapped 2026-05-20) ---

    #[test]
    fn picture_wrap_for_raster_original_no_lqip() {
        // After 2026-05-20: synthesizer always emits <picture> for png/jpg/jpeg
        // originals. The preview server's AssetRegistry intercept ensures the
        // webp URL resolves (LQIP bytes for Pending, real bytes for Ready);
        // publish-mode synchronous encoding ensures it never 404s in
        // production. data-placeholder-src is gone — iframe-bridge matches
        // by URL substring now.
        let s = snapshot_dims("photo.jpg", 800, 600);
        let html = synthesize_image_html(
            "photo.jpg",
            "A cat",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<picture><source srcset="photo.webp" type="image/webp"><img src="photo.jpg" width="800" height="600" loading="lazy" alt="A cat" /></picture>"#
        );
    }

    #[test]
    fn picture_wrap_for_raster_original_with_lqip() {
        // LQIP inline style is preserved (legitimate production technique;
        // cf. Vercel `blurDataURL`). Shown to the user instantly while the
        // actual bytes are being decoded.
        let s = snapshot_with(
            "photo.jpg",
            Some((800, 600)),
            None,
            Some("data:image/jpeg;base64,abc"),
            false,
        );
        let html = synthesize_image_html(
            "photo.jpg",
            "A cat",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<picture><source srcset="photo.webp" type="image/webp"><img src="photo.jpg" width="800" height="600" loading="lazy" style="background-image:url(data:image/jpeg;base64,abc);background-size:cover" alt="A cat" /></picture>"#
        );
    }

    #[test]
    fn bare_img_with_dominant_color_no_lqip() {
        let s = snapshot_with(
            "photo.jpg",
            Some((800, 600)),
            Some("#aabbcc"),
            None,
            false,
        );
        let html = synthesize_image_html(
            "photo.jpg",
            "Cat",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"style="background-color:#aabbcc""#),
            "Got: {html}"
        );
    }

    // --- <picture> wrap (WebP variant present) ----------------------------

    #[test]
    fn picture_wrap_when_webp_exists() {
        let s = snapshot_with("photo.jpg", Some((800, 600)), None, None, true);
        let html = synthesize_image_html(
            "photo.jpg",
            "Cat",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        // Outer: <picture>...</picture> with <source> first
        assert!(
            html.starts_with(r#"<picture><source srcset="photo.webp" type="image/webp"><img"#),
            "Got: {html}"
        );
        assert!(html.ends_with(r#"alt="Cat" /></picture>"#), "Got: {html}");
    }

    #[test]
    fn picture_srcset_uses_to_webp_of_src_not_stored_variant() {
        // CJK-path case: <img src> already carries dir_overrides + relative
        // prefix (e.g. ../assets/photo.jpg). srcset must derive from src
        // (../assets/photo.webp), not the manifest's stored value.
        // See wrap_img_in_picture's rationale at placeholder.rs:660.
        let s = snapshot_with("../assets/photo.jpg", Some((800, 600)), None, None, true);
        let html = synthesize_image_html(
            "../assets/photo.jpg",
            "",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"srcset="../assets/photo.webp""#),
            "srcset should inherit src's prefix; got: {html}"
        );
    }

    // --- eager loading ----------------------------------------------------

    #[test]
    fn eager_swaps_loading_attr_and_adds_fetchpriority() {
        let s = snapshot_dims("photo.jpg", 1, 1);
        let html = synthesize_image_html(
            "photo.jpg",
            "Hero",
            &s,
            ImageContext::Hero,
            &ImageRenderOptions {
                eager: true,
                ..Default::default()
            },
        );
        assert!(
            html.contains(r#"loading="eager" fetchpriority="high""#),
            "Got: {html}"
        );
        assert!(!html.contains(r#"loading="lazy""#), "Got: {html}");
    }

    // --- favicon short-circuit --------------------------------------------

    #[test]
    fn favicon_is_bare_16x16_with_class() {
        // No manifest data for the favicon URL — synthesizer must NOT probe
        // the snapshot (favicons are not registered as moss assets).
        let s = AssetSnapshot::new();
        let html = synthesize_image_html(
            "https://example.com/favicon.ico",
            "",
            &s,
            ImageContext::Favicon,
            &ImageRenderOptions {
                class: Some("link-preview-favicon"),
                ..Default::default()
            },
        );
        assert_eq!(
            html,
            r#"<img class="link-preview-favicon" src="https://example.com/favicon.ico" width="16" height="16" alt="">"#
        );
    }

    // --- site-logo short-circuit (Phase 2B carve-out) --------------------

    #[test]
    fn synthesize_site_logo_basic_shape() {
        let html = synthesize_image_html(
            "assets/logo.png",
            "",
            &AssetSnapshot::new(),
            ImageContext::SiteLogo,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<img class="site-logo" src="assets/logo.png" alt="" aria-hidden="true">"#
        );
    }

    #[test]
    fn synthesize_site_logo_escapes_src() {
        let html = synthesize_image_html(
            r#"logo "with quotes".png"#,
            "",
            &AssetSnapshot::new(),
            ImageContext::SiteLogo,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"logo &quot;with quotes&quot;.png"#),
            "got: {html}"
        );
    }

    #[test]
    fn synthesize_site_logo_does_not_emit_picture_or_lqip() {
        // Even when the snapshot would normally drive LQIP/<picture> for a
        // png, SiteLogo must short-circuit and emit a bare <img>.
        let mut snap = AssetSnapshot::new();
        snap.lqip
            .insert("logo.png".into(), "data:image/jpeg;base64,xxx".into());
        let html = synthesize_image_html(
            "logo.png",
            "moss",
            &snap,
            ImageContext::SiteLogo,
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("<picture"), "logo should not get picture wrap: {html}");
        assert!(
            !html.contains("background-image"),
            "logo should not get LQIP: {html}"
        );
        assert!(
            !html.contains("loading="),
            "logo above-the-fold, no lazy-load: {html}"
        );
    }

    // --- HTML-escape contract --------------------------------------------

    #[test]
    fn alt_with_quotes_is_escaped() {
        let s = snapshot_dims("photo.jpg", 1, 1);
        let html = synthesize_image_html(
            "photo.jpg",
            r#"Quote: "hi""#,
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        // moss_core::media::html_escape escapes " → &quot; — never leave a
        // raw quote inside an attribute value.
        assert!(html.contains(r#"alt="Quote: &quot;hi&quot;""#), "Got: {html}");
    }

    #[test]
    fn extra_attrs_passed_through_verbatim() {
        // :::hero passes `style="object-fit:cover;object-position:50% 50%"`
        // via MediaAttrs::to_inline_style — the caller pre-escapes, so we
        // just append.
        let s = snapshot_dims("hero.jpg", 1920, 1080);
        let html = synthesize_image_html(
            "hero.jpg",
            "",
            &s,
            ImageContext::Hero,
            &ImageRenderOptions {
                eager: true,
                extra_attrs: Some(r#"data-cover-fit="cover""#),
                ..Default::default()
            },
        );
        assert!(html.contains(r#"data-cover-fit="cover""#), "Got: {html}");
        // Extra attrs come AFTER alt — matches the current regex-pass order
        // where the regex preserves the original post-src attributes.
        let alt_pos = html.find("alt=").unwrap();
        let extra_pos = html.find("data-cover-fit=").unwrap();
        assert!(extra_pos > alt_pos, "extra attrs should come after alt");
    }

    /// Regression test for impl-review finding 2026-05-16: when extra_attrs
    /// already carries a `style=` attribute (the `:::hero {attrs=...}` case
    /// when MediaAttrs::to_inline_style() returns Some), the synthesizer
    /// must NOT also emit its own LQIP-derived `style=`. The legacy regex
    /// pass had a `has_style` guard at `placeholder.rs:413-422` that did the
    /// same suppression. Without this, browsers see two `style=` attributes
    /// on one element, honor the last one, and drop the LQIP placeholder.
    #[test]
    fn lqip_style_suppressed_when_extra_attrs_has_style() {
        let s = snapshot_with(
            "hero.jpg",
            Some((1920, 1080)),
            None,
            Some("data:image/jpeg;base64,abc"),
            false,
        );
        let html = synthesize_image_html(
            "hero.jpg",
            "",
            &s,
            ImageContext::Hero,
            &ImageRenderOptions {
                eager: true,
                extra_attrs: Some(r#"style="object-fit:cover;object-position:50% 50%""#),
                ..Default::default()
            },
        );
        // Exactly one `style=` substring — the one the caller passed.
        assert_eq!(
            html.matches("style=").count(),
            1,
            "expected exactly one style= attribute; got: {html}"
        );
        // Confirm the caller's style is what survived (not the LQIP).
        assert!(
            html.contains(r#"style="object-fit:cover"#),
            "caller-supplied style must survive; got: {html}"
        );
        assert!(
            !html.contains(r#"background-image:url(data:"#),
            "LQIP must be suppressed when extra_attrs carries style=; got: {html}"
        );
    }

    // --- Step 8: figure wrapper for MarkdownStandalone --------------------

    /// Phase 1 C1 test helper: build an empty extras BTreeMap.
    fn empty_extras() -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }

    #[test]
    fn markdown_standalone_no_caption_wraps_in_figure() {
        // No caption → `<figure class="moss-image">…</figure>` around the
        // synthesized `<picture>`/`<img>` with no `<figcaption>`.
        let s = snapshot_dims("photo.jpg", 800, 600);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "Alt text",
            &s,
            ImageContext::MarkdownStandalone {
                caption: None,
                width: None,
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(html.starts_with(r#"<figure class="moss-image">"#));
        assert!(html.ends_with("</figure>"));
        assert!(!html.contains("<figcaption>"));
        assert!(html.contains("<img"));
    }

    #[test]
    fn markdown_standalone_with_caption_adds_figcaption() {
        let s = snapshot_dims("photo.jpg", 800, 600);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "Alt text",
            &s,
            ImageContext::MarkdownStandalone {
                caption: Some("A nice photo"),
                width: None,
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(html.starts_with(r#"<figure class="moss-image">"#));
        assert!(html.contains("<figcaption>A nice photo</figcaption>"));
        assert!(html.ends_with("</figure>"));
    }

    #[test]
    fn markdown_standalone_caption_is_html_escaped() {
        let s = snapshot_dims("photo.jpg", 800, 600);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &s,
            ImageContext::MarkdownStandalone {
                caption: Some(r#"Q&A "best" of <em>2024</em>"#),
                width: None,
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains("<figcaption>Q&amp;A &quot;best&quot; of &lt;em&gt;2024&lt;/em&gt;</figcaption>"),
            "caption text must be HTML-escaped at the boundary; got: {html}"
        );
    }

    #[test]
    fn markdown_standalone_wraps_picture_when_webp_present() {
        // When the manifest carries a WebP variant, the synthesizer emits
        // a `<picture>` wrap inside the figure: the structural figure
        // and the responsive picture compose without conflict.
        let s = snapshot_with("photo.jpg", Some((1200, 800)), None, None, true);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "Alt",
            &s,
            ImageContext::MarkdownStandalone {
                caption: Some("Cap"),
                width: None,
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        // Structural order: figure > picture > source + img > figcaption
        let fig_idx = html.find(r#"<figure class="moss-image">"#).expect("figure");
        let pic_idx = html.find("<picture>").expect("picture");
        let src_idx = html.find("<source").expect("source");
        let img_idx = html.find("<img").expect("img");
        let cap_idx = html.find("<figcaption>").expect("figcaption");
        let fig_close = html.find("</figure>").expect("figure close");
        assert!(fig_idx < pic_idx, "<picture> must be inside <figure>");
        assert!(pic_idx < src_idx);
        assert!(src_idx < img_idx);
        assert!(img_idx < cap_idx, "<figcaption> follows <picture>");
        assert!(cap_idx < fig_close);
    }

    #[test]
    fn markdown_inline_does_not_wrap_in_figure() {
        // Inline images NEVER get a figure wrapper — they sit in prose.
        let s = snapshot_dims("photo.jpg", 800, 600);
        let html = synthesize_image_html(
            "photo.jpg",
            "Alt",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("<figure"));
        assert!(!html.contains("<figcaption>"));
    }

    // --- spec § P9 width: `data-width` on the figure wrapper -------------

    #[test]
    fn markdown_standalone_width_screen_emits_data_width_on_figure() {
        // Width pipe-alias `![[photo.jpg|full]]` → `screen` lands on the
        // figure wrapper, not the inner img. The image-side test below
        // pins the "absent by default" half of the contract.
        let s = snapshot_dims("photo.jpg", 800, 600);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &s,
            ImageContext::MarkdownStandalone {
                caption: None,
                width: Some("screen"),
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(
            html.starts_with(r#"<figure class="moss-image" data-width="screen">"#),
            "data-width must sit on the figure wrapper; got: {html}"
        );
        // The inner img must NOT carry data-width — the attribute is the
        // wrapper's responsibility per spec.
        assert!(
            !html.contains(r#"<img"#) || !html[html.find("<img").unwrap()..].contains("data-width="),
            "inner <img> must not carry data-width; got: {html}"
        );
    }

    #[test]
    fn markdown_standalone_width_wide_with_caption() {
        // width + caption compose: both attributes / children appear in
        // the wrapper.
        let s = snapshot_dims("photo.jpg", 800, 600);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "Alt",
            &s,
            ImageContext::MarkdownStandalone {
                caption: Some("A nice photo"),
                width: Some("wide"),
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(html.contains(r#"data-width="wide""#), "got: {html}");
        assert!(
            html.contains("<figcaption>A nice photo</figcaption>"),
            "got: {html}"
        );
    }

    #[test]
    fn markdown_standalone_width_none_omits_data_width() {
        // Negative test: the default (no width) must not emit the attribute,
        // so theme authors can target the absence via `:not([data-width])`.
        let s = snapshot_dims("photo.jpg", 800, 600);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &s,
            ImageContext::MarkdownStandalone {
                caption: None,
                width: None,
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("data-width="), "got: {html}");
    }

    #[test]
    fn wrap_in_figure_width_emits_data_width_attribute() {
        // Direct-call contract test: `wrap_in_figure` is the single source
        // of truth for the figure wrapper byte shape; the raw-HTML branch
        // of `emit_standalone_figure_image` calls it with the lifted width.
        let html = wrap_in_figure(r#"<img src="x" />"#, None, Some("page"));
        assert_eq!(
            html,
            r#"<figure class="moss-image" data-width="page"><img src="x" /></figure>"#
        );
    }

    #[test]
    fn wrap_in_figure_width_with_caption() {
        let html = wrap_in_figure(r#"<img src="x" />"#, Some("hello"), Some("screen"));
        assert_eq!(
            html,
            r#"<figure class="moss-image" data-width="screen"><img src="x" /><figcaption>hello</figcaption></figure>"#
        );
    }

    #[test]
    fn wrap_in_figure_no_width_no_attribute() {
        let html = wrap_in_figure(r#"<img src="x" />"#, None, None);
        assert_eq!(
            html,
            r#"<figure class="moss-image"><img src="x" /></figure>"#
        );
    }

    #[test]
    fn markdown_standalone_no_manifest_still_wraps_in_figure() {
        // The wrapper is structural identity (Step 8 contract), not
        // manifest-dependent. Even with an empty AssetSnapshot (test/
        // fragment-render path), `<figure class="moss-image">` still wraps
        // the synthesized `<img>`. The Phase 1 B1 migration (2026-05-25)
        // replaced the `Option<&MediaDimensionLookup>` parameter with
        // `&AssetSnapshot`; the empty-snapshot path is now the test
        // equivalent of the prior `None` lookup.
        let s = AssetSnapshot::new();
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "Alt",
            &s,
            ImageContext::MarkdownStandalone {
                caption: Some("Cap"),
                width: None,
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(html.starts_with(r#"<figure class="moss-image">"#));
        assert!(html.contains("<img"));
        assert!(html.contains("<figcaption>Cap</figcaption>"));
        assert!(html.ends_with("</figure>"));
    }

    // --- size fallback ----------------------------------------------------

    #[test]
    fn missing_dimensions_fall_back_to_800x600() {
        let s = AssetSnapshot::new();
        let html = synthesize_image_html(
            "ghost.jpg",
            "",
            &s,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        // FALLBACK_WIDTH / FALLBACK_HEIGHT (still 800×600, sourced from
        // `moss_core::asset_snapshot::FALLBACK_WIDTH` so the synthesizer and
        // the surviving regex pass agree on the absent-dims default).
        assert!(html.contains(r#"width="800" height="600""#), "Got: {html}");
    }

    // --- regex-pass idempotency on synthesizer output ----------------------
    //
    // Phase 2E v5 PR5 (2026-05-26) retired the Stage 3 regex post-pass; the
    // image synthesizer in this module is now the sole emitter of width /
    // height / loading / LQIP / dominant-color attributes for moss-emitted
    // <img> tags. The three idempotency tests at
    // `src-tauri/tests/image_synth_regex_parity.rs` that guarded the
    // regex+synth byte-shape parity were deleted alongside the regex.

    // --- TrackingPixel (Phase 2C, 2026-05-25) ---
    //
    // The RSS read-tracking pixel is a 1×1 invisible <img>. It must fire on
    // read (so NO loading="lazy"), carry empty alt (decorative), and never
    // be wrapped in <picture> / decorated with LQIP. Self-closing form
    // because the call site embeds it in CDATA-wrapped RSS XML.

    #[test]
    fn synthesize_tracking_pixel_basic() {
        let html = synthesize_image_html(
            "https://api.mosspub.com/pixel.gif?u=abc",
            "",
            &AssetSnapshot::new(),
            ImageContext::TrackingPixel,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<img src="https://api.mosspub.com/pixel.gif?u=abc" alt="" width="1" height="1" />"#
        );
    }

    #[test]
    fn synthesize_tracking_pixel_escapes_url() {
        let html = synthesize_image_html(
            r#"x.gif?u=a&b="c""#,
            "",
            &AssetSnapshot::new(),
            ImageContext::TrackingPixel,
            &ImageRenderOptions::default(),
        );
        assert!(html.contains("&amp;"), "& must be escaped");
        assert!(html.contains("&quot;"), "\" must be escaped");
    }

    #[test]
    fn synthesize_tracking_pixel_no_lazy_no_lqip() {
        // Even when the snapshot contains LQIP / dimensions for the pixel
        // path, the TrackingPixel short-circuit must ignore them — pixels
        // are tracking beacons, not images.
        let mut snap = AssetSnapshot::new();
        snap.lqip.insert(
            "pixel.gif".into(),
            "data:image/jpeg;base64,xxx".into(),
        );
        let html = synthesize_image_html(
            "pixel.gif",
            "",
            &snap,
            ImageContext::TrackingPixel,
            &ImageRenderOptions::default(),
        );
        assert!(
            !html.contains("loading=\"lazy\""),
            "must NOT lazy-load (must fire on read)"
        );
        assert!(
            !html.contains("background-image"),
            "must NOT carry LQIP"
        );
        assert!(
            !html.contains("<picture"),
            "must NOT wrap in picture"
        );
    }

    // --- ImageContext::EmailBody (Phase 2D, 2026-05-25) -------------------
    //
    // Email-client-safe carve-out: no <picture>, no data-*, no loading=lazy.
    // Inline `style="display:block;max-width:100%;height:auto;"` is the
    // cross-client responsive pattern. width/height attrs are Option<u32>:
    // emitted when known, omitted when None.

    #[test]
    fn synthesize_email_body_with_dims() {
        let html = synthesize_image_html(
            "https://media.example.com/photo.jpg",
            "Cover",
            &AssetSnapshot::new(),
            ImageContext::EmailBody {
                width: Some(600),
                height: Some(400),
            },
            &ImageRenderOptions::default(),
        );
        assert!(html.contains(r#"width="600""#));
        assert!(html.contains(r#"height="400""#));
        assert!(html.contains(r#"style="display:block;max-width:100%;height:auto;""#));
    }

    #[test]
    fn synthesize_email_body_without_dims() {
        let html = synthesize_image_html(
            "x.jpg",
            "alt",
            &AssetSnapshot::new(),
            ImageContext::EmailBody {
                width: None,
                height: None,
            },
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("width="), "width should be omitted when None");
        assert!(!html.contains("height="), "height should be omitted when None");
    }

    #[test]
    fn synthesize_email_body_no_picture_no_data() {
        let html = synthesize_image_html(
            "photo.jpg",
            "alt",
            &AssetSnapshot::new(),
            ImageContext::EmailBody {
                width: None,
                height: None,
            },
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("<picture"), "email images must not use <picture>");
        assert!(!html.contains("<source"), "no <source>");
        assert!(!html.contains("data-"), "no data-* (email clients strip)");
        assert!(!html.contains("loading="), "email clients ignore loading attr");
        assert!(
            !html.contains("background-image"),
            "email clients strip inline style URLs"
        );
    }

    #[test]
    fn synthesize_email_body_escapes() {
        let html = synthesize_image_html(
            r#"https://x.com/photo.jpg?a=1&b=2"#,
            r#"alt with "quotes""#,
            &AssetSnapshot::new(),
            ImageContext::EmailBody {
                width: None,
                height: None,
            },
            &ImageRenderOptions::default(),
        );
        assert!(html.contains("&amp;"), "& must be escaped");
        assert!(html.contains("&quot;"), "\" must be escaped");
    }

    // --- ImageContext::GalleryThumb (Phase 2E v5 PR3, 2026-05-26) ---------
    //
    // Gallery body images: below-the-fold thumbnail, same inner byte
    // shape as MarkdownInline (`<picture><source srcset=*.webp><img
    // loading="lazy" ...></picture>` for raster, bare `<img>` for
    // non-raster). The outer `.moss-gallery-item` wrapper is owned by
    // `DefaultHooks::render_shortcode`'s Gallery arm; this variant
    // emits only the inner image. Distinguishing it from
    // MarkdownInline at the type level keeps per-item passthrough
    // attributes (object-position from MediaAttrs) typed for future
    // evolution.

    #[test]
    fn synthesize_gallery_thumb_emits_picture_with_lazy() {
        let p = ImageRenderOptions::default();
        let mut snap = AssetSnapshot::new();
        snap.dimensions
            .insert(PathBuf::from("photo.jpg"), (1200, 800));
        let out = synthesize_image_html(
            "photo.jpg",
            "alt",
            &snap,
            ImageContext::GalleryThumb,
            &p,
        );
        assert!(out.contains("<picture"), "{out}");
        // 1200px-wide fixture → ladder rung at 800 + capped base descriptor,
        // with the gallery grid-cell sizes (Task 3, responsive-image-variants).
        assert!(
            out.contains(
                r#"srcset="photo.w800.webp 800w, photo.webp 1200w" type="image/webp" sizes="(min-width: 48rem) 33vw, 100vw""#
            ),
            "{out}"
        );
        assert!(out.contains(r#"loading="lazy""#), "{out}");
        assert!(out.contains(r#"width="1200""#), "{out}");
        assert!(out.contains(r#"height="800""#), "{out}");
        assert!(out.contains(r#"alt="alt""#), "{out}");
    }

    #[test]
    fn synthesize_gallery_thumb_non_raster_bare_img() {
        // SVG / .webp originals don't trigger the <picture> wrap (no
        // variant exists). Falls back to bare <img> with lazy loading +
        // dims from the snapshot.
        let p = ImageRenderOptions::default();
        let mut snap = AssetSnapshot::new();
        snap.dimensions
            .insert(PathBuf::from("icon.svg"), (64, 64));
        let out = synthesize_image_html(
            "icon.svg",
            "",
            &snap,
            ImageContext::GalleryThumb,
            &p,
        );
        assert!(!out.contains("<picture"), "{out}");
        assert!(!out.contains("<source"), "{out}");
        assert!(out.contains(r#"loading="lazy""#), "{out}");
        assert!(out.contains(r#"width="64""#), "{out}");
    }

    #[test]
    fn synthesize_gallery_thumb_threads_extra_attrs() {
        // The Gallery hook builds a `style="object-position:..."` fragment
        // from MediaAttrs and passes it via extra_attrs. The synthesizer
        // suppresses its own LQIP-derived style= when extra_attrs already
        // carries one — verify that suppression engages here too
        // (parity with Hero / MarkdownInline).
        let snap = snapshot_with(
            "photo.jpg",
            Some((1200, 800)),
            None,
            Some("data:image/jpeg;base64,abc"),
            false,
        );
        let opts = ImageRenderOptions {
            extra_attrs: Some(r#"style="object-position:50% 50%""#),
            ..Default::default()
        };
        let out = synthesize_image_html(
            "photo.jpg",
            "",
            &snap,
            ImageContext::GalleryThumb,
            &opts,
        );
        assert_eq!(
            out.matches("style=").count(),
            1,
            "expected exactly one style= attribute; got: {out}"
        );
        assert!(
            out.contains(r#"style="object-position:50% 50%""#),
            "caller-supplied style must survive; got: {out}"
        );
    }

    // --- ImageContext::HeroBare (Phase 2E PR2, 2026-05-26) ----------------
    //
    // The no-snapshot hero fallback. Emits a bare `<img>` with no
    // `<picture>`, no `<source>`, no LQIP, no dims, no `loading` attr.
    // The asset-publish invariant rules out emitting a
    // `<source srcset="*.webp">` for an unregistered variant — this
    // variant is the explicit opt-out for code paths that run before
    // `AssetRegistry::set_pending` has been called for the source's
    // `.webp` companion (test/fragment-render paths). The byte shape
    // mirrors the pre-PR2 fallback at
    // `typed_renderers.rs::render_hero_html_typed` lines 554-557.

    #[test]
    fn synthesize_hero_bare_basic_shape() {
        let out = synthesize_image_html(
            "cover.jpg",
            "",
            &AssetSnapshot::new(),
            ImageContext::HeroBare,
            &ImageRenderOptions::default(),
        );
        // No <picture>, no <source>, no class, no loading, no LQIP, no
        // width/height attrs. Exact byte shape with empty alt.
        assert_eq!(out, r#"<img src="cover.jpg" alt="" />"#, "got: {}", out);
    }

    // --- srcset ladder + sizes (responsive-image-variants Task 3) ---------
    //
    // Sources wider than the first ladder rung (800px) gain width
    // descriptors for each rung below the deployed base plus the base
    // itself (capped at DEPLOY_MAX_EDGE), and a per-context sizes=
    // attribute from contract::sizes. Sources at/below the first rung —
    // and unknown-dims sources — keep the legacy single-URL shape
    // byte-identical (no descriptors, no sizes).

    #[test]
    fn ladder_srcset_emitted_for_wide_raster() {
        let assets = snapshot_dims("photo.jpg", 2000, 1200);
        let html = synthesize_image_html(
            "photo.jpg",
            "alt",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"<source srcset="photo.w800.webp 800w, photo.w1600.webp 1600w, photo.webp 2000w" type="image/webp" sizes="(min-width: 48rem) 47.25rem, 100vw">"#
            ),
            "got: {html}"
        );
    }

    #[test]
    fn base_descriptor_caps_at_deploy_max_edge() {
        let assets = snapshot_dims("photo.jpg", 4000, 3000);
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(html.contains("photo.webp 2400w"), "got: {html}");
    }

    #[test]
    fn no_ladder_below_first_rung_keeps_legacy_shape() {
        let assets = snapshot_dims("photo.jpg", 800, 600);
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        // Byte-identical to today's single-URL source: no descriptors, no sizes.
        assert!(
            html.contains(r#"<source srcset="photo.webp" type="image/webp">"#),
            "got: {html}"
        );
        assert!(!html.contains("sizes="), "got: {html}");
    }

    #[test]
    fn unknown_dims_keep_legacy_shape() {
        // Empty snapshot → fallback 800×600 → no rungs → legacy single-URL shape.
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &AssetSnapshot::new(),
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"<source srcset="photo.webp" type="image/webp">"#),
            "got: {html}"
        );
        assert!(!html.contains("sizes="), "got: {html}");
    }

    #[test]
    fn portrait_below_first_rung_keeps_legacy_shape() {
        // 1200×3600 portrait: the encoder caps the longest EDGE, so the
        // deployed base is only 800 wide — no rung below it (strict `<`),
        // legacy single-URL shape.
        let assets = snapshot_dims("photo.jpg", 1200, 3600);
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"<source srcset="photo.webp" type="image/webp">"#),
            "got: {html}"
        );
        assert!(!html.contains("sizes="), "got: {html}");
    }

    #[test]
    fn portrait_base_descriptor_uses_post_resize_width() {
        // 3024×4032 portrait deploys at 1800×2400 — the base descriptor
        // must be the POST-RESIZE width (1800w), never min(w, 2400).
        let assets = snapshot_dims("photo.jpg", 3024, 4032);
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"srcset="photo.w800.webp 800w, photo.w1600.webp 1600w, photo.webp 1800w""#
            ),
            "got: {html}"
        );
    }

    #[test]
    fn folder_card_cover_uses_card_sizes() {
        let assets = snapshot_dims("cover.jpg", 2000, 1200);
        let html = synthesize_image_html(
            "cover.jpg",
            "",
            &assets,
            ImageContext::FolderCardCover,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"sizes="(min-width: 48rem) 24rem, 100vw""#),
            "got: {html}"
        );
    }

    #[test]
    fn percent_encoded_src_gets_encoded_rung_urls() {
        // Body/wikilink srcs arrive percent-encoded while snapshot keys are
        // the RAW source path: the dims probe decodes (BUG 6.2), but the
        // emitted rung URLs stay in the src's encoded URL space — same
        // derivation rule as the base to_webp(src).
        let assets = snapshot_dims("a b/photo.jpg", 2000, 1200);
        let html = synthesize_image_html(
            "a%20b/photo.jpg",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"srcset="a%20b/photo.w800.webp 800w, a%20b/photo.w1600.webp 1600w, a%20b/photo.webp 2000w""#
            ),
            "got: {html}"
        );
    }

    #[test]
    fn hero_context_uses_full_bleed_sizes() {
        let assets = snapshot_dims("hero.jpg", 2400, 985);
        let html = synthesize_image_html(
            "hero.jpg",
            "",
            &assets,
            ImageContext::Hero,
            &ImageRenderOptions::default(),
        );
        assert!(html.contains(r#"sizes="100vw""#), "got: {html}");
    }

    #[test]
    fn markdown_standalone_width_wide_uses_body_sizes() {
        // Task-3 review decision: wide/page map to SIZES_BODY, NOT full
        // bleed — today's site.css has no width rule for data-width
        // (ADR-021 follow-up), so a wide figure renders at the content
        // column and 100vw would over-fetch. When ADR-021 lands, this
        // mapping (and test) flips to SIZES_FULL_BLEED.
        let s = snapshot_dims("photo.jpg", 2000, 1200);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &s,
            ImageContext::MarkdownStandalone {
                caption: None,
                width: Some("wide"),
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(
            html.starts_with(r#"<figure class="moss-image" data-width="wide">"#),
            "got: {html}"
        );
        assert!(
            html.contains(r#"sizes="(min-width: 48rem) 47.25rem, 100vw""#),
            "wide maps to SIZES_BODY until ADR-021; got: {html}"
        );
    }

    #[test]
    fn markdown_standalone_width_screen_uses_full_bleed_sizes() {
        // Honest framing: today's site.css has NO data-width width rule for
        // ANY value (site.css:2346-2348 — ADR-021 follow-up), so
        // screen/full → 100vw is a forward-compat plan-owner call, not a
        // reflection of current CSS. Over-fetch is the safe direction for
        // an intended-full-bleed figure (never blurry, at worst wasted
        // bytes). Mirrors markdown_standalone_width_screen_emits_data_width
        // _on_figure but with ladder-triggering dims.
        let s = snapshot_dims("photo.jpg", 2000, 1200);
        let extras = empty_extras();
        let html = synthesize_image_html(
            "photo.jpg",
            "",
            &s,
            ImageContext::MarkdownStandalone {
                caption: None,
                width: Some("screen"),
                align: None,
                class_names: &[],
                extra_attrs: &extras,
            },
            &ImageRenderOptions::default(),
        );
        assert!(
            html.starts_with(r#"<figure class="moss-image" data-width="screen">"#),
            "got: {html}"
        );
        assert!(html.contains(r#"sizes="100vw""#), "got: {html}");
    }

    #[test]
    fn email_body_output_unchanged_by_ladder() {
        let assets = snapshot_dims("photo.jpg", 2000, 1200);
        let html = synthesize_image_html(
            "photo.jpg",
            "a",
            &assets,
            ImageContext::EmailBody {
                width: Some(2000),
                height: Some(1200),
            },
            &ImageRenderOptions::default(),
        );
        assert!(
            !html.contains("srcset"),
            "email must never carry srcset: {html}"
        );
        assert!(!html.contains("<picture"), "got: {html}");
    }

    #[test]
    fn synthesize_hero_bare_with_style_via_extra_attrs() {
        // The legacy fallback passed an inline `style="..."` fragment
        // built from MediaAttrs::to_inline_style(). PR2 threads that
        // fragment through `ImageRenderOptions::extra_attrs` — the
        // synthesizer prepends a single space, matching the legacy byte
        // shape.
        let out = synthesize_image_html(
            "cover.jpg",
            "",
            &AssetSnapshot::new(),
            ImageContext::HeroBare,
            &ImageRenderOptions {
                extra_attrs: Some(r#"style="object-fit:cover;object-position:50% 50%""#),
                ..Default::default()
            },
        );
        assert_eq!(
            out,
            r#"<img src="cover.jpg" alt="" style="object-fit:cover;object-position:50% 50%" />"#,
            "got: {}",
            out
        );
    }

    #[test]
    fn synthesize_hero_bare_with_lqip_in_snapshot_still_bare() {
        // Even when the snapshot has LQIP / dims for the source path,
        // the HeroBare variant must ignore them — the no-snapshot signal
        // is structural (this variant exists precisely because no
        // AssetRegistry has been primed), not data-driven.
        let mut snap = AssetSnapshot::new();
        snap.lqip
            .insert("cover.jpg".into(), "data:image/jpeg;base64,xxx".into());
        snap.dimensions.insert("cover.jpg".into(), (1920, 1080));
        let out = synthesize_image_html(
            "cover.jpg",
            "",
            &snap,
            ImageContext::HeroBare,
            &ImageRenderOptions::default(),
        );
        assert!(!out.contains("<picture"), "got: {}", out);
        assert!(!out.contains("<source"), "got: {}", out);
        assert!(!out.contains("background-image"), "got: {}", out);
        assert!(!out.contains("width="), "got: {}", out);
        assert!(!out.contains("height="), "got: {}", out);
        assert!(!out.contains("loading="), "got: {}", out);
    }

    #[test]
    fn synthesize_hero_bare_escapes_url() {
        // The synthesizer's html_escape covers `&` and `"`; the legacy
        // fallback used the same `html_escape` from build::media::cover.
        let out = synthesize_image_html(
            r#"x.jpg?a=1&b="c""#,
            "",
            &AssetSnapshot::new(),
            ImageContext::HeroBare,
            &ImageRenderOptions::default(),
        );
        assert!(out.contains("&amp;"), "& must be escaped; got: {}", out);
        assert!(out.contains("&quot;"), "\" must be escaped; got: {}", out);
    }

    #[test]
    fn synthesize_hero_bare_byte_shape_matches_legacy_fallback() {
        // Pin the byte shape against a literal reconstruction of the
        // pre-PR2 emission so a future "tidy" of the synthesizer's
        // HeroBare branch can't drift away from the legacy fallback
        // without flipping this assertion deliberately.
        //
        // Legacy line:
        //   format!("<img src=\"{}\" alt=\"\"{} />", html_escape(href), style)
        // where `style` was `""` or ` style="..."` (with leading space).
        let href = "covers/img.jpg";
        let style_fragment = r#" style="object-fit:contain""#;
        let legacy = format!(
            "<img src=\"{}\" alt=\"\"{} />",
            html_escape(href),
            style_fragment,
        );

        // PR2 path: thread the style through extra_attrs minus the leading
        // space (matches typed_renderers.rs migration).
        let pr2 = synthesize_image_html(
            href,
            "",
            &AssetSnapshot::new(),
            ImageContext::HeroBare,
            &ImageRenderOptions {
                extra_attrs: Some(style_fragment.trim_start()),
                ..Default::default()
            },
        );
        assert_eq!(pr2, legacy, "byte shape divergence: pr2={} legacy={}", pr2, legacy);
    }

    // --- Phase B: webp SOURCE responsive ladder (Task 12) -----------------
    //
    // A webp source is already webp (to_webp(src) == src), so the ladder rides
    // the `<img>` directly via `srcset`+`sizes` — NO `<picture>` wrap (a
    // `<source>` identical to the img would be pointless). A wide, non-animated
    // webp gains the ladder; small / animated / unknown-dims webp stays
    // byte-identical to the pre-Task-12 bare `<img>`. The `sizes=` value is the
    // SAME per-context mapping png/jpg/jpeg use.

    /// Build a snapshot carrying dims AND an explicit animated flag for the
    /// source path — the animated map is keyed like `dimensions`.
    fn snapshot_dims_animated(path: &str, w: u32, h: u32, animated: bool) -> AssetSnapshot {
        let mut s = snapshot_dims(path, w, h);
        s.animated.insert(PathBuf::from(path), animated);
        s
    }

    #[test]
    fn webp_source_wide_emits_img_srcset_no_picture() {
        // The yinlab.io case: a non-animated 1866×1866 webp original. Exact
        // byte shape — srcset on the <img>, no <picture>, base descriptor is
        // `photo.webp` itself at the deployed width (1866, under the cap).
        let assets = snapshot_dims("photo.webp", 1866, 1866);
        let html = synthesize_image_html(
            "photo.webp",
            "alt",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<img src="photo.webp" srcset="photo.w800.webp 800w, photo.w1600.webp 1600w, photo.webp 1866w" sizes="(min-width: 48rem) 47.25rem, 100vw" width="1866" height="1866" loading="lazy" alt="alt" />"#
        );
        assert!(!html.contains("<picture"), "webp source must NOT be wrapped in <picture>: {html}");
    }

    #[test]
    fn webp_source_small_is_byte_identical_bare_img() {
        // 600×400: deployed_width == 600, no rung < 600 → empty ladder →
        // byte-identical to today's bare <img> for a small webp (no srcset,
        // no <picture>, no sizes).
        let assets = snapshot_dims("photo.webp", 600, 400);
        let html = synthesize_image_html(
            "photo.webp",
            "alt",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<img src="photo.webp" width="600" height="400" loading="lazy" alt="alt" />"#
        );
    }

    #[test]
    fn webp_source_animated_is_byte_identical_bare_img() {
        // A 1866×1866 webp FLAGGED animated in the snapshot (Task 9/10 scan)
        // gets NO ladder — animation-preserving multi-size re-encode is out of
        // scope, and the pipeline's should_skip drops animated webp, so
        // emitting rungs would 404. Byte-identical to the bare <img>, despite
        // dims that would otherwise ladder.
        let assets = snapshot_dims_animated("loop.webp", 1866, 1866, true);
        let html = synthesize_image_html(
            "loop.webp",
            "alt",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert_eq!(
            html,
            r#"<img src="loop.webp" width="1866" height="1866" loading="lazy" alt="alt" />"#
        );
        assert!(!html.contains("srcset"), "animated webp must not carry srcset: {html}");
    }

    #[test]
    fn webp_source_explicit_non_animated_flag_ladders() {
        // Symmetry with the animated test: an explicit `false` in the snapshot
        // (present-false key) ladders exactly like a missing key.
        let assets = snapshot_dims_animated("photo.webp", 2000, 1200, false);
        let html = synthesize_image_html(
            "photo.webp",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"srcset="photo.w800.webp 800w, photo.w1600.webp 1600w, photo.webp 2000w""#
            ),
            "got: {html}"
        );
    }

    #[test]
    fn webp_source_card_context_uses_card_sizes() {
        let assets = snapshot_dims("cover.webp", 2000, 1200);
        let html = synthesize_image_html(
            "cover.webp",
            "",
            &assets,
            ImageContext::FolderCardCover,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"srcset="cover.w800.webp 800w, cover.w1600.webp 1600w, cover.webp 2000w" sizes="(min-width: 48rem) 24rem, 100vw""#
            ),
            "got: {html}"
        );
        assert!(!html.contains("<picture"), "got: {html}");
    }

    #[test]
    fn webp_source_gallery_context_uses_gallery_sizes() {
        let assets = snapshot_dims("g.webp", 2000, 1200);
        let html = synthesize_image_html(
            "g.webp",
            "",
            &assets,
            ImageContext::GalleryThumb,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(r#"sizes="(min-width: 48rem) 33vw, 100vw""#),
            "got: {html}"
        );
        assert!(!html.contains("<picture"), "got: {html}");
    }

    #[test]
    fn webp_source_hero_context_uses_full_bleed_sizes() {
        let assets = snapshot_dims("hero.webp", 2400, 985);
        let html = synthesize_image_html(
            "hero.webp",
            "",
            &assets,
            ImageContext::Hero,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"srcset="hero.w800.webp 800w, hero.w1600.webp 1600w, hero.webp 2400w" sizes="100vw""#
            ),
            "got: {html}"
        );
    }

    #[test]
    fn webp_source_portrait_base_descriptor_uses_post_resize_width() {
        // 3024×4032 portrait deploys at 1800×2400 — the base descriptor must be
        // the POST-RESIZE width (1800w), and width/height attrs stay natural
        // (aspect ratio hint), exactly like the png/jpg portrait picture path.
        let assets = snapshot_dims("photo.webp", 3024, 4032);
        let html = synthesize_image_html(
            "photo.webp",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(
            html.contains(
                r#"srcset="photo.w800.webp 800w, photo.w1600.webp 1600w, photo.webp 1800w""#
            ),
            "got: {html}"
        );
        assert!(html.contains(r#"width="3024" height="4032""#), "got: {html}");
    }

    #[test]
    fn webp_source_unknown_dims_is_bare_img() {
        // Empty snapshot → dims miss → no ladder → bare <img> (fallback dims,
        // no srcset, no <picture>). Matches the test/fragment-render path.
        let html = synthesize_image_html(
            "photo.webp",
            "",
            &AssetSnapshot::new(),
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("srcset"), "got: {html}");
        assert!(!html.contains("<picture"), "got: {html}");
        assert!(html.starts_with(r#"<img src="photo.webp""#), "got: {html}");
    }

    #[test]
    fn webp_source_uppercase_ext_ladders_and_uses_src_case() {
        // is_webp_source is case-insensitive; the emitted rung/base URLs derive
        // from `src` (to_webp_rung lowercases only the extension it appends).
        let assets = snapshot_dims("photo.WEBP", 2000, 1200);
        let html = synthesize_image_html(
            "photo.WEBP",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("<picture"), "uppercase .WEBP must ladder as <img srcset>: {html}");
        assert!(
            html.contains(r#"srcset="photo.w800.webp 800w, photo.w1600.webp 1600w, photo.WEBP 2000w""#),
            "got: {html}"
        );
    }

    #[test]
    fn webp_source_lqip_style_survives_on_laddered_img() {
        // The LQIP inline style is emitted on the webp <img> exactly as it is
        // for the png/jpg inner <img> — the srcset addition doesn't suppress it.
        let assets = snapshot_with(
            "photo.webp",
            Some((2000, 1200)),
            None,
            Some("data:image/jpeg;base64,abc"),
            false,
        );
        let html = synthesize_image_html(
            "photo.webp",
            "",
            &assets,
            ImageContext::MarkdownInline,
            &ImageRenderOptions::default(),
        );
        assert!(html.contains(r#"srcset="photo.w800.webp 800w"#), "got: {html}");
        assert!(
            html.contains(r#"style="background-image:url(data:image/jpeg;base64,abc);background-size:cover""#),
            "LQIP must survive on the laddered webp <img>: {html}"
        );
    }

    #[test]
    fn webp_source_in_email_body_never_leaks_srcset() {
        // EmailBody short-circuits BEFORE synthesize_inner, so even a >800px
        // webp (which WOULD ladder in a page context) emits the flat, email-
        // client-safe <img> — no srcset, no <picture>. Email clients support
        // neither; the webp ladder must never leak into an email body.
        let assets = snapshot_dims("photo.webp", 2000, 1200);
        let html = synthesize_image_html(
            "photo.webp",
            "alt",
            &assets,
            ImageContext::EmailBody {
                width: Some(2000),
                height: Some(1200),
            },
            &ImageRenderOptions::default(),
        );
        assert!(!html.contains("srcset"), "email webp must never carry srcset: {html}");
        assert!(!html.contains("<picture"), "email webp must never wrap in <picture>: {html}");
        assert!(
            html.contains(r#"style="display:block;max-width:100%;height:auto;""#),
            "email <img> shape preserved: {html}"
        );
    }
}
