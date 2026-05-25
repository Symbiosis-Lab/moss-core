//! Unified media reference resolution and display attributes.
//!
//! All media reference contexts in moss (frontmatter cover, hero, gallery,
//! inline images, wikilink embeds) call into this module. It parses pipe-
//! separated display attributes (`object-fit`, `object-position`) and
//! resolves paths via the [`ContentGraph`].
//!
//! Pure Rust, zero I/O.

use std::collections::BTreeMap;

use crate::content_graph::ContentGraph;

// ---------------------------------------------------------------------------
// Fit — maps to CSS `object-fit`
// ---------------------------------------------------------------------------

/// CSS `object-fit` values for media display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Cover,
    Contain,
    Fill,
    None,
    ScaleDown,
}

impl Fit {
    /// Return the CSS `object-fit` value.
    pub fn to_css_value(&self) -> &str {
        match self {
            Fit::Cover => "cover",
            Fit::Contain => "contain",
            Fit::Fill => "fill",
            Fit::None => "none",
            Fit::ScaleDown => "scale-down",
        }
    }

    /// Parse from a keyword string (case-insensitive).
    ///
    /// Accepts both CSS syntax (`"scale-down"`) and space-free forms (`"scaledown"`).
    pub fn from_keyword(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cover" => Some(Fit::Cover),
            "contain" => Some(Fit::Contain),
            "fill" => Some(Fit::Fill),
            "none" => Some(Fit::None),
            "scale-down" | "scaledown" => Some(Fit::ScaleDown),
            _ => Option::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Position — maps to CSS `object-position`
// ---------------------------------------------------------------------------

/// CSS `object-position` values for media display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Center,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Position {
    /// Return the CSS `object-position` value.
    pub fn to_css_value(&self) -> &str {
        match self {
            Position::Center => "center",
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
            Position::TopLeft => "top left",
            Position::TopRight => "top right",
            Position::BottomLeft => "bottom left",
            Position::BottomRight => "bottom right",
        }
    }

    /// Parse from a keyword string (case-insensitive).
    ///
    /// Accepts hyphenated (`"top-left"`), concatenated (`"topleft"`), and
    /// space-separated (`"top left"`) forms.
    pub fn from_keyword(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "center" => Some(Position::Center),
            "left" => Some(Position::Left),
            "right" => Some(Position::Right),
            "top" => Some(Position::Top),
            "bottom" => Some(Position::Bottom),
            "top-left" | "topleft" | "top left" => Some(Position::TopLeft),
            "top-right" | "topright" | "top right" => Some(Position::TopRight),
            "bottom-left" | "bottomleft" | "bottom left" => Some(Position::BottomLeft),
            "bottom-right" | "bottomright" | "bottom right" => Some(Position::BottomRight),
            _ => Option::None,
        }
    }
}

// ---------------------------------------------------------------------------
// AlignSide — editorial runaround alignment (text wraps around half-width image)
// ---------------------------------------------------------------------------

/// Image alignment for editorial runaround layout. Mirrors WordPress's
/// `alignleft` / `alignright` block-editor convention; the moss CSS class
/// is `moss-align-left` / `moss-align-right`. Float behavior plus mobile
/// collapse (≤48rem) live in `src-tauri/src/assets/css/site.css`.
///
/// Hyphenated `align-left` is the canonical pipe keyword; unhyphenated
/// `alignleft` (matching the WP class name) is a forgiveness alias.
/// Bare `left` / `right` remain object-position keywords (see [`Position`]);
/// the `align-` prefix disambiguates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignSide {
    Left,
    Right,
}

impl AlignSide {
    /// Parse from a keyword string (case-insensitive). Accepts hyphenated
    /// `align-left` and concatenated `alignleft` forms.
    pub fn from_keyword(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "align-left" | "alignleft" => Some(AlignSide::Left),
            "align-right" | "alignright" => Some(AlignSide::Right),
            _ => None,
        }
    }

    /// CSS class name emitted on the `<img>` (and escalated to the
    /// wrapping `<figure>` via `:has()` in site.css). Kept in lockstep
    /// with the entries in `crate::contract::components::COMPONENTS`.
    pub fn css_class(self) -> &'static str {
        match self {
            AlignSide::Left => "moss-align-left",
            AlignSide::Right => "moss-align-right",
        }
    }
}

// ---------------------------------------------------------------------------
// MediaAttrs
// ---------------------------------------------------------------------------

/// Parsed display attributes for a media reference.
///
/// In addition to moss's recognized vocabulary (`fit` / `position` / `align`),
/// `class_names` and `extra_attrs` carry author-provided passthroughs from
/// Pandoc attribute blocks (`{.theme-rounded key=value}`). The moss-vocabulary
/// fields map to typed enums and `moss-*` classes / inline style; the
/// passthrough fields flow through to the emitted HTML unmodified (classes
/// joined as a space-separated list, extras as additional attributes in
/// deterministic alphabetical order).
///
/// See `docs/architecture/unified-image-emission.md` Decision #10.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaAttrs {
    pub fit: Option<Fit>,
    pub position: Option<Position>,
    pub align: Option<AlignSide>,
    /// Author-provided class names that aren't in moss's recognized
    /// vocabulary (`.align-left` / `.alignleft` get folded into `align`
    /// upstream; everything else lands here). Joined with spaces by
    /// [`Self::class_attr`] after any `moss-*` class from `css_class()`.
    pub class_names: Vec<String>,
    /// Author-provided `key=value` attributes from Pandoc attribute blocks
    /// that aren't recognized moss vocabulary. Emitted as additional HTML
    /// attributes by [`format_img_tag`] in deterministic alphabetical order
    /// (BTreeMap iteration is sorted).
    pub extra_attrs: BTreeMap<String, String>,
}

