//! Pre-parse extraction of `:::shortcode` blocks from markdown source.
//!
//! Walks the markdown line-by-line, tracking fenced code blocks (so
//! `:::buttons` inside a code fence stays inert) and recognizing
//! `:::name ...args` / `:::` openers/closers. Each block is replaced with
//! a sentinel HTML comment (`<!--MOSS_SC_{nonce}_N-->`) that pulldown-cmark
//! emits as a `Block::Other` raw HTML; the final parser pass walks the
//! AST and substitutes the sentinels with typed [`Shortcode`] variants.
//!
//! Why this design:
//!
//! - `:::` block syntax is not standard CommonMark; pulldown-cmark sees
//!   it as plain text inside a paragraph. Post-parse text-matching is
//!   fragile (works only when the shortcode is the entire paragraph).
//! - Pre-parse extraction with a sentinel is the same pattern Zola uses
//!   and preserves parsing correctness for adjacent content.
//! - The sentinel is an HTML comment so it survives pulldown-cmark intact
//!   (pulldown-cmark passes HTML comments through `Event::Html` as
//!   `Block::HtmlBlock`).

use super::attrs::gather_multi_line_attrs;
use super::cells::split_cells;
use super::node::Block;
use super::shortcode::{
    ButtonItem, ButtonsShortcode, GalleryItem, GalleryShortcode, GridShortcode, HeroShortcode,
    RecentShortcode, Shortcode, SubscribeShortcode,
};
use super::url::Url;

/// One extracted shortcode block, with its body parsed into a typed variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedShortcode {
    /// 0-based index used in the placeholder sentinel.
    pub index: usize,
    /// Parsed shortcode (typed variants per Phase B).
    pub shortcode: Shortcode,
}

/// Result of pre-parse extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    /// Markdown source with `:::shortcode` blocks replaced by sentinel
    /// HTML comments. Pulldown-cmark sees this as the input.
    pub markdown_with_placeholders: String,
    /// One entry per extracted block, indexed by sentinel number.
    pub extracted: Vec<ExtractedShortcode>,
    /// Per-extraction nonce (8 hex chars). Derived from a hash of the
    /// input markdown so it's deterministic but collision-resistant
    /// against authored content. The placeholder format is
    /// `<!--MOSS_SC_{nonce}_{index}-->`; an authored markdown comment
    /// matching that exact shape would have to embed the same hash of
    /// itself, which is computationally improbable for any input shorter
    /// than the SHA universe.
    pub nonce: String,
    /// Build warnings collected during extraction (e.g. unknown shortcode
    /// names). Each entry is a one-line human-readable string. Caller
    /// surfaces these in the build log; presence does not abort the build.
    pub warnings: Vec<String>,
}

/// Names recognized by the typed AST. Other names fall through to the
/// unknown-name renderer (`<div class="moss-unknown-shortcode" data-name="…">`)
/// with a build warning.
const TYPED_KNOWN: &[&str] = &["subscribe", "buttons", "gallery", "hero", "grid", "recent"];

fn is_typed_known(name: &str) -> bool {
    TYPED_KNOWN.contains(&name)
}

/// Recognized shortcode names (Phase B Task 7+ adds variants here).
///
/// `args` is the trailing text after `:::name ` on the opening line
/// (e.g. for `:::buttons {.primary}`, args is `{.primary}`).
///
/// Returns `(Some(Shortcode), Vec<String>)` where the second element is
/// parse-time deprecation warnings. An empty warning vec means the block
/// used only current-grammar syntax.
fn parse_shortcode_block(name: &str, args: &str, body: &str) -> (Option<Shortcode>, Vec<String>) {
    match name {
        "subscribe" => (Some(Shortcode::Subscribe(parse_subscribe_args(args))), vec![]),
        "buttons" => (Some(Shortcode::Buttons(parse_buttons_body(args, body))), vec![]),
        "gallery" => (Some(Shortcode::Gallery(parse_gallery_body(args, body))), vec![]),
        "hero" => {
            let (sc, used_p3) = parse_hero(args, body);
            let mut warns = vec![];
            if used_p3 {
                warns.push(
                    "shortcode `:::hero` uses a body-image fallback (deprecated Priority 3). \
                     Move the image path to the `image=` attribute: \
                     `:::hero {image=path.jpg}`."
                        .to_string(),
                );
            }
            (Some(Shortcode::Hero(sc)), warns)
        }
        "grid" => {
            let (sc, legacy) = parse_grid(args, body);
            let mut warns = vec![];
            if legacy {
                warns.push(
                    "shortcode `:::grid` uses `---` cell dividers (deprecated). Migrate to `+++`.\n\
                     `---` support will be removed in a future release."
                        .to_string(),
                );
            }
            (Some(Shortcode::Grid(sc)), warns)
        }
        "recent" => (Some(Shortcode::Recent(parse_recent_args(args, body))), vec![]),
        _ => (None, vec![]),
    }
}

/// Parse `:::recent {since=... last=... count=...}` body into a typed struct.
///
/// `args` is the attribute block (e.g. `{since="2026-04-01" count="5"}`);
/// `body` is the content between the opening and closing `:::` fences,
/// captured verbatim (trimmed) as the fallback markdown for the zero-match
/// render path.
///
/// Tolerant: unknown keys are ignored. A `count=` value that fails to parse
/// as a `u32` becomes `None`; the renderer falls back to its default (10).
/// `since` and `last` are passed through as raw strings — the rendering
/// layer parses them into a `DateTime` / `Duration` so this stays I/O-free
/// and chrono-free (moss-core invariant: pure data in / data out).
pub fn parse_recent_args(args: &str, body: &str) -> RecentShortcode {
    let attrs = super::attrs::parse_attrs(args).unwrap_or_default();
    RecentShortcode {
        since: attrs.get("since").map(str::to_string),
        last: attrs.get("last").map(str::to_string),
        count: attrs.get("count").and_then(|v| v.parse::<u32>().ok()),
        fallback_markdown: body.trim().to_string(),
    }
}

/// Parse a `:::grid` block.
///
/// Args parsing supports both:
/// - **Positional** (legacy moss-releases): `:::grid 2 1:2 {.classes}` —
///   first token is column count, second optional token is the ratio.
/// - **Attribute** (new grammar): `:::grid {cols=2}` or `:::grid {cols=1:1:2}` —
///   `cols=integer` sets the column count; `cols=ratio` sets both the
///   ratio and the count (= ratio length).
///
/// Cells are split on lines containing only `+++` (new grammar) or
/// `---` (legacy moss-releases). Step 3 of #613 rewrites `---` to `+++`
/// in moss-releases content; the parser accepts both during the
/// migration window.
///
/// Returns `(GridShortcode, bool)` where the bool is `true` when any
/// `---` legacy divider was encountered (triggers a deprecation warning).
fn parse_grid(args: &str, body: &str) -> (GridShortcode, bool) {
    let trimmed = args.trim();
    let (positional, attr_block): (&str, &str) = if let Some(pos) = trimmed.find('{') {
        // char-aligned: pos points to ASCII '{' from str::find — safe to slice.
        #[allow(clippy::string_slice)]
        (trimmed[..pos].trim(), &trimmed[pos..])
    } else {
        (trimmed, "")
    };

    let parsed = if attr_block.is_empty() {
        Default::default()
    } else {
        super::attrs::parse_attrs(attr_block).unwrap_or_default()
    };
    let classes = parsed.class_string();
    let width = parsed.width.map(str::to_string);

    let mut columns: u32 = 1;
    let mut ratio: Option<String> = None;

    if let Some(cols_value) = parsed.get("cols") {
        if cols_value.contains(':') {
            ratio = Some(cols_value.to_string());
            columns = cols_value.split(':').count() as u32;
        } else if let Ok(n) = cols_value.parse::<u32>() {
            columns = n.max(1);
        }
    } else {
        // Positional fallback: e.g. `2 1:2`.
        let parts: Vec<&str> = positional.split_whitespace().collect();
        if let Some(first) = parts.first() {
            if first.contains(':') {
                ratio = Some(first.to_string());
                columns = first.split(':').count() as u32;
            } else if let Ok(n) = first.parse::<u32>() {
                columns = n.max(1);
                if let Some(second) = parts.get(1) {
                    if second.contains(':') {
                        ratio = Some(second.to_string());
                    }
                }
            }
        }
    }

    let (raw_cells, found_legacy_dash) = split_grid_cells(body);

    // Phase 4 PR4.5 (2026-05-28): cells become Vec<Vec<Block>>. Each raw
    // cell string is either:
    //
    // - A "compound-link" cell whose entire content is wrapped in a markdown
    //   link `[inner](url)` and whose `inner` carries block-level content
    //   (image + heading + paragraphs — the SoCiviC pattern). CommonMark's
    //   inline parser cannot represent a `[](url)` with `### heading` inside,
    //   so we detect this shape at the cell-string level FIRST and emit a
    //   typed [`Block::LinkCard { url, children }`] where `children` is the
    //   inner content parsed as blocks via [`super::parser::parse`].
    //
    // - A plain markdown cell. Parse via [`super::parser::parse`] (which
    //   re-runs extract_shortcodes so any nested `::::buttons` etc. get
    //   substituted) and drop the wrapping `Document`.
    let cells: Vec<Vec<Block>> = raw_cells
        .iter()
        .map(|raw| parse_cell_to_blocks(raw))
        .collect();

    (
        GridShortcode {
            columns,
            ratio,
            classes,
            cells,
            width,
        },
        found_legacy_dash,
    )
}

/// Parse one grid cell's raw markdown source into a `Vec<Block>`.
///
/// Phase 4 PR4.5 (2026-05-28): detects the compound-link shape first
/// (`[inner](url)` wrapping the entire trimmed cell content). On match,
/// emits a single-element `vec![Block::LinkCard { url, children }]` where
/// `children` is the inner parsed as blocks. On no match, parses the cell
/// directly via [`super::parser::parse`].
fn parse_cell_to_blocks(raw: &str) -> Vec<Block> {
    if let Some((url, inner)) = detect_compound_link(raw) {
        let inner_trimmed = inner.trim();
        // Simple compound-link special case: when the inner content is
        // plain phrasing text (no images, no nested links, no
        // block-level markdown) AND the URL is external, fall through
        // to the normal markdown parse so the cell renders as
        // `<p><a href="URL">text</a></p>` — the shape `build/render/
        // grid_post.rs::link_only_cell_href` detects to layer the
        // `<span class="link-preview-title">` post-pass enhancement
        // (title + favicon + domain). LinkCard's
        // `<a class="moss-grid-card link-preview">` shape would skip
        // the post-pass (tag != "div" guard) and lose the title row.
        //
        // Mirrors the pre-PR4.5 carve-out in
        // `crate::build::markdown::typed_renderers::render_compound_link_cell`
        // (the `if !inner.contains('!') && !inner.contains('[') && !inner.contains('\n')`
        // branch).
        let inner_is_plain_text = !inner_trimmed.contains('!')
            && !inner_trimmed.contains('[')
            && !inner_trimmed.contains('\n');
        let is_external = url.starts_with("http://") || url.starts_with("https://");
        if inner_is_plain_text && is_external {
            // Re-emit as standard markdown link inside a paragraph so the
            // grid_post post-pass owns the rendering.
            let linkified = format!("[{}]({})", inner_trimmed, url);
            return super::parser::parse(&linkified).blocks;
        }
        let inner_doc = super::parser::parse(inner_trimmed);
        return vec![Block::LinkCard {
            url: Url::unresolved(url),
            children: inner_doc.blocks,
        }];
    }
    // Phase 4 PR4.5 (2026-05-28): bare-URL cell auto-promotion. When the
    // entire cell content is a single bare URL on its own line (no
    // markdown link syntax), parse it as `[](URL)` so the cell renders as
    // `<p><a href="URL"></a></p>` (an empty-text link inside a paragraph).
    // The grid-render post-pass in `build/render/grid_post.rs` detects
    // this shape and replaces with a `<span class="link-preview-domain">…</span>`
    // wrapper carrying title/favicon (from cached link metadata).
    //
    // Matches the pre-PR4.5 `linkify_bare_urls_in_cell` behavior — the
    // helper turned `https://...` into `[](https://...)` so the downstream
    // compound-link pass picked it up. PR4.5 ports the linkification to
    // parse time so the bytes flow through the typed AST.
    if let Some(url) = detect_bare_url_cell(raw) {
        let linkified = format!("[]({})", url);
        let doc = super::parser::parse(&linkified);
        return doc.blocks;
    }
    let doc = super::parser::parse(raw);
    doc.blocks
}

