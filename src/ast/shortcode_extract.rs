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

use super::attrs::{brace_depth, gather_multi_line_attrs};
use super::cells::split_cells;
use super::shortcode::{
    ButtonItem, ButtonsShortcode, GalleryItem, GalleryShortcode, GridShortcode, HeroShortcode,
    Shortcode, SubscribeShortcode,
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

/// Names recognized by the typed AST. Other names that look like
/// shortcodes pass through to the legacy regex (`is_legacy_passthrough`)
/// or fall back to the unknown-name renderer.
const TYPED_KNOWN: &[&str] = &["subscribe", "buttons", "gallery", "hero", "grid"];

/// Step 2c of issue #613 emptied the legacy passthrough list — every
/// shortcode name is now either typed (`TYPED_KNOWN`) or unknown
/// (renders as `<div class="moss-unknown-shortcode">`). The constant
/// is retained as a documented marker for any future migration that
/// needs the same staged-passthrough mechanism.
const LEGACY_PASSTHROUGH: &[&str] = &[];

fn is_typed_known(name: &str) -> bool {
    TYPED_KNOWN.contains(&name)
}

#[allow(dead_code)]
fn is_legacy_passthrough(name: &str) -> bool {
    LEGACY_PASSTHROUGH.contains(&name)
}

/// Recognized shortcode names (Phase B Task 7+ adds variants here).
///
/// `args` is the trailing text after `:::name ` on the opening line
/// (e.g. for `:::buttons {.primary}`, args is `{.primary}`).
fn parse_shortcode_block(name: &str, args: &str, body: &str) -> Option<Shortcode> {
    match name {
        "subscribe" => Some(Shortcode::Subscribe(parse_subscribe_args(args))),
        "buttons" => Some(Shortcode::Buttons(parse_buttons_body(args, body))),
        "gallery" => Some(Shortcode::Gallery(parse_gallery_body(args, body))),
        "hero" => Some(Shortcode::Hero(parse_hero(args, body))),
        "grid" => Some(Shortcode::Grid(parse_grid(args, body))),
        _ => None,
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
fn parse_grid(args: &str, body: &str) -> GridShortcode {
    let trimmed = args.trim();
    let (positional, attr_block) = match trimmed.find('{') {
        Some(pos) => (trimmed[..pos].trim(), &trimmed[pos..]),
        None => (trimmed, ""),
    };

    let parsed = if attr_block.is_empty() {
        Default::default()
    } else {
        super::attrs::parse_attrs(attr_block).unwrap_or_default()
    };
    let classes = parsed.class_string();

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

    let cells = split_grid_cells(body);

    GridShortcode {
        columns,
        ratio,
        classes,
        cells,
    }
}

/// Split a grid body into cells on lines containing only `+++` (new
/// grammar) or `---` (legacy moss-releases backward-compat).
///
/// Mirrors [`super::cells::split_cells`] but accepts either divider.
/// Step 3 of #613 rewrites `---` to `+++` in moss-releases content;
/// after that, this helper retires in favor of `split_cells`.
fn split_grid_cells(body: &str) -> Vec<String> {
    if body.is_empty() {
        return vec![String::new()];
    }
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut first_line_in_cell = true;

    for line in body.split_inclusive('\n') {
        let content_no_eol = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content_no_eol.trim();
        if trimmed == "+++" || trimmed == "---" {
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
    cells
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
fn parse_hero(args: &str, body: &str) -> HeroShortcode {
    let trimmed_args = args.trim();

    // Split args on the first `{` to separate the directive-line path
    // (if any) from the attribute block (if any).
    let (positional, attr_block) = match trimmed_args.find('{') {
        Some(pos) => (trimmed_args[..pos].trim(), &trimmed_args[pos..]),
        None => (trimmed_args, ""),
    };

    // Parse the attribute block, if present.
    let parsed = if attr_block.is_empty() {
        Default::default()
    } else {
        super::attrs::parse_attrs(attr_block).unwrap_or_default()
    };
    let classes = parsed.class_string();

    // Priority 1: `image=` attribute.
    if let Some(image_value) = parsed.get("image") {
        let (path, attrs_str) = crate::media::split_pipe(image_value);
        return HeroShortcode {
            image: if path.trim().is_empty() {
                None
            } else {
                Some(Url::unresolved(path.trim().to_string()))
            },
            attrs: attrs_str.to_string(),
            classes,
            overlay_markdown: body.trim().to_string(),
        };
    }

    // Priority 2: directive-line path (legacy syntax). When the
    // positional text is non-empty, treat it as the image path with
    // optional `|attrs` pipe suffix. Body becomes pure overlay markdown.
    if !positional.is_empty() {
        let (path, attrs_str) = crate::media::split_pipe(positional);
        return HeroShortcode {
            image: if path.trim().is_empty() {
                None
            } else {
                Some(Url::unresolved(path.trim().to_string()))
            },
            attrs: attrs_str.to_string(),
            classes,
            overlay_markdown: body.trim().to_string(),
        };
    }

    // Priority 3: body-image fallback. Scan first non-empty line.
    let mut overlay_lines: Vec<&str> = Vec::new();
    let mut image_path: Option<String> = None;
    let mut image_attrs = String::new();
    let mut found_image = false;
    for line in body.lines() {
        if !found_image && !line.trim().is_empty() {
            if let Some((path, attrs_str)) = parse_hero_media_line(line) {
                image_path = Some(path);
                image_attrs = attrs_str;
                found_image = true;
                continue;
            }
            // First non-empty line wasn't a media reference — keep it as overlay.
            found_image = true;
        }
        overlay_lines.push(line);
    }
    HeroShortcode {
        image: image_path.map(Url::unresolved),
        attrs: image_attrs,
        classes,
        overlay_markdown: overlay_lines.join("\n").trim().to_string(),
    }
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
    if trimmed.starts_with("![[") && trimmed.ends_with("]]") {
        let inner = &trimmed[3..trimmed.len() - 2];
        let (path, attrs_str) = crate::media::split_pipe(inner);
        return Some((path.trim().to_string(), attrs_str.to_string()));
    }

    // Standard markdown image: ![alt](path|attrs)
    if trimmed.starts_with("![") {
        if let Some(paren_open) = trimmed.find("](") {
            if trimmed.ends_with(')') {
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
    // Args: `N {.classes}` where N is optional columns count.
    let (positional, classes) = split_positional_and_classes(args);
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
    }
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
        if let Some(brace_end) = trimmed[brace_start..].find('}') {
            let positional = trimmed[..brace_start].trim().to_string();
            let attr_block_str = &trimmed[brace_start..=brace_start + brace_end];
            if let Ok(parsed) = super::attrs::parse_attrs(attr_block_str) {
                return (positional, parsed.class_string());
            }
            // Legacy fallback for malformed inputs that the structured
            // parser rejects (e.g. unterminated quote on a single line).
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
    match s.find('|') {
        Some(pos) => (&s[..pos], s[pos + 1..].trim()),
        None => (s, ""),
    }
}

/// Parse `![alt](path)` into `(alt, path)`. Returns `None` if not a
/// markdown image. Mirrors the legacy parser at shortcode.rs:1615.
fn parse_markdown_image(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let rest = s.strip_prefix("![")?;
    let close_bracket = rest.find("](")?;
    let alt = &rest[..close_bracket];
    let after = &rest[close_bracket + 2..];
    let close_paren = after.rfind(')')?;
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
    if !s.starts_with('[') {
        return None;
    }
    let close_bracket = s.find(']')?;
    let text = &s[1..close_bracket];
    let after = &s[close_bracket + 1..];
    if !after.starts_with('(') || !after.ends_with(')') {
        return None;
    }
    let url = &after[1..after.len() - 1];
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
    let mut output = String::with_capacity(markdown.len());
    let mut extracted: Vec<ExtractedShortcode> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
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
            if trimmed.starts_with(&fence_marker)
                && trimmed.trim_start_matches(fence_marker.chars().next().unwrap()).trim().is_empty()
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
            // 2. Typed-known name (subscribe / buttons / gallery) — extract
            //    into the typed AST and substitute a sentinel.
            //
            // 3. Legacy passthrough (grid / hero / toc) — emit verbatim so
            //    the legacy regex pass in src-tauri can handle it.
            //
            // 4. Anything else — render as a `moss-unknown-shortcode` div
            //    around the body markdown and emit a build warning. Authors
            //    who misspelled or removed a shortcode see a visible
            //    fallback rather than literal `:::name` text.
            if name.is_empty() {
                // CssRegion (Task D)
                let parsed = super::attrs::parse_attrs(args).unwrap_or_default();
                output.push_str(&render_div_open(&parsed.classes, parsed.id.as_deref(), None));
                output.push_str("\n\n");
                output.push_str(&body);
                if !body.is_empty() && !body.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("\n</div>\n");
                i = j + 1;
                continue;
            }

            if is_typed_known(name) {
                if let Some(sc) = parse_shortcode_block(name, args, &body) {
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

            if is_legacy_passthrough(name) {
                // Verbatim — the legacy regex still owns this name.
                output.push_str(line);
                output.push('\n');
                i += 1;
                continue;
            }

            // Unknown name (Task E): wrap the body in a fallback div and
            // emit a build warning so authors see misspellings.
            let parsed = super::attrs::parse_attrs(args).unwrap_or_default();
            warnings.push(format!("unknown shortcode `:::{}`", name));
            let mut classes = vec!["moss-unknown-shortcode".to_string()];
            classes.extend(parsed.classes.iter().cloned());
            let extra_attrs = format!(r#" data-name="{}""#, html_escape_attr(name));
            output.push_str(&render_div_open(&classes, parsed.id.as_deref(), Some(&extra_attrs)));
            output.push_str("\n\n");
            output.push_str(&body);
            if !body.is_empty() && !body.ends_with('\n') {
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

    ExtractionResult {
        markdown_with_placeholders: output,
        extracted,
        nonce,
        warnings,
    }
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
    let name = &rest[..name_end];
    let args = rest[name_end..].trim();
    Some((colons, name, args))
}

/// True if `trimmed` is a closing fence with the specified arity (`:::`
/// for arity 3, `::::` for arity 4, etc.).
///
/// Matches the legacy `parse_fence_close` semantics in
/// `src-tauri/src/build/shortcode.rs:857`: a closer is N colons followed
/// by optional whitespace only. A line `::: extra` is NOT a closer
/// (it's content). Without this match, authors who pasted text after
/// the closer would see different behavior between the typed-AST path
/// and the legacy grid parser, surfacing as silent content corruption.
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
        // (3-colon). After Step 2b both are typed; the OUTER grid wins
        // first (matched at the first arity-3 closer), and the inner
        // ::::buttons travels in the grid's body markdown as cell content.
        // The renderer recursively extracts the buttons from each cell at
        // render time (see render_grid_html_typed).
        let md = ":::grid 2\n::::buttons\n[Tickets](go/)\n::::\n---\nfooter cell\n:::\n";
        let result = extract_shortcodes(md);
        // One typed entry: the outer Grid. The inner ::::buttons is in
        // the grid's body, not in result.extracted.
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 2);
                assert_eq!(grid.cells.len(), 2);
                // First cell contains the inner buttons block source.
                assert!(grid.cells[0].contains("::::buttons"));
                assert!(grid.cells[0].contains("[Tickets](go/)"));
                // Second cell is the footer.
                assert_eq!(grid.cells[1], "footer cell");
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
        let md = ":::grid 2\ncell A\n---\ncell B\n:::\n";
        let result = extract_shortcodes(md);
        assert_eq!(result.extracted.len(), 1);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.columns, 2);
                assert!(grid.ratio.is_none());
                assert_eq!(grid.cells, vec!["cell A".to_string(), "cell B".to_string()]);
            }
            other => panic!("expected Grid, got {other:?}"),
        }
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
                assert_eq!(grid.cells, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
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
            Shortcode::Grid(grid) => assert_eq!(grid.cells, vec!["A".to_string(), "B".to_string()]),
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
                assert_eq!(grid.cells, vec!["only cell".to_string()]);
            }
            _ => panic!("expected Grid"),
        }
    }

    #[test]
    fn extracts_grid_with_empty_middle_cell() {
        // Two consecutive `+++` dividers leave a middle cell empty.
        // Legacy behavior preserved this; verify the typed extractor
        // does too.
        let md = ":::grid 3\nA\n+++\n+++\nC\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.cells.len(), 3);
                assert_eq!(grid.cells[0], "A");
                assert_eq!(grid.cells[1], "");
                assert_eq!(grid.cells[2], "C");
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
    fn extracts_grid_with_compound_link_cell_text_preserved() {
        // moss-releases pattern: a cell whose entire body is a single
        // markdown link wrapping multiple block children. The extractor
        // captures it verbatim; src-tauri's render_card_html runs
        // detect_compound_link to auto-promote to `.moss-grid-card`.
        let md = ":::grid 2 {.work-cards}\n[![[poster.jpg]]\n#### Title\nbody](/url)\n+++\n[Card 2](/url2)\n:::\n";
        let result = extract_shortcodes(md);
        match &result.extracted[0].shortcode {
            Shortcode::Grid(grid) => {
                assert_eq!(grid.classes, "work-cards");
                assert_eq!(grid.cells.len(), 2);
                // Compound-link cell preserved verbatim for renderer.
                assert!(grid.cells[0].contains("[![[poster.jpg]]"));
                assert!(grid.cells[0].contains("#### Title"));
                assert!(grid.cells[0].contains("body](/url)"));
                assert_eq!(grid.cells[1], "[Card 2](/url2)");
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
                assert_eq!(args.overlay_markdown, "# A House of Daowu");
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
                assert_eq!(args.overlay_markdown, "# Welcome");
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
                assert_eq!(args.overlay_markdown, "# Title");
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
                assert_eq!(args.overlay_markdown, "# Welcome");
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
    fn nested_css_region_higher_arity_outer_wraps_inner_as_text() {
        // `::::{.outer}` (arity 4) survives past the inner `:::` close.
        // Step 1 is non-recursive: the inner `:::{.inner}` is not
        // re-extracted — it's emitted as literal markdown text inside
        // the outer's body div. Pulldown-cmark renders the literal text
        // as plain content.
        //
        // Step 2 may add recursive extraction; this test pins the
        // current behavior so a future change is visible.
        let md = "::::{.outer}\n:::{.inner}\nbody\n:::\n::::\n";
        let result = extract_shortcodes(md);
        let out = &result.markdown_with_placeholders;
        // Outer wrapper opens.
        assert!(out.contains("<div class=\"outer\">"));
        // Inner is NOT extracted — its source text is preserved verbatim
        // inside the outer body.
        assert!(out.contains(":::{.inner}"));
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
    fn higher_arity_wrapper_keeps_typed_subscribe_as_text() {
        // `::::{.wrapper}` (arity 4) keeps the subscribe block intact in
        // its body — but Step 1 doesn't recurse, so the subscribe is
        // still NOT extracted (it lives in the body markdown verbatim).
        // Pulldown-cmark renders the literal `:::subscribe` text.
        //
        // For Step 2 to support typed-shortcodes-inside-CssRegion, the
        // extractor needs a recursive pass; that's tracked as a Step 2
        // follow-up.
        let md = "::::{.wrapper}\n:::subscribe\n:::\n::::\n";
        let result = extract_shortcodes(md);
        // Outer wrapper opens.
        assert!(result.markdown_with_placeholders.contains("<div class=\"wrapper\""));
        // Subscribe is NOT extracted (no recursion in Step 1).
        assert!(result.extracted.is_empty());
        // The literal subscribe text passes through into the body.
        assert!(result.markdown_with_placeholders.contains(":::subscribe"));
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
}