impl MediaAttrs {
    /// True when no display attributes or passthroughs are set.
    pub fn is_empty(&self) -> bool {
        self.fit.is_none()
            && self.position.is_none()
            && self.align.is_none()
            && self.class_names.is_empty()
            && self.extra_attrs.is_empty()
    }

    /// Build an inline CSS style string, or `None` if empty.
    ///
    /// Example output: `"object-fit:contain;object-position:left"`.
    /// `align` does NOT contribute — it emits as a class (see [`Self::css_class`]).
    /// `class_names` and `extra_attrs` are also out of style: classes ride on
    /// the `class` attribute, extras ride on their own attribute slots.
    pub fn to_inline_style(&self) -> Option<String> {
        if self.fit.is_none() && self.position.is_none() {
            return None;
        }

        let mut parts = Vec::new();
        if let Some(ref fit) = self.fit {
            parts.push(format!("object-fit:{}", fit.to_css_value()));
        }
        if let Some(ref pos) = self.position {
            parts.push(format!("object-position:{}", pos.to_css_value()));
        }
        Some(parts.join(";"))
    }

    /// CSS class name for the moss-recognized vocabulary, or `None` if no
    /// class-bearing attribute is set. Today only `align` produces a class;
    /// future class-bearing attributes can extend this method.
    ///
    /// This is the moss-prefixed half — see [`Self::class_attr`] for the
    /// merged value that includes author-provided `class_names`.
    pub fn css_class(&self) -> Option<&'static str> {
        self.align.map(AlignSide::css_class)
    }

    /// Build the full `class` attribute value, merging the moss-vocabulary
    /// class (from [`Self::css_class`]) with author-provided `class_names`.
    /// Returns `None` if both sources are empty.
    ///
    /// Order: moss-vocabulary class first (e.g. `moss-align-left`), then
    /// `class_names` in author-provided order. Both halves are joined with a
    /// single space.
    pub fn class_attr(&self) -> Option<String> {
        let moss_class = self.css_class();
        if moss_class.is_none() && self.class_names.is_empty() {
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        if let Some(c) = moss_class {
            parts.push(c);
        }
        for c in &self.class_names {
            parts.push(c.as_str());
        }
        Some(parts.join(" "))
    }
}

// ---------------------------------------------------------------------------
// ResolvedMedia
// ---------------------------------------------------------------------------

/// A fully resolved media reference: path + display attributes.
/// Not yet consumed outside tests — kept `pub(crate)` until a real caller exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedMedia {
    /// Root-relative path (no leading `/`) or external URL.
    pub path: String,
    /// Parsed display attributes.
    pub attrs: MediaAttrs,
}

// ---------------------------------------------------------------------------
// Parsing functions
// ---------------------------------------------------------------------------

/// Strip `[[` and `]]` brackets from a wikilink reference, if present.
///
/// Returns the inner text. If brackets are not present, returns the input
/// unchanged.
pub fn strip_wikilink(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(trimmed)
}

/// Split a media reference on the first `|`, returning `(path, attrs_str)`.
///
/// If there is no `|`, `attrs_str` is an empty string.
pub fn split_pipe(raw: &str) -> (&str, &str) {
    raw.split_once('|').unwrap_or((raw, ""))
}

/// Parse space-separated display-attribute keywords from the portion after `|`.
///
/// Recognized keywords map to [`Fit`] and [`Position`] variants.
/// Unknown tokens are silently ignored (callers may add diagnostic reporting).
///
/// Two-word position keywords like `"top left"` are handled: if a bare
/// directional keyword (`top`, `bottom`) is followed by another (`left`,
/// `right`), they are combined.
pub fn parse_media_attrs(raw: &str) -> MediaAttrs {
    let mut fit: Option<Fit> = None;
    let mut position: Option<Position> = None;
    let mut align: Option<AlignSide> = None;

    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i];

        // Try combining with next token for two-word positions.
        if i + 1 < tokens.len() {
            let combined = format!("{} {}", token, tokens[i + 1]);
            if let Some(pos) = Position::from_keyword(&combined) {
                position = Some(pos);
                i += 2;
                continue;
            }
        }

        // Single-token fit.
        if let Some(f) = Fit::from_keyword(token) {
            fit = Some(f);
            i += 1;
            continue;
        }

        // Single-token position.
        if let Some(pos) = Position::from_keyword(token) {
            position = Some(pos);
            i += 1;
            continue;
        }

        // Single-token align (editorial runaround: align-left / align-right).
        if let Some(side) = AlignSide::from_keyword(token) {
            align = Some(side);
            i += 1;
            continue;
        }

        // Unknown token — skip.
        i += 1;
    }

    MediaAttrs {
        fit,
        position,
        align,
        ..Default::default()
    }
}