/// Detect a "bare URL cell": the entire cell content (after trim) is a
/// single `https?://...` URL on its own line, with no other content.
///
/// Returns the URL string on match, `None` otherwise. Used by
/// [`parse_cell_to_blocks`] to linkify bare-URL cells via `[](URL)` so
/// they thread through the grid_post link-preview post-pass like
/// authored `[Title](URL)` cells.
fn detect_bare_url_cell(cell_text: &str) -> Option<String> {
    let trimmed = cell_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.lines().count() > 1 {
        return None;
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return None;
    }
    if trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Detect the compound-link shape in a grid cell's markdown content.
///
/// Matches cells whose entire content (after trimming whitespace) begins
/// with `[` and ends with `](url)`. The inner content may span blank lines
/// and contain any markdown block syntax (headings, images, paragraphs,
/// lists, emphasis).
///
/// Returns `Some((url, inner_content))` on a match, `None` otherwise.
///
/// Ported from src-tauri's `crate::build::markdown::typed_renderers::
/// detect_compound_link` (Phase 4 PR4.5, 2026-05-28) — the AST-level
/// equivalent of the same string-level detection. The src-tauri version
/// is deleted in PR4.5.
///
/// Safety rules that cause this function to return `None`:
/// - Cell contains a top-level code fence (\`\`\` or ~~~).
/// - Cell content starts with a backtick (inline code on first line).
/// - The outer `[…](url)` shape cannot be confirmed by bracket-balance
///   scanning (multiple top-level links, bare `]` / `(` without a pair).
/// - There is non-whitespace content after the closing `)`.
///
/// Detection uses bracket balancing so nested `](` sequences inside images
/// (`![alt](src)`) or inline code do NOT prematurely end the outer link.
pub(super) fn detect_compound_link(cell_text: &str) -> Option<(String, String)> {
    let stripped = cell_text.trim();

    if !stripped.starts_with('[') {
        return None;
    }
    if !stripped.ends_with(')') {
        return None;
    }
    if stripped.len() > 1 && stripped.as_bytes()[1] == b'`' {
        return None;
    }

    for line in stripped.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            return None;
        }
    }

    let bytes = stripped.as_bytes();

    // Phase 1: find the outer closing `]` via bracket-balance scan.
    let mut i: usize = 1;
    let mut depth: usize = 1;
    let mut outer_close: Option<usize> = None;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => {
                let tick_start = i;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                let fence_len = i - tick_start;
                'code_scan: while i < bytes.len() {
                    if bytes[i] == b'`' {
                        let close_start = i;
                        while i < bytes.len() && bytes[i] == b'`' {
                            i += 1;
                        }
                        if i - close_start == fence_len {
                            break 'code_scan;
                        }
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            b'[' => {
                depth += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    outer_close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let close_bracket = outer_close?;

    if bytes.get(close_bracket + 1) != Some(&b'(') {
        return None;
    }

    // Phase 2: find the matching `)` with paren balance.
    let mut j = close_bracket + 2;
    let mut pdepth: usize = 1;
    let mut paren_close: Option<usize> = None;

    while j < bytes.len() {
        match bytes[j] {
            b'\\' => {
                j += 2;
                continue;
            }
            b'(' => pdepth += 1,
            b')' => {
                pdepth -= 1;
                if pdepth == 0 {
                    paren_close = Some(j);
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }

    let close_paren = paren_close?;

    // Phase 3: after `)`, only whitespace/blank lines.
    let tail = &stripped[close_paren + 1..];
    if !tail.chars().all(|c| c.is_whitespace()) {
        return None;
    }

    // Phase 4: validate inner content.
    let inner = &stripped[1..close_bracket];
    if inner.trim().is_empty() {
        return None;
    }

    // Phase 5: reject multiple top-level links (images allowed).
    {
        let inner_bytes = inner.as_bytes();
        let mut k: usize = 0;
        let mut image_stack: Vec<bool> = Vec::new();

        while k < inner_bytes.len() {
            match inner_bytes[k] {
                b'\\' => {
                    k += 2;
                    continue;
                }
                b'`' => {
                    let tick_start = k;
                    while k < inner_bytes.len() && inner_bytes[k] == b'`' {
                        k += 1;
                    }
                    let fence_len = k - tick_start;
                    'inner_code: while k < inner_bytes.len() {
                        if inner_bytes[k] == b'`' {
                            let cs = k;
                            while k < inner_bytes.len() && inner_bytes[k] == b'`' {
                                k += 1;
                            }
                            if k - cs == fence_len {
                                break 'inner_code;
                            }
                        } else {
                            k += 1;
                        }
                    }
                    continue;
                }
                b'[' => {
                    let preceded_by_bang = k > 0 && inner_bytes[k - 1] == b'!';
                    image_stack.push(preceded_by_bang);
                }
                b']' => {
                    if let Some(is_image) = image_stack.pop() {
                        if image_stack.is_empty() && inner_bytes.get(k + 1) == Some(&b'(') {
                            if !is_image {
                                return None;
                            }
                        }
                    }
                }
                _ => {}
            }
            k += 1;
        }
    }

    let url = &stripped[close_bracket + 2..close_paren];
    Some((url.to_string(), inner.to_string()))
}

/// Split a grid body into cells on lines containing only `+++` (new
/// grammar) or `---` (legacy moss-releases backward-compat).
///
/// Mirrors [`super::cells::split_cells`] but accepts either divider.
/// Step 3 of #613 rewrites `---` to `+++` in moss-releases content;
/// after that, this helper retires in favor of `split_cells`.
///
/// Returns `(cells, found_legacy_dash)` where `found_legacy_dash` is
/// `true` when at least one `---` divider was encountered, signaling
/// the caller to emit a deprecation warning.
fn split_grid_cells(body: &str) -> (Vec<String>, bool) {
    if body.is_empty() {
        return (vec![String::new()], false);
    }
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut first_line_in_cell = true;
    let mut found_legacy_dash = false;

    for line in body.split_inclusive('\n') {
        let content_no_eol = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content_no_eol.trim();
        if trimmed == "+++" || trimmed == "---" {
            if trimmed == "---" {
                found_legacy_dash = true;
            }
            if let Some(stripped) = current.strip_suffix('\n') {
                current.truncate(stripped.len());
            }
            cells.push(std::mem::take(&mut current));
            first_line_in_cell = true;
            continue;
        }
        if first_line_in_cell {
            first_line_in_cell = false;
            if trimmed.is_empty() {
                continue;
            }
        }
        current.push_str(line);
    }
    if let Some(stripped) = current.strip_suffix('\n') {
        current.truncate(stripped.len());
    }
    cells.push(current);
    (cells, found_legacy_dash)
}

/// Parse a `:::hero` block in any of three syntactic forms.
///
/// Image source priority:
/// 1. `image=path` attribute in the `{...}` block (new grammar).
/// 2. **Directive-line path**: `:::hero ./path.jpg` or
///    `:::hero ./path.jpg|attrs` or `:::hero ./path.jpg {.classes}` —
///    moss-releases / client-site backward-compat. The path appears as
///    raw text before any `{...}` attribute block.
/// 3. **Body-image fallback**: scan first non-empty body line for a
///    media reference (`![[path|attrs]]`, `![alt](path|attrs)`, or
///    bare media filename). Step 3 of the grammar migration rewrites
///    these to use the `image=` attribute.
/// 4. None — renderer emits a `<section>` with no `<img>`.
///
/// Returns `(HeroShortcode, bool)` where the bool is `true` when the
/// body-image fallback (Priority 3) fired, signaling the caller to
/// emit a deprecation warning.
fn parse_hero(args: &str, body: &str) -> (HeroShortcode, bool) {
    let trimmed_args = args.trim();

    // Split args on the first `{` to separate the directive-line path
    // (if any) from the attribute block (if any).
    let (positional, attr_block): (&str, &str) = if let Some(pos) = trimmed_args.find('{') {
        // char-aligned: pos points to ASCII '{' from str::find — safe to slice.
        #[allow(clippy::string_slice)]
        (trimmed_args[..pos].trim(), &trimmed_args[pos..])
    } else {
        (trimmed_args, "")
    };

    // Parse the attribute block, if present.
    let parsed = if attr_block.is_empty() {
        Default::default()
    } else {
        super::attrs::parse_attrs(attr_block).unwrap_or_default()
    };
    let classes = parsed.class_string();
    let width = parsed.width.map(str::to_string);
    let mobile = parsed.get("mobile").map(str::to_string);

    // Priority 1: `image=` attribute.
    if let Some(image_value) = parsed.get("image") {
        let (path, attrs_str) = crate::media::split_pipe(image_value);
        let overlay_text = body.trim().to_string();
        let overlay = parse_overlay_to_blocks(&overlay_text);
        return (
            HeroShortcode {
                image: if path.trim().is_empty() {
                    None
                } else {
                    Some(Url::unresolved(path.trim().to_string()))
                },
                attrs: attrs_str.to_string(),
                classes,
                overlay,
                overlay_text,
                width,
                mobile: mobile.clone(),
            },
            false,
        );
    }

    // Priority 2: directive-line path (legacy syntax). When the
    // positional text is non-empty, treat it as the image path with
    // optional `|attrs` pipe suffix. Body becomes pure overlay markdown.
    if !positional.is_empty() {
        let (path, attrs_str) = crate::media::split_pipe(positional);
        let overlay_text = body.trim().to_string();
        let overlay = parse_overlay_to_blocks(&overlay_text);
        return (
            HeroShortcode {
                image: if path.trim().is_empty() {
                    None
                } else {
                    Some(Url::unresolved(path.trim().to_string()))
                },
                attrs: attrs_str.to_string(),
                classes,
                overlay,
                overlay_text,
                width,
                mobile: mobile.clone(),
            },
            false,
        );
    }

    // Priority 3: body-image fallback. Scan first non-empty line.
    let mut overlay_lines: Vec<&str> = Vec::new();
    let mut image_path: Option<String> = None;
    let mut image_attrs = String::new();
    let mut found_image = false;
    let mut used_priority_3 = false;
    for line in body.lines() {
        if !found_image && !line.trim().is_empty() {
            if let Some((path, attrs_str)) = parse_hero_media_line(line) {
                image_path = Some(path);
                image_attrs = attrs_str;
                found_image = true;
                used_priority_3 = true;
                continue;
            }
            // First non-empty line wasn't a media reference — keep it as overlay.
            found_image = true;
        }
        overlay_lines.push(line);
    }
    let overlay_text = overlay_lines.join("\n").trim().to_string();
    let overlay = parse_overlay_to_blocks(&overlay_text);
    (
        HeroShortcode {
            image: image_path.map(Url::unresolved),
            attrs: image_attrs,
            classes,
            overlay,
            overlay_text,
            width,
            mobile,
        },
        used_priority_3,
    )
}

/// Parse a hero overlay's raw markdown source into `Vec<Block>`.
///
/// Phase 4 PR4.5 (2026-05-28): mirrors `parse_cell_to_blocks` for the
/// grid-cell path but without compound-link detection (an overlay is not
/// a compound-link surface; the SoCiviC pattern is grid-cell-specific).
/// Returns an empty vec when the overlay is empty.
fn parse_overlay_to_blocks(raw: &str) -> Vec<Block> {
    if raw.is_empty() {
        return Vec::new();
    }
    let doc = super::parser::parse(raw);
    doc.blocks
}

/// File extensions recognized as media for hero body-image fallback.
const HERO_MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "avif", "svg", "mp4", "webm", "mov",
];

fn is_bare_hero_media(s: &str) -> bool {
    let (path_part, _) = crate::media::split_pipe(s);
    let path = path_part.trim();
    path.rfind('.')
        .map(|dot| {
            // char-aligned: dot points to ASCII '.' from str::rfind — `dot + 1`
            // lands on the byte after '.', which is also a char boundary.
            #[allow(clippy::string_slice)]
            let ext = &path[dot + 1..];
            HERO_MEDIA_EXTENSIONS
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

/// Parse a line as a media reference. Returns `(path, attrs_str)`.
fn parse_hero_media_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();

    // Wikilink embed: ![[path|attrs]]
    if let Some(inner) = trimmed
        .strip_prefix("![[")
        .and_then(|s| s.strip_suffix("]]"))
    {
        let (path, attrs_str) = crate::media::split_pipe(inner);
        return Some((path.trim().to_string(), attrs_str.to_string()));
    }

    // Standard markdown image: ![alt](path|attrs)
    if trimmed.starts_with("![") {
        if let Some(paren_open) = trimmed.find("](") {
            if trimmed.ends_with(')') {
                // char-aligned: paren_open points to ASCII "](" from str::find
                // (paren_open + 2 lands on first byte after `](`, char boundary);
                // `trimmed.len() - 1` is the byte before the trailing ASCII ')'.
                #[allow(clippy::string_slice)]
                let inner = &trimmed[paren_open + 2..trimmed.len() - 1];
                let (path, attrs_str) = crate::media::split_pipe(inner);
                return Some((path.trim().to_string(), attrs_str.to_string()));
            }
        }
    }

    // Bare media filename: photo.jpg or photo.jpg|contain
    if is_bare_hero_media(trimmed) {
        let (path, attrs_str) = crate::media::split_pipe(trimmed);
        return Some((path.trim().to_string(), attrs_str.to_string()));
    }

    None
}

fn parse_gallery_body(args: &str, body: &str) -> GalleryShortcode {
    // Args: `N {.classes width}` where N is optional columns count and
    // `width` is one of the spec § P9 width tokens (handled inside
    // `split_positional_and_classes`).
    let (positional, classes, width) = split_positional_classes_and_width(args);
    let columns = if positional.is_empty() {
        None
    } else {
        positional.parse::<u32>().ok()
    };
    let mut items: Vec<GalleryItem> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Each line: `path|attrs`, `![alt](path)|attrs`, or bare `path`.
        // The pipe split (if any) is BEFORE the markdown-image pattern check.
        let (src_raw, attrs) = split_pipe(trimmed);
        let (src_url, alt) = match parse_markdown_image(src_raw) {
            Some((alt, path)) => (path, alt),
            None => (src_raw.trim().to_string(), String::new()),
        };
        items.push(GalleryItem {
            src: Url::unresolved(src_url),
            alt,
            attrs: attrs.to_string(),
        });
    }
    GalleryShortcode {
        columns,
        classes,
        items,
        width,
    }
}

/// Split `args` into `(positional_text, classes, width)`.
///
/// Same routing as [`split_positional_and_classes`], but also surfaces the
/// spec § P9 width token (`body | wide | page | screen`, with `full`
/// aliased to `screen`). Returns `width = None` when the author did not
/// set one, or when the legacy fallback path fires (malformed attrs
/// where the structured parser bailed).
fn split_positional_classes_and_width(args: &str) -> (String, String, Option<String>) {
    let trimmed = args.trim();
    if let Some(brace_start) = trimmed.find('{') {
        #[allow(clippy::string_slice)]
        let after_open = &trimmed[brace_start..];
        if let Some(brace_end) = after_open.find('}') {
            #[allow(clippy::string_slice)]
            let positional = trimmed[..brace_start].trim().to_string();
            #[allow(clippy::string_slice)]
            let attr_block_str = &trimmed[brace_start..=brace_start + brace_end];
            if let Ok(parsed) = super::attrs::parse_attrs(attr_block_str) {
                return (
                    positional,
                    parsed.class_string(),
                    parsed.width.map(str::to_string),
                );
            }
            // Legacy fallback for malformed inputs: scan only for `.class`.
            // Width tokens are skipped here on purpose — if attrs are
            // malformed enough to bail, the author's intent is unclear and
            // omitting the width is safer than guessing.
            #[allow(clippy::string_slice)]
            let inner = &trimmed[brace_start + 1..brace_start + brace_end];
            let mut classes = Vec::new();
            for token in inner.split_whitespace() {
                if let Some(class) = token.strip_prefix('.') {
                    if !class.is_empty() {
                        classes.push(class);
                    }
                }
            }
            return (positional, classes.join(" "), None);
        }
    }
    (trimmed.to_string(), String::new(), None)
}

/// Split `args` into `(positional_text, classes)` from `{...}` syntax.
///
/// Routes the attribute portion through [`crate::ast::attrs::parse_attrs`]
/// so the unified grammar's full surface (`.class`, `#id`, `key=value`,
/// quoted values, multi-line) is recognized — even though the legacy
/// shortcodes (Subscribe / Buttons / Gallery) only consume the class
/// list today. Step 2 migrates Hero / Grid; once they read `kvs` and
/// `id` via `parse_attrs` directly, this helper retires.
///
/// Falls back to the legacy whitespace-tokenized class scan when
/// `parse_attrs` returns `Err` (malformed attrs, unterminated quote,
/// etc.) so existing content with edge-case `{}` shapes still parses
/// the way it did before.
fn split_positional_and_classes(args: &str) -> (String, String) {
    let trimmed = args.trim();
    if let Some(brace_start) = trimmed.find('{') {
        // char-aligned: brace_start points to ASCII '{' from str::find — the
        // byte index is a char boundary, so slicing `trimmed[brace_start..]`
        // is safe to feed into the next find.
        #[allow(clippy::string_slice)]
        let after_open = &trimmed[brace_start..];
        if let Some(brace_end) = after_open.find('}') {
            // char-aligned: brace_start (ASCII '{') and brace_start+brace_end
            // (ASCII '}') are both char boundaries; `brace_start + 1` lands on
            // the byte after '{', also a boundary.
            #[allow(clippy::string_slice)]
            let positional = trimmed[..brace_start].trim().to_string();
            #[allow(clippy::string_slice)]
            let attr_block_str = &trimmed[brace_start..=brace_start + brace_end];
            if let Ok(parsed) = super::attrs::parse_attrs(attr_block_str) {
                return (positional, parsed.class_string());
            }
            // Legacy fallback for malformed inputs that the structured
            // parser rejects (e.g. unterminated quote on a single line).
            #[allow(clippy::string_slice)]
            let inner = &trimmed[brace_start + 1..brace_start + brace_end];
            let mut classes = Vec::new();
            for token in inner.split_whitespace() {
                if let Some(class) = token.strip_prefix('.') {
                    if !class.is_empty() {
                        classes.push(class);
                    }
                }
            }
            return (positional, classes.join(" "));
        }
    }
    (trimmed.to_string(), String::new())
}

/// Split `s` on `|` into `(before, after)`. If no pipe, returns `(s, "")`.
fn split_pipe(s: &str) -> (&str, &str) {
    match s.split_once('|') {
        Some((before, after)) => (before, after.trim()),
        None => (s, ""),
    }
}

/// Parse `![alt](path)` into `(alt, path)`. Returns `None` if not a
/// markdown image. Mirrors the legacy parser at shortcode.rs:1615.
fn parse_markdown_image(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let rest = s.strip_prefix("![")?;
    let (alt, after) = rest.split_once("](")?;
    let close_paren = after.rfind(')')?;
    // char-aligned: close_paren points to ASCII ')' from str::rfind.
    #[allow(clippy::string_slice)]
    let path = &after[..close_paren];
    if path.contains('(') {
        return None;
    }
    Some((alt.to_string(), path.to_string()))
}

fn parse_buttons_body(args: &str, body: &str) -> ButtonsShortcode {
    let (_positional, classes) = split_positional_and_classes(args);
    let mut items: Vec<ButtonItem> = Vec::new();
    // Split the body on `+++` cell dividers (unified grammar).
    // Bodies without `+++` produce a single cell containing the entire
    // body — backward-compatible with the legacy "one link per line"
    // shape.
    for cell in split_cells(body) {
        for line in cell.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((text, url)) = extract_markdown_link(trimmed) {
                items.push(ButtonItem {
                    text,
                    url: Url::unresolved(url),
                });
            }
            // Non-link lines silently ignored (matches legacy behavior).
        }
    }
    ButtonsShortcode { classes, items }
}

/// Extract a markdown link `[text](url)` from a single trimmed line.
/// Returns `(text, url)` if the line is a single link, else `None`.
fn extract_markdown_link(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let inside = s.strip_prefix('[')?;
    let (text, after) = inside.split_once(']')?;
    let url = after.strip_prefix('(').and_then(|r| r.strip_suffix(')'))?;
    if url.is_empty() {
        return None;
    }
    Some((text.to_string(), url.to_string()))
}

/// Parse `:::subscribe {placeholder="..." button="..."}` into a typed struct.
///
/// Reads `placeholder` and `button` from the attribute block; ignores
/// classes/id (the renderer uses fixed `moss-subscribe` chrome). Body
/// must be empty under the unified grammar — caller is responsible for
/// surfacing a deprecation warning if non-empty.
fn parse_subscribe_args(args: &str) -> SubscribeShortcode {
    // Empty args produce an empty AttrBlock; both fields stay None
    // and the renderer falls back to language defaults.
    let parsed = match super::attrs::parse_attrs(args) {
        Ok(b) => b,
        Err(_) => return SubscribeShortcode::default(),
    };
    let placeholder = parsed
        .get("placeholder")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let button = parsed
        .get("button")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    SubscribeShortcode {
        placeholder,
        button,
    }
}

/// The sentinel HTML comment used to mark an extracted shortcode in the
/// markdown source. Pulldown-cmark emits these as [`Event::Html`] inside
/// a [`Tag::HtmlBlock`], which surfaces as [`Block::Other`] in our AST.
///
/// `nonce` is the per-extraction hash from [`ExtractionResult::nonce`],
/// which forecloses the namespace-collision case where an author writes
/// `<!--MOSS_SC_*-->` literally in their markdown.
pub fn placeholder_for(nonce: &str, index: usize) -> String {
    format!("<!--MOSS_SC_{nonce}_{index}-->")
}

/// Try to interpret a [`Block::Other`] payload as a shortcode placeholder
/// matching the given `nonce`. Returns the `index` if it matches.
///
/// Any sentinel with a different (or absent) nonce is rejected — that's
/// what makes authored content with a similar comment shape inert.
pub fn parse_placeholder(nonce: &str, html: &str) -> Option<usize> {
    let trim = html.trim();
    let prefix = format!("<!--MOSS_SC_{nonce}_");
    let inner = trim.strip_prefix(&prefix)?;
    let inner = inner.strip_suffix("-->")?;
    inner.parse::<usize>().ok()
}

/// Compute the per-extraction nonce from the input markdown. Uses
/// `std::hash::DefaultHasher` (FxHash-like; not cryptographic, but good
/// enough to make a literal authored-content collision computationally
/// improbable for any short input). Returns 8 hex characters.
fn compute_nonce(input: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    // Truncate to 32 bits for an 8-char hex; collisions across two
    // sites are not a concern (each extraction uses its own nonce
    // for its own substitution). Per-extraction collision-resistance
    // requires only that the nonce differs from any literal string
    // in the same input — 32 bits is overkill for that.
    let h = hasher.finish() as u32;
    format!("{h:08x}")
}

/// Walk the markdown line-by-line, replace `:::name` blocks with sentinels.
///
/// Tracks fenced code blocks (` ``` ` and `~~~`) so `:::buttons` inside a
/// code fence stays inert. Currently recognizes `:::subscribe`; other
/// shortcodes are added in Phase B Tasks 8-11. Unrecognized `:::name`
/// blocks pass through verbatim (the legacy string-rewriter still
/// processes them during the staged migration).
pub fn extract_shortcodes(markdown: &str) -> ExtractionResult {
    let nonce = compute_nonce(markdown);
    let mut extracted: Vec<ExtractedShortcode> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let output = extract_with_state(markdown, &nonce, &mut extracted, &mut warnings);
    ExtractionResult {
        markdown_with_placeholders: output,
        extracted,
        nonce,
        warnings,
    }
}

/// Recursive worker for [`extract_shortcodes`]. Walks `markdown`
/// line-by-line and returns the body string with sentinels substituted
/// for typed shortcode blocks. Inner CssRegion / Unknown blocks recurse
/// here so their bodies also get scanned for typed shortcodes — the
/// shared `extracted` and `warnings` accumulators ensure all sentinels
/// across nesting levels share the same nonce and a flat index space.
fn extract_with_state(
    markdown: &str,
    nonce: &str,
    extracted: &mut Vec<ExtractedShortcode>,
    warnings: &mut Vec<String>,
) -> String {
    let mut output = String::with_capacity(markdown.len());
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    let mut in_code_fence = false;
    let mut fence_marker = String::new();

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Track code fences first; do not parse shortcodes inside them.
        if in_code_fence {
            output.push_str(line);
            output.push('\n');
            // `fence_marker` is set non-empty by `detect_code_fence_open`
            // when we entered this state, so `chars().next()` returns
            // `Some` in practice. `unwrap_or(' ')` is a safe degenerate
            // fallback: a literal space could only match a fence-close
            // line if the trimmed line *was* spaces, but `trimmed` has
            // already had its surrounding whitespace stripped, so the
            // is_empty check would still reject it.
            let fence_char = fence_marker.chars().next().unwrap_or(' ');
            if trimmed.starts_with(&fence_marker)
                && trimmed.trim_start_matches(fence_char).trim().is_empty()
            {
                in_code_fence = false;
                fence_marker.clear();
            }
            i += 1;
            continue;
        }
        if let Some(marker) = detect_code_fence_open(trimmed) {
            in_code_fence = true;
            fence_marker = marker;
            output.push_str(line);
            output.push('\n');
            i += 1;
            continue;
        }

        // Try to recognize a `:::name` (or `::::name`, etc.) opener.
        if let Some((arity, name, single_line_args)) = parse_shortcode_opener(trimmed) {
            // Multi-line attribute block support: if the args contain an
            // unclosed `{`, gather subsequent lines into the args string
            // until the brace closes (respecting quoted strings). The
            // body starts on the line AFTER the close-brace line.
            //
            // `:::name {key=value\n  key2=value2\n}` is valid; the
            // attribute parser sees the joined string and treats newlines
            // as whitespace.
            let (args_owned, opener_lines_consumed) =
                gather_multi_line_attrs(single_line_args, &lines[i + 1..]);
            let args: &str = args_owned.as_deref().unwrap_or(single_line_args);
            let body_start = i + 1 + opener_lines_consumed;

            // Look for the matching closer (same arity) on a subsequent line.
            let mut body_lines: Vec<&str> = Vec::new();
            let mut j = body_start;
            let mut closed = false;
            while j < lines.len() {
                if is_close_fence(lines[j].trim(), arity) {
                    closed = true;
                    break;
                }
                body_lines.push(lines[j]);
                j += 1;
            }

            if !closed {
                // Unclosed block: emit verbatim, let the legacy rewriter
                // surface the syntax error.
                output.push_str(line);
                output.push('\n');
                i += 1;
                continue;
            }

            let body = body_lines.join("\n");

            // Branch on the recognized name:
            //
            // 1. Pure-CSS region (empty name, e.g. `:::{.tagline}`) — emit
            //    a plain `<div class="...">` wrapper around the body markdown.
            //    Pulldown-cmark processes the body naturally because we
            //    insert blank lines around it.
            //
            // 2. Typed-known name (subscribe / buttons / gallery / hero / grid)
            //    — extract into the typed AST and substitute a sentinel.
            //    Parse-time deprecation warnings (e.g. legacy `---` dividers
            //    in grid, body-image fallback in hero) are threaded back via
            //    the warnings vector.
            //
            // 3. Anything else — render as a `moss-unknown-shortcode` div
            //    around the body markdown and emit a build warning.
            if name.is_empty() {
                // CssRegion (Task D). Recurse into the body so typed
                // shortcodes nested inside the styling wrapper (the
                // common SoCiviC pattern of `:::{.support-band}` around
                // `::::buttons`) also get extracted into sentinels.
                // Higher-arity inner blocks survive because the outer
                // closer-search only matches the outer's exact arity;
                // the recursive call then handles the inner.
                let parsed = super::attrs::parse_attrs(args).unwrap_or_default();
                let body_processed = extract_with_state(&body, nonce, extracted, warnings);
                output.push_str(&render_div_open(&parsed.classes, parsed.id.as_deref(), None));
                output.push_str("\n\n");
                output.push_str(&body_processed);
                if !body_processed.is_empty() && !body_processed.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("\n</div>\n");
                i = j + 1;
                continue;
            }

            if is_typed_known(name) {
                if let (Some(sc), parse_warnings) = parse_shortcode_block(name, args, &body) {
                    warnings.extend(parse_warnings);
                    let index = extracted.len();
                    output.push_str(&placeholder_for(&nonce, index));
                    output.push('\n');
                    extracted.push(ExtractedShortcode {
                        index,
                        shortcode: sc,
                    });
                    i = j + 1;
                    continue;
                }
                // Should not happen — typed-known is a closed set of
                // names handled by parse_shortcode_block. Fall through
                // to verbatim emission as defense-in-depth.
                output.push_str(line);
                output.push('\n');
                i += 1;
                continue;
            }

            // Unknown name (Task E): wrap the body in a fallback div and
            // emit a build warning so authors see misspellings. Recurse
            // into the body so a misspelled outer doesn't strand any
            // valid typed shortcodes nested inside it.
            let parsed = super::attrs::parse_attrs(args).unwrap_or_default();
            warnings.push(format!("unknown shortcode `:::{}`", name));
            let mut classes = vec!["moss-unknown-shortcode".to_string()];
            classes.extend(parsed.classes.iter().cloned());
            let extra_attrs = format!(r#" data-name="{}""#, html_escape_attr(name));
            let body_processed = extract_with_state(&body, nonce, extracted, warnings);
            output.push_str(&render_div_open(&classes, parsed.id.as_deref(), Some(&extra_attrs)));
            output.push_str("\n\n");
            output.push_str(&body_processed);
            if !body_processed.is_empty() && !body_processed.ends_with('\n') {
                output.push('\n');
            }
            output.push_str("\n</div>\n");
            i = j + 1;
            continue;
        }

        // Regular content line.
        output.push_str(line);
        output.push('\n');
        i += 1;
    }

    output
}

/// Render the opening `<div>` tag for a CssRegion or Unknown wrapper.
///
/// `extra_attrs` (already with leading space) is appended before `>`,
/// used by the unknown-name renderer to add `data-name="..."`.
fn render_div_open(classes: &[String], id: Option<&str>, extra_attrs: Option<&str>) -> String {
    let mut out = String::from("<div");
    if !classes.is_empty() {
        out.push_str(" class=\"");
        for (i, c) in classes.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&html_escape_attr(c));
        }
        out.push('"');
    }
    if let Some(id_val) = id {
        out.push_str(" id=\"");
        out.push_str(&html_escape_attr(id_val));
        out.push('"');
    }
    if let Some(extra) = extra_attrs {
        out.push_str(extra);
    }
    out.push('>');
    out
}

/// HTML-attribute-safe escape. Replaces the five XML special characters
/// so attribute values can't break out of `"..."` or close the tag.
fn html_escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn detect_code_fence_open(trimmed: &str) -> Option<String> {
    if trimmed.starts_with("```") {
        Some("```".to_string())
    } else if trimmed.starts_with("~~~") {
        Some("~~~".to_string())
    } else {
        None
    }
}

/// Parse an opening fence line into (colon_count, name, args). Returns
/// `None` if the line is not an opener.
///
/// Accepts any colon count >= 3 (`:::name`, `::::name`, `:::::name`, ...).
/// The colon count is preserved so the closer must match the same arity
/// (allows nested shortcodes like `::::buttons` inside `:::grid`).
///
/// **Pure-CSS region opener** — `:::{.class}` (no name, attrs only) is
/// also recognized. The returned `name` is empty, signaling the caller
/// to render the block as a plain styling wrapper. Empty name without
/// a following `{` is rejected (just colons followed by content is not
/// an opener).
fn parse_shortcode_opener(trimmed: &str) -> Option<(usize, &str, &str)> {
    let colons = trimmed.chars().take_while(|&c| c == ':').count();
    if colons < 3 {
        return None;
    }
    // char-aligned: `colons` is a count of ASCII ':' chars (each 1 byte in
    // UTF-8), so the byte offset equals the char count and lands on a
    // char boundary.
    #[allow(clippy::string_slice)]
    let rest = &trimmed[colons..];
    // Name = letters/digits/underscores/hyphens; rest of line is args.
    let name_end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    if name_end == 0 {
        // No name. Pure-CSS region grammar requires the rest to start
        // with `{` (after whitespace).
        let after_ws = rest.trim_start();
        if !after_ws.starts_with('{') {
            return None;
        }
        return Some((colons, "", rest.trim()));
    }
    // char-aligned: name_end is a byte index returned by str::find with a
    // char predicate, which is guaranteed to be a char boundary (or rest.len()).
    #[allow(clippy::string_slice)]
    let name = &rest[..name_end];
    #[allow(clippy::string_slice)]
    let args = rest[name_end..].trim();
    Some((colons, name, args))
}

/// True if `trimmed` is a closing fence with the specified arity (`:::`
/// for arity 3, `::::` for arity 4, etc.).
///
/// Closer semantics: N colons followed by optional whitespace only. A
/// line like `::: extra` is NOT a closer (it's body content). This was
/// the legacy `parse_fence_close` contract; the typed extractor preserves
/// it so author content with trailing text after `:::` still parses the
/// same way.
///
/// Implemented via char iteration (NOT `split_at(arity)`) because the
/// `arity` is a count of `:` characters (always ASCII, 1 byte each), but
/// the `trimmed` line might start with multi-byte UTF-8 characters
/// (e.g. `[申请测试版](...)` from Chinese-language buttons). `split_at`
/// is byte-indexed and would panic mid-character on such lines. Char
/// iteration sidesteps the issue and is also slightly faster — we early-exit
/// on the first non-`:` character.
fn is_close_fence(trimmed: &str, arity: usize) -> bool {
    let mut chars = trimmed.chars();
    for _ in 0..arity {
        match chars.next() {
            Some(':') => {}
            _ => return false,
        }
    }
    // Remaining chars (if any) must all be whitespace.
    chars.all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_shortcodes_round_trips_input() {
        let md = "# Heading\n\npara with [link](u).\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.markdown_with_placeholders, md);
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn extracts_subscribe_block_with_placeholder_and_button_attrs() {
        let md = r#":::subscribe {placeholder="you@domain.com" button="Sign me up"}
:::
"#;
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.placeholder.as_deref(), Some("you@domain.com"));
                assert_eq!(args.button.as_deref(), Some("Sign me up"));
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
        assert!(result
            .markdown_with_placeholders
            .contains(&placeholder_for(&result.nonce, 0)));
        assert!(!result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn extracts_subscribe_block_with_only_placeholder_attr() {
        let md = r#":::subscribe {placeholder="hi@example.com"}
:::
"#;
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.placeholder.as_deref(), Some("hi@example.com"));
                assert!(args.button.is_none());
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    #[test]
    fn extracts_subscribe_block_with_no_args() {
        let md = ":::subscribe\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert!(args.placeholder.is_none());
                assert!(args.button.is_none());
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    #[test]
    fn extracts_subscribe_block_with_multi_line_attrs() {
        let md = r#":::subscribe {
  placeholder="you@domain.com"
  button="Request access"
}
:::
"#;
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert_eq!(args.placeholder.as_deref(), Some("you@domain.com"));
                assert_eq!(args.button.as_deref(), Some("Request access"));
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    #[test]
    fn subscribe_legacy_body_keys_no_longer_parsed() {
        // The pre-grammar form `description: ...` / `button: ...` body
        // lines are no longer recognized. moss-releases content is
        // rewritten in Step 3; this test pins the post-cut behavior.
        let md = ":::subscribe\ndescription: Get updates\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(args) => {
                assert!(args.placeholder.is_none(), "old description body must not populate placeholder");
                assert!(args.button.is_none());
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    #[test]
    fn subscribe_inside_code_fence_is_not_extracted() {
        // Adversarial: `:::subscribe` inside a fenced code block is just
        // documentation text. The extractor must not treat it as a
        // shortcode.
        let md = "```\n:::subscribe\ndescription: doc\n:::\n```\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn subscribe_inside_tilde_fence_is_not_extracted() {
        let md = "~~~\n:::subscribe\n:::\n~~~\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn unclosed_subscribe_block_emits_verbatim() {
        // Unclosed: emit source verbatim so the author sees the typo.
        let md = ":::subscribe\nbutton: Go\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn extracts_hero_block_with_body_image_typed() {
        // Step 2: :::hero is now a typed variant. The extractor consumes
        // it and produces Shortcode::Hero with the body-image fallback
        // populating args.image when no `image=` attribute is present.
        let md = ":::hero\n![[bg.jpg]]\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "bg.jpg"),
                _ => panic!("expected Unresolved bg.jpg"),
            },
            _ => panic!("expected Hero"),
        }
        // The literal `:::hero` should be replaced by a sentinel.
        assert!(!result.markdown_with_placeholders.contains(":::hero"));
    }

    #[test]
    fn extracts_multiple_subscribes_with_increasing_indices() {
        let md = ":::subscribe\ndescription: a\n:::\n\nsome text\n\n:::subscribe\nbutton: b\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 2);
        assert_eq!(result.extracted[0].index, 0);
        assert_eq!(result.extracted[1].index, 1);
        assert!(result
            .markdown_with_placeholders
            .contains(&placeholder_for(&result.nonce, 0)));
        assert!(result
            .markdown_with_placeholders
            .contains(&placeholder_for(&result.nonce, 1)));
    }

    #[test]
    fn parse_placeholder_round_trips_index() {
        let nonce = "deadbeef";
        for index in [0, 1, 5, 99] {
            let s = placeholder_for(nonce, index);
            assert_eq!(parse_placeholder(nonce, &s), Some(index));
        }
    }

    #[test]
    fn parse_placeholder_rejects_non_placeholder_html() {
        let nonce = "deadbeef";
        assert!(parse_placeholder(nonce, "<div>hi</div>").is_none());
        assert!(parse_placeholder(nonce, "<!--just a comment-->").is_none());
    }

    #[test]
    fn parse_placeholder_rejects_wrong_nonce() {
        // Authored content collision case: an author writes a literal
        // <!--MOSS_SC_*_0--> in their markdown. parse_placeholder requires
        // the same nonce as this extraction's session, so a mismatched
        // nonce returns None. This forecloses the authored-content
        // namespace collision.
        let s = placeholder_for("aaaa1111", 5);
        assert_eq!(parse_placeholder("bbbb2222", &s), None);
    }

    #[test]
    fn extract_uses_content_derived_nonce() {
        // The nonce is deterministic per input — calling extract_shortcodes
        // twice on the same input produces the same nonce.
        let md = ":::subscribe\n:::\n";
        let r1 = extract_shortcodes(md);
        let r2 = extract_shortcodes(md);
        assert_eq!(r1.nonce, r2.nonce);
        // Different inputs produce different nonces (with overwhelming
        // probability — collision impossible to exhibit here).
        let r3 = extract_shortcodes(":::subscribe\ndescription: x\n:::\n");
        assert_ne!(r1.nonce, r3.nonce);
    }

    #[test]
    fn nonce_makes_authored_collision_inert() {
        // If an author writes a literal placeholder-shape comment, my
        // nonce will differ from theirs, so the substitution leaves
        // their text alone.
        let md = ":::subscribe\n:::\n\nLook: <!--MOSS_SC_00000000_0-->\n";
        let result = extract_shortcodes(md);
        // The author's comment survives because the embedded nonce
        // differs from the computed one (probability of collision = 1/2^32).
        assert_ne!(result.nonce, "00000000");
        assert!(result
            .markdown_with_placeholders
            .contains("MOSS_SC_00000000_0"));
    }

    #[test]
    fn parse_shortcode_opener_recognizes_simple_name() {
        assert_eq!(
            parse_shortcode_opener(":::subscribe"),
            Some((3, "subscribe", ""))
        );
    }

    #[test]
    fn parse_shortcode_opener_extracts_args() {
        assert_eq!(
            parse_shortcode_opener(":::grid 3 1:2:1"),
            Some((3, "grid", "3 1:2:1"))
        );
    }

    #[test]
    fn parse_shortcode_opener_recognizes_quadruple_colon() {
        // ::::buttons is the standard way to nest a shortcode inside
        // a :::grid cell. The arity is preserved so the closer matches.
        assert_eq!(
            parse_shortcode_opener("::::buttons"),
            Some((4, "buttons", ""))
        );
    }

    #[test]
    fn parse_shortcode_opener_rejects_two_colons() {
        // Two colons is not a fence.
        assert!(parse_shortcode_opener("::name").is_none());
    }

    #[test]
    fn extracts_quadruple_colon_buttons() {
        // ::::buttons inside hypothetical grid context. We just test the
        // extractor in isolation; grid integration lands in Task 11.
        let md = "::::buttons\n[Tickets](go/)\n::::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Tickets");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_grid_with_nested_buttons_via_arity() {
        // SoCiviC pattern: `::::buttons` (4-colon) nested inside `:::grid`
        // (3-colon). Phase 4 PR4.5 (2026-05-28) promoted cells from raw
        // markdown strings to `Vec<Vec<Block>>`. The inner `::::buttons`
        // now extracts into a typed `Block::Shortcode(Buttons)` inside the
        // cell at parse time (via the recursive `super::parser::parse` call
        // in `parse_cell_to_blocks`), not at render time.
        let md = ":::grid 2\n::::buttons\n[Tickets](go/)\n::::\n+++\nfooter cell\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 2);
                assert_eq!(grid.cells.len(), 2);
                // First cell now carries a typed Shortcode::Buttons block.
                let has_typed_buttons = grid.cells[0].iter().any(|b| matches!(
                    b,
                    Block::Shortcode(Shortcode::Buttons(args)) if args.items.len() == 1
                        && args.items[0].text == "Tickets"
                ));
                assert!(has_typed_buttons, "expected typed Buttons in cell[0]; got {:?}", grid.cells[0]);
                // Second cell is the footer paragraph.
                let has_footer_para = grid.cells[1].iter().any(|b| matches!(
                    b,
                    Block::Paragraph(inlines) if inlines.iter().any(|i| matches!(
                        i,
                        super::super::node::Inline::Text(t) if t.contains("footer cell")
                    ))
                ));
                assert!(has_footer_para, "expected footer paragraph in cell[1]; got {:?}", grid.cells[1]);
            }
            other => panic!("expected Grid, got {other:?}"),
        }
        // The literal ::: markers don't survive verbatim — they're in the
        // typed Grid's body now.
        assert!(!result.markdown_with_placeholders.contains(":::grid 2"));
    }

    #[test]
    fn arity_mismatch_does_not_close_block() {
        // A `:::` closer inside a `::::buttons` block must NOT close it.
        // Body content can contain `:::` strings as text (in code blocks
        // or grid cell separators when nested differently).
        let md = "::::buttons\n[t](u)\n:::\n[t2](u2)\n::::\n";
        let result = extract_shortcodes(md);
        // Only one extraction (the ::::buttons block).
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                // Both links should be captured (the `:::` was just body text).
                assert_eq!(args.items.len(), 2);
            }
            _ => panic!("expected Buttons"),
        }
    }

    // ---- Buttons (Phase B Task 8) ----

    #[test]
    fn extracts_buttons_block_with_one_link() {
        let md = ":::buttons\n[Documentation](docs/)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert!(args.classes.is_empty());
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Documentation");
                match &args.items[0].url {
                    Url::Unresolved(s) => assert_eq!(s, "docs/"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_buttons_block_with_multiple_links() {
        let md = ":::buttons\n[Docs](docs/)\n[GitHub](https://github.com)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].text, "Docs");
                assert_eq!(args.items[1].text, "GitHub");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_buttons_block_with_class_attrs() {
        let md = ":::buttons {.primary .large}\n[Go](go/)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.classes, "primary large");
                assert_eq!(args.items.len(), 1);
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_buttons_with_moss_resolved_url_intact() {
        // The upstream resolve pipeline rewrites internal links to
        // moss-resolved:foo.md before the AST sees them. The extractor
        // must preserve the prefix verbatim — visit_urls_mut classifies it.
        let md = ":::buttons\n[Docs](moss-resolved:docs/index.md)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => match &args.items[0].url {
                Url::Unresolved(s) => assert_eq!(s, "moss-resolved:docs/index.md"),
                _ => panic!("expected Unresolved"),
            },
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn buttons_skips_non_link_lines() {
        // Non-link lines (commentary, blank lines) are silently skipped
        // — matches the legacy rewriter behavior.
        let md = ":::buttons\nNot a link, just text.\n[Real](real/)\n\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Real");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn buttons_inside_code_fence_is_not_extracted() {
        let md = "```\n:::buttons\n[t](u)\n:::\n```\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn extract_markdown_link_rejects_text_with_close_bracket() {
        // Pinning test (code-review P2): the parser uses find(']') for the
        // first close-bracket. A link text containing ']' silently fails
        // to parse and the line is skipped (silently — matches legacy
        // shortcode.rs::extract_markdown_link). If/when this is relaxed,
        // this test fails and the change is deliberate.
        let md = ":::buttons\n[a]b](u)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => assert!(args.items.is_empty()),
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extract_markdown_link_requires_trailing_paren() {
        // Pinning test: trailing content after `)` causes the link to be
        // rejected (matches legacy behavior).
        let md = ":::buttons\n[t](u) <!-- trailing -->\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => assert!(args.items.is_empty()),
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn close_fence_with_trailing_whitespace_is_recognized() {
        // Whitespace after the colons is allowed (matches legacy
        // parse_fence_close at shortcode.rs:857).
        let md = ":::subscribe\nbutton: x\n:::   \n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
    }

    #[test]
    fn is_close_fence_handles_multibyte_utf8_lines() {
        // Regression test for the moss-releases panic at byte index 3
        // (inside `申`) of `[申请测试版](#青苔正在封闭测试)`. The buggy
        // `split_at(arity)` was byte-indexed; this line happens to be
        // longer than 3 bytes but the first 3 bytes land mid-character
        // because `[` (1 byte) + `申` (3 bytes, bytes 1..4). Char-based
        // iteration sidesteps the issue.
        assert!(!is_close_fence("[申请测试版](#青苔正在封闭测试)", 3));
        assert!(!is_close_fence("[申请测试版](#青苔正在封闭测试)", 4));
        // CJK lines that would have panicked the old split_at variant.
        assert!(!is_close_fence("中文内容", 3));
        assert!(!is_close_fence("日本語", 3));
        // Truly closing lines still match.
        assert!(is_close_fence(":::", 3));
        assert!(is_close_fence("::::", 4));
    }

    #[test]
    fn extract_shortcodes_handles_buttons_with_cjk_link_text() {
        // End-to-end regression for the moss-releases site bug: a
        // :::buttons block containing a markdown link with CJK text
        // and a CJK URL anchor. The extractor must not panic, must
        // extract the buttons block, and must capture both items.
        let md = ":::buttons\n[申请测试版](#青苔正在封闭测试)\n[文档](docs/)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].text, "申请测试版");
                match &args.items[0].url {
                    Url::Unresolved(s) => assert_eq!(s, "#青苔正在封闭测试"),
                    _ => panic!("expected Unresolved"),
                }
                assert_eq!(args.items[1].text, "文档");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extract_shortcodes_does_not_panic_on_arbitrary_cjk_content() {
        // Smoke test against the shape that triggered the moss-releases
        // panic: a document with mixed CJK content INCLUDING lines that
        // start with multi-byte characters but happen to have byte
        // length ≥ arity. None of these are close-fence candidates;
        // the extractor must scan past them without panic.
        let md = "# 标题\n\n中文段落，混合 English 单词。\n\n:::buttons\n[申请测试版](#锚点)\n:::\n\n## 二级标题\n\n更多内容。\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
    }

    #[test]
    fn close_fence_with_trailing_text_is_not_recognized() {
        // P1 #2 fix: `::: more text` does NOT close the block. Without
        // this match against legacy semantics, an author who pasted text
        // after the closer would see different behavior between the
        // typed-AST path and the legacy grid parser. Using buttons here
        // because subscribe under the unified grammar reads attrs only.
        let md = ":::buttons\n[a](u)\n::: more text\n[b](v)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        // Both links should be in the buttons body — the first `:::` is
        // body content; the second `:::` is the closer.
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].text, "a");
                assert_eq!(args.items[1].text, "b");
            }
            _ => panic!("expected Buttons"),
        }
    }

    // ---- Gallery (Phase B Task 9) ----

    #[test]
    fn extracts_gallery_with_bare_paths() {
        let md = ":::gallery\nphoto1.jpg\nphoto2.png\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert!(args.columns.is_none());
                assert_eq!(args.items.len(), 2);
                assert_eq!(args.items[0].alt, "");
                match &args.items[0].src {
                    Url::Unresolved(s) => assert_eq!(s, "photo1.jpg"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_columns_arg() {
        let md = ":::gallery 4\na.jpg\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => assert_eq!(args.columns, Some(4)),
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_classes() {
        let md = ":::gallery 3 {.showcase}\na.jpg\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.columns, Some(3));
                assert_eq!(args.classes, "showcase");
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_markdown_image_syntax() {
        let md = ":::gallery\n![A photo](photo.jpg)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.items[0].alt, "A photo");
                match &args.items[0].src {
                    Url::Unresolved(s) => assert_eq!(s, "photo.jpg"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn extracts_gallery_with_pipe_attrs() {
        let md = ":::gallery\nphoto.jpg|cover top\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.items[0].attrs, "cover top");
                match &args.items[0].src {
                    Url::Unresolved(s) => assert_eq!(s, "photo.jpg"),
                    _ => panic!("expected Unresolved"),
                }
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn gallery_skips_blank_lines() {
        let md = ":::gallery\n\na.jpg\n\nb.jpg\n\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => assert_eq!(args.items.len(), 2),
            _ => panic!("expected Gallery"),
        }
    }

    // ---- Multi-line attribute blocks (Step 1 Task B) ----
    // Low-level brace_depth and gather_multi_line_attrs unit tests live
    // in `attrs.rs` next to those helpers. The tests below pin the
    // extractor's end-to-end behavior on multi-line attribute blocks.

    #[test]
    fn extracts_buttons_with_multi_line_attrs() {
        // The attribute block spans three source lines; the body starts
        // after the closing brace's line.
        let md = ":::buttons {\n  .primary\n}\n[Go](go/)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.classes, "primary");
                assert_eq!(args.items.len(), 1);
                assert_eq!(args.items[0].text, "Go");
            }
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn extracts_gallery_with_multi_line_attrs() {
        let md = ":::gallery {\n  .showcase\n}\nphoto.jpg\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Gallery(args) => {
                assert_eq!(args.classes, "showcase");
                assert_eq!(args.items.len(), 1);
            }
            _ => panic!("expected Gallery"),
        }
    }

    #[test]
    fn multi_line_attrs_with_quoted_brace_inside() {
        // The `}` inside a quoted value must NOT close the attr block.
        // The block legitimately closes on the third line.
        let md = ":::buttons {\n  .a\n  .b\n}\n[Go](go/)\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                // Multi-line splits both classes — same as space-separated form.
                assert_eq!(args.classes, "a b");
            }
            _ => panic!("expected Buttons"),
        }
    }

    // ---- Pure-CSS regions (Step 1 Task D) ----

    #[test]
    fn css_region_unnamed_emits_div_wrapper() {
        let md = ":::{.tagline}\nA new way to publish.\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty());
        assert!(result
            .markdown_with_placeholders
            .contains("<div class=\"tagline\">"));
        assert!(result
            .markdown_with_placeholders
            .contains("A new way to publish."));
        assert!(result.markdown_with_placeholders.contains("</div>"));
    }

    #[test]
    fn css_region_with_id_only() {
        let md = ":::{#intro}\nIntro prose.\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result
            .markdown_with_placeholders
            .contains("<div id=\"intro\">"));
    }

    #[test]
    fn css_region_with_classes_and_id() {
        let md = ":::{.callout #important}\nWatch out.\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        assert!(out.contains("<div"));
        assert!(out.contains("class=\"callout\""));
        assert!(out.contains("id=\"important\""));
    }

    #[test]
    fn css_region_emits_blank_lines_around_body_for_markdown_processing() {
        // Pulldown-cmark needs a blank line between the `<div>` and the
        // body to treat the body as markdown rather than raw HTML.
        let md = ":::{.foo}\n# Heading\n:::\n";
        let out = extract_shortcodes(md).markdown_with_placeholders;
        // The `<div>` line is followed by a blank line.
        assert!(out.contains(">\n\n# Heading"));
        // The closing `</div>` is preceded by a blank line.
        assert!(out.contains("# Heading\n\n</div>"));
    }

    #[test]
    fn css_region_no_warning_emitted() {
        let md = ":::{.foo}\nbody\n:::\n";
        assert!(extract_shortcodes(md).warnings.is_empty());
    }

    // ---- Unknown-name fallback (Step 1 Task E) ----

    #[test]
    fn unknown_name_renders_fallback_wrapper() {
        let md = ":::nope {.extra}\nbody text\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        assert!(out.contains("class=\"moss-unknown-shortcode extra\""));
        assert!(out.contains(r#"data-name="nope""#));
        assert!(out.contains("body text"));
    }

    #[test]
    fn unknown_name_emits_build_warning() {
        let md = ":::nope\n:::\n";
        let warnings = extract_shortcodes(md).warnings;
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("nope"));
    }

    #[test]
    fn unknown_name_html_escapes_data_name() {
        // Defense: a maliciously crafted name (which the opener parser
        // wouldn't actually accept since names are [A-Za-z0-9_-]) shouldn't
        // be able to break out of the attribute. This test pins the
        // escape regardless.
        let md = ":::weird-name\nbody\n:::\n";
        let out = extract_shortcodes(md).markdown_with_placeholders;
        assert!(out.contains(r#"data-name="weird-name""#));
    }

    // Grid left LEGACY_PASSTHROUGH in Step 2b — it's now a typed variant.
    // Coverage moved to the Grid section below (extracts_grid_*).

    #[test]
    fn extracts_grid_with_positional_columns() {
        // Legacy moss-releases form: `:::grid 2` (positional column count)
        // with `---` cell divider. Both the positional cols and the legacy
        // `---` divider are accepted during the migration window.
        //
        // Phase 4 PR4.5 (2026-05-28): cells are now typed Vec<Vec<Block>>.
        // Each "cell A" / "cell B" parses to a single Paragraph block.
        let md = ":::grid 2\ncell A\n---\ncell B\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 2);
                assert!(grid.ratio.is_none());
                assert_eq!(grid.cells.len(), 2);
                assert_paragraph_text(&grid.cells[0], "cell A");
                assert_paragraph_text(&grid.cells[1], "cell B");
            }
            other => panic!("expected Grid, got {other:?}"),
        }
    }

    /// Test helper: assert that `cell_blocks` is a single `Block::Paragraph`
    /// whose inline text content (concatenated) equals `expected`.
    ///
    /// PR4.5 cells parse via pulldown-cmark; trivial cells like `"A"` yield
    /// `[Block::Paragraph(vec![Inline::Text("A".into())])]`.
    fn assert_paragraph_text(cell_blocks: &[Block], expected: &str) {
        if cell_blocks.is_empty() && expected.is_empty() {
            return;
        }
        let para = match cell_blocks {
            [Block::Paragraph(inlines)] => inlines,
            other => panic!(
                "expected single Paragraph cell with text {expected:?}, got: {other:?}"
            ),
        };
        let mut text = String::new();
        for inline in para {
            match inline {
                super::super::node::Inline::Text(t) => text.push_str(t),
                super::super::node::Inline::Code(c) => text.push_str(c),
                _ => {}
            }
        }
        assert_eq!(text, expected, "cell text mismatch");
    }

    #[test]
    fn extracts_grid_with_positional_ratio() {
        let md = ":::grid 2 1:2\nleft\n---\nright\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 2);
                assert_eq!(grid.ratio.as_deref(), Some("1:2"));
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_with_cols_attr_integer() {
        let md = ":::grid {cols=3}\nA\n+++\nB\n+++\nC\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 3);
                assert_eq!(grid.cells.len(), 3);
                assert_paragraph_text(&grid.cells[0], "A");
                assert_paragraph_text(&grid.cells[1], "B");
                assert_paragraph_text(&grid.cells[2], "C");
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_with_cols_attr_ratio_implies_count() {
        let md = ":::grid {cols=1:1:2}\nA\n+++\nB\n+++\nC\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 3, "ratio length implies column count");
                assert_eq!(grid.ratio.as_deref(), Some("1:1:2"));
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_accepts_plus_plus_plus_divider() {
        let md = ":::grid 2\nA\n+++\nB\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.cells.len(), 2);
                assert_paragraph_text(&grid.cells[0], "A");
                assert_paragraph_text(&grid.cells[1], "B");
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_with_classes() {
        let md = ":::grid 3 {.work-cards .featured}\nA\n---\nB\n---\nC\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 3);
                assert_eq!(grid.classes, "work-cards featured");
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_single_cell_no_separator() {
        let md = ":::grid 1\nonly cell\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 1);
                assert_eq!(grid.cells.len(), 1);
                assert_paragraph_text(&grid.cells[0], "only cell");
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_with_empty_middle_cell() {
        // Two consecutive `+++` dividers leave a middle cell empty.
        // Legacy behavior preserved this; verify the typed extractor
        // does too. PR4.5: empty cells are `Vec<Block>::new()` (the
        // parser sees no content and emits zero blocks).
        let md = ":::grid 3\nA\n+++\n+++\nC\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.cells.len(), 3);
                assert_paragraph_text(&grid.cells[0], "A");
                assert!(grid.cells[1].is_empty(), "empty cell should have no blocks");
                assert_paragraph_text(&grid.cells[2], "C");
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn nested_grid_via_arity_is_unsupported_authoring() {
        // Pinning test: `::::grid` (arity 4) wrapping `:::grid` (arity 3)
        // does NOT cleanly nest. The outer fence's body captures the
        // inner literally, but `split_grid_cells` then splits the outer's
        // body on the inner's `+++` divider — mis-attributing the inner's
        // cells to the outer. There's no separate "nested-cell-divider"
        // syntax in moss, so this nesting pattern isn't supported.
        //
        // Authors who need a "grid inside a grid" should use a CSS region
        // wrapper (`::::{.outer-grid}`) and set CSS-only column rules.
        // This test pins the actual extraction behavior so a future
        // refactor that changes it is visible.
        let md = "::::grid 1\n:::grid 2\nA\n+++\nB\n:::\n::::\n";
        let result = extract_shortcodes(md);
        // The outer ::::grid is extracted; the inner is captured as
        // literal text and the +++ inside the inner triggers the outer's
        // own cell split. The inner Grid is NOT a top-level entry.
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(outer) => {
                assert_eq!(outer.columns, 1);
                // The +++ in the inner's body split the OUTER's cells,
                // which is the documented limitation.
                assert!(outer.cells.len() >= 2,
                    "outer's body got split by inner's +++, demonstrating the \
                     unsupported-nesting failure mode");
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_with_compound_link_cell_typed_as_link_card() {
        // SoCiviC pattern: a cell whose entire body is a single markdown
        // link wrapping multiple block children. Phase 4 PR4.5
        // (2026-05-28) detects this at the cell-string level (before
        // pulldown-cmark, which can't represent `[heading](url)`) and
        // emits a typed [`Block::LinkCard { url, children }`] with the
        // inner content parsed as blocks. The second cell is a plain
        // markdown link that fits the compound shape too (single line,
        // no block children) — also typed as `Block::LinkCard`.
        let md = ":::grid 2 {.work-cards}\n[![[poster.jpg]]\n#### Title\nbody](/url)\n+++\n[Card 2](/url2)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.classes, "work-cards");
                assert_eq!(grid.cells.len(), 2);
                match &grid.cells[0][..] {
                    [Block::LinkCard { url, children }] => {
                        match url {
                            Url::Unresolved(u) => assert_eq!(u, "/url"),
                            _ => panic!("expected Unresolved /url"),
                        }
                        // children should include a Paragraph (with image)
                        // and a Heading (#### Title) — non-empty proves
                        // the inner block-parse ran.
                        assert!(!children.is_empty(), "compound-link inner blocks empty");
                    }
                    other => panic!("expected single LinkCard cell, got {other:?}"),
                }
                match &grid.cells[1][..] {
                    [Block::LinkCard { url, .. }] => match url {
                        Url::Unresolved(u) => assert_eq!(u, "/url2"),
                        _ => panic!("expected Unresolved /url2"),
                    },
                    other => panic!("expected LinkCard for cell[1], got {other:?}"),
                }
            }
            _ => panic!("expected Grid"),
        }
    }

    // Hero left LEGACY_PASSTHROUGH in Step 2 — it's now a typed variant.
    // The replacement test (`extracts_hero_block_with_no_image`) lives in
    // the Hero section above.

    #[test]
    fn toc_now_renders_as_unknown_shortcode() {
        // Step 2c removed `:::toc` without replacement. Sites still using
        // it fall through to the moss-unknown-shortcode wrapper with a
        // build warning. moss-releases content rewrite (Step 3) deletes
        // its 3 :::toc blocks.
        let md = ":::toc\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result.extracted.is_empty(), "toc is no longer typed");
        assert_eq!(result.warnings.len(), 1, "unknown-name fallback warning");
        assert!(result.warnings[0].contains("toc"));
        assert!(result
            .markdown_with_placeholders
            .contains(r#"data-name="toc""#));
    }

    // ---- Hero (Step 2) ----

    #[test]
    fn extracts_hero_block_with_no_image() {
        let md = ":::hero\n# A House of Daowu\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1, "hero should be extracted");
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                assert!(args.image.is_none());
                assert_eq!(args.overlay_text, "# A House of Daowu");
            }
            other => panic!("expected Hero, got {other:?}"),
        }
        // The literal `:::hero` should not survive in the output.
        assert!(!result.markdown_with_placeholders.contains(":::hero"));
    }

    #[test]
    fn extracts_hero_block_with_wikilink_body_image() {
        let md = ":::hero\n![[panorama.jpg]]\n# Welcome\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                match &args.image {
                    Some(Url::Unresolved(s)) => assert_eq!(s, "panorama.jpg"),
                    other => panic!("expected Unresolved url, got {other:?}"),
                }
                assert_eq!(args.overlay_text, "# Welcome");
            }
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn extracts_hero_block_with_image_attr() {
        let md = ":::hero {image=cover.jpg}\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                match &args.image {
                    Some(Url::Unresolved(s)) => assert_eq!(s, "cover.jpg"),
                    other => panic!("expected Unresolved, got {other:?}"),
                }
                assert_eq!(args.overlay_text, "# Title");
            }
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn extracts_hero_block_with_image_attr_and_pipe_attrs() {
        // The pipe character isn't in the bareword set, so values containing
        // `|` must be quoted under the unified grammar.
        let md = r#":::hero {image="cover.jpg|contain top"}
:::
"#;
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                match &args.image {
                    Some(Url::Unresolved(s)) => assert_eq!(s, "cover.jpg"),
                    _ => panic!("expected Unresolved"),
                }
                assert_eq!(args.attrs, "contain top");
            }
            _ => panic!("expected Hero"),
        }
    }

    #[test]
    fn extracts_hero_block_with_classes() {
        let md = ":::hero {.full .center}\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                assert_eq!(args.classes, "full center");
            }
            _ => panic!("expected Hero"),
        }
    }

    #[test]
    fn extracts_hero_block_with_directive_line_path() {
        // Legacy syntax used by Yi-website and chps-site:
        // `:::hero ./path.jpg` (image path on the directive line, empty body).
        // Step 3 rewrites these blocks to `:::hero {image=./path.jpg}`,
        // but the typed extractor must keep producing the same Hero AST
        // node until then to avoid silently dropping the homepage hero.
        let md = ":::hero ./assets/header.png\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => match &args.image {
                Some(Url::Unresolved(s)) => assert_eq!(s, "./assets/header.png"),
                other => panic!("expected Unresolved ./assets/header.png, got {other:?}"),
            },
            _ => panic!("expected Hero"),
        }
    }

    #[test]
    fn extracts_hero_block_with_directive_line_path_and_pipe_attrs() {
        let md = ":::hero ./bg.jpg|contain top\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                match &args.image {
                    Some(Url::Unresolved(s)) => assert_eq!(s, "./bg.jpg"),
                    _ => panic!("expected Unresolved"),
                }
                assert_eq!(args.attrs, "contain top");
            }
            _ => panic!("expected Hero"),
        }
    }

    #[test]
    fn extracts_hero_block_with_directive_line_path_and_classes() {
        // `:::hero ./path.jpg {.landing}` — directive-line path AND
        // an attribute block (classes only, no `image=` to avoid conflict).
        let md = ":::hero ./bg.jpg {.landing}\n# Welcome\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                match &args.image {
                    Some(Url::Unresolved(s)) => assert_eq!(s, "./bg.jpg"),
                    _ => panic!("expected Unresolved"),
                }
                assert_eq!(args.classes, "landing");
                assert_eq!(args.overlay_text, "# Welcome");
            }
            _ => panic!("expected Hero"),
        }
    }

    // ---- Adversarial cases for Step 1 (D/E semantics) ----

    #[test]
    fn nested_css_region_outer_closes_at_first_inner_close() {
        // Pinning test: same-arity nested `:::{.outer}` containing
        // `:::{.inner}` is NOT a Step 1 feature. The outer block closes at
        // the inner block's `:::` because both fences are arity 3.
        // Authors who need nesting must use mismatched arities
        // (`::::{.outer}` containing `:::{.inner}`).
        //
        // This test pins the current behavior so a future regression
        // surfaces.
        let md = ":::{.outer}\n:::{.inner}\nbody\n:::\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        // The outer `<div class="outer">` opens.
        assert!(out.contains("<div class=\"outer\""));
        // The inner `:::{.inner}` opener is left as literal text in the
        // outer body — the outer fence closed at the first arity-3 `:::`.
        assert!(out.contains(":::{.inner}"));
    }

    #[test]
    fn nested_css_region_higher_arity_outer_recurses_into_inner() {
        // `::::{.outer}` (arity 4) survives past the inner `:::{.inner}`
        // close. The extractor recurses into the outer's body, so the
        // inner CssRegion gets its own `<div class="inner">` wrapper.
        // Both wrappers are present in the rendered output.
        let md = "::::{.outer}\n:::{.inner}\nbody\n:::\n::::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        assert!(out.contains("<div class=\"outer\""));
        assert!(out.contains("<div class=\"inner\""));
        // No literal `:::{.inner}` should leak into the body.
        assert!(!out.contains(":::{.inner}"));
    }

    #[test]
    fn css_region_containing_typed_subscribe_is_not_recursively_extracted() {
        // Same-arity nesting: outer `:::{.wrapper}` closes at the first
        // matching `:::`, so the inner `:::subscribe` is never seen.
        let md = ":::{.wrapper}\n:::subscribe\n:::\n:::\n";
        let result = extract_shortcodes(md);
        // The wrapper opens. No subscribe is extracted because the
        // outer block consumed its arity-3 closer at the inner block's
        // first `:::`.
        assert!(result.markdown_with_placeholders.contains("<div class=\"wrapper\""));
        // Subscribe is NOT extracted in Step 1.
        assert!(result.extracted.is_empty());
    }

    #[test]
    fn higher_arity_wrapper_recursively_extracts_typed_subscribe() {
        // `::::{.wrapper}` (arity 4) keeps the inner `:::subscribe`
        // intact in its body, and the extractor recurses into the body
        // so subscribe is parsed into a typed Shortcode and replaced
        // with a sentinel. Body markdown contains the sentinel, not the
        // literal source.
        let md = "::::{.wrapper}\n:::subscribe\n:::\n::::\n";
        let result = extract_shortcodes(md);
        assert!(result.markdown_with_placeholders.contains("<div class=\"wrapper\""));
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Subscribe(_) => {}
            _ => panic!("expected Subscribe"),
        }
        // Source `:::subscribe` is replaced by a sentinel — must not
        // leak into the rendered body.
        assert!(!result.markdown_with_placeholders.contains(":::subscribe"));
    }

    #[test]
    fn lower_arity_outer_wraps_higher_arity_typed_inner() {
        // SoCiviC pattern: `:::{.support-band}` (arity 3) wraps
        // `::::buttons` (arity 4). The outer arity-3 closer at the end
        // closes the outer, so the inner arity-4 buttons block lives
        // intact inside the outer's body. Recursive extraction picks
        // it up and emits a sentinel.
        let md = ":::{.support-band}\n## Title\n\n::::buttons {.inverted}\n[Support Us](/support)\n::::\n*footnote*\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        // Outer CssRegion wrapper.
        assert!(out.contains("<div class=\"support-band\""));
        // Inner buttons extracted as typed Shortcode.
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(args) => {
                assert_eq!(args.items.len(), 1);
            }
            _ => panic!("expected Buttons"),
        }
        // No literal `::::buttons` text should leak through.
        assert!(!out.contains("::::buttons"));
        assert!(!out.contains("::::"));
    }

    #[test]
    fn lower_arity_outer_wraps_grid_with_buttons_in_cell() {
        // SoCiviC index pattern: 3-colon `:::{.hero-split}` outer,
        // 4-colon `::::grid 2 {.no-cards}` middle, 5-colon
        // `:::::buttons {.inverted}` innermost. The middle grid block
        // is the recursive-extraction target — its body in turn
        // contains the buttons block, but buttons-inside-grid-cells is
        // resolved by the grid renderer, not the extractor.
        let md = "::: {.hero-split}\n::::grid 2 {.no-cards}\nleft\n+++\nright\n::::\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        // Outer hero-split CssRegion wrapper.
        assert!(out.contains("<div class=\"hero-split\""));
        // Inner grid extracted as typed Grid.
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(_) => {}
            _ => panic!("expected Grid"),
        }
        // No literal `::::grid` text should leak through.
        assert!(!out.contains("::::grid"));
    }

    #[test]
    fn unknown_name_body_recursively_extracts_typed_inner() {
        // Unknown-name fallback (e.g. typo'd `:::buttosn`) wraps in a
        // moss-unknown-shortcode div. If the body contains a higher-
        // arity typed block (e.g. nested `::::buttons`), recursion
        // picks it up so authors can debug their typo without losing
        // valid inner content.
        let md = ":::buttosn\n::::buttons\n[a](u)\n::::\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        // Unknown wrapper.
        assert!(out.contains("data-name=\"buttosn\""));
        // Inner buttons extracted.
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Buttons(_) => {}
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn unknown_name_with_plus_plus_plus_in_body_passes_through() {
        // The `+++` cell divider is a Buttons-and-Grid concern, not
        // generic shortcode body syntax. Unknown blocks should emit
        // their body verbatim including any `+++` lines. Authors who
        // misspell `:::buttons` as `:::buttosn` shouldn't see their
        // dividers eaten.
        let md = ":::buttosn\n[a](u)\n+++\n[b](v)\n:::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        assert!(out.contains(r#"data-name="buttosn""#));
        assert!(out.contains("[a](u)"));
        assert!(out.contains("+++"));
        assert!(out.contains("[b](v)"));
    }

    #[test]
    fn parse_shortcode_opener_recognizes_empty_name_with_attrs() {
        assert_eq!(
            parse_shortcode_opener(":::{.tagline}"),
            Some((3, "", "{.tagline}"))
        );
    }

    #[test]
    fn parse_shortcode_opener_rejects_just_colons() {
        assert!(parse_shortcode_opener(":::").is_none());
        assert!(parse_shortcode_opener(":::   ").is_none());
    }

    #[test]
    fn unclosed_multi_line_attrs_block_emits_verbatim() {
        // A `{` that never closes within the doc should bubble up as
        // an unclosed block (verbatim emission).
        let md = ":::buttons {\n  .primary\n[Go](go/)\n:::\n";
        let result = extract_shortcodes(md);
        // The attribute parser surfaces an UnclosedBrace error inside
        // split_positional_and_classes' brace search. The block silently
        // falls through to the unrecognized-name path → verbatim.
        // (Step 1 Task E will tighten this into an explicit warning.)
        assert!(result.extracted.is_empty() || matches!(result.extracted[0].shortcode, Shortcode::Buttons(_)));
        // The opener is preserved either way.
    }

    // ---- Deprecation warnings (Step 3 E2) ----

    #[test]
    fn grid_legacy_dash_emits_deprecation_warning() {
        let md = ":::grid 2\ncell A\n---\ncell B\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("deprecated"));
        assert!(result.warnings[0].contains("+++"));
    }

    #[test]
    fn grid_plus_plus_plus_no_deprecation_warning() {
        let md = ":::grid 2\ncell A\n+++\ncell B\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn hero_priority3_body_image_emits_deprecation_warning() {
        let md = ":::hero\nphoto.jpg\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("deprecated"));
        assert!(result.warnings[0].contains("image="));
    }

    #[test]
    fn hero_explicit_image_attr_no_deprecation_warning() {
        let md = ":::hero {image=photo.jpg}\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        assert!(result.warnings.is_empty());
    }

    // ── spec § P9 width-flag extraction ─────────────────────────────
    //
    // `:::hero {full}` / `:::gallery {wide}` / `:::grid {page}` set the
    // `width` field on the typed shortcode. `full` aliases to `screen`.
    // Absence of a width flag leaves `width = None`, which the emitter
    // turns into "no `data-width` attribute on the wrapper".

    fn first_extracted(md: &str) -> Shortcode {
        let result = extract_shortcodes(md);
        result
            .extracted
            .into_iter()
            .next()
            .expect("at least one shortcode")
            .shortcode
    }

    #[test]
    fn hero_with_full_flag_sets_width_screen() {
        let md = ":::hero {image=photo.jpg full}\n# Title\n:::\n";
        match first_extracted(md) {
            Shortcode::Hero(h) => assert_eq!(h.width.as_deref(), Some("screen")),
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn hero_with_screen_flag_sets_width_screen() {
        let md = ":::hero {image=photo.jpg screen}\n# Title\n:::\n";
        match first_extracted(md) {
            Shortcode::Hero(h) => assert_eq!(h.width.as_deref(), Some("screen")),
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn hero_with_wide_flag_sets_width_wide() {
        let md = ":::hero {image=photo.jpg wide}\n# Title\n:::\n";
        match first_extracted(md) {
            Shortcode::Hero(h) => assert_eq!(h.width.as_deref(), Some("wide")),
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn hero_without_width_flag_leaves_width_none() {
        let md = ":::hero {image=photo.jpg}\n# Title\n:::\n";
        match first_extracted(md) {
            Shortcode::Hero(h) => assert!(h.width.is_none(), "got {:?}", h.width),
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn hero_mobile_overlay_attr_is_parsed() {
        let md = ":::hero {image=hero.jpg mobile=overlay}\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                assert_eq!(args.mobile.as_deref(), Some("overlay"));
            }
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn hero_without_mobile_attr_has_none() {
        let md = ":::hero {image=hero.jpg}\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                assert!(args.mobile.is_none());
            }
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn hero_mobile_overlay_with_body_image_fallback() {
        let md = ":::hero {mobile=overlay}\n![[bg.jpg]]\n# Title\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Hero(args) => {
                assert_eq!(args.mobile.as_deref(), Some("overlay"));
                assert!(args.image.is_some());
            }
            other => panic!("expected Hero, got {other:?}"),
        }
    }

    #[test]
    fn gallery_with_page_flag_sets_width_page() {
        let md = ":::gallery 3 {page}\nphoto.jpg\n:::\n";
        match first_extracted(md) {
            Shortcode::Gallery(g) => assert_eq!(g.width.as_deref(), Some("page")),
            other => panic!("expected Gallery, got {other:?}"),
        }
    }

    #[test]
    fn gallery_without_width_flag_leaves_width_none() {
        let md = ":::gallery 3\nphoto.jpg\n:::\n";
        match first_extracted(md) {
            Shortcode::Gallery(g) => assert!(g.width.is_none()),
            other => panic!("expected Gallery, got {other:?}"),
        }
    }

    #[test]
    fn grid_with_wide_flag_sets_width_wide() {
        let md = ":::grid {cols=2 wide}\ncell A\n+++\ncell B\n:::\n";
        match first_extracted(md) {
            Shortcode::Grid(g) => assert_eq!(g.width.as_deref(), Some("wide")),
            other => panic!("expected Grid, got {other:?}"),
        }
    }

    #[test]
    fn grid_with_full_flag_normalizes_to_screen() {
        let md = ":::grid {cols=2 full}\ncell A\n+++\ncell B\n:::\n";
        match first_extracted(md) {
            Shortcode::Grid(g) => assert_eq!(g.width.as_deref(), Some("screen")),
            other => panic!("expected Grid, got {other:?}"),
        }
    }

    #[test]
    fn grid_without_width_flag_leaves_width_none() {
        let md = ":::grid 2\ncell A\n+++\ncell B\n:::\n";
        match first_extracted(md) {
            Shortcode::Grid(g) => assert!(g.width.is_none()),
            other => panic!("expected Grid, got {other:?}"),
        }
    }

    // ---- Recent (Phase B / Task 4.2) ----

    #[test]
    fn parses_recent_with_since_and_count() {
        let (sc, warns) = parse_shortcode_block(
            "recent",
            r#"{since="2026-04-01" count="5"}"#,
            "",
        );
        assert!(warns.is_empty());
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => {
                assert_eq!(args.since.as_deref(), Some("2026-04-01"));
                assert_eq!(args.count, Some(5));
                assert!(args.last.is_none());
                assert!(args.fallback_markdown.is_empty());
            }
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn parses_recent_with_last_window() {
        let (sc, _) = parse_shortcode_block("recent", r#"{last="month"}"#, "");
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => {
                assert_eq!(args.last.as_deref(), Some("month"));
                assert!(args.since.is_none());
                assert!(args.count.is_none());
            }
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn captures_recent_body_as_fallback_markdown() {
        let body = "No posts yet. [Follow along](/).";
        let (sc, _) = parse_shortcode_block("recent", "", body);
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => {
                assert_eq!(args.fallback_markdown, body);
            }
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn recent_with_no_args_yields_all_none() {
        let (sc, warns) = parse_shortcode_block("recent", "", "");
        assert!(warns.is_empty());
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => {
                assert!(args.since.is_none());
                assert!(args.last.is_none());
                assert!(args.count.is_none());
                assert!(args.fallback_markdown.is_empty());
            }
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn parses_recent_with_all_three_attrs() {
        let (sc, warns) = parse_shortcode_block(
            "recent",
            r#"{since="2026-01-01" last="month" count="3"}"#,
            "",
        );
        assert!(warns.is_empty());
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => {
                assert_eq!(args.since.as_deref(), Some("2026-01-01"));
                assert_eq!(args.last.as_deref(), Some("month"));
                assert_eq!(args.count, Some(3));
            }
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn recent_with_malformed_count_yields_none_count() {
        // Tolerant parsing: a non-numeric count value drops to None
        // rather than failing the whole block. The renderer will fall
        // back to its default (10).
        let (sc, _) = parse_shortcode_block("recent", r#"{count="lots"}"#, "");
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => assert!(args.count.is_none()),
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn recent_body_is_trimmed() {
        // Surrounding whitespace and trailing newlines do not need to
        // travel as part of the fallback markdown.
        let (sc, _) = parse_shortcode_block("recent", "", "\n  hello world  \n\n");
        match sc.expect("expected Some(Shortcode)") {
            Shortcode::Recent(args) => assert_eq!(args.fallback_markdown, "hello world"),
            other => panic!("expected Recent, got {other:?}"),
        }
    }

    #[test]
    fn extracts_recent_end_to_end_with_sentinel() {
        // Full extraction path: `:::recent` opener is recognized as
        // typed-known, gets routed through parse_shortcode_block, and the
        // literal `:::recent` is replaced by a sentinel.
        let md = ":::recent {since=\"2026-04-01\" count=\"5\"}\nNo posts yet.\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Recent(args) => {
                assert_eq!(args.since.as_deref(), Some("2026-04-01"));
                assert_eq!(args.count, Some(5));
                assert_eq!(args.fallback_markdown, "No posts yet.");
            }
            other => panic!("expected Recent, got {other:?}"),
        }
        assert!(!result.markdown_with_placeholders.contains(":::recent"));
        assert!(result
            .markdown_with_placeholders
            .contains(&placeholder_for(&result.nonce, 0)));
    }
}
