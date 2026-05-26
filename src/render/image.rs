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

use crate::asset_paths::to_webp;
use crate::asset_snapshot::{AssetSnapshot, FALLBACK_HEIGHT, FALLBACK_WIDTH};
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
    let inner = synthesize_inner(src, alt, assets, options);

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
fn synthesize_inner(
    src: &str,
    alt: &str,
    assets: &AssetSnapshot,
    options: &ImageRenderOptions<'_>,
) -> String {
    let img_tag = render_img_tag(src, alt, assets, options);

    // For raster originals, always emit <picture><source srcset=X.webp>.
    // The preview server's AssetRegistry intercept (preview/server/placeholder.rs)
    // ensures the webp URL never 404s in preview mode — it returns LQIP bytes
    // for Pending entries — and publish mode encodes variants synchronously
    // before HTML ships (PluginMode::Blocking). So the URL is always live in
    // both modes.
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
        // on `src` is what keeps the synthesizer's emitted URL aligned with
        // the AssetRegistry's registered key (blocking.rs's set_pending loop
        // uses the same to_webp(mapped) derivation).
        let srcset_path = to_webp(src);
        format!(
            r#"<picture><source srcset="{}" type="image/webp">{}</picture>"#,
            html_escape(&srcset_path),
            img_tag,
        )
    } else {
        img_tag
    }
}

/// Returns true when `src` is a raster original that always gets a webp
/// variant from moss's image pipeline. Restricted to the three formats that
/// `should_skip` never filters out by content sniffing — png, jpg, jpeg. Gif
/// is excluded because animated GIFs skip webp encoding (image.rs § should_skip,
/// `is_animated_gif`); SVG is excluded as a vector format; webp originals are
/// excluded because the variant URL would equal the src URL. Restricting here
/// keeps the synthesizer's emitted `<source>` URL in lockstep with the
/// AssetRegistry's `set_pending` registration in blocking.rs (which loops over
/// `collect_images_for_conversion`'s output, also filtered by `should_skip`).
fn is_raster_original(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")
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

fn lookup_lqip<'a>(assets: &'a AssetSnapshot, src: &str) -> Option<&'a str> {
    probe_paths(src, |p| assets.lqip(&p))
}

fn lookup_color<'a>(assets: &'a AssetSnapshot, src: &str) -> Option<&'a String> {
    probe_paths(src, |p| assets.dominant_color.get(&p))
}

fn probe_paths<T>(src: &str, mut probe: impl FnMut(PathBuf) -> Option<T>) -> Option<T> {
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

/// Emit just the `<img>` tag with all attributes. Internal helper for
/// `synthesize_image_html` — exposed as `pub(crate)` only for snapshot tests
/// that want to assert against the bare img output without the optional
/// `<picture>` wrapper.
pub(crate) fn render_img_tag(
    src: &str,
    alt: &str,
    assets: &AssetSnapshot,
    options: &ImageRenderOptions<'_>,
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
        r#"<img{class_attr} src="{src_esc}" width="{w}" height="{h}"{loading}{fetch}{style} alt="{alt}"{extra} />"#,
        class_attr = class_attr,
        src_esc = html_escape(src),
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
    // The three idempotency tests that lived here through Phase 2E v5 PR2a
    // were extracted to `src-tauri/tests/image_synth_regex_parity.rs` during
    // PR2b's move of this file into moss-core: the tests import
    // `add_image_placeholder_attributes` and `MediaDimensionLookup` from
    // src-tauri's still-alive `build::media::placeholder`, which moss-core
    // cannot reach. They will be deleted alongside the surviving regex in
    // Phase 2E v5 PR5.

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
}