/// Recognize the spec § P9 width tokens (`body | wide | page | screen | full`).
///
/// `full` is the author-facing alias for `screen` — both at the fenced-div
/// AttrBlock layer (see [`crate::ast::attrs::match_width_token`]) and here at
/// the wikilink pipe-alias layer. The returned `&'static str` is the
/// canonical value-space term emitted as `data-width="..."`.
///
/// The check is exact-match on the full input (case-sensitive ASCII): a string
/// like `"wide screen"` returns `None` so that multi-word captions like
/// `![[img|wide angle shot]]` are not silently classified as a width hint.
/// Callers that handle multi-pipe wikilink aliases should split on `|` and
/// call this on each trimmed segment individually.
pub fn match_width_token(s: &str) -> Option<&'static str> {
    match s {
        "body" => Some("body"),
        "wide" => Some("wide"),
        "page" => Some("page"),
        "screen" | "full" => Some("screen"),
        _ => None,
    }
}

/// Parse a wikilink alias for an embedded width token plus the remaining
/// alias content.
///
/// The wikilink parser (`parse_wikilink_inner`) splits on the first `|` only,
/// so when an author writes `![[img|caption|full]]`, the resulting `alias`
/// string is `"caption|full"`. This helper splits the alias on `|` and pulls
/// out a bare width-token segment (per [`match_width_token`]) without
/// reordering the others. The remaining segments are rejoined with `|`.
///
/// Returns `(width, remaining_alias)`:
///
/// - `width = Some("body|wide|page|screen")` if exactly one segment matched
///   a width token (per the "entire alias-segment is exactly one of the
///   tokens" rule). Width tokens never shadow longer captions.
/// - `remaining_alias` is the trimmed concatenation of non-width segments,
///   joined with `|`. Empty if the only segment was the width token.
///
/// If no width token is found, returns `(None, alias.to_string())` — the
/// caller falls through to its existing alias handling.
pub fn extract_width_from_alias(alias: &str) -> (Option<&'static str>, String) {
    let segments: Vec<&str> = alias.split('|').collect();
    let mut width: Option<&'static str> = None;
    let mut remaining: Vec<&str> = Vec::with_capacity(segments.len());

    for seg in &segments {
        let trimmed = seg.trim();
        if width.is_none() {
            if let Some(canonical) = match_width_token(trimmed) {
                width = Some(canonical);
                continue;
            }
        }
        remaining.push(seg);
    }

    (width, remaining.join("|"))
}

/// Return `true` if every token in `text` is a recognized display keyword.
///
/// Handles single-token keywords (`"left"`, `"contain"`) and two-word position
/// keywords (`"top left"`).  An empty string returns `false`.
pub fn is_all_display_keywords(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }

    let mut i = 0;
    while i < tokens.len() {
        // Try combining current token with next for two-word positions.
        if i + 1 < tokens.len() {
            let combined = format!("{} {}", tokens[i], tokens[i + 1]);
            if Position::from_keyword(&combined).is_some() {
                i += 2;
                continue;
            }
        }

        if Fit::from_keyword(tokens[i]).is_some() {
            i += 1;
            continue;
        }

        if Position::from_keyword(tokens[i]).is_some() {
            i += 1;
            continue;
        }

        if AlignSide::from_keyword(tokens[i]).is_some() {
            i += 1;
            continue;
        }

        return false;
    }

    true
}

/// Escape a string for safe use in HTML text or attribute values.
///
/// Replaces `&`, `"`, `'`, `<`, and `>` with their HTML entities.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Format an `<img>` tag with optional class + inline style from [`MediaAttrs`].
///
/// All attribute values are HTML-escaped. Attribute order: `src`, `alt`,
/// `class` (if any), `style` (if any), then `extra_attrs` in deterministic
/// alphabetical order (BTreeMap iteration), self-closing slash. The
/// class+style pair is shape-locked by snapshot tests in `embed_renderer`.
///
/// The `class` value merges moss-vocabulary classes (e.g. `moss-align-left`)
/// with author-provided `class_names` (e.g. `theme-rounded`) — see
/// [`MediaAttrs::class_attr`].
pub fn format_img_tag(src: &str, alt: &str, attrs: &MediaAttrs) -> String {
    let escaped_src = html_escape(src);
    let escaped_alt = html_escape(alt);
    let class_attr = attrs
        .class_attr()
        .map(|c| format!(" class=\"{}\"", html_escape(&c)))
        .unwrap_or_default();
    let style_attr = attrs
        .to_inline_style()
        .map(|s| format!(" style=\"{}\"", s))
        .unwrap_or_default();
    let mut extras = String::new();
    for (k, v) in &attrs.extra_attrs {
        extras.push_str(&format!(" {}=\"{}\"", html_escape(k), html_escape(v)));
    }
    format!(
        "<img src=\"{}\" alt=\"{}\"{}{}{} />",
        escaped_src, escaped_alt, class_attr, style_attr, extras
    )
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Returns `true` if the path looks like an external URL or data URI.
fn is_external(path: &str) -> bool {
    path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("//")
        || path.starts_with("data:")
}

/// Full pipeline: strip wikilink → split pipe → resolve path → parse attrs.
///
/// - External URLs (`http://`, `https://`, `//`, `data:`) pass through unchanged.
/// - Root-relative paths (leading `/`) have the slash stripped.
/// - Everything else is resolved via [`ContentGraph::resolve_path`], falling
///   back to the raw path if unresolved.
pub(crate) fn resolve_media_ref(raw: &str, source_path: &str, graph: &ContentGraph) -> ResolvedMedia {
    let inner = strip_wikilink(raw);
    let (path_part, attrs_str) = split_pipe(inner);
    let path_trimmed = path_part.trim();
    let attrs = parse_media_attrs(attrs_str);

    let resolved_path = if is_external(path_trimmed) {
        // External URL — passthrough.
        path_trimmed.to_string()
    } else if let Some(stripped) = path_trimmed.strip_prefix('/') {
        // Root-relative — strip leading slash.
        stripped.to_string()
    } else {
        // Resolve via content graph, fall back to raw path.
        graph
            .resolve_path(path_trimmed, source_path)
            .unwrap_or_else(|| path_trimmed.to_string())
    };

    ResolvedMedia {
        path: resolved_path,
        attrs,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_graph::ContentGraphBuilder;

    // -- Fit ----------------------------------------------------------------

    #[test]
    fn test_fit_to_css_value() {
        assert_eq!(Fit::Cover.to_css_value(), "cover");
        assert_eq!(Fit::Contain.to_css_value(), "contain");
        assert_eq!(Fit::Fill.to_css_value(), "fill");
        assert_eq!(Fit::None.to_css_value(), "none");
        assert_eq!(Fit::ScaleDown.to_css_value(), "scale-down");
    }

    #[test]
    fn test_fit_from_keyword() {
        assert_eq!(Fit::from_keyword("cover"), Some(Fit::Cover));
        assert_eq!(Fit::from_keyword("contain"), Some(Fit::Contain));
        assert_eq!(Fit::from_keyword("fill"), Some(Fit::Fill));
        assert_eq!(Fit::from_keyword("none"), Some(Fit::None));
        assert_eq!(Fit::from_keyword("scale-down"), Some(Fit::ScaleDown));
        assert_eq!(Fit::from_keyword("scaledown"), Some(Fit::ScaleDown));
    }

    #[test]
    fn test_fit_from_keyword_case_insensitive() {
        assert_eq!(Fit::from_keyword("COVER"), Some(Fit::Cover));
        assert_eq!(Fit::from_keyword("Contain"), Some(Fit::Contain));
        assert_eq!(Fit::from_keyword("Scale-Down"), Some(Fit::ScaleDown));
        assert_eq!(Fit::from_keyword("SCALEDOWN"), Some(Fit::ScaleDown));
    }

    #[test]
    fn test_fit_from_keyword_unknown() {
        assert_eq!(Fit::from_keyword("zoom"), None);
        assert_eq!(Fit::from_keyword(""), None);
        assert_eq!(Fit::from_keyword("cover "), None); // trailing space — not trimmed
    }

    // -- AlignSide ----------------------------------------------------------

    #[test]
    fn test_align_side_from_keyword() {
        assert_eq!(AlignSide::from_keyword("align-left"), Some(AlignSide::Left));
        assert_eq!(AlignSide::from_keyword("align-right"), Some(AlignSide::Right));
        // WordPress-style unhyphenated alias.
        assert_eq!(AlignSide::from_keyword("alignleft"), Some(AlignSide::Left));
        assert_eq!(AlignSide::from_keyword("alignright"), Some(AlignSide::Right));
        // Case-insensitive.
        assert_eq!(AlignSide::from_keyword("ALIGN-LEFT"), Some(AlignSide::Left));
        assert_eq!(AlignSide::from_keyword("AlignRight"), Some(AlignSide::Right));
        // Bare directional keywords remain Position, not AlignSide.
        assert_eq!(AlignSide::from_keyword("left"), None);
        assert_eq!(AlignSide::from_keyword("right"), None);
        assert_eq!(AlignSide::from_keyword(""), None);
    }

    #[test]
    fn test_align_side_css_class() {
        assert_eq!(AlignSide::Left.css_class(), "moss-align-left");
        assert_eq!(AlignSide::Right.css_class(), "moss-align-right");
    }

    // -- Position -----------------------------------------------------------

    #[test]
    fn test_position_to_css_value() {
        assert_eq!(Position::Center.to_css_value(), "center");
        assert_eq!(Position::Left.to_css_value(), "left");
        assert_eq!(Position::Right.to_css_value(), "right");
        assert_eq!(Position::Top.to_css_value(), "top");
        assert_eq!(Position::Bottom.to_css_value(), "bottom");
        assert_eq!(Position::TopLeft.to_css_value(), "top left");
        assert_eq!(Position::TopRight.to_css_value(), "top right");
        assert_eq!(Position::BottomLeft.to_css_value(), "bottom left");
        assert_eq!(Position::BottomRight.to_css_value(), "bottom right");
    }

    #[test]
    fn test_position_from_keyword_single() {
        assert_eq!(Position::from_keyword("center"), Some(Position::Center));
        assert_eq!(Position::from_keyword("left"), Some(Position::Left));
        assert_eq!(Position::from_keyword("right"), Some(Position::Right));
        assert_eq!(Position::from_keyword("top"), Some(Position::Top));
        assert_eq!(Position::from_keyword("bottom"), Some(Position::Bottom));
    }

    #[test]
    fn test_position_from_keyword_compound() {
        // Hyphenated
        assert_eq!(Position::from_keyword("top-left"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("top-right"), Some(Position::TopRight));
        assert_eq!(Position::from_keyword("bottom-left"), Some(Position::BottomLeft));
        assert_eq!(Position::from_keyword("bottom-right"), Some(Position::BottomRight));

        // Concatenated
        assert_eq!(Position::from_keyword("topleft"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("bottomright"), Some(Position::BottomRight));

        // Space-separated (used when caller pre-joins tokens)
        assert_eq!(Position::from_keyword("top left"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("bottom right"), Some(Position::BottomRight));
    }

    #[test]
    fn test_position_from_keyword_case_insensitive() {
        assert_eq!(Position::from_keyword("CENTER"), Some(Position::Center));
        assert_eq!(Position::from_keyword("Top-Left"), Some(Position::TopLeft));
        assert_eq!(Position::from_keyword("BOTTOMRIGHT"), Some(Position::BottomRight));
    }

    #[test]
    fn test_position_from_keyword_unknown() {
        assert_eq!(Position::from_keyword("middle"), None);
        assert_eq!(Position::from_keyword(""), None);
    }

    // -- MediaAttrs ---------------------------------------------------------

    #[test]
    fn test_media_attrs_is_empty() {
        let empty = MediaAttrs {
            fit: None,
            position: None,
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert!(empty.is_empty());

        let with_fit = MediaAttrs {
            fit: Some(Fit::Cover),
            position: None,
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert!(!with_fit.is_empty());

        let with_pos = MediaAttrs {
            fit: None,
            position: Some(Position::Center),
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert!(!with_pos.is_empty());
    }

    #[test]
    fn test_to_inline_style_empty() {
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(attrs.to_inline_style(), None);
    }

    #[test]
    fn test_to_inline_style_fit_only() {
        let attrs = MediaAttrs {
            fit: Some(Fit::Contain),
            position: None,
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(attrs.to_inline_style(), Some("object-fit:contain".into()));
    }

    #[test]
    fn test_to_inline_style_position_only() {
        let attrs = MediaAttrs {
            fit: None,
            position: Some(Position::Left),
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            attrs.to_inline_style(),
            Some("object-position:left".into())
        );
    }

    #[test]
    fn test_to_inline_style_both() {
        let attrs = MediaAttrs {
            fit: Some(Fit::Cover),
            position: Some(Position::TopLeft),
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            attrs.to_inline_style(),
            Some("object-fit:cover;object-position:top left".into())
        );
    }

    // -- strip_wikilink -----------------------------------------------------

    #[test]
    fn test_strip_wikilink_with_brackets() {
        assert_eq!(strip_wikilink("[[photo.jpg]]"), "photo.jpg");
        assert_eq!(strip_wikilink("[[path/to/image.png]]"), "path/to/image.png");
    }

    #[test]
    fn test_strip_wikilink_without_brackets() {
        assert_eq!(strip_wikilink("photo.jpg"), "photo.jpg");
        assert_eq!(strip_wikilink("path/to/image.png"), "path/to/image.png");
    }

    #[test]
    fn test_strip_wikilink_with_pipe() {
        assert_eq!(strip_wikilink("[[photo.jpg|cover]]"), "photo.jpg|cover");
    }

    #[test]
    fn test_strip_wikilink_with_whitespace() {
        assert_eq!(strip_wikilink("  [[photo.jpg]]  "), "photo.jpg");
    }

    #[test]
    fn test_strip_wikilink_partial_brackets() {
        // Only opening bracket — no stripping.
        assert_eq!(strip_wikilink("[[photo.jpg"), "[[photo.jpg");
        // Only closing bracket — no stripping.
        assert_eq!(strip_wikilink("photo.jpg]]"), "photo.jpg]]");
    }

    #[test]
    fn test_strip_wikilink_empty() {
        assert_eq!(strip_wikilink("[[]]"), "");
        assert_eq!(strip_wikilink(""), "");
    }

    // -- split_pipe ---------------------------------------------------------

    #[test]
    fn test_split_pipe_with_pipe() {
        assert_eq!(split_pipe("photo.jpg|cover"), ("photo.jpg", "cover"));
        assert_eq!(
            split_pipe("path/to/img.png|contain center"),
            ("path/to/img.png", "contain center")
        );
    }

    #[test]
    fn test_split_pipe_no_pipe() {
        assert_eq!(split_pipe("photo.jpg"), ("photo.jpg", ""));
        assert_eq!(split_pipe(""), ("", ""));
    }

    #[test]
    fn test_split_pipe_multiple_pipes() {
        // Only split on the first pipe.
        assert_eq!(split_pipe("a|b|c"), ("a", "b|c"));
    }

    #[test]
    fn test_split_pipe_pipe_at_edges() {
        assert_eq!(split_pipe("|cover"), ("", "cover"));
        assert_eq!(split_pipe("photo.jpg|"), ("photo.jpg", ""));
    }

    // -- parse_media_attrs --------------------------------------------------

    #[test]
    fn test_parse_attrs_fit_only() {
        let attrs = parse_media_attrs("cover");
        assert_eq!(attrs.fit, Some(Fit::Cover));
        assert_eq!(attrs.position, None);
    }

    #[test]
    fn test_parse_attrs_position_only() {
        let attrs = parse_media_attrs("center");
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, Some(Position::Center));
    }

    #[test]
    fn test_parse_attrs_fit_and_position() {
        let attrs = parse_media_attrs("contain left");
        assert_eq!(attrs.fit, Some(Fit::Contain));
        assert_eq!(attrs.position, Some(Position::Left));
    }

    #[test]
    fn test_parse_attrs_two_word_position() {
        let attrs = parse_media_attrs("top left");
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, Some(Position::TopLeft));

        let attrs2 = parse_media_attrs("cover bottom right");
        assert_eq!(attrs2.fit, Some(Fit::Cover));
        assert_eq!(attrs2.position, Some(Position::BottomRight));
    }

    #[test]
    fn test_parse_attrs_hyphenated_compound_position() {
        let attrs = parse_media_attrs("top-right");
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, Some(Position::TopRight));

        let attrs2 = parse_media_attrs("fill bottom-left");
        assert_eq!(attrs2.fit, Some(Fit::Fill));
        assert_eq!(attrs2.position, Some(Position::BottomLeft));
    }

    #[test]
    fn test_parse_attrs_unknown_tokens_ignored() {
        let attrs = parse_media_attrs("cover unknown-token left");
        assert_eq!(attrs.fit, Some(Fit::Cover));
        assert_eq!(attrs.position, Some(Position::Left));
    }

    #[test]
    fn test_parse_attrs_empty_string() {
        let attrs = parse_media_attrs("");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_attrs_only_whitespace() {
        let attrs = parse_media_attrs("   ");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_attrs_all_unknown() {
        let attrs = parse_media_attrs("foo bar baz");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_attrs_case_insensitive() {
        let attrs = parse_media_attrs("COVER CENTER");
        assert_eq!(attrs.fit, Some(Fit::Cover));
        assert_eq!(attrs.position, Some(Position::Center));
    }

    #[test]
    fn test_parse_attrs_last_wins_for_duplicates() {
        // If multiple fit keywords appear, the last one wins.
        let attrs = parse_media_attrs("cover contain");
        assert_eq!(attrs.fit, Some(Fit::Contain));
    }

    #[test]
    fn test_parse_attrs_scale_down() {
        let attrs = parse_media_attrs("scale-down");
        assert_eq!(attrs.fit, Some(Fit::ScaleDown));
    }

    // -- resolve_media_ref --------------------------------------------------

    fn sample_graph() -> ContentGraph {
        let mut b = ContentGraphBuilder::new();
        b.add_file("images/photo.jpg", "images/photo");
        b.add_file("assets/banner.png", "assets/banner");
        b.add_file("posts/hello.md", "posts/hello");
        b.build()
    }

    #[test]
    fn test_resolve_simple_path() {
        let graph = sample_graph();
        let result = resolve_media_ref("photo.jpg", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert!(result.attrs.is_empty());
    }

    #[test]
    fn test_resolve_with_attrs() {
        let graph = sample_graph();
        let result = resolve_media_ref("photo.jpg|cover center", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
        assert_eq!(result.attrs.position, Some(Position::Center));
    }

    #[test]
    fn test_resolve_wikilink() {
        let graph = sample_graph();
        let result = resolve_media_ref("[[photo.jpg|contain]]", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Contain));
    }

    #[test]
    fn test_resolve_wikilink_no_attrs() {
        let graph = sample_graph();
        let result = resolve_media_ref("[[photo.jpg]]", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert!(result.attrs.is_empty());
    }

    #[test]
    fn test_resolve_external_http() {
        let graph = sample_graph();
        let result = resolve_media_ref(
            "https://example.com/img.jpg|cover",
            "posts/hello.md",
            &graph,
        );
        assert_eq!(result.path, "https://example.com/img.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
    }

    #[test]
    fn test_resolve_external_protocol_relative() {
        let graph = sample_graph();
        let result = resolve_media_ref("//cdn.example.com/img.jpg", "posts/hello.md", &graph);
        assert_eq!(result.path, "//cdn.example.com/img.jpg");
    }

    #[test]
    fn test_resolve_external_data_uri() {
        let graph = sample_graph();
        let result = resolve_media_ref("data:image/png;base64,abc", "posts/hello.md", &graph);
        assert_eq!(result.path, "data:image/png;base64,abc");
    }

    #[test]
    fn test_resolve_root_relative() {
        let graph = sample_graph();
        let result = resolve_media_ref("/images/photo.jpg|fill", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Fill));
    }

    #[test]
    fn test_resolve_unresolved_fallback() {
        let graph = sample_graph();
        let result = resolve_media_ref("missing.jpg", "posts/hello.md", &graph);
        // ContentGraph returns None → fallback to raw path.
        assert_eq!(result.path, "missing.jpg");
        assert!(result.attrs.is_empty());
    }

    #[test]
    fn test_resolve_wikilink_with_two_word_position() {
        let graph = sample_graph();
        let result =
            resolve_media_ref("[[banner.png|cover top left]]", "posts/hello.md", &graph);
        assert_eq!(result.path, "assets/banner.png");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
        assert_eq!(result.attrs.position, Some(Position::TopLeft));
    }

    #[test]
    fn test_resolve_external_in_wikilink() {
        let graph = sample_graph();
        let result = resolve_media_ref(
            "[[https://example.com/img.jpg|contain]]",
            "posts/hello.md",
            &graph,
        );
        assert_eq!(result.path, "https://example.com/img.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Contain));
    }

    #[test]
    fn test_resolve_path_with_spaces_trimmed() {
        let graph = sample_graph();
        let result = resolve_media_ref("  photo.jpg  | cover ", "posts/hello.md", &graph);
        assert_eq!(result.path, "images/photo.jpg");
        assert_eq!(result.attrs.fit, Some(Fit::Cover));
    }

    // -- is_all_display_keywords -------------------------------------------

    #[test]
    fn test_is_all_display_keywords_positions() {
        assert!(is_all_display_keywords("left"));
        assert!(is_all_display_keywords("right"));
        assert!(is_all_display_keywords("center"));
        assert!(is_all_display_keywords("top"));
        assert!(is_all_display_keywords("bottom"));
        assert!(is_all_display_keywords("top left"));
        assert!(is_all_display_keywords("bottom right"));
    }

    #[test]
    fn test_is_all_display_keywords_fits() {
        assert!(is_all_display_keywords("cover"));
        assert!(is_all_display_keywords("contain"));
        assert!(is_all_display_keywords("fill"));
        assert!(is_all_display_keywords("none"));
        assert!(is_all_display_keywords("scale-down"));
    }

    #[test]
    fn test_is_all_display_keywords_combined() {
        assert!(is_all_display_keywords("contain left"));
        assert!(is_all_display_keywords("cover top left"));
        assert!(is_all_display_keywords("cover top-right"));
        assert!(is_all_display_keywords("scale-down bottom-left"));
    }

    #[test]
    fn test_is_all_display_keywords_rejects_non_keywords() {
        assert!(!is_all_display_keywords("A beautiful sunset"));
        assert!(!is_all_display_keywords("left side"));
        assert!(!is_all_display_keywords(""));
        assert!(!is_all_display_keywords("   "));
    }

    // -- html_escape --------------------------------------------------

    #[test]
    fn test_html_escape_basic() {
        assert_eq!(html_escape("hello"), "hello");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("a\"b"), "a&quot;b");
        assert_eq!(html_escape("a'b"), "a&#39;b");
        assert_eq!(html_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(
            html_escape("<div class=\"x\">&'</div>"),
            "&lt;div class=&quot;x&quot;&gt;&amp;&#39;&lt;/div&gt;"
        );
    }

    // -- format_img_tag ----------------------------------------------------

    #[test]
    fn test_format_img_tag_with_style() {
        let attrs = MediaAttrs {
            fit: Some(Fit::Contain),
            position: Some(Position::Left),
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            format_img_tag("photo.jpg", "alt text", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt text\" style=\"object-fit:contain;object-position:left\" />"
        );
    }

    #[test]
    fn test_format_img_tag_no_style() {
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" />"
        );
    }

    #[test]
    fn test_format_img_tag_align_only() {
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: Some(AlignSide::Left),
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" class=\"moss-align-left\" />"
        );
    }

    #[test]
    fn test_format_img_tag_align_plus_style() {
        // Align (class) composes with fit/position (inline style); both attrs
        // emit in canonical order: class before style.
        let attrs = MediaAttrs {
            fit: Some(Fit::Cover),
            position: None,
            align: Some(AlignSide::Right),
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" class=\"moss-align-right\" style=\"object-fit:cover\" />"
        );
    }

    #[test]
    fn test_parse_media_attrs_align_alone() {
        let attrs = parse_media_attrs("align-left");
        assert_eq!(attrs.align, Some(AlignSide::Left));
        assert_eq!(attrs.fit, None);
        assert_eq!(attrs.position, None);
    }

    #[test]
    fn test_parse_media_attrs_align_with_cover() {
        // Order-free composition with Fit.
        let a = parse_media_attrs("cover align-right");
        assert_eq!(a.fit, Some(Fit::Cover));
        assert_eq!(a.align, Some(AlignSide::Right));

        let b = parse_media_attrs("align-right cover");
        assert_eq!(b, a);
    }

    #[test]
    fn test_parse_media_attrs_align_last_wins() {
        // Contradictory align keywords resolve last-wins (no error, no warning).
        // Locked here so a future refactor can't silently flip to first-wins or
        // None-on-conflict.
        let attrs = parse_media_attrs("align-left align-right");
        assert_eq!(attrs.align, Some(AlignSide::Right));

        let attrs = parse_media_attrs("align-right align-left");
        assert_eq!(attrs.align, Some(AlignSide::Left));
    }

    #[test]
    fn test_is_all_display_keywords_align() {
        assert!(is_all_display_keywords("align-left"));
        assert!(is_all_display_keywords("align-right"));
        assert!(is_all_display_keywords("cover align-left"));
        assert!(is_all_display_keywords("align-left cover"));
        // Composes with Position too.
        assert!(is_all_display_keywords("align-left top"));
    }

    #[test]
    fn test_format_img_tag_escapes_attributes() {
        let attrs = MediaAttrs {
            fit: None,
            position: Some(Position::Left),
            align: None,
            class_names: Vec::new(),
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            format_img_tag("img\"bad.jpg", "a\"<b>", &attrs),
            "<img src=\"img&quot;bad.jpg\" alt=\"a&quot;&lt;b&gt;\" style=\"object-position:left\" />"
        );
    }

    // -- match_width_token / extract_width_from_alias ---------------------

    #[test]
    fn test_match_width_token_recognized() {
        assert_eq!(match_width_token("body"), Some("body"));
        assert_eq!(match_width_token("wide"), Some("wide"));
        assert_eq!(match_width_token("page"), Some("page"));
        assert_eq!(match_width_token("screen"), Some("screen"));
        // `full` is the author-facing alias for `screen` (canonical value).
        assert_eq!(match_width_token("full"), Some("screen"));
    }

    #[test]
    fn test_match_width_token_rejects_non_width() {
        assert_eq!(match_width_token(""), None);
        assert_eq!(match_width_token("BODY"), None);
        assert_eq!(match_width_token("widely"), None);
        // Multi-token strings are exact-match only — no caption shadowing.
        assert_eq!(match_width_token("wide angle"), None);
        // Display keywords aren't width tokens.
        assert_eq!(match_width_token("contain"), None);
        assert_eq!(match_width_token("left"), None);
    }

    #[test]
    fn test_extract_width_from_alias_single_segment_width() {
        let (w, rest) = extract_width_from_alias("full");
        assert_eq!(w, Some("screen"));
        assert_eq!(rest, "");
    }

    #[test]
    fn test_extract_width_from_alias_caption_only() {
        // No width token — alias passes through unchanged.
        let (w, rest) = extract_width_from_alias("A beautiful sunset");
        assert_eq!(w, None);
        assert_eq!(rest, "A beautiful sunset");
    }

    #[test]
    fn test_extract_width_from_alias_caption_then_width() {
        // Multi-pipe alias `caption|full` (the wikilink parser hands us
        // the post-first-`|` slice intact).
        let (w, rest) = extract_width_from_alias("A nice photo|full");
        assert_eq!(w, Some("screen"));
        assert_eq!(rest, "A nice photo");
    }

    #[test]
    fn test_extract_width_from_alias_width_then_caption() {
        let (w, rest) = extract_width_from_alias("wide|A nice photo");
        assert_eq!(w, Some("wide"));
        assert_eq!(rest, "A nice photo");
    }

    #[test]
    fn test_extract_width_from_alias_caption_with_width_word_not_shadowed() {
        // The phrase "caption that says wide" must NOT trigger width
        // recognition — a width token only fires when an entire alias
        // segment is exactly the token.
        let (w, rest) = extract_width_from_alias("caption that says wide");
        assert_eq!(w, None);
        assert_eq!(rest, "caption that says wide");
    }

    #[test]
    fn test_extract_width_from_alias_only_first_width_extracted() {
        // If two width tokens appear, only the first one is canonical-ised;
        // the second stays in the caption text. Authors writing two width
        // tokens is malformed input, and rather than silently merging we
        // preserve the surplus for diagnostic visibility downstream.
        let (w, rest) = extract_width_from_alias("full|wide");
        assert_eq!(w, Some("screen"));
        assert_eq!(rest, "wide");
    }

    #[test]
    fn test_extract_width_from_alias_segment_whitespace_trimmed() {
        // Authors who write `caption | full` should still get width
        // recognition — leading/trailing whitespace on a segment is
        // ignored for the token check but preserved in the rejoined rest.
        let (w, rest) = extract_width_from_alias("caption | full");
        assert_eq!(w, Some("screen"));
        assert_eq!(rest, "caption ");
    }

    // -- MediaAttrs passthroughs: class_names + extra_attrs ----------------

    #[test]
    fn test_media_attrs_class_names_preserved() {
        // Author-provided class names (not in moss vocabulary) survive on
        // MediaAttrs and emit unprefixed via class_attr / format_img_tag.
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: None,
            class_names: vec!["theme-rounded".to_string(), "shadow-lg".to_string()],
            extra_attrs: BTreeMap::new(),
        };
        assert!(!attrs.is_empty());
        assert_eq!(
            attrs.class_attr(),
            Some("theme-rounded shadow-lg".to_string())
        );
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" class=\"theme-rounded shadow-lg\" />"
        );
    }

    #[test]
    fn test_media_attrs_class_names_compose_with_align() {
        // moss-vocabulary class (from align) and author class_names compose:
        // moss-prefixed class comes first, then author classes in order.
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: Some(AlignSide::Left),
            class_names: vec!["theme-rounded".to_string()],
            extra_attrs: BTreeMap::new(),
        };
        assert_eq!(
            attrs.class_attr(),
            Some("moss-align-left theme-rounded".to_string())
        );
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" class=\"moss-align-left theme-rounded\" />"
        );
    }

    #[test]
    fn test_media_attrs_extra_attrs_emitted() {
        // extra_attrs flow through to the emitted <img> in deterministic
        // alphabetical order (BTreeMap iteration). They appear AFTER
        // class/style and before the self-closing slash.
        let mut extras = BTreeMap::new();
        extras.insert("data-zoom".to_string(), "true".to_string());
        extras.insert("data-id".to_string(), "42".to_string());
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: None,
            class_names: vec![],
            extra_attrs: extras,
        };
        assert!(!attrs.is_empty());
        // data-id < data-zoom alphabetically — id appears first.
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" data-id=\"42\" data-zoom=\"true\" />"
        );
    }

    #[test]
    fn test_media_attrs_extra_attrs_escaped() {
        // extra_attrs keys and values are HTML-escaped on emission so a
        // hostile author can't break out of the attribute or inject tags.
        let mut extras = BTreeMap::new();
        extras.insert("data-title".to_string(), r#"a "quoted" & <b>bold</b>"#.to_string());
        let attrs = MediaAttrs {
            fit: None,
            position: None,
            align: None,
            class_names: vec![],
            extra_attrs: extras,
        };
        assert_eq!(
            format_img_tag("photo.jpg", "alt", &attrs),
            "<img src=\"photo.jpg\" alt=\"alt\" data-title=\"a &quot;quoted&quot; &amp; &lt;b&gt;bold&lt;/b&gt;\" />"
        );
    }
}
